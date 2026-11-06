//! 잔여물 검색: 제거 프로그램이 남긴 파일·폴더·레지스트리·바로가기·서비스·예약 작업·
//! 시작 프로그램·방화벽 규칙을 찾아 신뢰도와 함께 돌려준다.
//!
//! 신뢰도 기준
//! - **확실(Certain)**: 앱 설치 폴더 자체, 또는 설치 폴더를 직접 가리키는 항목
//!   (바로가기·서비스·예약 작업·Run 값·방화벽 규칙·App Paths), 앱의 제거 정보 키.
//! - **높음(Likely)**: 이름이 앱과 정확히 일치하는 데이터 폴더/레지스트리 키.
//! - **낮음(Possible)**: 부분 일치, 문서 폴더 등 — 기본 선택하지 않는다.

use std::collections::HashSet;

use rayon::prelude::*;

use crate::cmdline::{packed_guid, split_command};
use crate::inventory::{AppInfo, Context, DirTrust, Source};
use crate::pathutil::{self, join, key_within, path_key, text_mentions_dir};
use crate::safety::{Hive, registry_key_deletable, registry_value_deletable};
use crate::textmatch::{NameMatch, OtherApps};
use crate::win::shell::ShortcutResolver;
use crate::win::{ComGuard, expand_env, fsops, reg, services, tasks};

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Confidence {
    Possible = 0,
    Likely = 1,
    Certain = 2,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LeftoverKind {
    Dir(String),
    File(String),
    RegKey {
        hive: Hive,
        path: String,
    },
    RegValue {
        hive: Hive,
        key: String,
        name: String,
    },
    Service {
        name: String,
    },
    Task {
        path: String,
        file: String,
    },
}

impl LeftoverKind {
    fn order(&self) -> u8 {
        match self {
            LeftoverKind::Dir(_) => 0,
            LeftoverKind::File(_) => 1,
            LeftoverKind::RegKey { .. } => 2,
            LeftoverKind::RegValue { .. } => 3,
            LeftoverKind::Service { .. } => 4,
            LeftoverKind::Task { .. } => 5,
        }
    }

    /// 사람이 읽는 위치 문자열.
    pub fn location(&self) -> String {
        match self {
            LeftoverKind::Dir(p) | LeftoverKind::File(p) => p.clone(),
            LeftoverKind::RegKey { hive, path } => format!("{}\\{path}", hive.short_name()),
            LeftoverKind::RegValue { hive, key, name } => {
                format!("{}\\{key} → {name}", hive.short_name())
            }
            LeftoverKind::Service { name } => format!("서비스: {name}"),
            LeftoverKind::Task { path, .. } => format!("작업 스케줄러: {path}"),
        }
    }
}

#[derive(Debug, Clone)]
pub struct Leftover {
    pub kind: LeftoverKind,
    pub confidence: Confidence,
    pub title: String,
    pub reason: String,
    pub size: u64,
}

pub struct ScanRequest {
    pub app: AppInfo,
    /// 이 앱을 제외한 현재 설치된 앱들(공유 폴더 보호용).
    pub others: Vec<AppInfo>,
}

const RUN_KEYS: &[(Hive, &str)] = &[
    (
        Hive::CurrentUser,
        r"Software\Microsoft\Windows\CurrentVersion\Run",
    ),
    (
        Hive::CurrentUser,
        r"Software\Microsoft\Windows\CurrentVersion\RunOnce",
    ),
    (
        Hive::LocalMachine,
        r"SOFTWARE\Microsoft\Windows\CurrentVersion\Run",
    ),
    (
        Hive::LocalMachine,
        r"SOFTWARE\Microsoft\Windows\CurrentVersion\RunOnce",
    ),
    (
        Hive::LocalMachine,
        r"SOFTWARE\WOW6432Node\Microsoft\Windows\CurrentVersion\Run",
    ),
    (
        Hive::LocalMachine,
        r"SOFTWARE\WOW6432Node\Microsoft\Windows\CurrentVersion\RunOnce",
    ),
];

const SOFTWARE_ROOTS: &[(Hive, &str)] = &[
    (Hive::CurrentUser, "Software"),
    (Hive::CurrentUser, r"Software\WOW6432Node"),
    (Hive::CurrentUser, r"Software\AppDataLow\Software"),
    (Hive::LocalMachine, "SOFTWARE"),
    (Hive::LocalMachine, r"SOFTWARE\WOW6432Node"),
];

const APP_PATHS: &[(Hive, &str)] = &[
    (
        Hive::LocalMachine,
        r"SOFTWARE\Microsoft\Windows\CurrentVersion\App Paths",
    ),
    (
        Hive::LocalMachine,
        r"SOFTWARE\WOW6432Node\Microsoft\Windows\CurrentVersion\App Paths",
    ),
    (
        Hive::CurrentUser,
        r"Software\Microsoft\Windows\CurrentVersion\App Paths",
    ),
];

pub const FIREWALL_RULES: &str =
    r"SYSTEM\CurrentControlSet\Services\SharedAccess\Parameters\FirewallPolicy\FirewallRules";

struct Scanner<'a> {
    app: &'a AppInfo,
    ctx: &'a Context,
    others: OtherApps,
    /// 다른 앱의 설치 폴더·실행 파일 폴더(정규화 키).
    other_dirs: Vec<String>,
    /// 이 앱의 설치 폴더(정규화 키) — 확실한 근거.
    app_dirs: Vec<String>,
    /// MSI 설치 관리자 캐시(`C:\Windows\Installer\{코드}`) — 고급 바로가기 판정용.
    msi_cache: Option<String>,
    out: Vec<Leftover>,
}

