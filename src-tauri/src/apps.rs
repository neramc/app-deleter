use serde::Serialize;

/// 프론트엔드 `InstalledApp`(src/types.ts)와 1:1 대응한다.
#[derive(Debug, Clone, Serialize)]
pub struct InstalledApp {
    pub id: String,
    pub name: String,
    pub publisher: Option<String>,
    pub version: Option<String>,
    pub install_location: Option<String>,
    pub size_bytes: u64,
    /// 실제 폴더 walk가 불가능해 EstimatedSize 폴백을 쓴 경우 true.
    pub size_estimated: bool,
    pub uninstall_string: Option<String>,
    pub icon_path: Option<String>,
}

/// `app-size-updated` 이벤트 payload.
#[derive(Clone, Serialize)]
pub struct AppSizeUpdate {
    pub id: String,
    pub size_bytes: u64,
    pub size_estimated: bool,
}

/// 설치된 앱 목록을 반환한다.
///
/// 1차로 레지스트리 메타데이터(EstimatedSize 기준)를 즉시 반환하고,
/// 백그라운드에서 실제 설치 폴더 용량을 계산하며 `app-size-updated`
/// 이벤트를 스트리밍한다. 모두 끝나면 `app-sizes-complete`를 emit한다.
#[tauri::command]
pub fn list_installed_apps(
    app: tauri::AppHandle,
) -> Result<Vec<InstalledApp>, String> {
    #[cfg(windows)]
    {
        windows_impl::list(app)
    }
    #[cfg(not(windows))]
    {
        // 비-Windows에서는 동일 시그니처의 스텁(빈 목록).
        let _ = app;
        Ok(Vec::new())
    }
}

#[cfg(windows)]
mod windows_impl {
    use super::{AppSizeUpdate, InstalledApp};
    use crate::fastsize;
    use std::collections::HashMap;
    use std::path::{Path, PathBuf};
    use tauri::Emitter;
    use winreg::enums::*;
    use winreg::RegKey;

    /// 순회할 Uninstall 레지스트리 위치 (루트, 하위 경로).
    const UNINSTALL_LOCATIONS: &[(isize, &str)] = &[
        (
            HKEY_LOCAL_MACHINE,
            r"SOFTWARE\Microsoft\Windows\CurrentVersion\Uninstall",
        ),
        (
            HKEY_LOCAL_MACHINE,
            r"SOFTWARE\WOW6432Node\Microsoft\Windows\CurrentVersion\Uninstall",
        ),
        (
            HKEY_CURRENT_USER,
            r"SOFTWARE\Microsoft\Windows\CurrentVersion\Uninstall",
        ),
    ];

    pub fn list(app: tauri::AppHandle) -> Result<Vec<InstalledApp>, String> {
        let mut by_name: HashMap<String, InstalledApp> = HashMap::new();

        for (root, path) in UNINSTALL_LOCATIONS {
            let hive = RegKey::predef(*root);
            let Ok(uninstall) = hive.open_subkey_with_flags(path, KEY_READ) else {
                continue;
            };
            for key_name in uninstall.enum_keys().filter_map(Result::ok) {
                let Ok(entry) = uninstall.open_subkey_with_flags(&key_name, KEY_READ)
                else {
                    continue;
                };
                if let Some(app_info) = parse_entry(&entry, path, &key_name) {
                    // 동일 DisplayName 중복 제거: 언인스톨 정보가 있는 쪽을 우선.
                    by_name
                        .entry(app_info.name.to_lowercase())
                        .and_modify(|existing| {
                            if existing.uninstall_string.is_none()
                                && app_info.uninstall_string.is_some()
                            {
                                *existing = app_info.clone();
                            }
                        })
                        .or_insert(app_info);
                }
            }
        }

        let mut apps: Vec<InstalledApp> = by_name.into_values().collect();
        // 1차 정렬: EstimatedSize 내림차순.
        apps.sort_by(|a, b| b.size_bytes.cmp(&a.size_bytes));

        // 백그라운드에서 실제 폴더 용량 계산 후 이벤트 스트리밍.
        spawn_size_calculation(app, &apps);

        Ok(apps)
    }

