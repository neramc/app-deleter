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

        let uninstall_string =
            get_string(entry, "QuietUninstallString").or_else(|| get_string(entry, "UninstallString"));
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

    /// 실제 폴더 용량을 병렬 계산하고 앱별로 이벤트를 emit한다.
    fn spawn_size_calculation(app: tauri::AppHandle, apps: &[InstalledApp]) {
        struct Job {
            id: String,
            install_location: Option<String>,
            icon_dir: Option<String>,
            fallback_bytes: u64,
        }

        let jobs: Vec<Job> = apps
            .iter()
            .map(|a| Job {
                id: a.id.clone(),
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

            jobs.par_iter().for_each(|job| {
                let (size_bytes, size_estimated) = measure(job.install_location.as_deref())
                    .or_else(|| measure(job.icon_dir.as_deref()))
                    .map(|bytes| (bytes, false))
                    .unwrap_or((job.fallback_bytes, true));

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

    /// 경로가 실재하는 디렉터리면 재귀 용량을 계산한다(없으면 None).
    fn measure(path: Option<&str>) -> Option<u64> {
        let path = PathBuf::from(path?);
        if !path.is_dir() {
            return None;
        }
        Some(dir_size(&path))
    }

    /// 디렉터리를 재귀 순회하여 모든 파일 크기를 합산한다.
    /// - 심볼릭/리파스 포인트는 따라가지 않음(루프/중복 방지)
    /// - 접근 거부 항목은 건너뜀
    fn dir_size(path: &Path) -> u64 {
        walkdir::WalkDir::new(path)
            .follow_links(false)
            .into_iter()
            .filter_map(Result::ok)
            .filter_map(|e| e.metadata().ok())
            .filter(|m| m.is_file())
            .map(|m| m.len())
            .sum()
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