impl<'a> Scanner<'a> {
    fn new(req: &'a ScanRequest, ctx: &'a Context) -> Self {
        let app = &req.app;
        let mut others = OtherApps::default();
        let mut other_dirs = Vec::new();
        for o in &req.others {
            others.add(&o.aliases);
            if let Some(d) = &o.install_dir
                && !o.is_store()
            {
                other_dirs.push(path_key(d));
            }
            for f in &o.evidence_files {
                if let Some(p) = pathutil::parent(f) {
                    other_dirs.push(path_key(p));
                }
            }
        }
        let mut s = Self {
            app,
            ctx,
            others,
            other_dirs,
            app_dirs: Vec::new(),
            msi_cache: app
                .uninstall
                .msi_product_code
                .as_ref()
                .map(|c| path_key(&join(&ctx.folders.windows, &format!("Installer\\{c}")))),
            out: Vec::new(),
        };
        // 연관을 확인하지 못한 설치 폴더는 "확실" 판정의 근거로 쓰지 않는다.
        if let Some(d) = &app.install_dir
            && !app.is_store()
            && app.install_dir_trust != DirTrust::Unverified
            && !s.touches_other(d)
            && ctx.guard.can_delete(d)
        {
            s.app_dirs.push(path_key(d));
        }
        s
    }

    /// 후보 폴더 안(또는 같은 위치)에 다른 설치된 앱의 파일이 있는지.
    fn touches_other(&self, dir: &str) -> bool {
        let k = path_key(dir);
        self.other_dirs.iter().any(|o| key_within(o, &k))
    }

    /// 경로가 이 앱의 설치 폴더(또는 MSI 캐시) 안인지.
    fn in_app_dirs(&self, path: &str) -> bool {
        if path.trim().is_empty() {
            return false;
        }
        let k = path_key(path);
        self.app_dirs.iter().any(|d| key_within(&k, d))
            || self.msi_cache.as_ref().is_some_and(|m| key_within(&k, m))
    }

    /// 명령줄(실행 파일 + 인자)이 이 앱 폴더를 참조하는지.
    fn command_refs_app(&self, command: &str) -> bool {
        let expanded = expand_env(command);
        let (exe, _) = split_command(&expanded);
        self.in_app_dirs(&expand_env(&exe))
            || self
                .app_dirs
                .iter()
                .any(|d| text_mentions_dir(&expanded, d))
    }