    /// 레지스트리 하위 키 하나를 `InstalledApp`으로 변환(필터 포함).
    fn parse_entry(entry: &RegKey, base_path: &str, key_name: &str) -> Option<InstalledApp> {
        let name: String = entry.get_value("DisplayName").ok()?;
        if name.trim().is_empty() {
            return None;
        }

        // 시스템 컴포넌트 / 업데이트 / 컴포넌트성 항목 제외.
        if get_dword(entry, "SystemComponent") == Some(1) {
            return None;
        }
        if let Some(release_type) = get_string(entry, "ReleaseType") {
            let rt = release_type.to_lowercase();
            if rt.contains("update") || rt.contains("hotfix") || rt.contains("security") {
                return None;
            }
        }
        if get_string(entry, "ParentKeyName").is_some() {
            return None;
        }

        // 대화형 우선: 앱 본래의 제거 마법사 UI를 띄우는 UninstallString을
        // 우선 사용하고, 없을 때만 무인(Quiet) 제거 명령으로 폴백한다.
        let uninstall_string = get_string(entry, "UninstallString")
            .or_else(|| get_string(entry, "QuietUninstallString"));
        let install_location = get_string(entry, "InstallLocation").filter(|s| !s.trim().is_empty());
        let icon_raw = get_string(entry, "DisplayIcon");

        // EstimatedSize는 KB 단위 DWORD → 바이트 폴백.
        let estimated_bytes = get_dword(entry, "EstimatedSize")
            .map(|kb| kb as u64 * 1024)
            .unwrap_or(0);

        Some(InstalledApp {
            id: format!("{base_path}\\{key_name}"),
            name,
            publisher: get_string(entry, "Publisher"),
            version: get_string(entry, "DisplayVersion"),
            install_location,
            size_bytes: estimated_bytes,
            size_estimated: true,
            uninstall_string,
            icon_path: icon_to_image_path(icon_raw),
        })
    }

    /// 한 앱의 용량을 계산하는 데 필요한 정보.
    struct Job {
        id: String,
        name_norm: String,
        install_location: Option<String>,
        icon_dir: Option<String>,
        fallback_bytes: u64,
    }

    /// 실제 폴더 용량(설치 폴더 + AppData 데이터)을 병렬 계산하고
    /// 앱별로 `app-size-updated` 이벤트를 emit한다.
    fn spawn_size_calculation(app: tauri::AppHandle, apps: &[InstalledApp]) {
        let jobs: Vec<Job> = apps
            .iter()
            .map(|a| Job {
                id: a.id.clone(),
                name_norm: normalize(&a.name),
                install_location: a.install_location.clone(),
                icon_dir: a
                    .icon_path
                    .as_deref()
                    .and_then(|p| Path::new(p).parent())
                    .map(|p| p.to_string_lossy().into_owned()),
                fallback_bytes: a.size_bytes,
            })
            .collect();

        std::thread::spawn(move || {
            use rayon::prelude::*;

            // AppData(Roaming/Local/LocalLow/ProgramData) 하위 폴더를 한 번만
            // 인덱싱해 두고, 각 앱은 이 인덱스에서 자신의 데이터 폴더를 매칭한다.
            let index = AppDataIndex::build();

            jobs.par_iter().for_each(|job| {
                let roots = collect_roots(job, &index);
                let (size_bytes, size_estimated) = if roots.is_empty() {
                    (job.fallback_bytes, true)
                } else {
                    (roots.iter().map(|r| fastsize::dir_size(r)).sum(), false)
                };

                let _ = app.emit(
                    "app-size-updated",
                    AppSizeUpdate {
                        id: job.id.clone(),
                        size_bytes,
                        size_estimated,
                    },
                );
            });

            let _ = app.emit("app-sizes-complete", ());
        });
    }

    /// 한 앱에 대해 용량을 합산할 실제 디렉터리 목록을 구성한다.
    /// 설치 폴더(없으면 아이콘 폴더 폴백) + 매칭된 AppData 데이터 폴더들.
    /// 서로 포함 관계인 루트는 제거해 이중 합산을 방지한다.
    fn collect_roots(job: &Job, index: &AppDataIndex) -> Vec<PathBuf> {
        let mut roots: Vec<PathBuf> = Vec::new();

        let install = job
            .install_location
            .as_deref()
            .map(PathBuf::from)
            .filter(|p| p.is_dir());
        if let Some(p) = install {
            roots.push(p);
        } else if let Some(icd) = job.icon_dir.as_deref() {
            let p = PathBuf::from(icd);
            if p.is_dir() {
                roots.push(p);
            }
        }

        roots.extend(index.matches(&job.name_norm));
        dedupe_roots(roots)
    }

    /// 다른 루트의 하위 경로인 루트를 제거(대소문자 무시)해 중복 합산을 막는다.
    fn dedupe_roots(mut roots: Vec<PathBuf>) -> Vec<PathBuf> {
        roots.sort_by_key(|p| p.as_os_str().len());
        let mut kept: Vec<PathBuf> = Vec::new();
        for r in roots {
            let rl = r.to_string_lossy().to_lowercase();
            let is_sub = kept.iter().any(|k| {
                let kl = k.to_string_lossy().to_lowercase();
                rl == kl || rl.starts_with(&(kl + "\\"))
            });
            if !is_sub {
                kept.push(r);
            }
        }
        kept
    }

