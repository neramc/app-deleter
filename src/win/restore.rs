//! 시스템 복원 지점 생성 (`SRSetRestorePointW`).

use windows::Win32::Foundation::ERROR_SUCCESS;
use windows::Win32::System::Restore::{
    APPLICATION_UNINSTALL, BEGIN_SYSTEM_CHANGE, END_SYSTEM_CHANGE, RESTOREPOINTINFOW,
    SRSetRestorePointW, STATEMGRSTATUS,
};

/// 열린 복원 지점(작업 종료 시 `end`로 닫아야 한다).
#[derive(Debug, Clone, Copy)]
pub struct RestorePoint {
    sequence: i64,
}

/// 앱 제거용 복원 지점을 시작한다. 수십 초가 걸릴 수 있다.
pub fn begin(description: &str) -> Result<RestorePoint, String> {
    // RESTOREPOINTINFOW는 packed 구조체라 필드 참조 대신 값 전체를 만들어 대입한다.
    let mut desc = [0u16; 256];
    for (dst, src) in desc.iter_mut().zip(description.encode_utf16().take(63)) {
        *dst = src;
    }
    let info = RESTOREPOINTINFOW {
        dwEventType: BEGIN_SYSTEM_CHANGE,
        dwRestorePtType: APPLICATION_UNINSTALL,
        llSequenceNumber: 0,
        szDescription: desc,
    };
    let mut status = STATEMGRSTATUS::default();
    // SAFETY: 두 구조체 모두 올바르게 초기화되어 있다.
    let ok = unsafe { SRSetRestorePointW(&info, &mut status) }.as_bool();
    let code = { status.nStatus };
    let sequence = { status.llSequenceNumber };
    if ok && code == ERROR_SUCCESS {
        Ok(RestorePoint { sequence })
    } else {
        Err(match code.0 {
            1058 => "시스템 보호가 꺼져 있어 복원 지점을 만들 수 없습니다.".to_string(),
            code => format!("복원 지점 생성 실패(오류 {code})"),
        })
    }
}

impl RestorePoint {
    pub fn end(self) {
        let info = RESTOREPOINTINFOW {
            dwEventType: END_SYSTEM_CHANGE,
            dwRestorePtType: APPLICATION_UNINSTALL,
            llSequenceNumber: self.sequence,
            ..Default::default()
        };
        let mut status = STATEMGRSTATUS::default();
        // SAFETY: begin에서 받은 시퀀스 번호로 닫는다.
        unsafe {
            let _ = SRSetRestorePointW(&info, &mut status);
        }
    }
}
