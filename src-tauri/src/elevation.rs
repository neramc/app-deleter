//! 관리자 권한(UAC) 상승 관련 유틸 (Windows 전용).
//!
//! 앱은 기본적으로 일반 권한(asInvoker)으로 실행되어 시작 시 UAC를 띄우지
//! 않는다. 목록 조회·용량 스캔은 일반 권한으로 충분하다. 실제 삭제(언인스톨)는
//! 관리자 권한이 필요하므로, **최초 삭제 시도 시 한 번만** 자신을 상승된 권한으로
//! 재실행한다. 이후 같은 세션의 삭제는 추가 프롬프트 없이 진행된다.
#![cfg(windows)]

use std::ffi::c_void;
use std::mem::size_of;
use std::os::windows::ffi::OsStrExt;

use windows::core::PCWSTR;
use windows::Win32::Foundation::{CloseHandle, HANDLE};
use windows::Win32::Security::{GetTokenInformation, TokenElevation, TOKEN_ELEVATION, TOKEN_QUERY};
use windows::Win32::System::Threading::{GetCurrentProcess, OpenProcessToken};
use windows::Win32::UI::Shell::ShellExecuteW;
use windows::Win32::UI::WindowsAndMessaging::SW_SHOWNORMAL;

/// 현재 프로세스가 관리자 권한으로 상승되어 실행 중인지 여부.
pub fn is_elevated() -> bool {
    unsafe {
        let mut token = HANDLE::default();
        if OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut token).is_err() {
            return false;
        }
        let mut elevation = TOKEN_ELEVATION::default();
        let mut ret_len = 0u32;
        let ok = GetTokenInformation(
            token,
            TokenElevation,
            Some(&mut elevation as *mut _ as *mut c_void),
            size_of::<TOKEN_ELEVATION>() as u32,
            &mut ret_len,
        );
        let _ = CloseHandle(token);
        ok.is_ok() && elevation.TokenIsElevated != 0
    }
}

/// 현재 실행 파일을 관리자 권한으로 재실행한다(UAC 프롬프트 1회).
/// 성공하면 호출 측에서 현재(비상승) 인스턴스를 종료해야 한다.
pub fn relaunch_as_admin() -> Result<(), String> {
    let exe = std::env::current_exe().map_err(|e| format!("실행 경로 조회 실패: {e}"))?;
    let verb: Vec<u16> = "runas\0".encode_utf16().collect();
    let file: Vec<u16> = exe.as_os_str().encode_wide().chain(std::iter::once(0)).collect();

    let result = unsafe {
        ShellExecuteW(
            None,
            PCWSTR(verb.as_ptr()),
            PCWSTR(file.as_ptr()),
            PCWSTR::null(),
            PCWSTR::null(),
            SW_SHOWNORMAL,
        )
    };
    // ShellExecuteW: 반환 HINSTANCE 값이 32 초과면 성공. 사용자가 UAC를 취소하면 실패.
    if result.0 as isize > 32 {
        Ok(())
    } else {
        Err("권한 상승이 취소되었거나 실패했습니다.".into())
    }
}
