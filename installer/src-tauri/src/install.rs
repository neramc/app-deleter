use serde::Serialize;

/// 설치 진행 상황 이벤트(`install-progress`) payload.
#[derive(Clone, Serialize)]
pub struct Progress {
    pub step: u32,
    pub total: u32,
    pub message: String,
}

/// 기본 설치 경로(ASCII)를 반환한다: %LOCALAPPDATA%\AppDeleter
#[tauri::command]
pub fn default_install_dir() -> String {
    #[cfg(windows)]
    {
        let base = std::env::var("LOCALAPPDATA")
            .unwrap_or_else(|_| r"C:\Users\Public".to_string());
        format!(r"{base}\AppDeleter")
    }
    #[cfg(not(windows))]
    {
        "/tmp/AppDeleter".to_string()
    }
}

/// 메인 앱을 설치한다: 파일 복사 → 바로가기 생성 → 제거 정보 등록.
#[tauri::command]
pub fn install(window: tauri::Window, install_dir: String) -> Result<(), String> {
    #[cfg(windows)]
    {
        windows_impl::install(window, &install_dir)
    }
    #[cfg(not(windows))]
    {
        let _ = (window, install_dir);
        Err("설치는 Windows에서만 지원됩니다.".into())
    }
}

/// 설치된 메인 앱을 실행한다(관리자 권한이 필요하므로 UAC 프롬프트가 뜬다).
#[tauri::command]
pub fn launch_app(install_dir: String) -> Result<(), String> {
    #[cfg(windows)]
    {
        windows_impl::launch_app(&install_dir)
    }
    #[cfg(not(windows))]
    {
        let _ = install_dir;
        Err("실행은 Windows에서만 지원됩니다.".into())
    }
}

/// 설치 마법사를 종료한다.
#[tauri::command]
pub fn exit_installer(app: tauri::AppHandle) {
    app.exit(0);
}

#[cfg(windows)]
mod windows_impl {
    use super::Progress;
    use std::fs;
    use std::path::{Path, PathBuf};
    use std::process::Command;
    use tauri::Emitter;
    use winreg::enums::*;
    use winreg::RegKey;

