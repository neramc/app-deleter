//! UI 컨트롤러: Slint 창과 백엔드(목록·크기·아이콘·제거·정리)를 연결한다.
//!
//! - UI 스레드 상태는 `thread_local` 하나(`STATE`)에 모아 두고, 백그라운드 작업은
//!   `slint::invoke_from_event_loop`로 결과를 돌려보낸다.
//! - 폴더 크기·아이콘은 rayon으로 병렬 계산하며, 결과를 약 80ms 단위로 묶어 UI에 반영한다
//!   (수백 개 앱에서도 UI가 끊기지 않도록).

use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc;
use std::time::{Duration, Instant};

use rayon::prelude::*;
use slint::{
    ComponentHandle, Model, ModelRc, Rgba8Pixel, SharedPixelBuffer, SharedString, VecModel,
};

use crate::cleaner::{self, CleanOptions, CleanSummary};
use crate::format;
use crate::inventory::{self, AppInfo, Context, Source};
use crate::leftovers::{self, Confidence, Leftover, LeftoverKind, ScanRequest};
use crate::pathutil::path_key;
use crate::win::job::{self, RunEnd, RunError};
use crate::win::restore::{self, RestorePoint};
use crate::win::{self, ComGuard, icons, store};

slint::include_modules!();

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SizeState {
    /// 폴더 크기 계산 중.
    Pending,
    /// 실제 폴더 크기.
    Exact,
    /// 레지스트리 추정치.
    Estimated,
    /// 알 수 없음.
    Unknown,
}

struct AppRecord {
    info: AppInfo,
    uninstaller_missing: bool,
    size: u64,
    size_state: SizeState,
    icon: Option<slint::Image>,
}

struct WizardSession {
    app: AppInfo,
    skip: Arc<AtomicBool>,
    leftovers: Vec<Leftover>,
    checked: Vec<bool>,
    model: Rc<VecModel<LeftoverItem>>,
    restore: Option<RestorePoint>,
    busy: bool,
    changed_system: bool,
    backup_dir: Option<String>,
}

struct State {
    window: slint::Weak<AppWindow>,
    ctx: Arc<Context>,
    records: Vec<AppRecord>,
    model: Rc<VecModel<AppItem>>,
    /// 레코드 인덱스 → 현재 목록 행.
    row_of: Vec<Option<usize>>,
    max_size: u64,
    generation: u64,
    sizes_total: usize,
    sizes_done: usize,
    icon_px: u32,
    query: String,
    filter: i32,
    sort: i32,
    wizard: Option<WizardSession>,
}

thread_local! {
    static STATE: RefCell<Option<State>> = const { RefCell::new(None) };
}

fn with_state<R>(f: impl FnOnce(&mut State) -> R) -> Option<R> {
    STATE.with(|s| s.borrow_mut().as_mut().map(f))
}

/// 백그라운드 스레드에서 UI 스레드의 상태를 갱신한다.
fn post(f: impl FnOnce(&mut State) + Send + 'static) {
    let _ = slint::invoke_from_event_loop(move || {
        with_state(f);
    });
}

// ─────────────────────────────────────────────────────────────
// 진입점
// ─────────────────────────────────────────────────────────────

