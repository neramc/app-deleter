//! 설치된 앱 목록 수집 (레지스트리 Uninstall 키 + Microsoft Store 패키지).

use std::collections::HashSet;

use crate::cmdline::{self, UninstallInfo};
use crate::format;
use crate::installdir;
use crate::pathutil;
use crate::safety::{Hive, PathGuard};
use crate::textmatch::{self, AppAliases, NameMatch};
use crate::win::icons::IconSource;
use crate::win::known::Folders;
use crate::win::{expand_env, fsops, reg, store};

/// 앱 출처.
#[derive(Debug, Clone)]
pub enum Source {
    Win32 {
        hive: Hive,
        key_path: String,
        key_name: String,
    },
    Store {
        full_name: String,
        family_name: String,
    },
}

/// 설치 폴더 신뢰도.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DirTrust {
    /// `InstallLocation`이며 앱 실행 파일이 그 안에 있거나 폴더 이름이 앱과 일치.
    Verified,
    /// 실행 파일 경로에서 이름 일치로 추정했거나, 게시자 이름 폴더.
    Derived,
    /// `InstallLocation`이지만 앱과의 연관을 확인하지 못함 — 기본 선택하지 않는다.
    Unverified,
}

/// 설치된 앱 하나.
#[derive(Debug, Clone)]
pub struct AppInfo {
    pub name: String,
    pub publisher: String,
    pub version: String,
    pub install_date: String,
    pub source: Source,
    /// 설치 폴더(검증됨). Store 앱은 용량 계산용으로만 쓴다.
    pub install_dir: Option<String>,
    /// 설치 폴더를 얼마나 믿을 수 있는지(잔여물 신뢰도의 근거).
    pub install_dir_trust: DirTrust,
    pub icon: Option<IconSource>,
    /// 레지스트리 `EstimatedSize`(바이트).
    pub estimated_size: u64,
    pub uninstall: UninstallInfo,
    pub aliases: AppAliases,
    /// 앱 고유 파일 경로(실행 파일·아이콘) — 다른 앱 보호 판단에 쓴다.
    pub evidence_files: Vec<String>,
}

impl AppInfo {
    pub fn is_store(&self) -> bool {
        matches!(self.source, Source::Store { .. })
    }

    /// 목록 갱신 후에도 같은 앱을 찾기 위한 고유 키.
    pub fn identity(&self) -> String {
        match &self.source {
            Source::Win32 { hive, key_path, .. } => {
                format!("w:{}\\{}", hive.short_name(), key_path.to_lowercase())
            }
            Source::Store { family_name, .. } => format!("s:{}", family_name.to_lowercase()),
        }
    }

    /// 제거 프로그램 없이 정리만 가능한지(제거 명령이 없거나 실행 파일이 사라짐).
    pub fn uninstaller_missing(&self) -> bool {
        if self.is_store() {
            return false;
        }
        match self.uninstall.interactive_command() {
            None => true,
            Some(cmd) => {
                let (exe, _) = cmdline::split_command(&expand_env(&cmd));
                let exe = expand_env(&exe);
                pathutil::is_absolute(&exe) && !std::path::Path::new(&exe).exists()
            }
        }
    }
}

/// 검색·삭제에 공통으로 쓰는 시스템 정보.
pub struct Context {
    pub folders: Folders,
    pub guard: PathGuard,
    pub roots: Vec<String>,
}

impl Context {
    pub fn load() -> Self {
        let folders = Folders::load();
        let guard = folders.guard();
        let roots = folders.install_roots();
        Self {
            folders,
            guard,
            roots,
        }
    }
}

pub const UNINSTALL_ROOTS: &[(Hive, &str)] = &[
    (
        Hive::LocalMachine,
        r"SOFTWARE\Microsoft\Windows\CurrentVersion\Uninstall",
    ),
    (
        Hive::LocalMachine,
        r"SOFTWARE\WOW6432Node\Microsoft\Windows\CurrentVersion\Uninstall",
    ),
    (
        Hive::CurrentUser,
        r"Software\Microsoft\Windows\CurrentVersion\Uninstall",
    ),
    (
        Hive::CurrentUser,
        r"Software\WOW6432Node\Microsoft\Windows\CurrentVersion\Uninstall",
    ),
];

/// 제거 프로그램 실행 파일이 앱 고유 파일(근거)로 쓸 만한지.
fn is_app_owned_exe(exe: &str, folders: &Folders) -> bool {
    let name = pathutil::file_name(exe).to_ascii_lowercase();
    if matches!(
        name.as_str(),
        "msiexec.exe" | "rundll32.exe" | "cmd.exe" | "powershell.exe" | "wscript.exe"
    ) {
        return false;
    }
    pathutil::is_absolute(exe) && !pathutil::is_same_or_within(exe, &folders.windows)
}