    // 컴파일 타임에 임베드되는 페이로드(메인 앱 exe + 제거 마법사 exe + 아이콘).
    static APP_EXE: &[u8] =
        include_bytes!(concat!(env!("CARGO_MANIFEST_DIR"), "/payload/app-deleter.exe"));
    static UNINSTALLER_EXE: &[u8] = include_bytes!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/payload/app-deleter-uninstaller.exe"
    ));
    static APP_ICON: &[u8] =
        include_bytes!(concat!(env!("CARGO_MANIFEST_DIR"), "/icons/icon.ico"));

    const UNINSTALL_KEY: &str =
        r"Software\Microsoft\Windows\CurrentVersion\Uninstall\AppDeleter";
    const TOTAL_STEPS: u32 = 5;

    fn emit(window: &tauri::Window, step: u32, message: &str) {
        let _ = window.emit(
            "install-progress",
            Progress {
                step,
                total: TOTAL_STEPS,
                message: message.to_string(),
            },
        );
    }

    pub fn install(window: tauri::Window, install_dir: &str) -> Result<(), String> {
        if APP_EXE.is_empty() {
            return Err(
                "임베드된 앱 페이로드가 비어 있습니다. CI 빌드에서만 정상 동작합니다.".into(),
            );
        }

        let dir = PathBuf::from(install_dir);

        emit(&window, 1, "설치 폴더 생성 중…");
        fs::create_dir_all(&dir).map_err(|e| format!("폴더 생성 실패: {e}"))?;

        emit(&window, 2, "앱 파일 복사 중…");
        let exe_path = dir.join("app-deleter.exe");
        fs::write(&exe_path, APP_EXE).map_err(|e| format!("앱 파일 쓰기 실패: {e}"))?;
        let icon_path = dir.join("icon.ico");
        fs::write(&icon_path, APP_ICON).map_err(|e| format!("아이콘 쓰기 실패: {e}"))?;
        // 제거 마법사도 함께 설치한다(제어판 "프로그램 제거"가 이 exe를 호출).
        let uninstaller_path = dir.join("uninstall.exe");
        fs::write(&uninstaller_path, UNINSTALLER_EXE)
            .map_err(|e| format!("제거 마법사 쓰기 실패: {e}"))?;

        emit(&window, 3, "바로가기 생성 중…");
        let (start_menu_lnk, desktop_lnk) = shortcut_paths();
        create_shortcuts(&exe_path, &icon_path, &start_menu_lnk, &desktop_lnk)?;

        emit(&window, 4, "제거 정보 등록 중…");
        write_uninstall_entry(&dir, &uninstaller_path, &icon_path)?;

        emit(&window, 5, "설치 완료");
        Ok(())
    }

    pub fn launch_app(install_dir: &str) -> Result<(), String> {
        let exe = PathBuf::from(install_dir).join("app-deleter.exe");
        // 메인 앱은 관리자 권한을 요구하므로 ShellExecute(Start-Process)로 실행해
        // UAC 상승 프롬프트가 정상적으로 뜨게 한다.
        let script = format!("Start-Process -FilePath '{}'", exe.display());
        Command::new("powershell")
            .args(["-NoProfile", "-NonInteractive", "-Command", &script])
            .spawn()
            .map(|_| ())
            .map_err(|e| format!("앱 실행 실패: {e}"))
    }

    /// 시작 메뉴 / 바탕화면 바로가기(.lnk)의 전체 경로를 계산한다.
    fn shortcut_paths() -> (PathBuf, PathBuf) {
        let appdata = std::env::var("APPDATA").unwrap_or_default();
        let userprofile = std::env::var("USERPROFILE").unwrap_or_default();
        let start_menu = PathBuf::from(appdata)
            .join(r"Microsoft\Windows\Start Menu\Programs")
            .join("App Deleter.lnk");
        let desktop = PathBuf::from(userprofile)
            .join("Desktop")
            .join("App Deleter.lnk");
        (start_menu, desktop)
    }

    /// WScript.Shell COM을 통해 바로가기 두 개를 생성한다(PowerShell 스크립트).
    fn create_shortcuts(
        exe: &Path,
        icon: &Path,
        start_menu: &Path,
        desktop: &Path,
    ) -> Result<(), String> {
        let workdir = exe.parent().unwrap_or(Path::new("."));
        let script = format!(
            "$ws = New-Object -ComObject WScript.Shell\n\
             foreach ($lnk in @(\"{sm}\", \"{dt}\")) {{\n\
             $s = $ws.CreateShortcut($lnk)\n\
             $s.TargetPath = \"{exe}\"\n\
             $s.WorkingDirectory = \"{wd}\"\n\
             $s.IconLocation = \"{icon}\"\n\
             $s.Description = \"App Deleter\"\n\
             $s.Save()\n\
             }}",
            sm = start_menu.display(),
            dt = desktop.display(),
            exe = exe.display(),
            wd = workdir.display(),
            icon = icon.display(),
        );
        run_powershell_file(&script).map_err(|e| format!("바로가기 생성 실패: {e}"))
    }

    /// 제어판 "프로그램 제거" 목록에 표시될 제거 정보를 HKCU에 기록한다.
    /// UninstallString은 함께 설치한 제거 마법사(uninstall.exe)를 가리킨다.
    fn write_uninstall_entry(
        dir: &Path,
        uninstaller: &Path,
        icon: &Path,
    ) -> Result<(), String> {
        let hkcu = RegKey::predef(HKEY_CURRENT_USER);
        let (key, _) = hkcu
            .create_subkey(UNINSTALL_KEY)
            .map_err(|e| format!("레지스트리 키 생성 실패: {e}"))?;

        let set = |name: &str, value: &str| -> Result<(), String> {
            key.set_value(name, &value.to_string())
                .map_err(|e| format!("레지스트리 값 쓰기 실패({name}): {e}"))
        };

        set("DisplayName", "App Deleter")?;
        set("DisplayVersion", env!("CARGO_PKG_VERSION"))?;
        set("Publisher", "neramc")?;
        set("InstallLocation", &dir.display().to_string())?;
        set("DisplayIcon", &icon.display().to_string())?;
        key.set_value("NoModify", &1u32).ok();
        key.set_value("NoRepair", &1u32).ok();
        key.set_value("EstimatedSize", &((APP_EXE.len() / 1024) as u32)).ok();

        set("UninstallString", &format!("\"{}\"", uninstaller.display()))?;
        Ok(())
    }

    /// 스크립트를 임시 .ps1 파일로 써서 실행한다(인라인 따옴표 이스케이프 회피).
    fn run_powershell_file(script: &str) -> Result<(), String> {
        let mut path = std::env::temp_dir();
        path.push(format!("appdeleter_setup_{}.ps1", std::process::id()));
        fs::write(&path, script).map_err(|e| format!("스크립트 파일 쓰기 실패: {e}"))?;

        let status = Command::new("powershell")
            .args([
                "-NoProfile",
                "-NonInteractive",
                "-ExecutionPolicy",
                "Bypass",
                "-File",
            ])
            .arg(&path)
            .status();
        let _ = fs::remove_file(&path);

        match status {
            Ok(s) if s.success() => Ok(()),
            Ok(s) => Err(format!("PowerShell 종료 코드 {s}")),
            Err(e) => Err(format!("PowerShell 실행 실패: {e}")),
        }
    }
}