pub fn run() -> Result<(), slint::PlatformError> {
    let build = win::windows_build();
    if build != 0 && build < 22000 {
        win::message_box(
            "App Deleter",
            &format!(
                "App Deleter는 Windows 11(빌드 22000 이상) 전용입니다.\n현재 Windows 빌드: {build}"
            ),
            false,
        );
        return Ok(());
    }

    let ui = AppWindow::new()?;
    let ctx = Arc::new(Context::load());
    let model: Rc<VecModel<AppItem>> = Rc::new(VecModel::default());

    ui.set_version(env!("CARGO_PKG_VERSION").into());
    ui.set_elevated(win::is_elevated());
    ui.set_backup_path(cleaner::backup_root(&ctx).into());
    ui.set_apps(ModelRc::from(model.clone()));

    let scale = ui.window().scale_factor();
    let icon_px = if scale > 1.5 {
        64
    } else if scale > 1.0 {
        48
    } else {
        32
    };

    STATE.with(|s| {
        *s.borrow_mut() = Some(State {
            window: ui.as_weak(),
            ctx,
            records: Vec::new(),
            model,
            row_of: Vec::new(),
            max_size: 1,
            generation: 0,
            sizes_total: 0,
            sizes_done: 0,
            icon_px,
            query: String::new(),
            filter: 0,
            sort: 0,
            wizard: None,
        });
    });

    wire_main(&ui);
    wire_wizard(&ui);

    ui.window().on_close_requested(|| {
        let busy = with_state(|s| s.wizard.as_ref().is_some_and(|w| w.busy)).unwrap_or(false);
        if busy {
            slint::CloseRequestResponse::KeepWindowShown
        } else {
            with_state(|s| {
                if let Some(rp) = s.wizard.as_mut().and_then(|w| w.restore.take()) {
                    rp.end();
                }
            });
            slint::CloseRequestResponse::HideWindow
        }
    });

    refresh();
    ui.run()
}

fn wire_main(ui: &AppWindow) {
    ui.on_refresh(refresh);
    ui.on_search(|text| {
        with_state(|s| {
            s.query = text.to_string();
            rebuild_view(s);
        });
    });
    ui.on_filter_changed(|i| {
        with_state(|s| {
            s.filter = i;
            rebuild_view(s);
        });
    });
    ui.on_sort_changed(|i| {
        with_state(|s| {
            s.sort = i;
            rebuild_view(s);
        });
    });
    ui.on_uninstall(open_wizard);
    ui.on_relaunch_admin(|| {
        if win::relaunch_as_admin() {
            let _ = slint::quit_event_loop();
        }
    });
    ui.on_open_backups(|| {
        if let Some(path) = with_state(|s| cleaner::backup_root(&s.ctx)) {
            let _ = std::fs::create_dir_all(&path);
            win::open_in_explorer(&path);
        }
    });
}

// ─────────────────────────────────────────────────────────────
// 앱 목록
// ─────────────────────────────────────────────────────────────

enum Update {
    Size(usize, u64),
    Icon(usize, SharedPixelBuffer<Rgba8Pixel>),
}

fn refresh() {
    let Some((generation, ctx, icon_px)) = with_state(|s| {
        s.generation += 1;
        s.records.clear();
        s.row_of.clear();
        s.model.set_vec(Vec::new());
        if let Some(w) = s.window.upgrade() {
            w.set_loading(true);
            w.set_sizing(false);
            w.set_summary(SharedString::default());
        }
        (s.generation, s.ctx.clone(), s.icon_px)
    }) else {
        return;
    };

    std::thread::spawn(move || {
        let _com = ComGuard::mta();
        let started = Instant::now();
        let apps = inventory::enumerate(&ctx);
        let missing: Vec<bool> = apps.par_iter().map(AppInfo::uninstaller_missing).collect();
        let infos: Vec<AppInfo> = apps.clone();
        let elapsed = started.elapsed();
        post(move |s| on_apps_loaded(s, generation, apps, missing, elapsed));
        background_scans(generation, infos, icon_px);
    });
}

fn on_apps_loaded(
    s: &mut State,
    generation: u64,
    apps: Vec<AppInfo>,
    missing: Vec<bool>,
    elapsed: Duration,
) {
    if s.generation != generation {
        return;
    }
    s.records = apps
        .into_iter()
        .zip(missing)
        .map(|(info, uninstaller_missing)| {
            let size_state = if info.install_dir.is_some() {
                SizeState::Pending
            } else if info.estimated_size > 0 {
                SizeState::Estimated
            } else {
                SizeState::Unknown
            };
            AppRecord {
                size: info.estimated_size,
                info,
                uninstaller_missing,
                size_state,
                icon: None,
            }
        })
        .collect();
    s.sizes_total = s
        .records
        .iter()
        .filter(|r| r.size_state == SizeState::Pending)
        .count();
    s.sizes_done = 0;
    s.max_size = s.records.iter().map(|r| r.size).max().unwrap_or(1).max(1);
    rebuild_view(s);
    if let Some(w) = s.window.upgrade() {
        w.set_loading(false);
        w.set_sizing(s.sizes_total > 0);
        w.set_sizing_progress(0.0);
    }
    eprintln!("[app-deleter] {}개 앱 목록 {:?}", s.records.len(), elapsed);
}

