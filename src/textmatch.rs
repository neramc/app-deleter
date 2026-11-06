//! 앱 이름 ↔ 폴더/레지스트리 키 이름 매칭 (순수 함수).
//!
//! 잔여물 검색의 정확도(누락 없이, 오탐 없이)가 이 모듈에 달려 있다.
//! 이름을 토큰으로 정규화한 뒤 "압축형"(공백·기호 제거) 문자열로 비교한다.
//!
//! - `Mozilla Firefox (x64 ko)` → 토큰 `mozilla firefox` → 압축형 `mozillafirefox`
//! - `7-Zip 23.01 (x64)`       → `7 zip` → `7zip`  (첫 토큰의 숫자는 유지)
//! - `GIMP 2.10.36`            → `gimp`            (버전 숫자 제거)

use std::collections::HashSet;

/// 이름 매칭 결과의 강도.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum NameMatch {
    /// 일치하지 않음.
    None,
    /// 일부 일치(접두사 등) — 오탐 가능성이 있어 기본 선택하지 않는다.
    Partial,
    /// 게시자 이름과 일치 — 하위 항목을 다시 확인해야 한다.
    Publisher,
    /// 앱 이름(또는 별칭)과 정확히 일치.
    Exact,
}

/// 아키텍처·잡음 토큰: 이름 비교에서 제외한다.
const NOISE_TOKENS: &[&str] = &[
    "x64", "x86", "x8664", "amd64", "arm64", "ia64", "win64", "win32", "64bit", "32bit", "bit",
];

/// 회사명 접미사: 게시자 비교에서 제외한다.
const LEGAL_SUFFIXES: &[&str] = &[
    "inc",
    "llc",
    "ltd",
    "limited",
    "corp",
    "corporation",
    "co",
    "company",
    "gmbh",
    "ag",
    "sa",
    "sas",
    "srl",
    "bv",
    "nv",
    "plc",
    "pty",
    "kk",
    "oy",
    "ab",
    "spa",
    "sro",
    "as",
    "lp",
    "llp",
];

/// 이 이름들은 별칭/게시자로 쓰기에 너무 일반적이다(오탐 방지).
const GENERIC_NAMES: &[&str] = &[
    "app",
    "apps",
    "application",
    "applications",
    "bin",
    "cache",
    "caches",
    "client",
    "common",
    "commonfiles",
    "components",
    "config",
    "core",
    "crashdumps",
    "crashpad",
    "data",
    "default",
    "desktop",
    "documents",
    "downloads",
    "drivers",
    "files",
    "games",
    "helper",
    "install",
    "installer",
    "installers",
    "launcher",
    "lib",
    "library",
    "local",
    "locallow",
    "logs",
    "log",
    "main",
    "microsoft",
    "music",
    "packages",
    "pictures",
    "plugin",
    "plugins",
    "program",
    "programdata",
    "programfiles",
    "programs",
    "public",
    "resources",
    "roaming",
    "runtime",
    "runtimes",
    "server",
    "service",
    "services",
    "settings",
    "setup",
    "shared",
    "software",
    "start",
    "startmenu",
    "system",
    "system32",
    "temp",
    "tmp",
    "tools",
    "uninstall",
    "update",
    "updater",
    "updates",
    "user",
    "users",
    "videos",
    "windows",
    "windowsapps",
    "the",
    "my",
    "new",
    "free",
    "open",
    "pro",
    "studio",
    "studios",
    "systems",
    "technologies",
    "tech",
    "labs",
    "group",
    "team",
    "project",
    "foundation",
    "digital",
    "media",
    "online",
    "global",
    "international",
    "development",
    "dev",
    "inc",
    "llc",
    "ltd",
    "corp",
    "corporation",
    "company",
];

/// 제거 프로그램/설치 관리자 계열 실행 파일 이름 — 별칭으로 쓰지 않는다.
const UNINSTALLER_STEMS: &[&str] = &[
    "unins",
    "uninst",
    "uninstall",
    "uninstaller",
    "setup",
    "install",
    "installer",
    "update",
    "updater",
    "helper",
    "maintenance",
    "remove",
    "msiexec",
    "rundll32",
    "cmd",
    "powershell",
    "squirrel",
    "au_",
    "un_a",
    "launcher",
    "elevate",
    "crashreporter",
    "bootstrapper",
];

fn is_generic(compact: &str) -> bool {
    GENERIC_NAMES.contains(&compact)
}

/// 괄호/대괄호 안의 내용(아키텍처·언어·버전 표기)을 제거한다.
fn strip_brackets(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut depth = 0usize;
    for ch in s.chars() {
        match ch {
            '(' | '[' | '{' => depth += 1,
            ')' | ']' | '}' => depth = depth.saturating_sub(1),
            c if depth == 0 => out.push(c),
            _ => {}
        }
    }
    out
}

/// 버전처럼 보이는 토큰(`2`, `10`, `v3`, `2024`)인지.
fn is_version_token(t: &str) -> bool {
    let digits = t.strip_prefix('v').unwrap_or(t);
    !digits.is_empty() && digits.chars().all(|c| c.is_ascii_digit())
}

