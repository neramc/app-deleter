//! `.reg` 백업 파일 생성기 (순수 함수).
//!
//! 삭제하는 레지스트리 키/값을 regedit가 그대로 가져올 수 있는 형식으로 기록한다.
//! 사용자는 백업 폴더의 `.reg` 파일을 더블클릭해 복원할 수 있다.

use std::fmt::Write as _;

#[cfg(test)]
pub const REG_NONE: u32 = 0;
pub const REG_SZ: u32 = 1;
pub const REG_EXPAND_SZ: u32 = 2;
pub const REG_BINARY: u32 = 3;
pub const REG_DWORD: u32 = 4;
#[cfg(test)]
pub const REG_MULTI_SZ: u32 = 7;
#[cfg(test)]
pub const REG_QWORD: u32 = 11;

/// `.reg` 파일 머리말.
pub const HEADER: &str = "Windows Registry Editor Version 5.00\r\n";

/// `.reg` 문자열 리터럴 이스케이프.
fn escape(s: &str) -> String {
    s.replace('\\', "\\\\").replace('"', "\\\"")
}

/// `[HKEY_...\path]` 형식 키 헤더.
pub fn key_header(full_key_path: &str) -> String {
    format!("\r\n[{full_key_path}]\r\n")
}

/// 키 삭제 지시(`[-HKEY_...]`) — 백업 파일에는 쓰지 않지만 테스트/참조용으로 제공한다.
#[cfg(test)]
pub fn key_delete_header(full_key_path: &str) -> String {
    format!("\r\n[-{full_key_path}]\r\n")
}

fn value_name(name: &str) -> String {
    if name.is_empty() {
        "@".to_string()
    } else {
        format!("\"{}\"", escape(name))
    }
}

fn hex_list(bytes: &[u8]) -> String {
    let mut out = String::with_capacity(bytes.len() * 3);
    for (i, b) in bytes.iter().enumerate() {
        if i > 0 {
            out.push(',');
        }
        let _ = write!(out, "{b:02x}");
    }
    out
}

/// UTF-16LE 바이트를 문자열로 (끝의 NUL 제거). 홀수 길이/잘못된 UTF-16이면 `None`.
fn utf16_string(bytes: &[u8]) -> Option<String> {
    if !bytes.len().is_multiple_of(2) {
        return None;
    }
    let mut words: Vec<u16> = bytes
        .chunks_exact(2)
        .map(|c| u16::from_le_bytes([c[0], c[1]]))
        .collect();
    while words.last() == Some(&0) {
        words.pop();
    }
    if words.contains(&0) {
        return None;
    }
    String::from_utf16(&words).ok()
}

/// 값 한 줄(`"name"=...`)을 만든다.
pub fn value_line(name: &str, vtype: u32, data: &[u8]) -> String {
    let n = value_name(name);
    match vtype {
        REG_SZ => match utf16_string(data) {
            Some(s) if !s.contains(['\r', '\n']) => format!("{n}=\"{}\"\r\n", escape(&s)),
            _ => format!("{n}=hex(1):{}\r\n", hex_list(data)),
        },
        REG_DWORD if data.len() == 4 => {
            let v = u32::from_le_bytes([data[0], data[1], data[2], data[3]]);
            format!("{n}=dword:{v:08x}\r\n")
        }
        REG_BINARY => format!("{n}=hex:{}\r\n", hex_list(data)),
        other => format!("{n}=hex({other:x}):{}\r\n", hex_list(data)),
    }
}

/// regedit가 읽는 UTF-16LE(BOM 포함) 바이트로 인코딩한다.
pub fn encode_utf16le(text: &str) -> Vec<u8> {
    let mut out = Vec::with_capacity(text.len() * 2 + 2);
    out.extend_from_slice(&[0xFF, 0xFE]);
    for w in text.encode_utf16() {
        out.extend_from_slice(&w.to_le_bytes());
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn utf16z(s: &str) -> Vec<u8> {
        let mut v: Vec<u8> = s.encode_utf16().flat_map(|w| w.to_le_bytes()).collect();
        v.extend_from_slice(&[0, 0]);
        v
    }

    #[test]
    fn string_values_are_escaped() {
        assert_eq!(
            value_line(
                "InstallLocation",
                REG_SZ,
                &utf16z(r#"C:\Program Files\"Foo""#)
            ),
            "\"InstallLocation\"=\"C:\\\\Program Files\\\\\\\"Foo\\\"\"\r\n"
        );
        assert_eq!(value_line("", REG_SZ, &utf16z("x")), "@=\"x\"\r\n");
    }

    #[test]
    fn dword_and_hex_values() {
        assert_eq!(
            value_line("N", REG_DWORD, &1u32.to_le_bytes()),
            "\"N\"=dword:00000001\r\n"
        );
        assert_eq!(
            value_line("B", REG_BINARY, &[0xde, 0xad]),
            "\"B\"=hex:de,ad\r\n"
        );
        assert_eq!(
            value_line("Q", REG_QWORD, &[1, 0, 0, 0, 0, 0, 0, 0]),
            "\"Q\"=hex(b):01,00,00,00,00,00,00,00\r\n"
        );
        assert_eq!(
            value_line("E", REG_EXPAND_SZ, &[0x25, 0, 0, 0]),
            "\"E\"=hex(2):25,00,00,00\r\n"
        );
        // 줄바꿈을 포함한 REG_SZ는 hex(1)로 기록한다.
        assert!(value_line("M", REG_SZ, &utf16z("a\nb")).starts_with("\"M\"=hex(1):"));
        assert!(value_line("Z", REG_NONE, &[]).starts_with("\"Z\"=hex(0):"));
        assert!(value_line("S", REG_MULTI_SZ, &utf16z("a")).starts_with("\"S\"=hex(7):"));
    }

    #[test]
    fn headers_and_encoding() {
        assert_eq!(
            key_header(r"HKEY_CURRENT_USER\Software\Foo"),
            "\r\n[HKEY_CURRENT_USER\\Software\\Foo]\r\n"
        );
        assert_eq!(
            key_delete_header(r"HKEY_CURRENT_USER\Software\Foo"),
            "\r\n[-HKEY_CURRENT_USER\\Software\\Foo]\r\n"
        );
        let enc = encode_utf16le("A");
        assert_eq!(enc, vec![0xFF, 0xFE, 0x41, 0x00]);
    }
}