/// 아이콘·폴더 크기를 병렬로 계산해 묶음 단위로 UI에 반영한다.
fn background_scans(generation: u64, apps: Vec<AppInfo>, icon_px: u32) {
    let (tx, rx) = mpsc::channel::<Update>();

    // 수신 스레드: 80ms마다 모아서 UI로 보낸다.
    let pump = std::thread::spawn(move || {
        let mut batch: Vec<Update> = Vec::new();
        let mut last = Instant::now();
        loop {
            match rx.recv_timeout(Duration::from_millis(40)) {
                Ok(u) => batch.push(u),
                Err(mpsc::RecvTimeoutError::Timeout) => {}
                Err(mpsc::RecvTimeoutError::Disconnected) => break,
            }
            if !batch.is_empty() && last.elapsed() >= Duration::from_millis(80) {
                let b = std::mem::take(&mut batch);
                post(move |s| apply_updates(s, generation, b, false));
                last = Instant::now();
            }
        }
        post(move |s| apply_updates(s, generation, batch, true));
    });

    let icon_tx = tx.clone();
    let size_tx = tx;
    let apps_ref = &apps;
    rayon::join(
        || {
            apps_ref
                .par_iter()
                .enumerate()
                .for_each_with(icon_tx, |tx, (i, app)| {
                    if let Some(img) = app.icon.as_ref().and_then(|src| icons::load(src, icon_px)) {
                        let mut buf = SharedPixelBuffer::<Rgba8Pixel>::new(img.width, img.height);
                        buf.make_mut_bytes().copy_from_slice(&img.pixels);
                        let _ = tx.send(Update::Icon(i, buf));
                    }
                });
        },
        || {
            // 같은 폴더를 가리키는 앱은 한 번만 계산한다. 큰 앱부터 시작해 부하를 고르게 한다.
            let mut groups: HashMap<String, (String, Vec<usize>, u64)> = HashMap::new();
            for (i, app) in apps_ref.iter().enumerate() {
                if let Some(dir) = &app.install_dir {
                    let e = groups
                        .entry(path_key(dir))
                        .or_insert_with(|| (dir.clone(), Vec::new(), 0));
                    e.1.push(i);
                    e.2 = e.2.max(app.estimated_size);
                }
            }
            let mut work: Vec<(String, Vec<usize>, u64)> = groups.into_values().collect();
            work.sort_by_key(|w| std::cmp::Reverse(w.2));
            work.into_par_iter()
                .for_each_with(size_tx, |tx, (dir, idxs, _)| {
                    let size = win::fsops::dir_size(&dir);
                    for i in idxs {
                        let _ = tx.send(Update::Size(i, size));
                    }
                });
        },
    );
    let _ = pump.join();
}

fn apply_updates(s: &mut State, generation: u64, batch: Vec<Update>, finished: bool) {
    if s.generation != generation {
        return;
    }
    let mut touched: Vec<usize> = Vec::with_capacity(batch.len());
    let old_max = s.max_size;
    for u in batch {
        match u {
            Update::Size(i, size) => {
                if let Some(r) = s.records.get_mut(i) {
                    if r.size_state == SizeState::Pending {
                        s.sizes_done += 1;
                    }
                    // 폴더를 읽을 수 없어 0이면 레지스트리 추정치를 유지한다.
                    if size == 0 && r.info.estimated_size > 0 {
                        r.size = r.info.estimated_size;
                        r.size_state = SizeState::Estimated;
                    } else {
                        r.size = size;
                        r.size_state = SizeState::Exact;
                    }
                    s.max_size = s.max_size.max(r.size);
                    touched.push(i);
                }
            }
            Update::Icon(i, buf) => {
                if let Some(r) = s.records.get_mut(i) {
                    r.icon = Some(slint::Image::from_rgba8(buf));
                    touched.push(i);
                }
            }
        }
    }

    if finished {
        for r in &mut s.records {
            if r.size_state == SizeState::Pending {
                r.size_state = if r.info.estimated_size > 0 {
                    SizeState::Estimated
                } else {
                    SizeState::Unknown
                };
            }
        }
        // 크기순 정렬이면 최종 크기로 다시 정렬한다.
        rebuild_view(s);
    } else if s.max_size != old_max {
        refresh_rows(s);
    } else {
        for i in touched {
            if let Some(Some(row)) = s.row_of.get(i).copied() {
                s.model
                    .set_row_data(row, to_item(i, &s.records[i], s.max_size));
            }
        }
    }

    if let Some(w) = s.window.upgrade() {
        let total = s.sizes_total.max(1) as f32;
        w.set_sizing(!finished && s.sizes_total > 0);
        w.set_sizing_progress((s.sizes_done as f32 / total).min(1.0));
        w.set_summary(summary_text(s).into());
    }
}