fn is_kb_update(key_name: &str) -> bool {
    let k = key_name.to_ascii_uppercase();
    k.starts_with("KB") && k[2..].chars().take_while(char::is_ascii_digit).count() >= 6
}

fn clean_dir(raw: &str) -> String {
    let s = expand_env(raw.trim().trim_matches('"').trim());
    let t = s.trim_end_matches('\\');
    if t.len() == 2 && t.ends_with(':') {
        format!("{t}\\")
    } else {
        t.to_string()
    }
}

fn read_win32(hive: Hive, root: &str, key_name: &str, ctx: &Context) -> Option<AppInfo> {
    let key_path = format!(r"{root}\{key_name}");
    let key = reg::open(hive, &key_path).ok()?;
    let name = reg::get_string(&key, "DisplayName")?;
    if reg::get_dword(&key, "SystemComponent") == Some(1)
        || reg::get_string(&key, "ParentKeyName").is_some()
    {
        return None;
    }
    if let Some(rt) = reg::get_string(&key, "ReleaseType") {
        let rt = rt.to_lowercase();
        if rt.contains("update") || rt.contains("hotfix") {
            return None;
        }
    }
    if is_kb_update(key_name) {
        return None;
    }

    let uninstall_string = reg::get_string(&key, "UninstallString");
    let quiet = reg::get_string(&key, "QuietUninstallString");
    let windows_installer = reg::get_dword(&key, "WindowsInstaller") == Some(1);
    let msi = cmdline::msi_product_code(uninstall_string.as_deref(), key_name, windows_installer);
    let inno = key_name.ends_with("_is1")
        || uninstall_string
            .as_deref()
            .is_some_and(cmdline::is_inno_uninstaller);
    let install_location = reg::get_string(&key, "InstallLocation").map(|s| clean_dir(&s));
    let display_icon = reg::get_string(&key, "DisplayIcon").map(|s| expand_env(&s));

    if uninstall_string.is_none() && quiet.is_none() && msi.is_none() && install_location.is_none()
    {
        return None; // 지울 방법도, 지울 위치도 없는 항목
    }

    let publisher = reg::get_string(&key, "Publisher").unwrap_or_default();
    let version = reg::get_string(&key, "DisplayVersion").unwrap_or_default();
    let install_date = reg::get_string(&key, "InstallDate")
        .and_then(|d| format::install_date(&d))
        .or_else(|| {
            let t = key.query_info().ok()?.get_last_write_time_system();
            (t.wYear >= 1990).then(|| format!("{:04}-{:02}-{:02}", t.wYear, t.wMonth, t.wDay))
        })
        .unwrap_or_default();
    let estimated_size = reg::get_dword(&key, "EstimatedSize").map_or(0, |kb| u64::from(kb) * 1024);

    // 근거 파일: 아이콘 실행 파일, 제거 프로그램 실행 파일.
    let mut evidence_files = Vec::new();
    let icon_path = display_icon
        .as_deref()
        .and_then(cmdline::parse_icon_location)
        .map(|(p, _)| expand_env(&p));
    if let Some(p) = &icon_path
        && pathutil::is_absolute(p)
        && !pathutil::is_same_or_within(p, &ctx.folders.windows)
    {
        evidence_files.push(p.clone());
    }
    if let Some(cmd) = uninstall_string.as_deref() {
        let (exe, _) = cmdline::split_command(&expand_env(cmd));
        let exe = expand_env(&exe);
        if is_app_owned_exe(&exe, &ctx.folders) {
            evidence_files.push(exe);
        }
    }

    // 별칭: 아이콘 실행 파일 이름, Uninstall 키 이름 (+ 설치 폴더 이름).
    let mut extra: Vec<String> = Vec::new();
    if let Some(p) = &icon_path
        && !pathutil::is_same_or_within(p, &ctx.folders.windows)
    {
        let stem = pathutil::file_stem(p);
        if textmatch::is_meaningful_exe_stem(stem) {
            extra.push(stem.to_string());
        }
    }
    if let Some(k) = installdir::key_name_alias(key_name) {
        extra.push(k);
    }
    // 설치 폴더 이름을 빼고 만든 별칭 — InstallLocation 검증용.
    let name_refs: Vec<&str> = extra.iter().map(String::as_str).collect();
    let name_aliases = AppAliases::new(&name, &publisher, &name_refs);
    if let Some(dir) = &install_location {
        extra.push(pathutil::file_name(dir).to_string());
    }
    let extra_refs: Vec<&str> = extra.iter().map(String::as_str).collect();
    let aliases = AppAliases::new(&name, &publisher, &extra_refs);

    // 설치 폴더: InstallLocation(검증) → 근거 파일에서 추정.
    let mut install_dir = None;
    let mut trust = DirTrust::Unverified;
    if let Some(dir) = install_location
        .filter(|d| pathutil::is_absolute(d) && ctx.guard.can_delete(d) && fsops::is_dir_path(d))
    {
        let evidence_inside = evidence_files
            .iter()
            .any(|f| pathutil::is_same_or_within(f, &dir));
        trust = match name_aliases.classify(pathutil::file_name(&dir)) {
            _ if evidence_inside => DirTrust::Verified,
            NameMatch::Exact | NameMatch::Partial => DirTrust::Verified,
            NameMatch::Publisher => DirTrust::Derived,
            NameMatch::None => DirTrust::Unverified,
        };
        install_dir = Some(dir);
    } else {
        for f in &evidence_files {
            if let Some(d) = installdir::derive_from_file(f, &ctx.roots, &aliases, &ctx.guard)
                && fsops::is_dir_path(&d)
            {
                install_dir = Some(d);
                trust = DirTrust::Derived;
                break;
            }
        }
    }

    let icon = display_icon.map(IconSource::Resource);

    Some(AppInfo {
        name,
        publisher,
        version,
        install_date,
        source: Source::Win32 {
            hive,
            key_path,
            key_name: key_name.to_string(),
        },
        install_dir,
        install_dir_trust: trust,
        icon,
        estimated_size,
        uninstall: UninstallInfo {
            uninstall: uninstall_string,
            quiet_uninstall: quiet,
            msi_product_code: msi,
            inno_setup: inno,
        },
        aliases,
        evidence_files,
    })
}

