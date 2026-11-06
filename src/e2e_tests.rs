//! 종단 간 테스트: 가짜 앱을 실제로 "설치"(폴더·레지스트리·시작 프로그램·제거 정보)한 뒤
//! 목록 수집 → 잔여물 검색 → 정리까지 실행해 모두 지워지는지 확인한다.
//! 시스템을 변경하므로 기본으로는 건너뛰고 CI(Windows 러너)에서만 실행한다.

use crate::cleaner::{self, CleanOptions};
use crate::inventory::{self, Context, DirTrust};
use crate::leftovers::{self, Confidence, LeftoverKind, ScanRequest};
use crate::pathutil::{join, path_key};
use crate::safety::Hive;
use crate::win::{fsops, reg};

const RUN: &str = r"Software\Microsoft\Windows\CurrentVersion\Run";
const UNINSTALL: &str = r"Software\Microsoft\Windows\CurrentVersion\Uninstall";

#[test]
#[ignore = "HKCU·LocalAppData에 임시 앱을 만들었다 지운다 — CI에서 실행"]
fn fake_app_is_found_and_removed_completely() {
    let _com = crate::win::ComGuard::mta();
    let ctx = Context::load();
    let name = format!("ZqxFakeApp{}", std::process::id());
    let hkcu = winreg::RegKey::predef(winreg::enums::HKEY_CURRENT_USER);

    // ── 가짜 설치 ──────────────────────────────────────────
    let install = join(&ctx.folders.local_programs, &name);
    let exe = join(&install, r"bin\app.exe");
    std::fs::create_dir_all(join(&install, "bin")).expect("설치 폴더");
    std::fs::write(&exe, vec![0u8; 4096]).expect("실행 파일");
    std::fs::write(join(&install, "readme.txt"), b"hello").expect("파일");
    let data = join(&ctx.folders.roaming, &name);
    std::fs::create_dir_all(join(&data, "cache")).expect("데이터 폴더");
    std::fs::write(join(&data, r"cache\settings.json"), b"{}").expect("설정");

    let (k, _) = hkcu
        .create_subkey(format!(r"Software\{name}\Settings"))
        .expect("설정 키");
    k.set_value("Theme", &"dark").expect("값");
    let (run, _) = hkcu.create_subkey(RUN).expect("Run 키");
    run.set_value(&name, &format!("\"{exe}\" --tray"))
        .expect("Run 값");
    let (u, _) = hkcu
        .create_subkey(format!(r"{UNINSTALL}\{name}"))
        .expect("제거 정보");
    u.set_value("DisplayName", &name).expect("이름");
    u.set_value("Publisher", &"Zqx Test Corp.").expect("게시자");
    u.set_value("DisplayVersion", &"1.0.0").expect("버전");
    u.set_value("InstallLocation", &install).expect("설치 위치");
    u.set_value("DisplayIcon", &format!("{exe},0"))
        .expect("아이콘");
    u.set_value(
        "UninstallString",
        &format!("\"{}\"", join(&install, "uninstall.exe")),
    )
    .expect("제거 명령");

    // ── 목록 수집 ──────────────────────────────────────────
    let apps = inventory::enumerate(&ctx);
    assert!(apps.len() > 1, "설치된 앱 목록이 비어 있음");
    let app = apps
        .iter()
        .find(|a| a.name == name)
        .expect("가짜 앱이 목록에 있어야 함")
        .clone();
    assert_eq!(
        app.install_dir.as_deref().map(path_key),
        Some(path_key(&install))
    );
    assert_eq!(app.install_dir_trust, DirTrust::Verified);
    assert!(
        app.uninstaller_missing(),
        "제거 프로그램이 없으므로 강제 제거 대상"
    );

    // ── 잔여물 검색 ────────────────────────────────────────
    let others = apps
        .into_iter()
        .filter(|o| o.identity() != app.identity())
        .collect();
    let items = leftovers::scan(
        &ScanRequest {
            app: app.clone(),
            others,
        },
        &ctx,
    );
    let has = |pred: &dyn Fn(&LeftoverKind) -> bool, min: Confidence| {
        items.iter().any(|i| pred(&i.kind) && i.confidence >= min)
    };
    let dir_is = |p: &str| {
        let want = path_key(p);
        move |k: &LeftoverKind| matches!(k, LeftoverKind::Dir(d) if path_key(d) == want)
    };
    assert!(
        has(&dir_is(&install), Confidence::Certain),
        "설치 폴더: {items:#?}"
    );
    assert!(
        has(&dir_is(&data), Confidence::Likely),
        "데이터 폴더: {items:#?}"
    );
    assert!(
        has(
            &|k| matches!(k, LeftoverKind::RegKey { hive: Hive::CurrentUser, path } if path.eq_ignore_ascii_case(&format!(r"Software\{name}"))),
            Confidence::Likely
        ),
        "설정 키: {items:#?}"
    );
    assert!(
        has(
            &|k| matches!(k, LeftoverKind::RegValue { name: n, .. } if *n == name),
            Confidence::Certain
        ),
        "시작 프로그램 값: {items:#?}"
    );
    assert!(
        has(
            &|k| matches!(k, LeftoverKind::RegKey { path, .. } if path.eq_ignore_ascii_case(&format!(r"{UNINSTALL}\{name}"))),
            Confidence::Certain
        ),
        "제거 정보 키: {items:#?}"
    );
    // 다른 앱/시스템 항목을 "확실"로 잡으면 안 된다.
    for i in &items {
        let loc = i.kind.location().to_lowercase();
        assert!(
            loc.contains(&name.to_lowercase()),
            "관계없는 항목이 검색됨: {i:#?}"
        );
    }

    // ── 정리 ──────────────────────────────────────────────
    let selected: Vec<_> = items
        .iter()
        .filter(|i| i.confidence >= Confidence::Likely)
        .cloned()
        .collect();
    let summary = cleaner::clean(
        &app.name,
        &app.aliases,
        &selected,
        CleanOptions { recycle: false },
        &ctx,
        &|_, _, _| {},
    );
    assert!(summary.failed.is_empty(), "{:?}", summary.failed);
    assert!(
        summary.reboot_pending.is_empty(),
        "{:?}",
        summary.reboot_pending
    );
    assert!(summary.freed >= 4096);

    assert!(!fsops::exists(&install));
    assert!(!fsops::exists(&data));
    assert!(!reg::key_exists(
        Hive::CurrentUser,
        &format!(r"Software\{name}")
    ));
    assert!(!reg::key_exists(
        Hive::CurrentUser,
        &format!(r"{UNINSTALL}\{name}")
    ));
    assert!(
        hkcu.open_subkey(RUN)
            .expect("Run")
            .get_raw_value(&name)
            .is_err()
    );
    assert!(!inventory::still_installed(&app));

    // 백업: 지운 레지스트리가 .reg로 남아 있어야 한다.
    let backup = summary.backup_dir.expect("백업 폴더");
    let reg_bytes = std::fs::read(join(&backup, "registry.reg")).expect("registry.reg");
    let words: Vec<u16> = reg_bytes[2..]
        .chunks_exact(2)
        .map(|c| u16::from_le_bytes([c[0], c[1]]))
        .collect();
    let reg_text = String::from_utf16_lossy(&words);
    assert!(reg_text.starts_with("Windows Registry Editor Version 5.00"));
    assert!(reg_text.contains(&format!(r"[HKEY_CURRENT_USER\Software\{name}\Settings]")));
    assert!(reg_text.contains("\"Theme\"=\"dark\""));
    let _ = std::fs::remove_dir_all(&backup);
}