    fn push(
        &mut self,
        kind: LeftoverKind,
        confidence: Confidence,
        title: impl Into<String>,
        reason: &str,
    ) {
        self.out.push(Leftover {
            kind,
            confidence,
            title: title.into(),
            reason: reason.to_string(),
            size: 0,
        });
    }

    fn consider_dir(&mut self, path: &str, confidence: Confidence, reason: &str) {
        if !self.ctx.guard.can_delete(path) || self.touches_other(path) {
            return;
        }
        if self.others.claims(pathutil::file_name(path)) && !self.in_app_dirs(path) {
            return;
        }
        self.push(
            LeftoverKind::Dir(path.to_string()),
            confidence,
            pathutil::file_name(path),
            reason,
        );
    }

    // ── 1. 설치 폴더 ─────────────────────────────────────────────
    fn install_dir(&mut self) {
        let Some(dir) = self.app.install_dir.clone() else {
            return;
        };
        if self.app.is_store() || !fsops::is_dir_path(&dir) {
            return;
        }
        let conf = match self.app.install_dir_trust {
            DirTrust::Verified => Confidence::Certain,
            DirTrust::Derived => Confidence::Likely,
            DirTrust::Unverified => Confidence::Possible,
        };
        self.consider_dir(&dir, conf, "설치 폴더");
    }

    // ── 2. Store 앱 데이터 ───────────────────────────────────────
    fn store_data(&mut self) {
        if let Source::Store { family_name, .. } = &self.app.source {
            let p = join(&self.ctx.folders.packages, family_name);
            if fsops::is_dir_path(&p) && self.ctx.guard.can_delete(&p) {
                self.push(
                    LeftoverKind::Dir(p),
                    Confidence::Certain,
                    family_name.clone(),
                    "Store 앱 데이터 폴더",
                );
            }
        }
    }

    // ── 3. 이름이 일치하는 데이터 폴더 ───────────────────────────
    fn named_dirs(&mut self) {
        let f = &self.ctx.folders;
        let roots: Vec<(String, Confidence)> = [
            (&f.roaming, Confidence::Likely),
            (&f.local, Confidence::Likely),
            (&f.local_low, Confidence::Likely),
            (&f.program_data, Confidence::Likely),
            (&f.program_files, Confidence::Likely),
            (&f.program_files_x86, Confidence::Likely),
            (&f.local_programs, Confidence::Likely),
            (&f.common_files, Confidence::Likely),
            (&f.common_files_x86, Confidence::Likely),
            (&f.documents, Confidence::Possible),
            (&f.public_documents, Confidence::Possible),
        ]
        .into_iter()
        .filter(|(r, _)| !r.is_empty())
        .map(|(r, c)| (r.clone(), c))
        .collect();
        // Microsoft 게시자 앱 데이터(예: Local\Microsoft\Teams)는 Microsoft 폴더 안에 있다.
        let ms_roots: Vec<String> = [&f.local, &f.roaming]
            .into_iter()
            .filter(|r| !r.is_empty())
            .map(|r| join(r, "Microsoft"))
            .collect();

        for (root, cap) in roots {
            self.scan_named_children(&root, cap);
        }
        if self.app.publisher.to_lowercase().contains("microsoft") {
            for root in ms_roots {
                for name in fsops::subdir_names(&root) {
                    if self.app.aliases.classify(&name) == NameMatch::Exact {
                        self.consider_dir(
                            &join(&root, &name),
                            Confidence::Likely,
                            "앱 이름과 일치하는 데이터 폴더",
                        );
                    }
                }
            }
        }
    }

    fn scan_named_children(&mut self, root: &str, cap: Confidence) {
        for name in fsops::subdir_names(root) {
            let path = join(root, &name);
            match self.app.aliases.classify(&name) {
                NameMatch::Exact => self.consider_dir(
                    &path,
                    cap.min(Confidence::Likely),
                    "앱 이름과 일치하는 폴더",
                ),
                NameMatch::Partial => {
                    self.consider_dir(&path, Confidence::Possible, "앱 이름과 일부 일치하는 폴더")
                }
                NameMatch::Publisher => self.publisher_dir(&path, cap),
                NameMatch::None => {}
            }
        }
    }