fn to_item(id: usize, r: &AppRecord, max: u64) -> AppItem {
    let info = &r.info;
    AppItem {
        id: id as i32,
        name: info.name.as_str().into(),
        publisher: info.publisher.as_str().into(),
        version: info.version.as_str().into(),
        date: info.install_date.as_str().into(),
        size_text: if r.size > 0 {
            format::bytes(r.size).into()
        } else {
            "—".into()
        },
        size_ratio: if max > 0 {
            (r.size as f64 / max as f64) as f32
        } else {
            0.0
        },
        size_pending: r.size_state == SizeState::Pending,
        size_estimated: r.size_state == SizeState::Estimated,
        icon: r.icon.clone().unwrap_or_default(),
        has_icon: r.icon.is_some(),
        is_store: info.is_store(),
        uninstaller_missing: r.uninstaller_missing,
    }
}

fn matches(s: &State, r: &AppRecord) -> bool {
    let kind_ok = match s.filter {
        1 => !r.info.is_store(),
        2 => r.info.is_store(),
        _ => true,
    };
    if !kind_ok {
        return false;
    }
    let q = s.query.trim().to_lowercase();
    q.is_empty()
        || r.info.name.to_lowercase().contains(&q)
        || r.info.publisher.to_lowercase().contains(&q)
}

fn rebuild_view(s: &mut State) {
    let mut idx: Vec<usize> = (0..s.records.len())
        .filter(|&i| matches(s, &s.records[i]))
        .collect();
    let recs = &s.records;
    match s.sort {
        1 => idx.sort_by_cached_key(|&i| recs[i].info.name.to_lowercase()),
        2 => idx.sort_by(|&a, &b| {
            format::date_sort_key(&recs[b].info.install_date)
                .cmp(&format::date_sort_key(&recs[a].info.install_date))
        }),
        3 => idx.sort_by_cached_key(|&i| {
            (
                recs[i].info.publisher.to_lowercase(),
                recs[i].info.name.to_lowercase(),
            )
        }),
        _ => idx.sort_by(|&a, &b| {
            recs[b]
                .size
                .cmp(&recs[a].size)
                .then_with(|| recs[a].info.name.cmp(&recs[b].info.name))
        }),
    }
    s.row_of = vec![None; s.records.len()];
    let items: Vec<AppItem> = idx
        .iter()
        .enumerate()
        .map(|(row, &i)| {
            s.row_of[i] = Some(row);
            to_item(i, &s.records[i], s.max_size)
        })
        .collect();
    s.model.set_vec(items);
    if let Some(w) = s.window.upgrade() {
        w.set_summary(summary_text(s).into());
    }
}

fn refresh_rows(s: &mut State) {
    for (i, row) in s.row_of.iter().enumerate() {
        if let Some(row) = row {
            s.model
                .set_row_data(*row, to_item(i, &s.records[i], s.max_size));
        }
    }
}

fn summary_text(s: &State) -> String {
    let total = s.records.len();
    let visible: Vec<&AppRecord> = s
        .row_of
        .iter()
        .enumerate()
        .filter_map(|(i, r)| r.map(|_| &s.records[i]))
        .collect();
    let bytes: u64 = visible.iter().map(|r| r.size).sum();
    if visible.len() == total {
        format!("설치된 앱 {total}개 · 합계 {}", format::bytes(bytes))
    } else {
        format!(
            "{}개 표시 (전체 {total}개) · 합계 {}",
            visible.len(),
            format::bytes(bytes)
        )
    }
}

