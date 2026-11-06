//! 삭제 안전 가드 (순수 로직).
//!
//! 잘못된 `InstallLocation`(예: `C:\Program Files` 자체)이나 이름 오탐 때문에 시스템/사용자
//! 데이터를 지우는 일이 절대 없도록, 모든 삭제 직전에 이 가드를 다시 통과시킨다.

use std::collections::HashSet;

use crate::pathutil::{self, key_within, path_key};

/// 파일 시스템 삭제 가드.
#[derive(Debug, Default, Clone)]
pub struct PathGuard {
    /// 그 자체는 절대 지우면 안 되는 경로(하위 항목은 허용).
    exact: HashSet<String>,
    /// 하위 전체를 지우면 안 되는 경로.
    deny_within: Vec<String>,
    /// `deny_within`보다 우선해 허용하는 하위 트리(예: 시작 메뉴 바로가기).
    allow_within: Vec<String>,
}

impl PathGuard {
    pub fn new() -> Self {
        Self::default()
    }

    /// 이 경로 자체는 지우지 않는다.
    pub fn protect(&mut self, path: &str) {
        if !path.trim().is_empty() {
            self.exact.insert(path_key(path));
        }
    }

    /// 이 경로와 그 하위 전체를 지우지 않는다.
    pub fn deny_tree(&mut self, path: &str) {
        if !path.trim().is_empty() {
            self.deny_within.push(path_key(path));
        }
    }

    /// 거부 트리 안이라도 이 하위 트리는 허용한다(단, `protect`된 경로 자체는 여전히 금지).
    pub fn allow_tree(&mut self, path: &str) {
        if !path.trim().is_empty() {
            self.allow_within.push(path_key(path));
        }
    }

    /// 이 경로를 삭제해도 되는지.
    pub fn can_delete(&self, path: &str) -> bool {
        let k = path_key(path);
        if !pathutil::is_absolute(&k) || pathutil::is_root_like(&k) {
            return false;
        }
        // 경로 탐색 문자열이 섞인 비정상 경로는 거부.
        if k.split('\\').any(|c| c == "." || c == "..") {
            return false;
        }
        if self.exact.contains(&k) {
            return false;
        }
        // 보호 경로의 "상위"를 지우는 것도 금지(예: 사용자 프로필 상위 폴더).
        if self.exact.iter().any(|e| key_within(e, &k))
            || self.deny_within.iter().any(|d| key_within(d, &k))
        {
            return false;
        }
        if self.allow_within.iter().any(|a| key_within(&k, a)) {
            return true;
        }
        !self.deny_within.iter().any(|d| key_within(&k, d))
    }
}

/// 레지스트리 하이브.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Hive {
    LocalMachine,
    CurrentUser,
}

impl Hive {
    pub fn long_name(self) -> &'static str {
        match self {
            Hive::LocalMachine => "HKEY_LOCAL_MACHINE",
            Hive::CurrentUser => "HKEY_CURRENT_USER",
        }
    }

    pub fn short_name(self) -> &'static str {
        match self {
            Hive::LocalMachine => "HKLM",
            Hive::CurrentUser => "HKCU",
        }
    }
}

/// `Software` 바로 아래에서 절대 지우지 않는 키(소문자).
const PROTECTED_SOFTWARE_CHILDREN: &[&str] = &[
    "microsoft",
    "classes",
    "clients",
    "policies",
    "registeredapplications",
    "wow6432node",
    "odbc",
    "windows",
    "appdatalow",
    "partner",
    "oem",
    "setup",
    "defaultuserenvironment",
    "description",
    "khronos",
];

fn components(path: &str) -> Vec<String> {
    path.split('\\')
        .filter(|c| !c.is_empty())
        .map(str::to_lowercase)
        .collect()
}

fn is_packed_guid(s: &str) -> bool {
    s.len() == 32 && s.chars().all(|c| c.is_ascii_hexdigit())
}