    fn publisher_dir(&mut self, path: &str, cap: Confidence) {
        let entries = fsops::entries(path);
        if entries.is_empty() {
            self.consider_dir(path, cap.min(Confidence::Likely), "비어 있는 게시자 폴더");
            return;
        }
        let matched: Vec<(String, Confidence)> = entries
            .iter()
            .filter(|(_, is_dir)| *is_dir)
            .filter_map(|(name, _)| match self.app.aliases.classify(name) {
                NameMatch::Exact if !self.others.claims(name) => {
                    Some((name.clone(), Confidence::Likely))
                }
                NameMatch::Partial if !self.others.claims(name) => {
                    Some((name.clone(), Confidence::Possible))
                }
                _ => None,
            })
            .collect();
        let only_this_app =
            matched.len() == entries.len() && matched.iter().all(|(_, c)| *c == Confidence::Likely);
        if only_this_app {
            self.consider_dir(
                path,
                cap.min(Confidence::Likely),
                "이 앱 데이터만 들어 있는 게시자 폴더",
            );
        } else {
            for (name, conf) in matched {
                self.consider_dir(
                    &join(path, &name),
                    cap.min(conf),
                    "게시자 폴더 안의 앱 폴더",
                );
            }
        }
    }

    // ── 4. 바로가기 ─────────────────────────────────────────────
    fn shortcuts(&mut self) {
        let f = &self.ctx.folders;
        let link_roots: Vec<String> = [
            &f.common_programs,
            &f.programs,
            &f.desktop,
            &f.public_desktop,
            &f.quick_launch,
        ]
        .into_iter()
        .filter(|r| !r.is_empty())
        .cloned()
        .collect();
        let start_roots: Vec<String> = [&f.common_programs, &f.programs]
            .into_iter()
            .filter(|r| !r.is_empty())
            .cloned()
            .collect();

        let _com = ComGuard::mta();
        let resolver = ShortcutResolver::new();
        let target_of = |lnk: &str| resolver.as_ref().and_then(|r| r.target(lnk));

        // 시작 메뉴의 앱 이름 폴더(안의 바로가기가 모두 이 앱 것일 때만).
        for root in &start_roots {
            for name in fsops::subdir_names(root) {
                if self.app.aliases.classify(&name) != NameMatch::Exact || self.others.claims(&name)
                {
                    continue;
                }
                let dir = join(root, &name);
                let all_ours =
                    fsops::find_files(&dir, &[".lnk"], 3)
                        .iter()
                        .all(|l| match target_of(l) {
                            Some((t, args)) => {
                                t.is_empty()
                                    || self.command_refs_app(&format!("\"{t}\" {args}"))
                                    || !fsops::exists(&t)
                            }
                            None => true,
                        });
                if all_ours {
                    self.consider_dir(&dir, Confidence::Likely, "시작 메뉴 폴더");
                }
            }
        }

        for root in &link_roots {
            for file in fsops::find_files(root, &[".lnk", ".url"], 6) {
                if !self.ctx.guard.can_delete(&file) {
                    continue;
                }
                let stem = pathutil::file_stem(&file).to_string();
                let name_exact = self.app.aliases.classify(&stem) == NameMatch::Exact
                    && !self.others.claims(&stem);
                if file.to_lowercase().ends_with(".url") {
                    if name_exact {
                        self.push(
                            LeftoverKind::File(file.clone()),
                            Confidence::Possible,
                            stem,
                            "앱 이름의 인터넷 바로가기",
                        );
                    }
                    continue;
                }
                let Some((target, args)) = target_of(&file) else {
                    continue;
                };
                if !target.is_empty() && self.command_refs_app(&format!("\"{target}\" {args}")) {
                    self.push(
                        LeftoverKind::File(file.clone()),
                        Confidence::Certain,
                        stem,
                        "앱을 가리키는 바로가기",
                    );
                } else if name_exact && (target.is_empty() || !fsops::exists(&target)) {
                    self.push(
                        LeftoverKind::File(file.clone()),
                        Confidence::Likely,
                        stem,
                        "대상이 사라진 앱 바로가기",
                    );
                }
            }
        }
    }

