//! Windows 전용 시스템 계층. Win32 / WinRT 호출은 모두 이 모듈 아래에 모아 둔다.

pub mod fsops;
pub mod icons;
pub mod job;
pub mod known;
pub mod procs;
pub mod reg;
pub mod restore;
pub mod services;
pub mod shell;
pub mod store;
pub mod tasks;

use std::ffi::c_void;
use std::mem::size_of;
use std::os::windows::process::CommandExt;
use std::process::Command;

use windows::Win32::Foundation::{CloseHandle, HANDLE};
use windows::Win32::Security::{GetTokenInformation, TOKEN_ELEVATION, TOKEN_QUERY, TokenElevation};
use windows::Win32::System::Com::{COINIT_MULTITHREADED, CoInitializeEx, CoUninitialize};
use windows::Win32::System::Environment::ExpandEnvironmentStringsW;
use windows::Win32::System::SystemInformation::GetLocalTime;
use windows::Win32::System::Threading::{GetCurrentProcess, OpenProcessToken};
use windows::Win32::UI::Shell::ShellExecuteW;
use windows::Win32::UI::WindowsAndMessaging::{
    MB_ICONERROR, MB_ICONWARNING, MB_OK, MB_SETFOREGROUND, MessageBoxW, SW_SHOWNORMAL,
};
use windows::core::PCWSTR;

/// 콘솔 창을 띄우지 않고 자식 프로세스를 실행하는 생성 플래그.
pub const CREATE_NO_WINDOW: u32 = 0x0800_0000;

/// NUL 종료 UTF-16 버퍼.
pub fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

/// NUL 종료(또는 전체) UTF-16 버퍼를 문자열로.
pub fn from_wide(buf: &[u16]) -> String {
    let n = buf.iter().position(|&c| c == 0).unwrap_or(buf.len());
    String::from_utf16_lossy(&buf[..n])
}

/// 소유한 Win32 HANDLE — drop 시 닫는다.
pub struct OwnedHandle(pub HANDLE);

impl OwnedHandle {
    pub fn raw(&self) -> HANDLE {
        self.0
    }
}

impl Drop for OwnedHandle {
    fn drop(&mut self) {
        if !self.0.is_invalid() && !self.0.0.is_null() {
            // SAFETY: 이 구조체가 유일한 소유자이며 한 번만 닫는다.
            unsafe {
                let _ = CloseHandle(self.0);
            }
        }
    }
}

// SAFETY: 커널 HANDLE은 스레드 간 이동이 안전하다.
unsafe impl Send for OwnedHandle {}

/// 현재 스레드에 COM(MTA)을 초기화하고, drop 시 해제한다.
pub struct ComGuard {
    initialized: bool,
}

impl ComGuard {
    pub fn mta() -> Self {
        // SAFETY: 짝이 맞는 CoUninitialize는 Drop에서 호출한다.
        let hr = unsafe { CoInitializeEx(None, COINIT_MULTITHREADED) };
        Self {
            initialized: hr.is_ok(),
        }
    }
}

impl Drop for ComGuard {
    fn drop(&mut self) {
        if self.initialized {
            // SAFETY: 성공한 CoInitializeEx와 짝을 이룬다.
            unsafe { CoUninitialize() };
        }
    }
}

/// `%SystemRoot%` 같은 환경 변수를 확장한다.
pub fn expand_env(s: &str) -> String {
    if !s.contains('%') {
        return s.to_string();
    }
    let src = wide(s);
    // SAFETY: src는 NUL 종료 버퍼이며, 출력 버퍼 길이를 정확히 전달한다.
    unsafe {
        let needed = ExpandEnvironmentStringsW(PCWSTR(src.as_ptr()), None);
        if needed == 0 {
            return s.to_string();
        }
        let mut buf = vec![0u16; needed as usize + 1];
        let n = ExpandEnvironmentStringsW(PCWSTR(src.as_ptr()), Some(&mut buf));
        if n == 0 || n as usize > buf.len() {
            return s.to_string();
        }
        from_wide(&buf)
    }
}

/// 현재 프로세스가 관리자 권한(상승된 토큰)으로 실행 중인지.
pub fn is_elevated() -> bool {
    // SAFETY: 토큰 핸들은 OwnedHandle이 닫고, 출력 구조체 크기를 정확히 전달한다.
    unsafe {
        let mut token = HANDLE::default();
        if OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut token).is_err() {
            return false;
        }
        let token = OwnedHandle(token);
        let mut elevation = TOKEN_ELEVATION::default();
        let mut len = 0u32;
        GetTokenInformation(
            token.raw(),
            TokenElevation,
            Some(&mut elevation as *mut _ as *mut c_void),
            size_of::<TOKEN_ELEVATION>() as u32,
            &mut len,
        )
        .is_ok()
            && elevation.TokenIsElevated != 0
    }
}

