//! 선택한 잔여물을 실제로 삭제한다(백업 포함).
//!
//! 순서: 관련 프로세스 종료 → 서비스 → 예약 작업 → 레지스트리 값 → 레지스트리 키 → 파일/폴더
//! 삭제 직전에 안전 가드를 **다시** 확인하고, 레지스트리·예약 작업은 `.reg`/`.xml`로 백업한다.
//! 사용 중이라 지우지 못한 파일은 재부팅 시 삭제되도록 예약한다.

use std::fmt::Write as _;

use crate::format;
use crate::inventory::Context;
use crate::leftovers::{Leftover, LeftoverKind};
use crate::pathutil::{self, join, key_within, path_key};
use crate::regfile;
use crate::safety::{Hive, registry_key_deletable, registry_value_deletable};
use crate::textmatch::{AppAliases, NameMatch};
use crate::win::{fsops, local_timestamp, procs, reg, services, tasks};

#[derive(Debug, Clone, Copy)]
pub struct CleanOptions {
    /// 파일·폴더를 영구 삭제 대신 휴지통으로 보낸다.
    pub recycle: bool,
}

#[derive(Debug, Default, Clone)]
pub struct CleanSummary {
    pub removed: usize,
    pub freed: u64,
    pub reboot_pending: Vec<String>,
    pub failed: Vec<(String, String)>,
    pub killed: Vec<String>,
    pub backup_dir: Option<String>,
}

const STARTUP_APPROVED: &[&str] = &[
    r"Software\Microsoft\Windows\CurrentVersion\Explorer\StartupApproved\Run",
    r"Software\Microsoft\Windows\CurrentVersion\Explorer\StartupApproved\Run32",
];

struct Cleaner<'a> {
    ctx: &'a Context,
    aliases: &'a AppAliases,
    opts: CleanOptions,
    backup_dir: String,
    reg_text: String,
    log: String,
    summary: CleanSummary,
}

impl Cleaner<'_> {
    fn ok(&mut self, what: &str) {
        self.summary.removed += 1;
        let _ = writeln!(self.log, "[삭제] {what}");
    }

    fn fail(&mut self, what: &str, why: String) {
        let _ = writeln!(self.log, "[실패] {what} — {why}");
        self.summary.failed.push((what.to_string(), why));
    }

    fn kill_processes(&mut self, items: &[&Leftover]) {
        let dirs: Vec<String> = items
            .iter()
            .filter_map(|i| match &i.kind {
                LeftoverKind::Dir(p) => Some(path_key(p)),
                _ => None,
            })
            .collect();
        if dirs.is_empty() {
            return;
        }
        for (pid, path) in procs::list() {
            let k = path_key(&path);
            if dirs.iter().any(|d| key_within(&k, d)) && procs::terminate(pid) {
                let _ = writeln!(self.log, "[종료] 실행 중이던 프로세스 {path} (PID {pid})");
                self.summary.killed.push(path);
            }
        }
        if !self.summary.killed.is_empty() {
            std::thread::sleep(std::time::Duration::from_millis(600));
        }
    }

    fn service(&mut self, name: &str) {
        let what = format!("서비스 {name}");
        let _ = reg::export_key(
            Hive::LocalMachine,
            &services::services_key_path(name),
            &mut self.reg_text,
        );
        match services::stop_and_delete(name) {
            Ok(()) => self.ok(&what),
            Err(e) => self.fail(&what, e),
        }
    }

    fn task(&mut self, path: &str, file: &str) {
        let what = format!("예약 작업 {path}");
        let backup = join(
            &join(&self.backup_dir, "tasks"),
            &format!("{}.xml", format::sanitize_file_name(path)),
        );
        if std::fs::create_dir_all(join(&self.backup_dir, "tasks")).is_ok() {
            let _ = std::fs::copy(file, &backup);
        }
        match tasks::delete(path) {
            Ok(()) => self.ok(&what),
            Err(e) => self.fail(&what, e),
        }
    }

    fn reg_value(&mut self, hive: Hive, key: &str, name: &str) {
        let what = format!("{}\\{key} → {name}", hive.short_name());
        if !registry_value_deletable(hive, key) {
            self.fail(&what, "보호된 레지스트리 위치".into());
            return;
        }
        let _ = reg::export_value(hive, key, name, &mut self.reg_text);
        match reg::delete_value(hive, key, name) {
            Ok(()) => self.ok(&what),
            Err(e) => self.fail(&what, e.to_string()),
        }
        // Run 값이면 "시작 앱" 사용/사용 안 함 상태 값도 함께 정리한다.
        if key.to_lowercase().ends_with(r"\run") || key.to_lowercase().ends_with(r"\runonce") {
            for sa in STARTUP_APPROVED {
                if let Ok(k) = reg::open(hive, sa)
                    && k.get_raw_value(name).is_ok()
                {
                    let _ = reg::export_value(hive, sa, name, &mut self.reg_text);
                    let _ = reg::delete_value(hive, sa, name);
                }
            }
        }
    }

    fn reg_key(&mut self, hive: Hive, path: &str) {
        let what = format!("{}\\{path}", hive.short_name());
        if !registry_key_deletable(hive, path) {
            self.fail(&what, "보호된 레지스트리 위치".into());
            return;
        }
        if !reg::key_exists(hive, path) {
            self.ok(&what);
            return;
        }
        let _ = reg::export_key(hive, path, &mut self.reg_text);
        match reg::delete_key(hive, path) {
            Ok(()) => self.ok(&what),
            Err(e) => self.fail(&what, e.to_string()),
        }
    }

    fn fs_item(&mut self, path: &str) {
        if !self.ctx.guard.can_delete(path) {
            self.fail(path, "보호된 위치라 삭제하지 않음".into());
            return;
        }
        if !fsops::exists(path) {
            self.ok(path);
            return;
        }
        let stats = if self.opts.recycle {
            fsops::recycle_or_delete(path)
        } else {
            fsops::delete_path(path)
        };
        self.summary.freed += stats.bytes;
        let scheduled = !stats.scheduled.is_empty();
        for s in &stats.scheduled {
            let _ = writeln!(self.log, "[재부팅 시 삭제] {s}");
        }
        self.summary.reboot_pending.extend(stats.scheduled);
        if stats.failed.is_empty() {
            if scheduled {
                let _ = writeln!(self.log, "[일부 예약] {path}");
                self.summary.removed += 1;
            } else {
                self.ok(path);
            }
        } else {
            let first = stats.failed[0].1.clone();
            self.fail(
                path,
                format!("{}개 항목을 지우지 못함: {first}", stats.failed.len()),
            );
        }
        self.remove_empty_parents(path);
    }

    /// 삭제 후 비게 된 상위 폴더(게시자 폴더·시작 메뉴 폴더)를 정리한다.
    fn remove_empty_parents(&mut self, path: &str) {
        let f = &self.ctx.folders;
        let start_menus = [path_key(&f.programs), path_key(&f.common_programs)];
        let mut cur = pathutil::parent(path).map(str::to_string);
        for _ in 0..2 {
            let Some(p) = cur else { break };
            let k = path_key(&p);
            let in_start_menu = start_menus
                .iter()
                .any(|s| !s.is_empty() && key_within(&k, s));
            let named = self.aliases.classify(pathutil::file_name(&p)) != NameMatch::None;
            if !(in_start_menu || named)
                || !self.ctx.guard.can_delete(&p)
                || !fsops::remove_empty_dir(&p)
            {
                break;
            }
            let _ = writeln!(self.log, "[삭제] 빈 폴더 {p}");
            cur = pathutil::parent(&p).map(str::to_string);
        }
    }

    fn write_backup(&mut self) {
        let _ = std::fs::create_dir_all(&self.backup_dir);
        if self.reg_text.len() > regfile::HEADER.len() {
            let _ = std::fs::write(
                join(&self.backup_dir, "registry.reg"),
                regfile::encode_utf16le(&self.reg_text),
            );
        }
        let mut text = String::from("\u{FEFF}");
        text.push_str(&self.log);
        let _ = std::fs::write(join(&self.backup_dir, "log.txt"), text);
        self.summary.backup_dir = Some(self.backup_dir.clone());
    }
}

