//! 고성능 파일 시스템 작업: 폴더 용량 계산, 안전한 트리 삭제, 휴지통 이동.
//!
//! - `FindFirstFileExW` + `FindExInfoBasic`(8.3 이름 생략) + `FIND_FIRST_EX_LARGE_FETCH`(배치 읽기)로
//!   항목당 추가 stat 호출 없이 크기·속성을 얻는다.
//! - 하위 폴더는 rayon 작업 훔치기로 병렬 처리한다.
//! - 재분석 지점(정션/심볼릭 링크)은 **절대 따라가지 않는다** — 용량 중복 계산과
//!   링크 너머(다른 폴더)의 데이터를 지우는 사고를 막는다.
//! - 모든 경로는 `\\?\` 접두사로 260자 제한 없이 처리한다.

use std::ffi::c_void;

use rayon::prelude::*;
use windows::Win32::Foundation::HWND;
use windows::Win32::Storage::FileSystem::{
    DeleteFileW, FILE_ATTRIBUTE_DIRECTORY, FILE_ATTRIBUTE_HIDDEN, FILE_ATTRIBUTE_NORMAL,
    FILE_ATTRIBUTE_READONLY, FILE_ATTRIBUTE_REPARSE_POINT, FILE_ATTRIBUTE_SYSTEM,
    FIND_FIRST_EX_LARGE_FETCH, FindClose, FindExInfoBasic, FindExSearchNameMatch, FindFirstFileExW,
    FindNextFileW, GetFileAttributesW, INVALID_FILE_ATTRIBUTES, MOVEFILE_DELAY_UNTIL_REBOOT,
    MoveFileExW, RemoveDirectoryW, SetFileAttributesW, WIN32_FIND_DATAW,
};
use windows::Win32::UI::Shell::{
    FO_DELETE, FOF_ALLOWUNDO, FOF_NOCONFIRMATION, FOF_NOERRORUI, FOF_SILENT, SHFILEOPSTRUCTW,
    SHFileOperationW,
};
use windows::core::PCWSTR;

/// `\\?\` 긴 경로 형식(UTF-16, NUL 미포함)으로 변환한다.
fn long_path(path: &str) -> Vec<u16> {
    let p = path.trim().trim_matches('"').replace('/', "\\");
    let p = p.trim_end_matches('\\');
    let s = if p.starts_with(r"\\?\") {
        p.to_string()
    } else if let Some(unc) = p.strip_prefix(r"\\") {
        format!(r"\\?\UNC\{unc}")
    } else {
        format!(r"\\?\{p}")
    };
    // "\\?\C:" 처럼 드라이브만 남으면 루트 구분자를 붙인다.
    let s = if s.len() == 6 && s.ends_with(':') {
        format!("{s}\\")
    } else {
        s
    };
    s.encode_utf16().collect()
}

fn with_nul(p: &[u16]) -> Vec<u16> {
    let mut v = Vec::with_capacity(p.len() + 1);
    v.extend_from_slice(p);
    v.push(0);
    v
}

fn child_path(dir: &[u16], name: &[u16]) -> Vec<u16> {
    let mut v = Vec::with_capacity(dir.len() + name.len() + 1);
    v.extend_from_slice(dir);
    if v.last() != Some(&(b'\\' as u16)) {
        v.push(b'\\' as u16);
    }
    v.extend_from_slice(name);
    v
}

