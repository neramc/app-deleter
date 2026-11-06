//! 레지스트리 접근 헬퍼(64비트 뷰 고정) + `.reg` 백업.

use std::io;

use winreg::RegKey;
use winreg::enums::{
    HKEY_CURRENT_USER, HKEY_LOCAL_MACHINE, KEY_ALL_ACCESS, KEY_READ, KEY_WOW64_64KEY,
};

use crate::regfile;
use crate::safety::Hive;

fn root(hive: Hive) -> RegKey {
    match hive {
        Hive::LocalMachine => RegKey::predef(HKEY_LOCAL_MACHINE),
        Hive::CurrentUser => RegKey::predef(HKEY_CURRENT_USER),
    }
}

/// 읽기 전용으로 연다. WOW64 리디렉션 없이 경로 그대로(64비트 뷰) 접근한다.
pub fn open(hive: Hive, path: &str) -> io::Result<RegKey> {
    root(hive).open_subkey_with_flags(path, KEY_READ | KEY_WOW64_64KEY)
}

/// 쓰기 가능하게 연다.
pub fn open_write(hive: Hive, path: &str) -> io::Result<RegKey> {
    root(hive).open_subkey_with_flags(path, KEY_ALL_ACCESS | KEY_WOW64_64KEY)
}

pub fn key_exists(hive: Hive, path: &str) -> bool {
    open(hive, path).is_ok()
}

/// 문자열 값(REG_SZ/REG_EXPAND_SZ). 없거나 비어 있으면 `None`.
pub fn get_string(key: &RegKey, name: &str) -> Option<String> {
    key.get_value::<String, _>(name)
        .ok()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
}

pub fn get_dword(key: &RegKey, name: &str) -> Option<u32> {
    key.get_value::<u32, _>(name).ok()
}

/// 하위 키 이름 목록.
pub fn subkeys(key: &RegKey) -> Vec<String> {
    key.enum_keys().filter_map(Result::ok).collect()
}

/// 값 (이름, 문자열 데이터) 목록 — 문자열 형식 값만.
pub fn string_values(key: &RegKey) -> Vec<(String, String)> {
    key.enum_values()
        .filter_map(Result::ok)
        .filter_map(|(name, v)| {
            let t = v.vtype.clone() as u32;
            if t == regfile::REG_SZ || t == regfile::REG_EXPAND_SZ {
                let words: Vec<u16> = v
                    .bytes
                    .chunks_exact(2)
                    .map(|c| u16::from_le_bytes([c[0], c[1]]))
                    .take_while(|w| *w != 0)
                    .collect();
                Some((name, String::from_utf16_lossy(&words)))
            } else {
                None
            }
        })
        .collect()
}

/// 키(하위 트리 포함)를 `.reg` 텍스트로 덧붙인다.
pub fn export_key(hive: Hive, path: &str, out: &mut String) -> io::Result<()> {
    let key = open(hive, path)?;
    export_rec(&key, &format!("{}\\{}", hive.long_name(), path), out, 0);
    Ok(())
}

fn export_rec(key: &RegKey, full: &str, out: &mut String, depth: u32) {
    out.push_str(&regfile::key_header(full));
    for (name, value) in key.enum_values().filter_map(Result::ok) {
        out.push_str(&regfile::value_line(
            &name,
            value.vtype.clone() as u32,
            &value.bytes,
        ));
    }
    if depth > 64 {
        return;
    }
    for sub in subkeys(key) {
        if let Ok(child) = key.open_subkey_with_flags(&sub, KEY_READ | KEY_WOW64_64KEY) {
            export_rec(&child, &format!("{full}\\{sub}"), out, depth + 1);
        }
    }
}

/// 값 하나를 `.reg` 텍스트로 덧붙인다.
pub fn export_value(hive: Hive, key_path: &str, name: &str, out: &mut String) -> io::Result<()> {
    let key = open(hive, key_path)?;
    let value = key.get_raw_value(name)?;
    out.push_str(&regfile::key_header(&format!(
        "{}\\{}",
        hive.long_name(),
        key_path
    )));
    out.push_str(&regfile::value_line(name, value.vtype as u32, &value.bytes));
    Ok(())
}

/// 키와 하위 트리 전체를 삭제한다. 이미 없으면 성공으로 본다.
pub fn delete_key(hive: Hive, path: &str) -> io::Result<()> {
    let (parent, leaf) = match path.rfind('\\') {
        Some(i) => (&path[..i], &path[i + 1..]),
        None => {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "루트 키는 삭제할 수 없음",
            ));
        }
    };
    let parent_key = match open_write(hive, parent) {
        Ok(k) => k,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(()),
        Err(e) => return Err(e),
    };
    match parent_key.delete_subkey_all(leaf) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(e),
    }
}

/// 값 하나를 삭제한다. 이미 없으면 성공으로 본다.
pub fn delete_value(hive: Hive, key_path: &str, name: &str) -> io::Result<()> {
    let key = match open_write(hive, key_path) {
        Ok(k) => k,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(()),
        Err(e) => return Err(e),
    };
    match key.delete_value(name) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(e),
    }
}

/// 키에 하위 키와 값이 하나도 없는지(게시자 키 정리 판단용).
pub fn key_info(hive: Hive, path: &str) -> Option<(u32, u32)> {
    let key = open(hive, path).ok()?;
    let info = key.query_info().ok()?;
    Some((info.sub_keys, info.values))
}
