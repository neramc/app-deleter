//! 고성능 디렉터리 용량 스캐너 (Windows 전용).
//!
//! `std::fs`/`walkdir`는 항목마다 별도의 메타데이터 조회를 유발하기 쉽다.
//! 이 모듈은 Win32 `FindFirstFileExW`를 다음 최적화와 함께 직접 사용한다:
//!   * `FindExInfoBasic`   — 8.3 단축 파일명 조회를 생략(I/O·CPU 절감)
//!   * `FIND_FIRST_EX_LARGE_FETCH` — 디렉터리 항목을 큰 배치로 읽음
//!   * 파일 크기를 `WIN32_FIND_DATAW`에서 즉시 취득 → 추가 stat 호출 0회
//!   * 명시적 스택 순회(재귀 X) + 리파스 포인트(심볼릭/정션) 스킵
//!   * 최상위 하위 폴더를 `rayon`으로 병렬 스캔
#![cfg(windows)]

use std::ffi::c_void;
use std::os::windows::ffi::OsStrExt;
use std::path::Path;

use rayon::prelude::*;
use windows::core::PCWSTR;
use windows::Win32::Foundation::HANDLE;
use windows::Win32::Storage::FileSystem::{
    FindClose, FindExInfoBasic, FindExSearchNameMatch, FindFirstFileExW, FindNextFileW,
    FILE_ATTRIBUTE_DIRECTORY, FILE_ATTRIBUTE_REPARSE_POINT, FIND_FIRST_EX_LARGE_FETCH,
    WIN32_FIND_DATAW,
};

const DOT: u16 = b'.' as u16;
const SEP: u16 = b'\\' as u16;
const STAR: u16 = b'*' as u16;

fn to_wide(path: &Path) -> Vec<u16> {
    path.as_os_str().encode_wide().collect()
}

/// `cFileName`(널 종단 고정 배열)에서 실제 이름 길이를 구한다.
fn name_len(name: &[u16]) -> usize {
    name.iter().position(|&c| c == 0).unwrap_or(name.len())
}

/// "." / ".." 항목 여부.
fn is_dot_entry(name: &[u16]) -> bool {
    name[0] == DOT && (name[1] == 0 || (name[1] == DOT && name[2] == 0))
}

/// 단일 디렉터리를 열거하여 직속 파일 크기를 `total`에 더하고,
/// 하위 디렉터리(리파스 포인트 제외)의 전체 경로를 `on_subdir`로 넘긴다.
fn enum_dir(dir: &[u16], total: &mut u64, mut on_subdir: impl FnMut(Vec<u16>)) {
    // 검색 패턴 "<dir>\*\0" 구성.
    let mut pattern = Vec::with_capacity(dir.len() + 3);
    pattern.extend_from_slice(dir);
    if pattern.last() != Some(&SEP) {
        pattern.push(SEP);
    }
    pattern.push(STAR);
    pattern.push(0);

    let mut data = WIN32_FIND_DATAW::default();
    let handle = unsafe {
        FindFirstFileExW(
            PCWSTR(pattern.as_ptr()),
            FindExInfoBasic,
            &mut data as *mut _ as *mut c_void,
            FindExSearchNameMatch,
            None,
            FIND_FIRST_EX_LARGE_FETCH,
        )
    };
    let handle: HANDLE = match handle {
        Ok(h) if !h.is_invalid() => h,
        _ => return,
    };

    loop {
        let name = &data.cFileName;
        if !is_dot_entry(name) {
            let attrs = data.dwFileAttributes;
            let is_reparse = attrs & FILE_ATTRIBUTE_REPARSE_POINT.0 != 0;
            if !is_reparse {
                if attrs & FILE_ATTRIBUTE_DIRECTORY.0 != 0 {
                    let len = name_len(name);
                    let mut child = Vec::with_capacity(dir.len() + 1 + len);
                    child.extend_from_slice(dir);
                    if child.last() != Some(&SEP) {
                        child.push(SEP);
                    }
                    child.extend_from_slice(&name[..len]);
                    on_subdir(child);
                } else {
                    *total += ((data.nFileSizeHigh as u64) << 32) | data.nFileSizeLow as u64;
                }
            }
        }
        if unsafe { FindNextFileW(handle, &mut data) }.is_err() {
            break; // ERROR_NO_MORE_FILES 등 → 종료
        }
    }
    unsafe {
        let _ = FindClose(handle);
    }
}

/// 하위 트리 전체 용량(순차, 명시적 스택).
fn seq_dir_size(start: Vec<u16>) -> u64 {
    let mut total = 0u64;
    let mut stack: Vec<Vec<u16>> = vec![start];
    while let Some(dir) = stack.pop() {
        enum_dir(&dir, &mut total, |sub| stack.push(sub));
    }
    total
}

/// 디렉터리 전체 용량(바이트). 최상위 하위 폴더를 rayon으로 병렬 스캔한다.
pub fn dir_size(root: &Path) -> u64 {
    let start = to_wide(root);
    let mut direct = 0u64;
    let mut subdirs: Vec<Vec<u16>> = Vec::new();
    enum_dir(&start, &mut direct, |sub| subdirs.push(sub));

    // 최상위 하위 폴더들을 병렬로 — 단일 거대 폴더도 코어를 활용한다.
    let sub_total: u64 = subdirs.into_par_iter().map(seq_dir_size).sum();
    direct + sub_total
}
