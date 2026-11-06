//! 제거 명령줄(UninstallString) 분석 (순수 함수).

use crate::pathutil;

/// 명령줄에서 실행 파일 경로와 나머지 인자를 분리한다.
///
/// `CreateProcess`의 해석 규칙을 따른다.
/// - `"C:\Program Files\App\unins000.exe" /SILENT` → 따옴표 안이 실행 파일
/// - `C:\Program Files\App\uninstall.exe /S` → 따옴표가 없어도 `.exe`까지를 경로로 본다
/// - `MsiExec.exe /X{GUID}` → 첫 토큰
pub fn split_command(cmd: &str) -> (String, String) {
    let s = cmd.trim();
    if let Some(rest) = s.strip_prefix('"') {
        return match rest.find('"') {
            Some(end) => (rest[..end].to_string(), rest[end + 1..].trim().to_string()),
            None => (rest.to_string(), String::new()),
        };
    }
    // 따옴표 없는 경로: 실행 확장자 + (공백 또는 끝)까지를 실행 파일로 본다.
    let lower = s.to_ascii_lowercase();
    for ext in [".exe", ".com", ".bat", ".cmd"] {
        let mut from = 0;
        while let Some(pos) = lower[from..].find(ext) {
            let end = from + pos + ext.len();
            let at_boundary = lower[end..]
                .chars()
                .next()
                .is_none_or(|c| c.is_whitespace() || c == '"');
            if at_boundary {
                return (s[..end].to_string(), s[end..].trim().to_string());
            }
            from = end;
        }
    }
    match s.find(char::is_whitespace) {
        Some(i) => (s[..i].to_string(), s[i..].trim().to_string()),
        None => (s.to_string(), String::new()),
    }
}

/// 명령줄의 실행 파일 이름(소문자, 확장자 포함).
fn exe_file_name(cmd: &str) -> String {
    let (exe, _) = split_command(cmd);
    pathutil::file_name(&exe).to_ascii_lowercase()
}

/// `msiexec` 기반 명령인지.
pub fn is_msiexec(cmd: &str) -> bool {
    matches!(exe_file_name(cmd).as_str(), "msiexec.exe" | "msiexec")
}

/// `{XXXXXXXX-XXXX-XXXX-XXXX-XXXXXXXXXXXX}` 형식 GUID인지.
pub fn is_guid(s: &str) -> bool {
    let b = s.as_bytes();
    if b.len() != 38 || b[0] != b'{' || b[37] != b'}' {
        return false;
    }
    b[1..37].iter().enumerate().all(|(i, c)| match i {
        8 | 13 | 18 | 23 => *c == b'-',
        _ => c.is_ascii_hexdigit(),
    })
}

/// 문자열에서 첫 번째 GUID를 찾는다.
pub fn find_guid(s: &str) -> Option<String> {
    let bytes = s.as_bytes();
    let mut i = 0;
    while let Some(off) = s[i..].find('{') {
        let start = i + off;
        if let Some(cand) = s.get(start..start + 38)
            && is_guid(cand)
        {
            return Some(cand.to_ascii_uppercase());
        }
        i = start + 1;
        if i >= bytes.len() {
            break;
        }
    }
    None
}

/// MSI 제품 코드: msiexec 명령이면 그 안의 GUID, 아니면 레지스트리 키 이름이 GUID일 때.
pub fn msi_product_code(
    uninstall_cmd: Option<&str>,
    key_name: &str,
    windows_installer: bool,
) -> Option<String> {
    if let Some(cmd) = uninstall_cmd
        && is_msiexec(cmd)
        && let Some(g) = find_guid(cmd)
    {
        return Some(g);
    }
    if windows_installer && is_guid(key_name) {
        return Some(key_name.to_ascii_uppercase());
    }
    None
}