fn display(p: &[u16]) -> String {
    let s = String::from_utf16_lossy(p);
    s.strip_prefix(r"\\?\UNC\")
        .map(|r| format!(r"\\{r}"))
        .or_else(|| s.strip_prefix(r"\\?\").map(str::to_string))
        .unwrap_or(s)
}

/// 한 폴더의 직속 항목을 나열한다. 열 수 없으면 false.
fn list_dir(dir: &[u16], mut on_entry: impl FnMut(&[u16], &WIN32_FIND_DATAW)) -> bool {
    let mut pattern = Vec::with_capacity(dir.len() + 3);
    pattern.extend_from_slice(dir);
    if pattern.last() != Some(&(b'\\' as u16)) {
        pattern.push(b'\\' as u16);
    }
    pattern.push(b'*' as u16);
    pattern.push(0);

    let mut data = WIN32_FIND_DATAW::default();
    // SAFETY: pattern은 NUL 종료, data는 WIN32_FIND_DATAW(FindExInfoBasic 출력 형식)이다.
    let handle = unsafe {
        FindFirstFileExW(
            PCWSTR(pattern.as_ptr()),
            FindExInfoBasic,
            &mut data as *mut WIN32_FIND_DATAW as *mut c_void,
            FindExSearchNameMatch,
            None,
            FIND_FIRST_EX_LARGE_FETCH,
        )
    };
    let Ok(handle) = handle else {
        return false;
    };
    loop {
        let len = data
            .cFileName
            .iter()
            .position(|&c| c == 0)
            .unwrap_or(data.cFileName.len());
        let name = &data.cFileName[..len];
        let is_dot = name == [b'.' as u16] || name == [b'.' as u16, b'.' as u16];
        if !is_dot {
            on_entry(name, &data);
        }
        // SAFETY: 유효한 검색 핸들과 출력 버퍼.
        if unsafe { FindNextFileW(handle, &mut data) }.is_err() {
            break;
        }
    }
    // SAFETY: FindFirstFileExW가 돌려준 핸들을 한 번만 닫는다.
    unsafe {
        let _ = FindClose(handle);
    }
    true
}

fn is_dir(d: &WIN32_FIND_DATAW) -> bool {
    d.dwFileAttributes & FILE_ATTRIBUTE_DIRECTORY.0 != 0
}

fn is_reparse(d: &WIN32_FIND_DATAW) -> bool {
    d.dwFileAttributes & FILE_ATTRIBUTE_REPARSE_POINT.0 != 0
}

fn file_size(d: &WIN32_FIND_DATAW) -> u64 {
    (u64::from(d.nFileSizeHigh) << 32) | u64::from(d.nFileSizeLow)
}

/// 폴더 전체의 논리 크기(바이트). 정션/심볼릭 링크는 따라가지 않는다.
pub fn dir_size(path: &str) -> u64 {
    size_rec(&long_path(path), 0)
}

fn size_rec(dir: &[u16], depth: u32) -> u64 {
    let mut total = 0u64;
    let mut subdirs: Vec<Vec<u16>> = Vec::new();
    list_dir(dir, |name, d| {
        if is_dir(d) {
            if !is_reparse(d) {
                subdirs.push(child_path(dir, name));
            }
        } else {
            total += file_size(d);
        }
    });
    if depth > 128 {
        return total;
    }
    total
        + if subdirs.len() > 1 {
            subdirs
                .par_iter()
                .map(|s| size_rec(s, depth + 1))
                .sum::<u64>()
        } else {
            subdirs.iter().map(|s| size_rec(s, depth + 1)).sum::<u64>()
        }
}

/// 경로가 존재하는지와 폴더인지.
pub fn path_kind(path: &str) -> Option<(bool, bool)> {
    let p = with_nul(&long_path(path));
    // SAFETY: NUL 종료 경로.
    let attrs = unsafe { GetFileAttributesW(PCWSTR(p.as_ptr())) };
    if attrs == INVALID_FILE_ATTRIBUTES {
        return None;
    }
    Some((
        attrs & FILE_ATTRIBUTE_DIRECTORY.0 != 0,
        attrs & FILE_ATTRIBUTE_REPARSE_POINT.0 != 0,
    ))
}

pub fn exists(path: &str) -> bool {
    path_kind(path).is_some()
}

pub fn is_dir_path(path: &str) -> bool {
    matches!(path_kind(path), Some((true, _)))
}

/// 파일 하나의 크기.
pub fn file_len(path: &str) -> u64 {
    std::fs::metadata(path).map(|m| m.len()).unwrap_or(0)
}

/// 폴더의 직속 하위 폴더 이름(재분석 지점 제외).
pub fn subdir_names(path: &str) -> Vec<String> {
    let mut out = Vec::new();
    list_dir(&long_path(path), |name, d| {
        if is_dir(d) && !is_reparse(d) {
            out.push(String::from_utf16_lossy(name));
        }
    });
    out
}

/// 폴더의 직속 항목 (이름, 폴더 여부).
pub fn entries(path: &str) -> Vec<(String, bool)> {
    let mut out = Vec::new();
    list_dir(&long_path(path), |name, d| {
        out.push((String::from_utf16_lossy(name), is_dir(d)));
    });
    out
}

/// 폴더가 비어 있는지(열 수 없으면 false).
pub fn is_empty_dir(path: &str) -> bool {
    let mut any = false;
    let opened = list_dir(&long_path(path), |_, _| any = true);
    opened && !any
}

/// 하위 트리 파일을 재귀 열거한다(확장자 필터, 최대 깊이). 정션은 따라가지 않는다.
pub fn find_files(root: &str, exts: &[&str], max_depth: u32) -> Vec<String> {
    fn rec(dir: &[u16], exts: &[&str], depth: u32, max: u32, out: &mut Vec<String>) {
        let mut subs = Vec::new();
        list_dir(dir, |name, d| {
            if is_dir(d) {
                if !is_reparse(d) && depth < max {
                    subs.push(child_path(dir, name));
                }
            } else {
                let n = String::from_utf16_lossy(name).to_lowercase();
                if exts.iter().any(|e| n.ends_with(e)) {
                    out.push(display(&child_path(dir, name)));
                }
            }
        });
        for s in subs {
            rec(&s, exts, depth + 1, max, out);
        }
    }
    let mut out = Vec::new();
    if !root.is_empty() {
        rec(&long_path(root), exts, 0, max_depth, &mut out);
    }
    out
}

/// 삭제 결과 집계.
#[derive(Debug, Default, Clone)]
pub struct DeleteStats {
    pub files: u64,
    pub dirs: u64,
    pub bytes: u64,
    /// 사용 중이라 재부팅 후 삭제하도록 예약된 경로.
    pub scheduled: Vec<String>,
    /// 삭제도 예약도 실패한 경로와 사유.
    pub failed: Vec<(String, String)>,
}

impl DeleteStats {
    pub fn merge(mut self, other: DeleteStats) -> DeleteStats {
        self.files += other.files;
        self.dirs += other.dirs;
        self.bytes += other.bytes;
        self.scheduled.extend(other.scheduled);
        self.failed.extend(other.failed);
        self
    }
}

fn clear_attributes(p: &[u16]) {
    // SAFETY: NUL 종료 경로.
    unsafe {
        let _ = SetFileAttributesW(PCWSTR(p.as_ptr()), FILE_ATTRIBUTE_NORMAL);
    }
}

fn schedule_on_reboot(p: &[u16]) -> bool {
    // SAFETY: NUL 종료 경로, 대상 없음(=삭제 예약).
    unsafe {
        MoveFileExW(
            PCWSTR(p.as_ptr()),
            PCWSTR::null(),
            MOVEFILE_DELAY_UNTIL_REBOOT,
        )
        .is_ok()
    }
}

fn delete_file_raw(path: &[u16], attrs: u32, size: u64, stats: &mut DeleteStats) {
    let p = with_nul(path);
    let sticky = FILE_ATTRIBUTE_READONLY.0 | FILE_ATTRIBUTE_HIDDEN.0 | FILE_ATTRIBUTE_SYSTEM.0;
    if attrs & sticky != 0 {
        clear_attributes(&p);
    }
    // SAFETY: NUL 종료 경로.
    match unsafe { DeleteFileW(PCWSTR(p.as_ptr())) } {
        Ok(()) => {
            stats.files += 1;
            stats.bytes += size;
        }
        Err(e) => {
            if schedule_on_reboot(&p) {
                stats.scheduled.push(display(path));
            } else {
                stats.failed.push((display(path), e.message()));
            }
        }
    }
}

fn remove_dir_raw(path: &[u16], stats: &mut DeleteStats) {
    let p = with_nul(path);
    clear_attributes(&p);
    // SAFETY: NUL 종료 경로. 재분석 지점이면 링크 자체만 제거된다.
    match unsafe { RemoveDirectoryW(PCWSTR(p.as_ptr())) } {
        Ok(()) => stats.dirs += 1,
        Err(e) => {
            if schedule_on_reboot(&p) {
                stats.scheduled.push(display(path));
            } else {
                stats.failed.push((display(path), e.message()));
            }
        }
    }
}

fn delete_rec(dir: &[u16]) -> DeleteStats {
    let mut stats = DeleteStats::default();
    let mut subdirs: Vec<Vec<u16>> = Vec::new();
    list_dir(dir, |name, d| {
        let child = child_path(dir, name);
        if is_dir(d) {
            if is_reparse(d) {
                // 정션/디렉터리 심볼릭 링크: 링크만 제거, 대상은 건드리지 않는다.
                remove_dir_raw(&child, &mut stats);
            } else {
                subdirs.push(child);
            }
        } else {
            delete_file_raw(&child, d.dwFileAttributes, file_size(d), &mut stats);
        }
    });
    let sub = subdirs
        .par_iter()
        .map(|s| delete_rec(s))
        .reduce(DeleteStats::default, DeleteStats::merge);
    stats = stats.merge(sub);
    remove_dir_raw(dir, &mut stats);
    stats
}

/// 파일 또는 폴더(하위 전체)를 영구 삭제한다. 사용 중인 항목은 재부팅 시 삭제로 예약한다.
pub fn delete_path(path: &str) -> DeleteStats {
    let lp = long_path(path);
    let mut stats = DeleteStats::default();
    let p = with_nul(&lp);
    // SAFETY: NUL 종료 경로.
    let attrs = unsafe { GetFileAttributesW(PCWSTR(p.as_ptr())) };
    if attrs == INVALID_FILE_ATTRIBUTES {
        return stats; // 이미 없음
    }
    if attrs & FILE_ATTRIBUTE_DIRECTORY.0 != 0 {
        if attrs & FILE_ATTRIBUTE_REPARSE_POINT.0 != 0 {
            remove_dir_raw(&lp, &mut stats);
        } else {
            stats = delete_rec(&lp);
        }
    } else {
        let size = file_len(path);
        delete_file_raw(&lp, attrs, size, &mut stats);
    }
    stats
}

/// 빈 폴더 하나를 제거한다(비어 있지 않으면 아무것도 하지 않음).
pub fn remove_empty_dir(path: &str) -> bool {
    if !is_empty_dir(path) {
        return false;
    }
    let p = with_nul(&long_path(path));
    // SAFETY: NUL 종료 경로.
    unsafe { RemoveDirectoryW(PCWSTR(p.as_ptr())).is_ok() }
}

/// 여러 경로를 휴지통으로 이동한다. 전부 성공하면 Ok.
pub fn recycle(paths: &[String]) -> Result<(), String> {
    if paths.is_empty() {
        return Ok(());
    }
    // pFrom은 NUL로 구분하고 이중 NUL로 끝나는 목록이다. (`\\?\` 형식은 지원하지 않는다.)
    let mut from: Vec<u16> = Vec::new();
    for p in paths {
        from.extend(p.encode_utf16());
        from.push(0);
    }
    from.push(0);
    let mut op = SHFILEOPSTRUCTW {
        hwnd: HWND::default(),
        wFunc: FO_DELETE,
        pFrom: PCWSTR(from.as_ptr()),
        pTo: PCWSTR::null(),
        fFlags: (FOF_ALLOWUNDO | FOF_NOCONFIRMATION | FOF_SILENT | FOF_NOERRORUI).0 as u16,
        ..Default::default()
    };
    // SAFETY: op의 모든 포인터가 호출 동안 유효하다.
    let r = unsafe { SHFileOperationW(&mut op) };
    if r == 0 && !op.fAnyOperationsAborted.as_bool() {
        Ok(())
    } else {
        Err(format!("휴지통 이동 실패(코드 0x{r:X})"))
    }
}

/// 휴지통 이동 후 남은 항목만 영구 삭제로 마무리한다.
pub fn recycle_or_delete(path: &str) -> DeleteStats {
    let is_dir = is_dir_path(path);
    let size = if is_dir {
        dir_size(path)
    } else {
        file_len(path)
    };
    if recycle(&[path.to_string()]).is_ok() && !exists(path) {
        return DeleteStats {
            files: u64::from(!is_dir),
            dirs: u64::from(is_dir),
            bytes: size,
            ..Default::default()
        };
    }
    delete_path(path)
}
