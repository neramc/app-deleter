//! 제거 프로그램 실행 + 완료 대기 (Job Object 기반).
//!
//! 많은 제거 프로그램(Inno Setup, NSIS 등)은 자신을 임시 폴더로 복사해 자식 프로세스로
//! 다시 실행한 뒤 원래 프로세스는 바로 종료한다. 루트 프로세스만 기다리면 제거가 끝나기도
//! 전에 잔여물 검색을 시작하게 된다. 그래서 프로세스를 일시 정지 상태로 만들어 Job Object에
//! 넣고, **Job 안의 모든 프로세스가 끝날 때까지** 기다린다.

use std::ffi::c_void;
use std::mem::size_of;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use windows::Win32::Foundation::{
    ERROR_ELEVATION_REQUIRED, ERROR_FILE_NOT_FOUND, ERROR_PATH_NOT_FOUND, WAIT_OBJECT_0,
};
use windows::Win32::System::JobObjects::{
    AssignProcessToJobObject, CreateJobObjectW, JobObjectBasicProcessIdList,
    QueryInformationJobObject,
};
use windows::Win32::System::Threading::{
    CREATE_SUSPENDED, CreateProcessW, GetExitCodeProcess, PROCESS_INFORMATION, ResumeThread,
    STARTUPINFOW, WaitForSingleObject,
};
use windows::Win32::UI::Shell::{SEE_MASK_NOCLOSEPROCESS, SHELLEXECUTEINFOW, ShellExecuteExW};
use windows::Win32::UI::WindowsAndMessaging::SW_SHOWNORMAL;
use windows::core::{HRESULT, PCWSTR, PWSTR};

use crate::cmdline::split_command;
use crate::pathutil;
use crate::win::{OwnedHandle, expand_env, procs, wide};

/// 실행 결과.
#[derive(Debug)]
pub enum RunEnd {
    /// 제거 프로그램(과 그 자식들)이 모두 종료됨.
    Completed { exit_code: Option<u32> },
    /// 사용자가 기다리지 않고 다음 단계로 넘어감.
    Skipped,
}

#[derive(Debug)]
pub enum RunError {
    /// 제거 프로그램 실행 파일이 없음 → 강제 제거로 전환 가능.
    NotFound(String),
    Failed(String),
}

/// 대기 중 남아 있어도 "제거 완료"로 간주하는 프로세스(제거 후 설문 페이지를 여는 브라우저 등).
const IGNORABLE: &[&str] = &[
    "msedge.exe",
    "msedgewebview2.exe",
    "chrome.exe",
    "firefox.exe",
    "whale.exe",
    "opera.exe",
    "brave.exe",
    "vivaldi.exe",
    "iexplore.exe",
    "explorer.exe",
    "conhost.exe",
    "werfault.exe",
];

#[repr(C)]
struct PidList {
    assigned: u32,
    in_list: u32,
    ids: [usize; 512],
}

fn job_pids(job: &OwnedHandle) -> Option<Vec<u32>> {
    let mut list = Box::new(PidList {
        assigned: 0,
        in_list: 0,
        ids: [0; 512],
    });
    // SAFETY: 출력 버퍼 크기를 정확히 전달한다.
    let r = unsafe {
        QueryInformationJobObject(
            Some(job.raw()),
            JobObjectBasicProcessIdList,
            &mut *list as *mut PidList as *mut c_void,
            size_of::<PidList>() as u32,
            None,
        )
    };
    r.ok()?;
    let n = (list.in_list as usize).min(list.ids.len());
    Some(list.ids[..n].iter().map(|&p| p as u32).collect())
}

fn all_ignorable(pids: &[u32]) -> bool {
    !pids.is_empty()
        && pids.iter().all(|&pid| {
            procs::image_path(pid)
                .map(|p| IGNORABLE.contains(&pathutil::file_name(&p).to_ascii_lowercase().as_str()))
                .unwrap_or(false)
        })
}