    // ── 5. 레지스트리 ───────────────────────────────────────────
    fn consider_key(&mut self, hive: Hive, path: &str, confidence: Confidence, reason: &str) {
        if !registry_key_deletable(hive, path) {
            return;
        }
        let leaf = path.rsplit('\\').next().unwrap_or(path).to_string();
        self.push(
            LeftoverKind::RegKey {
                hive,
                path: path.to_string(),
            },
            confidence,
            leaf,
            reason,
        );
    }

    fn uninstall_entry(&mut self) {
        if let Source::Win32 {
            hive,
            key_path,
            key_name,
        } = &self.app.source
            && reg::key_exists(*hive, key_path)
        {
            let (hive, key_path, key_name) = (*hive, key_path.clone(), key_name.clone());
            if registry_key_deletable(hive, &key_path) {
                self.push(
                    LeftoverKind::RegKey {
                        hive,
                        path: key_path,
                    },
                    Confidence::Certain,
                    key_name,
                    "프로그램 목록(제거 정보) 항목",
                );
            }
        }
    }

    fn software_keys(&mut self) {
        for (hive, root) in SOFTWARE_ROOTS {
            let Ok(key) = reg::open(*hive, root) else {
                continue;
            };
            for sub in reg::subkeys(&key) {
                let path = format!(r"{root}\{sub}");
                match self.app.aliases.classify(&sub) {
                    NameMatch::Exact if !self.others.claims(&sub) => {
                        self.consider_key(
                            *hive,
                            &path,
                            Confidence::Likely,
                            "앱 이름과 일치하는 레지스트리 키",
                        );
                    }
                    NameMatch::Partial if !self.others.claims(&sub) => {
                        self.consider_key(
                            *hive,
                            &path,
                            Confidence::Possible,
                            "앱 이름과 일부 일치하는 레지스트리 키",
                        );
                    }
                    NameMatch::Publisher => self.publisher_key(*hive, &path),
                    _ => {}
                }
            }
        }
    }

    fn publisher_key(&mut self, hive: Hive, path: &str) {
        let Ok(key) = reg::open(hive, path) else {
            return;
        };
        let children = reg::subkeys(&key);
        let values = reg::key_info(hive, path).map_or(1, |(_, v)| v);
        if children.is_empty() && values == 0 {
            self.consider_key(hive, path, Confidence::Likely, "비어 있는 게시자 키");
            return;
        }
        let matched: Vec<(String, Confidence)> = children
            .iter()
            .filter_map(|c| match self.app.aliases.classify(c) {
                NameMatch::Exact if !self.others.claims(c) => Some((c.clone(), Confidence::Likely)),
                NameMatch::Partial if !self.others.claims(c) => {
                    Some((c.clone(), Confidence::Possible))
                }
                _ => None,
            })
            .collect();
        if values == 0
            && matched.len() == children.len()
            && matched.iter().all(|(_, c)| *c == Confidence::Likely)
        {
            self.consider_key(
                hive,
                path,
                Confidence::Likely,
                "이 앱 설정만 들어 있는 게시자 키",
            );
        } else {
            for (c, conf) in matched {
                self.consider_key(hive, &format!(r"{path}\{c}"), conf, "게시자 키 안의 앱 키");
            }
        }
    }

    fn app_paths(&mut self) {
        for (hive, root) in APP_PATHS {
            let Ok(key) = reg::open(*hive, root) else {
                continue;
            };
            for sub in reg::subkeys(&key) {
                let Ok(k) = key.open_subkey(&sub) else {
                    continue;
                };
                let exe = reg::get_string(&k, "").unwrap_or_default();
                let dir = reg::get_string(&k, "Path").unwrap_or_default();
                if self.command_refs_app(&exe) || self.in_app_dirs(&expand_env(&dir)) {
                    self.consider_key(
                        *hive,
                        &format!(r"{root}\{sub}"),
                        Confidence::Certain,
                        "실행 경로 등록(App Paths)",
                    );
                }
            }
        }
    }