/// MSI가 레지스트리에 쓰는 "압축 GUID" 형식으로 변환한다.
///
/// `{90120000-0011-0000-0000-0000000FF1CE}` → `00002109110000000000000000F01FEC`
pub fn packed_guid(guid: &str) -> Option<String> {
    if !is_guid(guid) {
        return None;
    }
    let hex: String = guid[1..37]
        .chars()
        .filter(|c| *c != '-')
        .collect::<String>()
        .to_ascii_uppercase();
    let rev = |s: &str| s.chars().rev().collect::<String>();
    let swap_pairs = |s: &str| {
        s.as_bytes()
            .chunks(2)
            .map(|p| format!("{}{}", p[1] as char, p[0] as char))
            .collect::<String>()
    };
    Some(format!(
        "{}{}{}{}",
        rev(&hex[0..8]),
        rev(&hex[8..12]),
        rev(&hex[12..16]),
        swap_pairs(&hex[16..32])
    ))
}

/// Inno Setup 제거 프로그램(`unins000.exe`)인지.
pub fn is_inno_uninstaller(cmd: &str) -> bool {
    let name = exe_file_name(cmd);
    name.len() == 12
        && name.starts_with("unins")
        && name.ends_with(".exe")
        && name[5..8].chars().all(|c| c.is_ascii_digit())
}

/// `DisplayIcon` 값(`"C:\\app.exe",0` / `C:\\app.ico` / `app.exe,-101`)을 (경로, 인덱스)로 분리한다.
pub fn parse_icon_location(raw: &str) -> Option<(String, i32)> {
    let s = raw.trim();
    if s.is_empty() {
        return None;
    }
    let (path, rest) = if let Some(r) = s.strip_prefix('"') {
        let end = r.find('"')?;
        (r[..end].to_string(), r[end + 1..].trim())
    } else {
        match s.rfind(',') {
            Some(i) if s[i + 1..].trim().parse::<i32>().is_ok() => (s[..i].to_string(), &s[i..]),
            _ => (s.to_string(), ""),
        }
    };
    let index = rest
        .trim_start_matches(',')
        .trim()
        .parse::<i32>()
        .unwrap_or(0);
    let path = path.trim().to_string();
    (!path.is_empty()).then_some((path, index))
}

/// 대화형/무인 제거 명령을 결정하는 데 필요한 정보.
#[derive(Debug, Clone, Default)]
pub struct UninstallInfo {
    pub uninstall: Option<String>,
    pub quiet_uninstall: Option<String>,
    pub msi_product_code: Option<String>,
    pub inno_setup: bool,
}

impl UninstallInfo {
    /// 대화형(앱 고유 제거 마법사) 명령.
    pub fn interactive_command(&self) -> Option<String> {
        if let Some(code) = &self.msi_product_code {
            // `/I{GUID}`는 "변경/복구" 화면을 띄우므로 항상 `/X`로 제거한다.
            return Some(format!("MsiExec.exe /X{code}"));
        }
        self.uninstall
            .clone()
            .or_else(|| self.quiet_uninstall.clone())
    }

    /// 무인(자동) 제거 명령 — 지원하지 않으면 `None`.
    pub fn quiet_command(&self) -> Option<String> {
        if let Some(code) = &self.msi_product_code {
            return Some(format!("MsiExec.exe /X{code} /passive /norestart"));
        }
        if let Some(q) = &self.quiet_uninstall {
            return Some(q.clone());
        }
        if self.inno_setup
            && let Some(u) = &self.uninstall
        {
            return Some(format!("{u} /SILENT /SUPPRESSMSGBOXES /NORESTART"));
        }
        None
    }

