//! Windows 서비스 조회/중지/삭제.

use std::time::{Duration, Instant};

use windows::Win32::System::Services::{
    CloseServiceHandle, ControlService, DeleteService, OpenSCManagerW, OpenServiceW,
    QueryServiceStatus, SC_HANDLE, SERVICE_CONTROL_STOP, SERVICE_STATUS, SERVICE_STOPPED,
};
use windows::core::PCWSTR;

use crate::cmdline::split_command;
use crate::safety::Hive;
use crate::win::{expand_env, reg, wide};

const SERVICES_KEY: &str = r"SYSTEM\CurrentControlSet\Services";

const SC_MANAGER_CONNECT: u32 = 0x0001;
const SERVICE_QUERY_STATUS: u32 = 0x0004;
const SERVICE_STOP: u32 = 0x0020;
const DELETE: u32 = 0x0001_0000;

#[derive(Debug, Clone)]
pub struct ServiceInfo {
    pub name: String,
    pub display_name: String,
    /// 확장·정규화된 실행 파일 경로.
    pub image: String,
}

pub fn services_key_path(name: &str) -> String {
    format!(r"{SERVICES_KEY}\{name}")
}

/// 레지스트리에서 서비스/드라이버 목록과 실행 파일 경로를 읽는다.
pub fn list() -> Vec<ServiceInfo> {
    let Ok(root) = reg::open(Hive::LocalMachine, SERVICES_KEY) else {
        return Vec::new();
    };
    reg::subkeys(&root)
        .into_iter()
        .filter_map(|name| {
            let key = root.open_subkey(&name).ok()?;
            let raw = reg::get_string(&key, "ImagePath")?;
            let (exe, _) = split_command(&raw);
            let mut image = expand_env(&exe);
            if let Some(rest) = image.strip_prefix(r"\??\") {
                image = rest.to_string();
            }
            let display_name = reg::get_string(&key, "DisplayName")
                .filter(|d| !d.starts_with('@'))
                .unwrap_or_else(|| name.clone());
            Some(ServiceInfo {
                name,
                display_name,
                image,
            })
        })
        .collect()
}

struct ScHandle(SC_HANDLE);

impl Drop for ScHandle {
    fn drop(&mut self) {
        // SAFETY: OpenSCManager/OpenService가 돌려준 핸들을 한 번만 닫는다.
        unsafe {
            let _ = CloseServiceHandle(self.0);
        }
    }
}

/// 서비스를 중지(최대 15초 대기)한 뒤 삭제 표시한다.
pub fn stop_and_delete(name: &str) -> Result<(), String> {
    let wname = wide(name);
    // SAFETY: 모든 핸들은 ScHandle이 닫는다.
    unsafe {
        let scm = ScHandle(
            OpenSCManagerW(PCWSTR::null(), PCWSTR::null(), SC_MANAGER_CONNECT)
                .map_err(|e| format!("서비스 관리자 열기 실패: {}", e.message()))?,
        );
        let svc = ScHandle(
            OpenServiceW(
                scm.0,
                PCWSTR(wname.as_ptr()),
                SERVICE_STOP | SERVICE_QUERY_STATUS | DELETE,
            )
            .map_err(|e| format!("서비스 열기 실패: {}", e.message()))?,
        );
        let mut status = SERVICE_STATUS::default();
        if QueryServiceStatus(svc.0, &mut status).is_ok()
            && status.dwCurrentState != SERVICE_STOPPED
        {
            let _ = ControlService(svc.0, SERVICE_CONTROL_STOP, &mut status);
            let start = Instant::now();
            while start.elapsed() < Duration::from_secs(15) {
                if QueryServiceStatus(svc.0, &mut status).is_err()
                    || status.dwCurrentState == SERVICE_STOPPED
                {
                    break;
                }
                std::thread::sleep(Duration::from_millis(250));
            }
        }
        DeleteService(svc.0).map_err(|e| format!("서비스 삭제 실패: {}", e.message()))
    }
}