/// Windows 빌드 번호(예: 22631). 읽지 못하면 0.
pub fn windows_build() -> u32 {
    let key = reg::open(
        crate::safety::Hive::LocalMachine,
        r"SOFTWARE\Microsoft\Windows NT\CurrentVersion",
    );
    key.ok()
        .and_then(|k| k.get_value::<String, _>("CurrentBuildNumber").ok())
        .and_then(|s| s.trim().parse().ok())
        .unwrap_or(0)
}

/// 현지 시각 `YYYYMMDD-HHMMSS`.
pub fn local_timestamp() -> String {
    // SAFETY: 인자가 없는 단순 조회.
    let t = unsafe { GetLocalTime() };
    format!(
        "{:04}{:02}{:02}-{:02}{:02}{:02}",
        t.wYear, t.wMonth, t.wDay, t.wHour, t.wMinute, t.wSecond
    )
}

/// UI가 뜨기 전 치명적 오류를 알리는 메시지 상자.
pub fn message_box(title: &str, text: &str, warning: bool) {
    let t = wide(title);
    let m = wide(text);
    let icon = if warning {
        MB_ICONWARNING
    } else {
        MB_ICONERROR
    };
    // SAFETY: 두 버퍼 모두 NUL 종료이며 호출 동안 살아 있다.
    unsafe {
        MessageBoxW(
            None,
            PCWSTR(m.as_ptr()),
            PCWSTR(t.as_ptr()),
            MB_OK | icon | MB_SETFOREGROUND,
        );
    }
}

/// 자신을 관리자 권한으로 다시 실행한다(UAC). 성공하면 true.
pub fn relaunch_as_admin() -> bool {
    let Ok(exe) = std::env::current_exe() else {
        return false;
    };
    let verb = wide("runas");
    let file = wide(&exe.to_string_lossy());
    // SAFETY: 모든 문자열 버퍼가 NUL 종료이며 호출 동안 살아 있다.
    let r = unsafe {
        ShellExecuteW(
            None,
            PCWSTR(verb.as_ptr()),
            PCWSTR(file.as_ptr()),
            PCWSTR::null(),
            PCWSTR::null(),
            SW_SHOWNORMAL,
        )
    };
    r.0 as isize > 32
}

/// 탐색기로 폴더를 연다.
pub fn open_in_explorer(path: &str) {
    let _ = Command::new("explorer.exe").arg(path).spawn();
}

/// 콘솔 창 없이 명령을 실행하고 성공 여부와 출력을 돌려준다.
pub fn run_hidden(program: &str, args: &[&str]) -> Result<String, String> {
    let out = Command::new(program)
        .args(args)
        .creation_flags(CREATE_NO_WINDOW)
        .output()
        .map_err(|e| format!("{program} 실행 실패: {e}"))?;
    let text = oem_to_string(&out.stdout) + &oem_to_string(&out.stderr);
    if out.status.success() {
        Ok(text)
    } else {
        Err(text.trim().to_string())
    }
}

/// 콘솔 프로그램 출력(OEM 코드 페이지, 예: CP949)을 문자열로 변환한다.
pub fn oem_to_string(bytes: &[u8]) -> String {
    use windows::Win32::Globalization::{
        CP_OEMCP, MULTI_BYTE_TO_WIDE_CHAR_FLAGS, MultiByteToWideChar,
    };
    if bytes.is_empty() {
        return String::new();
    }
    // SAFETY: 입력/출력 슬라이스 길이를 API가 직접 받는다.
    unsafe {
        let n = MultiByteToWideChar(CP_OEMCP, MULTI_BYTE_TO_WIDE_CHAR_FLAGS(0), bytes, None);
        if n <= 0 {
            return String::from_utf8_lossy(bytes).into_owned();
        }
        let mut buf = vec![0u16; n as usize];
        let n = MultiByteToWideChar(
            CP_OEMCP,
            MULTI_BYTE_TO_WIDE_CHAR_FLAGS(0),
            bytes,
            Some(&mut buf),
        );
        String::from_utf16_lossy(&buf[..n.max(0) as usize])
    }
}

#[cfg(test)]
mod tests;