    fn run_values(&mut self) {
        for (hive, key_path) in RUN_KEYS {
            let Ok(key) = reg::open(*hive, key_path) else {
                continue;
            };
            for (name, data) in reg::string_values(&key) {
                if !registry_value_deletable(*hive, key_path) {
                    continue;
                }
                let (exe, _) = split_command(&expand_env(&data));
                let exe = expand_env(&exe);
                let conf = if self.command_refs_app(&data) {
                    Some(Confidence::Certain)
                } else if self.app.aliases.classify(&name) == NameMatch::Exact
                    && !self.others.claims(&name)
                    && pathutil::is_absolute(&exe)
                    && !fsops::exists(&exe)
                {
                    Some(Confidence::Likely)
                } else {
                    None
                };
                if let Some(c) = conf {
                    self.push(
                        LeftoverKind::RegValue {
                            hive: *hive,
                            key: key_path.to_string(),
                            name: name.clone(),
                        },
                        c,
                        name,
                        "시작 프로그램 등록",
                    );
                }
            }
        }
    }

    fn firewall_rules(&mut self) {
        if self.app_dirs.is_empty() {
            return;
        }
        let Ok(key) = reg::open(Hive::LocalMachine, FIREWALL_RULES) else {
            return;
        };
        for (value_name, data) in reg::string_values(&key) {
            let mut app_path = None;
            let mut rule_name = None;
            for field in data.split('|') {
                if let Some(v) = field.strip_prefix("App=") {
                    app_path = Some(expand_env(v));
                } else if let Some(v) = field.strip_prefix("Name=") {
                    rule_name = Some(v.to_string());
                }
            }
            if let Some(p) = app_path
                && self.in_app_dirs(&p)
            {
                let title = rule_name
                    .filter(|n| !n.starts_with('@'))
                    .unwrap_or_else(|| value_name.clone());
                self.push(
                    LeftoverKind::RegValue {
                        hive: Hive::LocalMachine,
                        key: FIREWALL_RULES.to_string(),
                        name: value_name,
                    },
                    Confidence::Certain,
                    title,
                    "방화벽 규칙",
                );
            }
        }
    }

    fn msi_registration(&mut self) {
        let Some(code) = self.app.uninstall.msi_product_code.clone() else {
            return;
        };
        let Some(packed) = packed_guid(&code) else {
            return;
        };
        let mut keys: Vec<(Hive, String)> = vec![
            (
                Hive::LocalMachine,
                format!(r"SOFTWARE\Classes\Installer\Products\{packed}"),
            ),
            (
                Hive::LocalMachine,
                format!(r"SOFTWARE\Classes\Installer\Features\{packed}"),
            ),
            (
                Hive::CurrentUser,
                format!(r"Software\Microsoft\Installer\Products\{packed}"),
            ),
            (
                Hive::CurrentUser,
                format!(r"Software\Microsoft\Installer\Features\{packed}"),
            ),
        ];
        let user_data = r"SOFTWARE\Microsoft\Windows\CurrentVersion\Installer\UserData";
        if let Ok(k) = reg::open(Hive::LocalMachine, user_data) {
            for sid in reg::subkeys(&k) {
                keys.push((
                    Hive::LocalMachine,
                    format!(r"{user_data}\{sid}\Products\{packed}"),
                ));
            }
        }
        for (hive, path) in keys {
            if reg::key_exists(hive, &path) {
                self.consider_key(
                    hive,
                    &path,
                    Confidence::Certain,
                    "Windows Installer 등록 정보",
                );
            }
        }
    }

    // ── 6. 서비스 · 예약 작업 ───────────────────────────────────
    fn services(&mut self) {
        if self.app_dirs.is_empty() {
            return;
        }
        for s in services::list() {
            if !s.image.is_empty() && self.in_app_dirs(&s.image) {
                self.push(
                    LeftoverKind::Service {
                        name: s.name.clone(),
                    },
                    Confidence::Certain,
                    s.display_name,
                    "앱 폴더의 서비스",
                );
            }
        }
    }