    pub fn supports_quiet(&self) -> bool {
        self.quiet_command().is_some()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn splits_quoted_unquoted_and_plain() {
        assert_eq!(
            split_command(r#""C:\Program Files\App\unins000.exe" /SILENT"#),
            (
                r"C:\Program Files\App\unins000.exe".into(),
                "/SILENT".into()
            )
        );
        assert_eq!(
            split_command(r"C:\Program Files\App Dir\uninstall.exe /S"),
            (
                r"C:\Program Files\App Dir\uninstall.exe".into(),
                "/S".into()
            )
        );
        assert_eq!(
            split_command("MsiExec.exe /X{12345678-1234-1234-1234-123456789ABC}"),
            (
                "MsiExec.exe".into(),
                "/X{12345678-1234-1234-1234-123456789ABC}".into()
            )
        );
        assert_eq!(
            split_command(r"C:\Apps\my.exe.tool\run.exe --uninstall"),
            (r"C:\Apps\my.exe.tool\run.exe".into(), "--uninstall".into())
        );
        assert_eq!(
            split_command(r"rundll32.exe dfshim.dll,ShArpMaintain x"),
            ("rundll32.exe".into(), "dfshim.dll,ShArpMaintain x".into())
        );
    }

    #[test]
    fn detects_msi_and_guid() {
        let cmd = "MsiExec.exe /I{90120000-0011-0000-0000-0000000ff1ce}";
        assert!(is_msiexec(cmd));
        assert_eq!(
            find_guid(cmd).as_deref(),
            Some("{90120000-0011-0000-0000-0000000FF1CE}")
        );
        assert_eq!(
            msi_product_code(Some(cmd), "whatever", false).as_deref(),
            Some("{90120000-0011-0000-0000-0000000FF1CE}")
        );
        assert_eq!(
            msi_product_code(None, "{90120000-0011-0000-0000-0000000FF1CE}", true).as_deref(),
            Some("{90120000-0011-0000-0000-0000000FF1CE}")
        );
        assert!(msi_product_code(Some(r#""C:\x\uninst.exe""#), "Foo", false).is_none());
        assert!(!is_guid("{not-a-guid}"));
    }

    #[test]
    fn parses_icon_locations() {
        assert_eq!(
            parse_icon_location(r#""C:\Program Files\A\a.exe",0"#),
            Some((r"C:\Program Files\A\a.exe".into(), 0))
        );
        assert_eq!(
            parse_icon_location(r"C:\A\a.exe,-101"),
            Some((r"C:\A\a.exe".into(), -101))
        );
        assert_eq!(
            parse_icon_location(r"C:\A\app.ico"),
            Some((r"C:\A\app.ico".into(), 0))
        );
        assert_eq!(
            parse_icon_location(r"C:\A,B\app.ico"),
            Some((r"C:\A,B\app.ico".into(), 0))
        );
        assert_eq!(parse_icon_location("  "), None);
    }

    #[test]
    fn packs_guid_like_msi() {
        assert_eq!(
            packed_guid("{90120000-0011-0000-0000-0000000FF1CE}").as_deref(),
            Some("00002109110000000000000000F01FEC")
        );
        assert!(packed_guid("nope").is_none());
    }

    #[test]
    fn builds_interactive_and_quiet_commands() {
        let msi = UninstallInfo {
            uninstall: Some("MsiExec.exe /I{90120000-0011-0000-0000-0000000FF1CE}".into()),
            msi_product_code: Some("{90120000-0011-0000-0000-0000000FF1CE}".into()),
            ..Default::default()
        };
        assert_eq!(
            msi.interactive_command().as_deref(),
            Some("MsiExec.exe /X{90120000-0011-0000-0000-0000000FF1CE}")
        );
        assert!(msi.quiet_command().unwrap().contains("/passive"));

        let inno = UninstallInfo {
            uninstall: Some(r#""C:\Program Files\App\unins000.exe""#.into()),
            inno_setup: is_inno_uninstaller(r#""C:\Program Files\App\unins000.exe""#),
            ..Default::default()
        };
        assert!(inno.inno_setup);
        assert!(
            inno.quiet_command()
                .unwrap()
                .ends_with("/SILENT /SUPPRESSMSGBOXES /NORESTART")
        );

        let plain = UninstallInfo {
            uninstall: Some(r"C:\x\uninstall.exe".into()),
            ..Default::default()
        };
        assert!(!plain.supports_quiet());
        assert_eq!(
            plain.interactive_command().as_deref(),
            Some(r"C:\x\uninstall.exe")
        );
    }
}