/// 이름을 비교용 토큰 목록으로 정규화한다.
///
/// 영숫자와 `+`, `#`(예: `Notepad++`, `C#`)만 토큰 문자로 인정한다.
/// 첫 토큰을 제외한 버전 숫자와 아키텍처 표기는 버린다.
pub fn tokens(name: &str) -> Vec<String> {
    let stripped = strip_brackets(name);
    let mut out: Vec<String> = Vec::new();
    let mut cur = String::new();
    let flush = |cur: &mut String, out: &mut Vec<String>| {
        if cur.is_empty() {
            return;
        }
        let t = std::mem::take(cur);
        if NOISE_TOKENS.contains(&t.as_str()) {
            return;
        }
        if !out.is_empty() && is_version_token(&t) {
            return;
        }
        out.push(t);
    };
    for ch in stripped.chars() {
        if ch.is_alphanumeric() || ch == '+' || ch == '#' {
            cur.extend(ch.to_lowercase());
        } else {
            flush(&mut cur, &mut out);
        }
    }
    flush(&mut cur, &mut out);
    // 첫 토큰만 남았는데 그게 버전 숫자뿐이면 의미가 없다.
    if out.len() == 1 && is_version_token(&out[0]) && out[0].len() > 1 {
        out.clear();
    }
    out
}

/// 공백·기호를 모두 제거한 압축형.
pub fn compact(name: &str) -> String {
    tokens(name).concat()
}

/// 게시자 이름에서 회사 접미사를 뗀 토큰 목록.
fn publisher_tokens(publisher: &str) -> Vec<String> {
    let mut t = tokens(publisher);
    while t.len() > 1
        && t.last()
            .is_some_and(|l| LEGAL_SUFFIXES.contains(&l.as_str()))
    {
        t.pop();
    }
    t
}

/// 실행 파일 이름이 앱 별칭으로 쓸 만한지(제거 프로그램 이름 등은 제외).
pub fn is_meaningful_exe_stem(stem: &str) -> bool {
    let s = stem.to_lowercase();
    if s.len() < 3 {
        return false;
    }
    if UNINSTALLER_STEMS.iter().any(|u| s.starts_with(u)) {
        return false;
    }
    !is_generic(&compact(&s))
}

/// 한 앱을 가리킬 수 있는 이름들의 집합.
#[derive(Debug, Clone, Default)]
pub struct AppAliases {
    /// 압축형 별칭들(앱 이름, 게시자 접두어를 뗀 이름, 설치 폴더 이름, 실행 파일 이름).
    names: Vec<String>,
    /// 게시자 압축형(회사 접미사 제거).
    publisher: Option<String>,
    /// 게시자 첫 토큰(4자 이상, 앱 이름에도 등장하는 경우만).
    publisher_first: Option<String>,
}

impl AppAliases {
    pub fn new(display_name: &str, publisher: &str, extra_names: &[&str]) -> Self {
        let mut names: Vec<String> = Vec::new();
        let mut push = |c: String| {
            if c.chars().count() >= 3 && !is_generic(&c) && !names.contains(&c) {
                names.push(c);
            }
        };

        let name_tokens = tokens(display_name);
        push(name_tokens.concat());

        let pub_tokens = publisher_tokens(publisher);
        // "Mozilla Firefox" + 게시자 "Mozilla" → "Firefox"도 별칭.
        if let Some(first_pub) = pub_tokens.first()
            && name_tokens.len() > 1
            && name_tokens.first() == Some(first_pub)
        {
            let rest = name_tokens[1..].concat();
            if rest.chars().count() >= 4 {
                push(rest);
            }
        }
        for extra in extra_names {
            push(compact(extra));
        }

        let publisher_c = pub_tokens.concat();
        let publisher_c =
            (publisher_c.chars().count() >= 3 && !is_generic(&publisher_c)).then_some(publisher_c);
        let publisher_first = pub_tokens
            .first()
            .filter(|f| f.chars().count() >= 4 && !is_generic(f) && name_tokens.contains(f))
            .cloned();

        Self {
            names,
            publisher: publisher_c,
            publisher_first,
        }
    }

    #[cfg(test)]
    pub fn is_empty(&self) -> bool {
        self.names.is_empty()
    }

    /// 폴더/키 이름이 이 앱과 얼마나 일치하는지.
    pub fn classify(&self, candidate: &str) -> NameMatch {
        let c = compact(candidate);
        if c.chars().count() < 2 || is_generic(&c) {
            return NameMatch::None;
        }
        if self.names.contains(&c) {
            return NameMatch::Exact;
        }
        if self.publisher.as_deref() == Some(c.as_str())
            || self.publisher_first.as_deref() == Some(c.as_str())
        {
            return NameMatch::Publisher;
        }
        for n in &self.names {
            let (short, long) = if n.len() <= c.len() {
                (n.as_str(), c.as_str())
            } else {
                (c.as_str(), n.as_str())
            };
            if short.chars().count() >= 5
                && long.starts_with(short)
                && short.len() * 10 >= long.len() * 6
            {
                return NameMatch::Partial;
            }
        }
        NameMatch::None
    }
}