    /// 문자열을 소문자 영숫자만 남겨 정규화한다("Visual Studio Code" → "visualstudiocode").
    fn normalize(s: &str) -> String {
        s.chars()
            .filter(|c| c.is_alphanumeric())
            .flat_map(|c| c.to_lowercase())
            .collect()
    }

    /// AppData 데이터 폴더 인덱스: (정규화된 폴더명, 전체 경로).
    /// 각 루트의 1~2단계 하위 폴더를 한 번만 수집한다(앱마다 재탐색하지 않음).
    struct AppDataIndex {
        entries: Vec<(String, PathBuf)>,
    }

    impl AppDataIndex {
        fn build() -> Self {
            let mut entries = Vec::new();
            for root in Self::roots() {
                Self::index_two_levels(&root, &mut entries);
            }
            AppDataIndex { entries }
        }

        /// 검색 대상 AppData 루트들.
        fn roots() -> Vec<PathBuf> {
            let mut v = Vec::new();
            if let Ok(p) = std::env::var("APPDATA") {
                v.push(PathBuf::from(p)); // Roaming
            }
            if let Ok(p) = std::env::var("LOCALAPPDATA") {
                let local = PathBuf::from(&p);
                if let Some(parent) = local.parent() {
                    v.push(parent.join("LocalLow"));
                }
                v.push(local); // Local
            }
            if let Ok(p) = std::env::var("ProgramData") {
                v.push(PathBuf::from(p));
            }
            v
        }

        /// 루트의 직속 폴더 + 그 한 단계 하위 폴더(예: Roaming\Publisher\App)를 인덱싱.
        fn index_two_levels(root: &Path, out: &mut Vec<(String, PathBuf)>) {
            let Ok(rd) = std::fs::read_dir(root) else {
                return;
            };
            for e in rd.flatten() {
                let Ok(ft) = e.file_type() else { continue };
                if !ft.is_dir() || ft.is_symlink() {
                    continue;
                }
                let path = e.path();
                out.push((normalize(&e.file_name().to_string_lossy()), path.clone()));

                if let Ok(rd2) = std::fs::read_dir(&path) {
                    for e2 in rd2.flatten() {
                        if e2
                            .file_type()
                            .map(|t| t.is_dir() && !t.is_symlink())
                            .unwrap_or(false)
                        {
                            out.push((
                                normalize(&e2.file_name().to_string_lossy()),
                                e2.path(),
                            ));
                        }
                    }
                }
            }
        }

        /// 앱 이름(정규화)과 강하게 일치하는 데이터 폴더 경로들을 반환한다.
        fn matches(&self, app_norm: &str) -> Vec<PathBuf> {
            if app_norm.len() < 4 {
                return Vec::new(); // 너무 짧은 이름은 오탐 위험이 커서 제외
            }
            self.entries
                .iter()
                .filter(|(folder, _)| strong_match(folder, app_norm))
                .map(|(_, p)| p.clone())
                .collect()
        }
    }

    /// 폴더명과 앱 이름의 강한 일치 판정(오탐 최소화):
    /// 완전 일치, 또는 짧은 쪽이 4자 이상이면서 긴 쪽의 접두사이고
    /// 길이 차가 2배를 넘지 않는 경우만 매칭으로 본다.
    fn strong_match(folder: &str, app: &str) -> bool {
        if folder.is_empty() || app.is_empty() {
            return false;
        }
        if folder == app {
            return true;
        }
        let (short, long) = if folder.len() <= app.len() {
            (folder, app)
        } else {
            (app, folder)
        };
        short.len() >= 4 && long.starts_with(short) && long.len() <= short.len() * 2
    }

    /// DisplayIcon 값을 `<img>`로 렌더 가능한 경로로 정규화한다.
    /// 현재는 `.ico` 파일만 지원(따옴표/`,index` 접미 제거). 그 외(.exe 등)는 None.
    fn icon_to_image_path(raw: Option<String>) -> Option<String> {
        let raw = raw?;
        let mut path = raw.trim().trim_matches('"').to_string();
        // "C:\app\icon.ico,0" 형태의 인덱스 접미 제거.
        if let Some(idx) = path.rfind(',') {
            if path[idx + 1..].trim().chars().all(|c| c.is_ascii_digit() || c == '-') {
                path.truncate(idx);
            }
        }
        let path = path.trim_matches('"').to_string();
        if path.to_lowercase().ends_with(".ico") && Path::new(&path).is_file() {
            Some(path)
        } else {
            None
        }
    }

    fn get_string(key: &RegKey, name: &str) -> Option<String> {
        key.get_value::<String, _>(name)
            .ok()
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
    }

    fn get_dword(key: &RegKey, name: &str) -> Option<u32> {
        key.get_value::<u32, _>(name).ok()
    }
}