fn win32_apps(ctx: &Context) -> Vec<AppInfo> {
    let mut out = Vec::new();
    for (hive, root) in UNINSTALL_ROOTS {
        let Ok(key) = reg::open(*hive, root) else {
            continue;
        };
        for name in reg::subkeys(&key) {
            if let Some(app) = read_win32(*hive, root, &name, ctx) {
                out.push(app);
            }
        }
    }
    out
}

fn store_apps() -> Vec<AppInfo> {
    store::list()
        .into_iter()
        .map(|s| {
            let aliases = AppAliases::new(&s.display_name, &s.publisher, &[]);
            AppInfo {
                name: s.display_name,
                publisher: s.publisher,
                version: s.version,
                install_date: s.install_date,
                source: Source::Store {
                    full_name: s.full_name,
                    family_name: s.family_name,
                },
                install_dir: (!s.install_path.is_empty()).then_some(s.install_path),
                install_dir_trust: DirTrust::Verified,
                icon: s.logo.map(IconSource::Png),
                estimated_size: 0,
                uninstall: UninstallInfo::default(),
                aliases,
                evidence_files: Vec::new(),
            }
        })
        .collect()
}

/// 설치된 앱 전체(데스크톱 + Store). 같은 이름·버전·게시자 중복은 하나로 합친다.
pub fn enumerate(ctx: &Context) -> Vec<AppInfo> {
    let (win32, store) = rayon::join(|| win32_apps(ctx), store_apps);
    let mut seen: HashSet<(String, String, String)> = HashSet::new();
    let mut out: Vec<AppInfo> = Vec::with_capacity(win32.len() + store.len());
    // 제거 정보가 풍부한 항목이 먼저 오도록: 설치 폴더 있는 항목 우선.
    let mut win32 = win32;
    win32.sort_by_key(|a| {
        (
            a.install_dir.is_none(),
            a.uninstall.interactive_command().is_none(),
        )
    });
    for app in win32.into_iter().chain(store) {
        let key = (
            app.name.to_lowercase(),
            app.version.to_lowercase(),
            app.publisher.to_lowercase(),
        );
        if seen.insert(key) {
            out.push(app);
        }
    }
    out
}

/// 제거 후에도 앱이 여전히 설치된 상태로 남아 있는지.
pub fn still_installed(app: &AppInfo) -> bool {
    match &app.source {
        Source::Win32 { hive, key_path, .. } => reg::key_exists(*hive, key_path),
        Source::Store { family_name, .. } => store::is_installed(family_name),
    }
}