// ─────────────────────────────────────────────────────────────
// 제거 마법사
// ─────────────────────────────────────────────────────────────

fn open_wizard(id: i32) {
    with_state(|s| {
        if s.wizard.is_some() {
            return;
        }
        let Some(rec) = usize::try_from(id).ok().and_then(|i| s.records.get(i)) else {
            return;
        };
        let item = to_item(id as usize, rec, s.max_size);
        let session = WizardSession {
            app: rec.info.clone(),
            skip: Arc::new(AtomicBool::new(false)),
            leftovers: Vec::new(),
            checked: Vec::new(),
            model: Rc::new(VecModel::default()),
            restore: None,
            busy: false,
            changed_system: false,
            backup_dir: None,
        };
        let Some(w) = s.window.upgrade() else { return };
        let g = w.global::<Wizard>();
        g.set_target(item);
        g.set_quiet_supported(rec.info.uninstall.supports_quiet());
        g.set_uninstaller_missing(rec.uninstaller_missing);
        g.set_restore_point(false);
        g.set_quiet(false);
        g.set_force(rec.uninstaller_missing);
        g.set_recycle(true);
        g.set_banner(SharedString::default());
        g.set_banner_level(1);
        g.set_status_title(SharedString::default());
        g.set_status_detail(SharedString::default());
        g.set_elapsed(SharedString::default());
        g.set_progress(0.0);
        g.set_leftovers(ModelRc::from(session.model.clone()));
        g.set_selected_count(0);
        g.set_selected_size(SharedString::default());
        g.set_has_backup(false);
        g.set_step(WizardStep::Confirm);
        g.set_open(true);
        s.wizard = Some(session);
    });
}

fn wire_wizard(ui: &AppWindow) {
    let g = ui.global::<Wizard>();
    g.on_start(start_uninstall);
    g.on_skip_wait(|| {
        with_state(|s| {
            if let Some(w) = &s.wizard {
                w.skip.store(true, Ordering::Relaxed);
            }
        });
    });
    g.on_toggle(|i| {
        with_state(|s| {
            let Some(w) = s.wizard.as_mut() else { return };
            let Some(i) = usize::try_from(i).ok().filter(|i| *i < w.checked.len()) else {
                return;
            };
            w.checked[i] = !w.checked[i];
            if let Some(mut row) = w.model.row_data(i) {
                row.checked = w.checked[i];
                w.model.set_row_data(i, row);
            }
            update_selection(s);
        });
    });
    g.on_select(|mode| {
        with_state(|s| {
            let Some(w) = s.wizard.as_mut() else { return };
            for (i, item) in w.leftovers.iter().enumerate() {
                w.checked[i] = match mode {
                    0 => false,
                    2 => true,
                    _ => item.confidence >= Confidence::Likely,
                };
                if let Some(mut row) = w.model.row_data(i) {
                    row.checked = w.checked[i];
                    w.model.set_row_data(i, row);
                }
            }
            update_selection(s);
        });
    });
    g.on_clean(start_clean);
    g.on_finish(|| close_wizard(true));
    g.on_cancel(|| close_wizard(false));
    g.on_open_backup(|| {
        if let Some(Some(dir)) =
            with_state(|s| s.wizard.as_ref().and_then(|w| w.backup_dir.clone()))
        {
            win::open_in_explorer(&dir);
        }
    });
}

fn close_wizard(refresh_after: bool) {
    let changed = with_state(|s| {
        let Some(mut w) = s.wizard.take() else {
            return false;
        };
        if let Some(rp) = w.restore.take() {
            std::thread::spawn(move || rp.end());
        }
        if let Some(win) = s.window.upgrade() {
            win.global::<Wizard>().set_open(false);
        }
        w.changed_system
    })
    .unwrap_or(false);
    if refresh_after && changed {
        refresh();
    }
}