/// 다른 설치된 앱들의 별칭 집합 — 공유 폴더 오탐 방지용.
#[derive(Debug, Default)]
pub struct OtherApps {
    names: HashSet<String>,
}

impl OtherApps {
    pub fn add(&mut self, aliases: &AppAliases) {
        self.names.extend(aliases.names.iter().cloned());
    }

    /// 이 이름이 다른(아직 설치된) 앱의 별칭과 정확히 일치하는지.
    pub fn claims(&self, candidate: &str) -> bool {
        let c = compact(candidate);
        !c.is_empty() && self.names.contains(&c)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tokenizes_versions_arch_and_brackets() {
        assert_eq!(
            tokens("Mozilla Firefox (x64 ko)"),
            vec!["mozilla", "firefox"]
        );
        assert_eq!(tokens("7-Zip 23.01 (x64)"), vec!["7", "zip"]);
        assert_eq!(tokens("GIMP 2.10.36"), vec!["gimp"]);
        assert_eq!(tokens("Notepad++ (64-bit x64)"), vec!["notepad++"]);
        assert_eq!(tokens("Python 3.12.1 (64-bit)"), vec!["python"]);
        assert_eq!(tokens("Node.js"), vec!["node", "js"]);
        assert_eq!(tokens("카카오톡"), vec!["카카오톡"]);
        assert_eq!(compact("OBS Studio"), "obsstudio");
        assert_eq!(compact("obs-studio"), "obsstudio");
        assert_eq!(compact("Visual Studio Code"), compact("VisualStudioCode"));
    }

    #[test]
    fn exact_and_publisher_matches() {
        let a = AppAliases::new(
            "Mozilla Firefox (x64 ko)",
            "Mozilla",
            &["Mozilla Firefox", "firefox"],
        );
        assert_eq!(a.classify("Mozilla Firefox"), NameMatch::Exact);
        assert_eq!(a.classify("Firefox"), NameMatch::Exact);
        assert_eq!(a.classify("Mozilla"), NameMatch::Publisher);
        assert_eq!(a.classify("Thunderbird"), NameMatch::None);

        let z = AppAliases::new("Zoom Workplace", "Zoom Video Communications, Inc.", &[]);
        assert_eq!(z.classify("Zoom"), NameMatch::Publisher);
        assert_eq!(z.classify("ZoomWorkplace"), NameMatch::Exact);
        assert_eq!(
            z.classify("Zoom Video Communications"),
            NameMatch::Publisher
        );
    }

    #[test]
    fn generic_and_short_names_never_match() {
        let a = AppAliases::new("Update", "Microsoft Corporation", &["setup"]);
        assert!(a.is_empty());
        assert_eq!(a.classify("Update"), NameMatch::None);
        assert_eq!(a.classify("Microsoft"), NameMatch::None);
        let b = AppAliases::new("Steam", "Valve Corporation", &[]);
        assert_eq!(b.classify("Temp"), NameMatch::None);
        assert_eq!(b.classify("Packages"), NameMatch::None);
    }

    #[test]
    fn partial_matches_require_length_and_ratio() {
        let a = AppAliases::new("Discord PTB", "Discord Inc.", &[]);
        assert_eq!(a.classify("discordptb"), NameMatch::Exact);
        // 게시자 폴더이기도 하므로 Publisher로 분류되어 하위 항목을 다시 확인한다.
        assert_eq!(a.classify("Discord"), NameMatch::Publisher);
        let c = AppAliases::new("Blender 4.2", "Blender Foundation", &[]);
        assert_eq!(c.classify("Blender"), NameMatch::Exact);
        let d = AppAliases::new("Bandizip", "Bandisoft.com", &[]);
        assert_eq!(d.classify("Bandizip64"), NameMatch::Partial);
        assert_eq!(d.classify("Bandizip Settings Backup"), NameMatch::None);
        let b = AppAliases::new("Krita", "Krita Foundation", &[]);
        // 너무 길이 차이가 나는 접두사는 부분 일치로 보지 않는다.
        assert_eq!(b.classify("KritaPluginsAndExtraThings"), NameMatch::None);
    }

    #[test]
    fn exe_stems_filter_uninstallers() {
        assert!(!is_meaningful_exe_stem("unins000"));
        assert!(!is_meaningful_exe_stem("Uninstall"));
        assert!(!is_meaningful_exe_stem("Update"));
        assert!(is_meaningful_exe_stem("firefox"));
        assert!(is_meaningful_exe_stem("Code"));
    }

    #[test]
    fn other_apps_claim_shared_names() {
        let mut others = OtherApps::default();
        others.add(&AppAliases::new("Mozilla Thunderbird", "Mozilla", &[]));
        assert!(others.claims("Thunderbird"));
        assert!(others.claims("Mozilla Thunderbird"));
        assert!(!others.claims("Firefox"));
    }
}
