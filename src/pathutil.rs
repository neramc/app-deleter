//! Windows 경로 문자열 유틸리티 (순수 함수 — 모든 OS에서 단위 테스트 가능).
//!
//! Windows 경로는 대소문자를 구분하지 않고 `/`와 `\`를 모두 구분자로 쓴다.
//! 비교·포함 판정은 항상 [`path_key`]로 정규화한 문자열로 수행한다.

/// 비교용으로 정규화한 경로 키.
///
/// - 앞뒤 공백·따옴표 제거, `/` → `\`
/// - `\\?\` / `\??\` 접두사 제거 (`\\?\UNC\` → `\\`)
/// - 끝의 `\` 제거(단, `C:\` 같은 드라이브 루트는 유지)
/// - 소문자화
pub fn path_key(path: &str) -> String {
    let mut s = path.trim().trim_matches('"').trim().replace('/', "\\");
    for prefix in [r"\\?\UNC\", r"\??\UNC\"] {
        if let Some(rest) = s.strip_prefix(prefix) {
            s = format!(r"\\{rest}");
        }
    }
    for prefix in [r"\\?\", r"\??\"] {
        if let Some(rest) = s.strip_prefix(prefix) {
            s = rest.to_string();
        }
    }
    // 중복 구분자 정리(UNC 시작 `\\`는 유지).
    let unc = s.starts_with(r"\\");
    let mut out = String::with_capacity(s.len());
    let mut prev_sep = false;
    for ch in s.chars() {
        if ch == '\\' {
            if prev_sep {
                continue;
            }
            prev_sep = true;
        } else {
            prev_sep = false;
        }
        out.push(ch);
    }
    if unc {
        out.insert(0, '\\');
    }
    while out.ends_with('\\') && !is_drive_root(&out) && out.len() > 2 {
        out.pop();
    }
    if out.len() == 2 && out.as_bytes()[1] == b':' {
        out.push('\\');
    }
    out.to_lowercase()
}

/// `C:\` 형태의 드라이브 루트인지.
fn is_drive_root(s: &str) -> bool {
    let b = s.as_bytes();
    b.len() == 3 && b[0].is_ascii_alphabetic() && b[1] == b':' && b[2] == b'\\'
}

/// 절대 경로(`C:\...` 또는 UNC `\\server\share`)인지.
pub fn is_absolute(path: &str) -> bool {
    let k = path_key(path);
    let b = k.as_bytes();
    (b.len() >= 3 && b[0].is_ascii_alphabetic() && b[1] == b':' && b[2] == b'\\')
        || k.starts_with(r"\\")
}

/// 드라이브 루트(`C:\`)나 UNC 공유 루트처럼 "너무 넓은" 경로인지.
pub fn is_root_like(path: &str) -> bool {
    let k = path_key(path);
    if is_drive_root(&k) || k.len() <= 3 {
        return true;
    }
    if let Some(rest) = k.strip_prefix(r"\\") {
        // \\server\share 까지는 루트로 간주한다.
        return rest.split('\\').filter(|p| !p.is_empty()).count() <= 2;
    }
    false
}

/// `child`가 `parent`와 같거나 그 하위 경로인지 (대소문자 무시).
pub fn is_same_or_within(child: &str, parent: &str) -> bool {
    let c = path_key(child);
    let p = path_key(parent);
    key_within(&c, &p)
}

/// 이미 정규화된 키끼리의 포함 판정.
pub fn key_within(child_key: &str, parent_key: &str) -> bool {
    if parent_key.is_empty() {
        return false;
    }
    if child_key == parent_key {
        return true;
    }
    if !child_key.starts_with(parent_key) {
        return false;
    }
    if parent_key.ends_with('\\') {
        return true;
    }
    child_key.as_bytes().get(parent_key.len()) == Some(&b'\\')
}

/// 경로의 마지막 구성요소(파일/폴더 이름).
pub fn file_name(path: &str) -> &str {
    let t = path.trim_end_matches(['\\', '/']);
    match t.rfind(['\\', '/']) {
        Some(i) => &t[i + 1..],
        None => t,
    }
}

/// 상위 경로. 드라이브 루트이거나 구분자가 없으면 `None`.
pub fn parent(path: &str) -> Option<&str> {
    let t = path.trim_end_matches(['\\', '/']);
    let i = t.rfind(['\\', '/'])?;
    let p = &t[..i];
    if p.is_empty() {
        return None;
    }
    // "C:" 만 남으면 "C:\"가 되도록 구분자 포함해 반환.
    if p.len() == 2 && p.as_bytes()[1] == b':' {
        return Some(&t[..=i]);
    }
    Some(p)
}