/// 백업 루트: `%LOCALAPPDATA%\AppDeleter\Backups`.
pub fn backup_root(ctx: &Context) -> String {
    join(&ctx.folders.local, r"AppDeleter\Backups")
}

/// 선택 항목을 삭제한다. `progress(완료 수, 전체 수, 현재 항목 이름)`.
pub fn clean(
    app_name: &str,
    aliases: &AppAliases,
    items: &[Leftover],
    opts: CleanOptions,
    ctx: &Context,
    progress: &dyn Fn(usize, usize, &str),
) -> CleanSummary {
    let backup_dir = join(
        &backup_root(ctx),
        &format!(
            "{} {}",
            local_timestamp(),
            format::sanitize_file_name(app_name)
        ),
    );
    let mut c = Cleaner {
        ctx,
        aliases,
        opts,
        backup_dir,
        reg_text: regfile::HEADER.to_string(),
        log: format!(
            "App Deleter — {app_name} 정리 기록 ({})\r\n\r\n",
            local_timestamp()
        ),
        summary: CleanSummary::default(),
    };

    let mut ordered: Vec<&Leftover> = items.iter().collect();
    ordered.sort_by_key(|i| match i.kind {
        LeftoverKind::Service { .. } => 0,
        LeftoverKind::Task { .. } => 1,
        LeftoverKind::RegValue { .. } => 2,
        LeftoverKind::RegKey { .. } => 3,
        LeftoverKind::File(_) => 4,
        LeftoverKind::Dir(_) => 5,
    });

    let total = ordered.len();
    progress(0, total, "실행 중인 관련 프로세스 확인");
    c.kill_processes(&ordered);

    for (i, item) in ordered.iter().enumerate() {
        progress(i, total, &item.title);
        match &item.kind {
            LeftoverKind::Service { name } => c.service(name),
            LeftoverKind::Task { path, file } => c.task(path, file),
            LeftoverKind::RegValue { hive, key, name } => c.reg_value(*hive, key, name),
            LeftoverKind::RegKey { hive, path } => c.reg_key(*hive, path),
            LeftoverKind::File(p) | LeftoverKind::Dir(p) => c.fs_item(p),
        }
    }
    progress(total, total, "백업 저장");
    c.write_backup();
    c.summary
}