    fn tasks(&mut self) {
        if self.app_dirs.is_empty() {
            return;
        }
        for t in tasks::list(&self.ctx.folders.tasks) {
            if t.actions.iter().any(|a| self.command_refs_app(a)) {
                let title = pathutil::file_name(&t.path).to_string();
                self.push(
                    LeftoverKind::Task {
                        path: t.path,
                        file: t.file,
                    },
                    Confidence::Certain,
                    title,
                    "앱을 실행하는 예약 작업",
                );
            }
        }
    }

    /// 중복 제거: 같은 항목은 높은 신뢰도 하나만, 상위 폴더/키가 이미 같거나 높은 신뢰도로
    /// 포함하는 하위 항목은 뺀다.
    fn finish(mut self) -> Vec<Leftover> {
        let mut items = std::mem::take(&mut self.out);
        items.sort_by_key(|i| std::cmp::Reverse(i.confidence));
        let mut seen: HashSet<String> = HashSet::new();
        items.retain(|i| seen.insert(identity(&i.kind)));

        let snapshot: Vec<(LeftoverKind, Confidence)> = items
            .iter()
            .map(|i| (i.kind.clone(), i.confidence))
            .collect();
        items.retain(|i| {
            !snapshot.iter().any(|(other, conf)| {
                *other != i.kind && *conf >= i.confidence && contains(other, &i.kind)
            })
        });

        // 크기 계산(병렬).
        items.par_iter_mut().for_each(|i| {
            i.size = match &i.kind {
                LeftoverKind::Dir(p) => fsops::dir_size(p),
                LeftoverKind::File(p) => fsops::file_len(p),
                _ => 0,
            };
        });

        items.sort_by(|a, b| {
            b.confidence
                .cmp(&a.confidence)
                .then(a.kind.order().cmp(&b.kind.order()))
                .then_with(|| {
                    a.kind
                        .location()
                        .to_lowercase()
                        .cmp(&b.kind.location().to_lowercase())
                })
        });
        items
    }
}

fn identity(kind: &LeftoverKind) -> String {
    match kind {
        LeftoverKind::Dir(p) | LeftoverKind::File(p) => format!("f:{}", path_key(p)),
        LeftoverKind::RegKey { hive, path } => {
            format!("k:{}\\{}", hive.short_name(), path.to_lowercase())
        }
        LeftoverKind::RegValue { hive, key, name } => {
            format!(
                "v:{}\\{}|{}",
                hive.short_name(),
                key.to_lowercase(),
                name.to_lowercase()
            )
        }
        LeftoverKind::Service { name } => format!("s:{}", name.to_lowercase()),
        LeftoverKind::Task { path, .. } => format!("t:{}", path.to_lowercase()),
    }
}

/// `outer`가 `inner`를 하위 항목으로 포함하는지(폴더⊃파일/폴더, 키⊃키).
fn contains(outer: &LeftoverKind, inner: &LeftoverKind) -> bool {
    match (outer, inner) {
        (LeftoverKind::Dir(o), LeftoverKind::Dir(i) | LeftoverKind::File(i)) => {
            key_within(&path_key(i), &path_key(o))
        }
        (
            LeftoverKind::RegKey { hive: ho, path: po },
            LeftoverKind::RegKey { hive: hi, path: pi },
        ) => {
            let (po, pi) = (po.to_lowercase(), pi.to_lowercase());
            ho == hi && pi.starts_with(&po) && pi.as_bytes().get(po.len()) == Some(&b'\\')
        }
        _ => false,
    }
}

/// 잔여물 전체 검색.
pub fn scan(req: &ScanRequest, ctx: &Context) -> Vec<Leftover> {
    let mut s = Scanner::new(req, ctx);
    s.install_dir();
    s.store_data();
    s.named_dirs();
    s.shortcuts();
    s.uninstall_entry();
    s.software_keys();
    s.app_paths();
    s.run_values();
    s.firewall_rules();
    s.msi_registration();
    s.services();
    s.tasks();
    s.finish()
}
