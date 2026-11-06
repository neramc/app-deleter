//! 실행 중인 프로세스 조회/종료.

use std::mem::size_of;

use windows::Win32::System::Diagnostics::ToolHelp::{
    CreateToolhelp32Snapshot, PROCESSENTRY32W, Process32FirstW, Process32NextW, TH32CS_SNAPPROCESS,
};
use windows::Win32::System::Threading::{
    OpenProcess, PROCESS_NAME_WIN32, PROCESS_QUERY_LIMITED_INFORMATION, PROCESS_TERMINATE,
    QueryFullProcessImageNameW, TerminateProcess,
};
use windows::core::PWSTR;

use crate::win::{OwnedHandle, from_wide};

/// 프로세스 실행 파일 전체 경로.
pub fn image_path(pid: u32) -> Option<String> {
    // SAFETY: 조회 전용 권한으로 열고 OwnedHandle이 닫는다.
    unsafe {
        let h = OwnedHandle(OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid).ok()?);
        let mut buf = vec![0u16; 32768];
        let mut len = buf.len() as u32;
        QueryFullProcessImageNameW(
            h.raw(),
            PROCESS_NAME_WIN32,
            PWSTR(buf.as_mut_ptr()),
            &mut len,
        )
        .ok()?;
        Some(String::from_utf16_lossy(&buf[..len as usize]))
    }
}

/// (PID, 실행 파일 경로) 목록.
pub fn list() -> Vec<(u32, String)> {
    let mut out = Vec::new();
    // SAFETY: 스냅샷 핸들은 OwnedHandle이 닫는다.
    unsafe {
        let Ok(snap) = CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0) else {
            return out;
        };
        let snap = OwnedHandle(snap);
        let mut entry = PROCESSENTRY32W {
            dwSize: size_of::<PROCESSENTRY32W>() as u32,
            ..Default::default()
        };
        if Process32FirstW(snap.raw(), &mut entry).is_err() {
            return out;
        }
        loop {
            let pid = entry.th32ProcessID;
            if pid > 4 {
                let path = image_path(pid).unwrap_or_else(|| from_wide(&entry.szExeFile));
                out.push((pid, path));
            }
            if Process32NextW(snap.raw(), &mut entry).is_err() {
                break;
            }
        }
    }
    out
}

/// 프로세스를 강제 종료한다.
pub fn terminate(pid: u32) -> bool {
    if pid == std::process::id() {
        return false;
    }
    // SAFETY: 종료 권한으로 열고 OwnedHandle이 닫는다.
    unsafe {
        let Ok(h) = OpenProcess(PROCESS_TERMINATE, false, pid) else {
            return false;
        };
        let h = OwnedHandle(h);
        TerminateProcess(h.raw(), 1).is_ok()
    }
}