/// `Software` 계열 루트 길이(구성요소 수)를 반환한다. 루트가 아니면 `None`.
fn software_root_len(c: &[String]) -> Option<usize> {
    match c {
        [s, a, s2, ..] if s == "software" && a == "appdatalow" && s2 == "software" => Some(3),
        [s, w, ..] if s == "software" && w == "wow6432node" => Some(2),
        [s, ..] if s == "software" => Some(1),
        _ => None,
    }
}

/// 이 레지스트리 키(하위 트리 포함)를 삭제해도 되는지.
pub fn registry_key_deletable(hive: Hive, path: &str) -> bool {
    let c = components(path);
    let Some(root) = software_root_len(&c) else {
        return false;
    };
    let rest = &c[root..];
    let Some(first) = rest.first() else {
        return false; // Software 자체
    };

    // Microsoft 아래는 정해진 위치만 허용.
    if first == "microsoft" {
        let tail: Vec<&str> = rest.iter().map(String::as_str).collect();
        return match tail.as_slice() {
            ["microsoft", "windows", "currentversion", "uninstall", _k] => true,
            ["microsoft", "windows", "currentversion", "app paths", _k] => true,
            [
                "microsoft",
                "windows",
                "currentversion",
                "installer",
                "userdata",
                _sid,
                "products",
                p,
            ] => hive == Hive::LocalMachine && root == 1 && is_packed_guid(p),
            ["microsoft", "installer", "products" | "features", p] => {
                hive == Hive::CurrentUser && root == 1 && is_packed_guid(p)
            }
            _ => false,
        };
    }
    if first == "classes" {
        let tail: Vec<&str> = rest.iter().map(String::as_str).collect();
        return match tail.as_slice() {
            ["classes", "installer", "products" | "features", p] => {
                hive == Hive::LocalMachine && root == 1 && is_packed_guid(p)
            }
            _ => false,
        };
    }
    !PROTECTED_SOFTWARE_CHILDREN.contains(&first.as_str())
}

