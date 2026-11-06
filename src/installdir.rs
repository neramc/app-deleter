//! 설치 폴더 추정 (순수 로직).
//!
//! 레지스트리에 `InstallLocation`이 없는 앱이 많다. 이때 `DisplayIcon`이나 제거 프로그램
//! 경로에서 설치 폴더를 추정하되, **이름이 앱과 일치하는 폴더만** 채택한다.
//! (예: `C:\Program Files\Common Files\Vendor\Shared\icon.ico` → 공유 폴더이므로 채택하지 않음)

use crate::pathutil::{self, key_within, path_key};
use crate::safety::PathGuard;
use crate::textmatch::{AppAliases, NameMatch};

/// `evidence_file`(앱의 실행 파일/아이콘 경로)에서 설치 폴더를 추정한다.
///
/// `roots`는 `Program Files`, `%LOCALAPPDATA%\Programs` 등 "그 바로 아래가 보통 앱 폴더"인
/// 위치들이다. 루트 아래 경로 구성요소를 위에서부터 내려가며 앱 별칭과 정확히(또는 부분)
/// 일치하는 첫 폴더를 반환한다. 게시자 폴더는 건너뛰고 더 내려간다.
pub fn derive_from_file(
    evidence_file: &str,
    roots: &[String],
    aliases: &AppAliases,
    guard: &PathGuard,
) -> Option<String> {
    let dir = pathutil::parent(evidence_file)?;
    if !pathutil::is_absolute(dir) || !guard.can_delete(dir) {
        return None;
    }
    let dir_key = path_key(dir);
    let root = roots
        .iter()
        .filter(|r| {
            let rk = path_key(r);
            key_within(&dir_key, &rk) && dir_key != rk
        })
        .max_by_key(|r| path_key(r).len())
        .map(|r| r.trim_end_matches('\\').to_string())
        .or_else(|| {
            // 알려진 루트 밖(예: D:\Games\Foo): 드라이브 루트를 기준으로 한다.
            let b = dir.as_bytes();
            (b.len() > 3 && b[1] == b':').then(|| dir[..2].to_string())
        })?;

    let rel = dir.get(root.len()..)?.trim_start_matches('\\');
    let comps: Vec<&str> = rel.split('\\').filter(|c| !c.is_empty()).collect();
    let mut cur = root.clone();
    for comp in comps {
        cur = format!("{cur}\\{comp}");
        if !guard.can_delete(&cur) {
            continue;
        }
        match aliases.classify(comp) {
            NameMatch::Exact | NameMatch::Partial => return Some(cur),
            NameMatch::Publisher | NameMatch::None => continue,
        }
    }
    None
}

/// 레지스트리 Uninstall 키 이름을 별칭 후보로 정리한다(`Foo_is1` → `Foo`, GUID는 제외).
pub fn key_name_alias(key_name: &str) -> Option<String> {
    if crate::cmdline::is_guid(key_name) {
        return None;
    }
    let s = key_name.strip_suffix("_is1").unwrap_or(key_name).trim();
    (s.len() >= 3).then(|| s.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn roots() -> Vec<String> {
        vec![
            r"C:\Program Files".into(),
            r"C:\Program Files (x86)".into(),
            r"C:\Users\kim\AppData\Local".into(),
            r"C:\Users\kim\AppData\Local\Programs".into(),
        ]
    }

    fn guard() -> PathGuard {
        let mut g = PathGuard::new();
        for r in roots() {
            g.protect(&r);
        }
        g.protect(r"C:\Program Files\Common Files");
        g.deny_tree(r"C:\Windows");
        g
    }

    #[test]
    fn finds_app_folder_below_root() {
        let a = AppAliases::new("Foo Editor 2.1", "Foo Inc.", &[]);
        assert_eq!(
            derive_from_file(
                r"C:\Program Files\Foo Editor\bin\foo.exe",
                &roots(),
                &a,
                &guard()
            )
            .as_deref(),
            Some(r"C:\Program Files\Foo Editor")
        );
    }

    #[test]
    fn skips_publisher_folder() {
        let a = AppAliases::new("KakaoTalk", "Kakao Corp.", &[]);
        assert_eq!(
            derive_from_file(
                r"C:\Program Files (x86)\Kakao\KakaoTalk\KakaoTalk.exe",
                &roots(),
                &a,
                &guard()
            )
            .as_deref(),
            Some(r"C:\Program Files (x86)\Kakao\KakaoTalk")
        );
    }

    #[test]
    fn uses_most_specific_root_and_exe_alias() {
        // 한글 표시 이름 + 영문 폴더: 실행 파일 이름 별칭으로 매칭된다.
        let a = AppAliases::new("카카오톡", "Kakao", &["KakaoTalk"]);
        assert_eq!(
            derive_from_file(
                r"C:\Users\kim\AppData\Local\Programs\KakaoTalk\KakaoTalk.exe",
                &roots(),
                &a,
                &guard()
            )
            .as_deref(),
            Some(r"C:\Users\kim\AppData\Local\Programs\KakaoTalk")
        );
        let d = AppAliases::new("Discord", "Discord Inc.", &[]);
        assert_eq!(
            derive_from_file(
                r"C:\Users\kim\AppData\Local\Discord\app-1.0.9\Discord.exe",
                &roots(),
                &d,
                &guard()
            )
            .as_deref(),
            Some(r"C:\Users\kim\AppData\Local\Discord")
        );
    }

    #[test]
    fn rejects_unrelated_shared_and_system_folders() {
        let a = AppAliases::new("Foo", "Foo Inc.", &[]);
        assert_eq!(
            derive_from_file(
                r"C:\Program Files\Common Files\Vendor\Shared\icon.ico",
                &roots(),
                &a,
                &guard()
            ),
            None
        );
        assert_eq!(
            derive_from_file(r"C:\Windows\System32\msiexec.exe", &roots(), &a, &guard()),
            None
        );
        assert_eq!(
            derive_from_file(r"C:\Program Files\foo.exe", &roots(), &a, &guard()),
            None
        );
    }

    #[test]
    fn handles_folders_outside_known_roots() {
        let a = AppAliases::new("Some Game", "Studio", &[]);
        assert_eq!(
            derive_from_file(r"D:\Games\SomeGame\bin\game.exe", &roots(), &a, &guard()).as_deref(),
            Some(r"D:\Games\SomeGame")
        );
    }

    #[test]
    fn key_aliases() {
        assert_eq!(key_name_alias("Foo_is1").as_deref(), Some("Foo"));
        assert_eq!(
            key_name_alias("{90120000-0011-0000-0000-0000000FF1CE}"),
            None
        );
        assert_eq!(key_name_alias("7-Zip").as_deref(), Some("7-Zip"));
    }
}