/// 명령을 실행하고 (자식 포함) 끝날 때까지 기다린다.
///
/// `skip`이 true가 되면 즉시 `Skipped`를 반환한다. `tick`은 약 0.25초마다 호출된다.
pub fn run_and_wait(
    command: &str,
    skip: &AtomicBool,
    mut tick: impl FnMut(Duration),
) -> Result<RunEnd, RunError> {
    let cmd = expand_env(command.trim());
    if cmd.is_empty() {
        return Err(RunError::NotFound(String::new()));
    }
    let (exe, args) = split_command(&cmd);
    let exe = expand_env(&exe);
    if pathutil::is_absolute(&exe) && !std::path::Path::new(&exe).exists() {
        return Err(RunError::NotFound(exe));
    }

    // SAFETY: 이름 없는 Job 생성.
    let job = unsafe { CreateJobObjectW(None, PCWSTR::null()) }
        .map(OwnedHandle)
        .map_err(|e| RunError::Failed(format!("Job 생성 실패: {}", e.message())))?;

    let mut cmdline = wide(&cmd);
    let si = STARTUPINFOW {
        cb: size_of::<STARTUPINFOW>() as u32,
        ..Default::default()
    };
    let mut pi = PROCESS_INFORMATION::default();
    // SAFETY: cmdline은 쓰기 가능한 NUL 종료 버퍼, si/pi는 올바른 크기의 구조체.
    let created = unsafe {
        CreateProcessW(
            PCWSTR::null(),
            Some(PWSTR(cmdline.as_mut_ptr())),
            None,
            None,
            false,
            CREATE_SUSPENDED,
            None,
            PCWSTR::null(),
            &si,
            &mut pi,
        )
    };
    if let Err(e) = created {
        let code = e.code();
        if code == HRESULT::from_win32(ERROR_ELEVATION_REQUIRED.0) {
            return run_elevated(&exe, &args, skip, tick);
        }
        if code == HRESULT::from_win32(ERROR_FILE_NOT_FOUND.0)
            || code == HRESULT::from_win32(ERROR_PATH_NOT_FOUND.0)
        {
            return Err(RunError::NotFound(exe));
        }
        return Err(RunError::Failed(format!(
            "제거 프로그램 실행 실패: {}",
            e.message()
        )));
    }
    let process = OwnedHandle(pi.hProcess);
    let thread = OwnedHandle(pi.hThread);
    // SAFETY: 유효한 Job/프로세스 핸들. 실패하면 루트 프로세스만 기다린다.
    let in_job = unsafe { AssignProcessToJobObject(job.raw(), process.raw()) }.is_ok();
    // SAFETY: CREATE_SUSPENDED로 만든 주 스레드를 재개한다.
    unsafe {
        ResumeThread(thread.raw());
    }
    drop(thread);

    let started = Instant::now();
    let mut exit_code = None;
    loop {
        if skip.load(Ordering::Relaxed) {
            return Ok(RunEnd::Skipped);
        }
        // SAFETY: 유효한 프로세스 핸들, 0ms 대기(상태 조회).
        let root_done = unsafe { WaitForSingleObject(process.raw(), 0) } == WAIT_OBJECT_0;
        if root_done && exit_code.is_none() {
            let mut code = 0u32;
            // SAFETY: 종료된 프로세스의 종료 코드 조회.
            if unsafe { GetExitCodeProcess(process.raw(), &mut code) }.is_ok() {
                exit_code = Some(code);
            }
        }
        if in_job {
            match job_pids(&job) {
                Some(pids) if pids.is_empty() => break,
                Some(pids) if root_done && all_ignorable(&pids) => break,
                Some(_) => {}
                None if root_done => break,
                None => {}
            }
        } else if root_done {
            break;
        }
        tick(started.elapsed());
        std::thread::sleep(Duration::from_millis(250));
    }
    Ok(RunEnd::Completed { exit_code })
}

/// 상승이 필요한 경우(일반 권한으로 실행됐을 때) ShellExecuteEx("runas")로 실행한다.
fn run_elevated(
    exe: &str,
    args: &str,
    skip: &AtomicBool,
    mut tick: impl FnMut(Duration),
) -> Result<RunEnd, RunError> {
    let verb = wide("runas");
    let file = wide(exe);
    let params = wide(args);
    let mut info = SHELLEXECUTEINFOW {
        cbSize: size_of::<SHELLEXECUTEINFOW>() as u32,
        fMask: SEE_MASK_NOCLOSEPROCESS,
        lpVerb: PCWSTR(verb.as_ptr()),
        lpFile: PCWSTR(file.as_ptr()),
        lpParameters: PCWSTR(params.as_ptr()),
        nShow: SW_SHOWNORMAL.0,
        ..Default::default()
    };
    // SAFETY: info의 모든 문자열 버퍼는 호출 동안 유효하다.
    unsafe { ShellExecuteExW(&mut info) }
        .map_err(|e| RunError::Failed(format!("관리자 권한 실행 실패: {}", e.message())))?;
    if info.hProcess.is_invalid() {
        return Ok(RunEnd::Completed { exit_code: None });
    }
    let process = OwnedHandle(info.hProcess);
    let started = Instant::now();
    loop {
        if skip.load(Ordering::Relaxed) {
            return Ok(RunEnd::Skipped);
        }
        // SAFETY: 유효한 프로세스 핸들.
        if unsafe { WaitForSingleObject(process.raw(), 250) } == WAIT_OBJECT_0 {
            let mut code = 0u32;
            // SAFETY: 종료된 프로세스.
            let ok = unsafe { GetExitCodeProcess(process.raw(), &mut code) }.is_ok();
            return Ok(RunEnd::Completed {
                exit_code: ok.then_some(code),
            });
        }
        tick(started.elapsed());
    }
}