/// 값 삭제가 허용되는 키(소문자 경로, Software 기준) — Run 계열·방화벽 규칙.
pub fn registry_value_deletable(hive: Hive, key_path: &str) -> bool {
    let c = components(key_path);
    let tail: Vec<&str> = c.iter().map(String::as_str).collect();
    match tail.as_slice() {
        [
            "software",
            "microsoft",
            "windows",
            "currentversion",
            "run" | "runonce",
        ] => true,
        [
            "software",
            "wow6432node",
            "microsoft",
            "windows",
            "currentversion",
            "run" | "runonce",
        ] => true,
        [
            "software",
            "microsoft",
            "windows",
            "currentversion",
            "explorer",
            "startupapproved",
            "run" | "run32",
        ] => true,
        [
            "system",
            "currentcontrolset",
            "services",
            "sharedaccess",
            "parameters",
            "firewallpolicy",
            "firewallrules",
        ] => hive == Hive::LocalMachine,
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn guard() -> PathGuard {
        let mut g = PathGuard::new();
        g.protect(r"C:\Program Files");
        g.protect(r"C:\Program Files\Common Files");
        g.protect(r"C:\Users\kim");
        g.protect(r"C:\Users\kim\AppData\Roaming");
        g.protect(r"C:\ProgramData\Microsoft\Windows\Start Menu\Programs");
        g.deny_tree(r"C:\Windows");
        g.deny_tree(r"C:\ProgramData\Microsoft");
        g.allow_tree(r"C:\ProgramData\Microsoft\Windows\Start Menu\Programs");
        g
    }

    #[test]
    fn blocks_roots_protected_and_denied() {
        let g = guard();
        assert!(!g.can_delete(r"C:\"));
        assert!(!g.can_delete(r"C:\Program Files"));
        assert!(!g.can_delete(r"c:\program files\"));
        assert!(!g.can_delete(r"C:\Windows\System32\foo.dll"));
        assert!(!g.can_delete(r"C:\Users"));
        assert!(!g.can_delete(r"C:\Users\kim\AppData\Roaming"));
        assert!(!g.can_delete(r"C:\ProgramData\Microsoft\Crypto"));
        assert!(!g.can_delete(r"relative\path"));
        assert!(!g.can_delete(r"C:\Program Files\Foo\..\..\Windows"));
    }

    #[test]
    fn allows_app_folders_and_start_menu_entries() {
        let g = guard();
        assert!(g.can_delete(r"C:\Program Files\Foo"));
        assert!(g.can_delete(r"C:\Users\kim\AppData\Roaming\Foo"));
        assert!(g.can_delete(r"C:\ProgramData\Microsoft\Windows\Start Menu\Programs\Foo\Foo.lnk"));
        assert!(!g.can_delete(r"C:\ProgramData\Microsoft\Windows\Start Menu\Programs"));
    }

    #[test]
    fn registry_key_rules() {
        use Hive::*;
        assert!(registry_key_deletable(CurrentUser, r"Software\Foo"));
        assert!(registry_key_deletable(
            LocalMachine,
            r"SOFTWARE\WOW6432Node\Foo\Bar"
        ));
        assert!(registry_key_deletable(
            CurrentUser,
            r"Software\AppDataLow\Software\Foo"
        ));
        assert!(!registry_key_deletable(CurrentUser, r"Software"));
        assert!(!registry_key_deletable(LocalMachine, r"SOFTWARE\Microsoft"));
        assert!(!registry_key_deletable(
            LocalMachine,
            r"SOFTWARE\Microsoft\Windows"
        ));
        assert!(!registry_key_deletable(LocalMachine, r"SOFTWARE\Classes"));
        assert!(!registry_key_deletable(
            LocalMachine,
            r"SOFTWARE\Policies\Foo"
        ));
        assert!(!registry_key_deletable(
            LocalMachine,
            r"SYSTEM\CurrentControlSet\Services\Foo"
        ));
        assert!(registry_key_deletable(
            LocalMachine,
            r"SOFTWARE\Microsoft\Windows\CurrentVersion\Uninstall\Foo_is1"
        ));
        assert!(registry_key_deletable(
            LocalMachine,
            r"SOFTWARE\WOW6432Node\Microsoft\Windows\CurrentVersion\Uninstall\{GUID}"
        ));
        assert!(!registry_key_deletable(
            LocalMachine,
            r"SOFTWARE\Microsoft\Windows\CurrentVersion\Uninstall"
        ));
        assert!(registry_key_deletable(
            LocalMachine,
            r"SOFTWARE\Microsoft\Windows\CurrentVersion\App Paths\foo.exe"
        ));
        let packed = "00002109110000000000000000F01FEC";
        assert!(registry_key_deletable(
            LocalMachine,
            &format!(r"SOFTWARE\Classes\Installer\Products\{packed}")
        ));
        assert!(!registry_key_deletable(
            LocalMachine,
            r"SOFTWARE\Classes\Installer\Products"
        ));
        assert!(registry_key_deletable(
            LocalMachine,
            &format!(
                r"SOFTWARE\Microsoft\Windows\CurrentVersion\Installer\UserData\S-1-5-18\Products\{packed}"
            )
        ));
        assert!(registry_key_deletable(
            CurrentUser,
            &format!(r"Software\Microsoft\Installer\Products\{packed}")
        ));
        assert!(!registry_key_deletable(
            CurrentUser,
            r"Software\Microsoft\Installer\Products\notpacked"
        ));
    }

    #[test]
    fn registry_value_rules() {
        use Hive::*;
        assert!(registry_value_deletable(
            CurrentUser,
            r"Software\Microsoft\Windows\CurrentVersion\Run"
        ));
        assert!(registry_value_deletable(
            LocalMachine,
            r"SOFTWARE\WOW6432Node\Microsoft\Windows\CurrentVersion\RunOnce"
        ));
        assert!(registry_value_deletable(
            LocalMachine,
            r"SYSTEM\CurrentControlSet\Services\SharedAccess\Parameters\FirewallPolicy\FirewallRules"
        ));
        assert!(!registry_value_deletable(
            CurrentUser,
            r"Software\Microsoft\Windows\CurrentVersion"
        ));
        assert!(!registry_value_deletable(CurrentUser, r"Environment"));
    }
}