/// 마법사 UI를 갱신한다(백그라운드 스레드에서 호출).
fn post_wizard(f: impl FnOnce(&Wizard<'_>) + Send + 'static) {
    post(move |s| {
        if let Some(w) = s.window.upgrade() {
            f(&w.global::<Wizard>());
        }
    });
}

fn post_status(step: WizardStep, title: &str, detail: &str) {
    let (title, detail) = (SharedString::from(title), SharedString::from(detail));
    post_wizard(move |g| {
        g.set_step(step);
        g.set_status_title(title);
        g.set_status_detail(detail);
    });
}

struct StartOptions {
    restore_point: bool,
    quiet: bool,
    force: bool,
}

fn start_uninstall() {
    let Some(Some((app, skip, ctx, opts))) = with_state(|s| {
        let win = s.window.upgrade()?;
        let g = win.global::<Wizard>();
        let w = s.wizard.as_mut()?;
        if w.busy {
            return None;
        }
        w.busy = true;
        w.changed_system = true;
        let opts = StartOptions {
            restore_point: g.get_restore_point(),
            quiet: g.get_quiet(),
            force: g.get_force(),
        };
        g.set_step(WizardStep::Running);
        g.set_status_title("준비 중…".into());
        Some((w.app.clone(), w.skip.clone(), s.ctx.clone(), opts))
    }) else {
        return;
    };

    std::thread::spawn(move || {
        let _com = ComGuard::mta();
        let mut notes: Vec<(i32, String)> = Vec::new();
        let mut restore_point = None;

        if opts.restore_point {
            post_status(
                WizardStep::Running,
                "복원 지점을 만드는 중…",
                "Windows 시스템 복원 지점을 만들고 있습니다. 잠시 기다려 주세요.",
            );
            match restore::begin(&format!("App Deleter: {} 제거", app.name)) {
                Ok(rp) => restore_point = Some(rp),
                Err(e) => notes.push((2, e)),
            }
        }

        let forced = opts.force || app.uninstaller_missing();
        if !forced {
            match &app.source {
                Source::Store { full_name, .. } => {
                    post_status(
                        WizardStep::Running,
                        "Store 앱을 제거하는 중…",
                        "Windows 패키지 관리자가 앱을 제거하고 있습니다.",
                    );
                    if let Err(e) = store::remove(full_name) {
                        notes.push((3, e));
                    }
                }
                Source::Win32 { .. } => run_win32_uninstaller(&app, opts.quiet, &skip, &mut notes),
            }
        }

        post_status(
            WizardStep::Scanning,
            "남은 항목을 찾는 중…",
            "파일·폴더·레지스트리·바로가기·서비스·예약 작업·시작 프로그램·방화벽 규칙을 확인하고 있습니다.",
        );
        if !forced {
            // 제거 프로그램의 마지막 정리(레지스트리 키 삭제 등)가 반영될 시간을 준다.
            std::thread::sleep(Duration::from_millis(600));
        }
        let still = inventory::still_installed(&app);
        let me = app.identity();
        let (my_name, my_pub) = (app.name.to_lowercase(), app.publisher.to_lowercase());
        let others: Vec<AppInfo> = inventory::enumerate(&ctx)
            .into_iter()
            .filter(|o| {
                o.identity() != me
                    && !(o.name.to_lowercase() == my_name && o.publisher.to_lowercase() == my_pub)
            })
            .collect();
        let started = Instant::now();
        let items = leftovers::scan(
            &ScanRequest {
                app: app.clone(),
                others,
            },
            &ctx,
        );
        eprintln!(
            "[app-deleter] 잔여물 {}개 검색 {:?}",
            items.len(),
            started.elapsed()
        );

        if still && !forced && !app.is_store() {
            notes.push((
                2,
                "앱이 아직 프로그램 목록에 남아 있습니다. 제거가 취소되었거나 재부팅 후 완료될 수 있습니다. \
                 아래 항목을 삭제하면 강제로 정리됩니다."
                    .to_string(),
            ));
        }
        post(move |s| on_scan_done(s, items, notes, restore_point));
    });
}

fn run_win32_uninstaller(
    app: &AppInfo,
    quiet: bool,
    skip: &AtomicBool,
    notes: &mut Vec<(i32, String)>,
) {
    let cmd = quiet
        .then(|| app.uninstall.quiet_command())
        .flatten()
        .or_else(|| app.uninstall.interactive_command());
    let Some(cmd) = cmd else {
        notes.push((2, "제거 명령이 없어 강제 정리로 진행합니다.".into()));
        return;
    };
    post_status(
        WizardStep::Running,
        "앱의 제거 프로그램이 실행 중입니다",
        if quiet {
            "자동 제거를 진행하고 있습니다. 제거가 끝나면 남은 항목 검색으로 넘어갑니다."
        } else {
            "제거 마법사가 열리면 안내에 따라 제거를 완료하세요. 창이 보이지 않으면 작업 표시줄을 확인하세요."
        },
    );
    let mut last_sec = u64::MAX;
    let result = job::run_and_wait(&cmd, skip, |elapsed| {
        let sec = elapsed.as_secs();
        if sec != last_sec {
            last_sec = sec;
            let text = SharedString::from(format!("경과 시간 {}:{:02}", sec / 60, sec % 60));
            post_wizard(move |g| g.set_elapsed(text));
        }
    });
    match result {
        Ok(RunEnd::Completed { exit_code: Some(code) }) if !matches!(code, 0 | 3010 | 1641) => {
            notes.push((1, format!("제거 프로그램이 종료 코드 {code}로 끝났습니다.")));
        }
        Ok(RunEnd::Completed { .. }) => {}
        Ok(RunEnd::Skipped) => notes.push((
            1,
            "제거 프로그램이 끝나기 전에 다음 단계로 넘어갔습니다. 제거 프로그램이 아직 실행 중이라면 끝난 뒤 다시 확인하세요."
                .into(),
        )),
        Err(RunError::NotFound(exe)) => {
            notes.push((2, format!("제거 프로그램을 찾을 수 없어 강제 정리로 진행합니다. ({exe})")));
        }
        Err(RunError::Failed(e)) => notes.push((3, e)),
    }
}

fn leftover_item(l: &Leftover, checked: bool) -> LeftoverItem {
    LeftoverItem {
        title: l.title.as_str().into(),
        location: l.kind.location().into(),
        reason: l.reason.as_str().into(),
        kind: match l.kind {
            LeftoverKind::Dir(_) => 0,
            LeftoverKind::File(_) => 1,
            LeftoverKind::RegKey { .. } => 2,
            LeftoverKind::RegValue { .. } => 3,
            LeftoverKind::Service { .. } => 4,
            LeftoverKind::Task { .. } => 5,
        },
        confidence: l.confidence as i32,
        size_text: match l.kind {
            LeftoverKind::Dir(_) | LeftoverKind::File(_) if l.size > 0 => {
                format::bytes(l.size).into()
            }
            _ => SharedString::default(),
        },
        checked,
    }
}

fn on_scan_done(
    s: &mut State,
    items: Vec<Leftover>,
    notes: Vec<(i32, String)>,
    restore_point: Option<RestorePoint>,
) {
    let Some(w) = s.wizard.as_mut() else {
        // 마법사가 이미 닫혔다면 복원 지점만 정리한다.
        if let Some(rp) = restore_point {
            std::thread::spawn(move || rp.end());
        }
        return;
    };
    w.busy = false;
    w.restore = restore_point;
    w.checked = items
        .iter()
        .map(|l| l.confidence >= Confidence::Likely)
        .collect();
    let rows: Vec<LeftoverItem> = items
        .iter()
        .zip(&w.checked)
        .map(|(l, c)| leftover_item(l, *c))
        .collect();
    w.model.set_vec(rows);
    w.leftovers = items;
    if let Some(win) = s.window.upgrade() {
        let g = win.global::<Wizard>();
        let level = notes.iter().map(|(l, _)| *l).max().unwrap_or(1);
        let text = notes
            .iter()
            .map(|(_, t)| t.as_str())
            .collect::<Vec<_>>()
            .join("\n");
        g.set_banner(text.into());
        g.set_banner_level(level);
        g.set_step(WizardStep::Review);
    }
    update_selection(s);
}

fn update_selection(s: &mut State) {
    let Some(w) = s.wizard.as_ref() else { return };
    let (count, bytes) = w
        .leftovers
        .iter()
        .zip(&w.checked)
        .filter(|(_, c)| **c)
        .fold((0i32, 0u64), |(n, b), (l, _)| (n + 1, b + l.size));
    if let Some(win) = s.window.upgrade() {
        let g = win.global::<Wizard>();
        g.set_selected_count(count);
        g.set_selected_size(format::bytes(bytes).into());
    }
}

fn start_clean() {
    let Some(Some((app, items, ctx, opts))) = with_state(|s| {
        let win = s.window.upgrade()?;
        let g = win.global::<Wizard>();
        let w = s.wizard.as_mut()?;
        if w.busy {
            return None;
        }
        let items: Vec<Leftover> = w
            .leftovers
            .iter()
            .zip(&w.checked)
            .filter(|(_, c)| **c)
            .map(|(l, _)| l.clone())
            .collect();
        if items.is_empty() {
            return None;
        }
        w.busy = true;
        w.changed_system = true;
        g.set_step(WizardStep::Cleaning);
        g.set_progress(0.0);
        g.set_status_title("선택한 항목을 삭제하는 중…".into());
        Some((
            w.app.clone(),
            items,
            s.ctx.clone(),
            CleanOptions {
                recycle: g.get_recycle(),
            },
        ))
    }) else {
        return;
    };

    std::thread::spawn(move || {
        let _com = ComGuard::mta();
        let last = std::sync::Mutex::new(Instant::now() - Duration::from_secs(1));
        let progress = |done: usize, total: usize, label: &str| {
            let mut l = last.lock().unwrap_or_else(|e| e.into_inner());
            if done < total && l.elapsed() < Duration::from_millis(50) {
                return;
            }
            *l = Instant::now();
            let p = if total == 0 {
                1.0
            } else {
                done as f32 / total as f32
            };
            let label = SharedString::from(label);
            post_wizard(move |g| {
                g.set_progress(p);
                g.set_status_detail(label);
            });
        };
        let summary = cleaner::clean(&app.name, &app.aliases, &items, opts, &ctx, &progress);
        post(move |s| on_clean_done(s, summary));
    });
}

fn on_clean_done(s: &mut State, summary: CleanSummary) {
    let Some(w) = s.wizard.as_mut() else { return };
    w.busy = false;
    w.backup_dir = summary.backup_dir.clone();
    let restore_made = w.restore.is_some();
    if let Some(rp) = w.restore.take() {
        std::thread::spawn(move || rp.end());
    }

    let mut lines: Vec<String> = vec![
        format!("삭제한 항목: {}개", summary.removed),
        format!("확보한 공간: {}", format::bytes(summary.freed)),
    ];
    if !summary.reboot_pending.is_empty() {
        lines.push(format!(
            "사용 중이라 다음 재부팅 때 삭제되도록 예약한 파일: {}개",
            summary.reboot_pending.len()
        ));
    }
    if !summary.killed.is_empty() {
        lines.push(format!("종료한 관련 프로세스: {}개", summary.killed.len()));
    }
    for (what, why) in summary.failed.iter().take(3) {
        lines.push(format!("실패: {what} — {why}"));
    }
    if summary.failed.len() > 3 {
        lines.push(format!(
            "…외 {}개 실패(자세한 내용은 백업 폴더의 log.txt)",
            summary.failed.len() - 3
        ));
    }
    if restore_made {
        lines.push("시스템 복원 지점을 만들었습니다.".into());
    }
    if let Some(dir) = &summary.backup_dir {
        lines.push(format!("백업·기록: {dir}"));
    }

    if let Some(win) = s.window.upgrade() {
        let g = win.global::<Wizard>();
        let ok = summary.failed.is_empty();
        g.set_result_ok(ok);
        g.set_result_title(
            if ok {
                "깨끗하게 정리했습니다"
            } else {
                "일부 항목을 지우지 못했습니다"
            }
            .into(),
        );
        g.set_result_detail(lines.join("\n").into());
        g.set_has_backup(summary.backup_dir.is_some());
        g.set_step(WizardStep::Done);
    }
}