/// 확장자를 뺀 파일 이름.
pub fn file_stem(path: &str) -> &str {
    let name = file_name(path);
    match name.rfind('.') {
        Some(0) | None => name,
        Some(i) => &name[..i],
    }
}

/// 두 경로를 `\`로 잇는다.
pub fn join(base: &str, child: &str) -> String {
    let b = base.trim_end_matches(['\\', '/']);
    let c = child.trim_start_matches(['\\', '/']);
    format!("{b}\\{c}")
}

/// 명령줄/텍스트 안에 폴더 경로(정규화 키)가 경로 경계와 함께 등장하는지.
///
/// `"C:\Program Files\Foo\a.exe" --x`는 `c:\program files\foo`를 포함하지만
/// `C:\Program Files\FooBar\a.exe`는 포함하지 않는다.
pub fn text_mentions_dir(text: &str, dir_key: &str) -> bool {
    if dir_key.len() < 4 {
        return false;
    }
    let hay = text.replace('/', "\\").to_lowercase();
    let mut from = 0;
    while let Some(pos) = hay[from..].find(dir_key) {
        let end = from + pos + dir_key.len();
        match hay[end..].chars().next() {
            None | Some('\\' | '"' | ' ' | ',' | ';' | '\'') => return true,
            _ => from = end,
        }
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn key_normalizes_case_separators_and_prefixes() {
        assert_eq!(path_key(r"C:/Program Files\Foo\\"), r"c:\program files\foo");
        assert_eq!(path_key(r"\\?\C:\Foo"), r"c:\foo");
        assert_eq!(path_key(r"\??\C:\Foo\bar.sys"), r"c:\foo\bar.sys");
        assert_eq!(path_key(r#""C:\Foo""#), r"c:\foo");
        assert_eq!(path_key("C:"), r"c:\");
        assert_eq!(path_key(r"C:\"), r"c:\");
        assert_eq!(path_key(r"\\?\UNC\srv\share\x"), r"\\srv\share\x");
    }

    #[test]
    fn within_respects_component_boundaries() {
        assert!(is_same_or_within(
            r"C:\Program Files\Foo\a.exe",
            r"c:\program files\foo"
        ));
        assert!(is_same_or_within(
            r"C:\Program Files\Foo",
            r"C:\Program Files\Foo\"
        ));
        assert!(!is_same_or_within(
            r"C:\Program Files\FooBar",
            r"C:\Program Files\Foo"
        ));
        assert!(is_same_or_within(r"C:\x", r"C:\"));
        assert!(!is_same_or_within(r"D:\x", r"C:\"));
    }

    #[test]
    fn roots_are_detected() {
        assert!(is_root_like(r"C:\"));
        assert!(is_root_like("C:"));
        assert!(is_root_like(r"\\server\share"));
        assert!(!is_root_like(r"\\server\share\dir"));
        assert!(!is_root_like(r"C:\Foo"));
        assert!(is_absolute(r"C:\Foo"));
        assert!(!is_absolute(r"Foo\bar"));
    }

    #[test]
    fn mentions_dir_with_boundaries() {
        let d = path_key(r"C:\Program Files\Foo");
        assert!(text_mentions_dir(
            r#""C:\Program Files\Foo\a.exe" --run"#,
            &d
        ));
        assert!(text_mentions_dir(
            r"rundll32 C:\PROGRAM FILES\FOO\x.dll,Run",
            &d
        ));
        assert!(text_mentions_dir(r"C:\Program Files\Foo", &d));
        assert!(!text_mentions_dir(r"C:\Program Files\FooBar\a.exe", &d));
    }

    #[test]
    fn name_parent_stem_join() {
        assert_eq!(file_name(r"C:\a\b\c.exe"), "c.exe");
        assert_eq!(file_name(r"C:\a\b\"), "b");
        assert_eq!(parent(r"C:\a\b\c.exe"), Some(r"C:\a\b"));
        assert_eq!(parent(r"C:\a"), Some(r"C:\"));
        assert_eq!(parent("abc"), None);
        assert_eq!(file_stem(r"C:\a\unins000.exe"), "unins000");
        assert_eq!(file_stem(r"C:\a\.hidden"), ".hidden");
        assert_eq!(join(r"C:\a\", r"\b"), r"C:\a\b");
    }
}
