//! Windows 통합 테스트. 실제 파일 시스템·레지스트리(HKCU)·프로세스를 사용하므로
//! 기본으로는 건너뛰고(`#[ignore]`), CI(Windows 러너)에서 `--include-ignored`로 실행한다.

use std::sync::atomic::AtomicBool;
use std::time::{Duration, Instant};

use crate::pathutil::join;
use crate::safety::Hive;
use crate::win::job::{RunEnd, RunError, run_and_wait};
use crate::win::{fsops, icons, reg};

fn temp_root(tag: &str) -> String {
    let base = std::env::temp_dir().to_string_lossy().into_owned();
    let dir = join(
        &base,
        &format!("app-deleter-test-{tag}-{}", std::process::id()),
    );
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("임시 폴더 생성");
    dir
}

fn write(path: &str, len: usize) {
    if let Some(p) = crate::pathutil::parent(path) {
        std::fs::create_dir_all(p).expect("상위 폴더 생성");
    }
    std::fs::write(path, vec![7u8; len]).expect("파일 쓰기");
}

#[test]
#[ignore = "실제 파일 시스템 사용 — CI에서 실행"]
fn dir_size_counts_files_and_skips_junctions() {
    let root = temp_root("size");
    write(&join(&root, "a.bin"), 1000);
    write(&join(&root, r"sub\b.bin"), 2345);
    write(&join(&root, r"sub\deep\deeper\c.bin"), 10);
    for i in 0..50 {
        write(&join(&root, &format!(r"many\d{i}\f.bin")), 100);
    }
    let outside = temp_root("outside");
    write(&join(&outside, "big.bin"), 50_000);
    let link = join(&root, "junction");
    let status = std::process::Command::new("cmd.exe")
        .args(["/c", "mklink", "/J", &link, &outside])
        .status()
        .expect("mklink 실행");
    assert!(status.success(), "정션 생성 실패");

    assert_eq!(fsops::dir_size(&root), 1000 + 2345 + 10 + 50 * 100);

    // 정션은 링크만 지우고 대상 폴더 내용은 보존해야 한다.
    let stats = fsops::delete_path(&root);
    assert!(stats.failed.is_empty(), "{:?}", stats.failed);
    assert!(!fsops::exists(&root));
    assert!(
        fsops::exists(&join(&outside, "big.bin")),
        "정션 대상이 지워지면 안 됨"
    );
    let _ = std::fs::remove_dir_all(&outside);
}

#[test]
#[ignore = "실제 파일 시스템 사용 — CI에서 실행"]
fn delete_handles_readonly_files_and_long_paths() {
    let root = temp_root("del");
    let ro = join(&root, "readonly.txt");
    write(&ro, 10);
    let mut perm = std::fs::metadata(&ro).expect("메타데이터").permissions();
    perm.set_readonly(true);
    std::fs::set_permissions(&ro, perm).expect("읽기 전용 설정");

    // 260자를 넘는 경로.
    let mut long = root.clone();
    while long.len() < 320 {
        long = join(&long, "very_long_directory_name_for_testing");
    }
    let long_path = format!(r"\\?\{}", join(&long, "f.txt"));
    std::fs::create_dir_all(format!(r"\\?\{long}")).expect("긴 경로 폴더");
    std::fs::write(&long_path, b"x").expect("긴 경로 파일");

    let stats = fsops::delete_path(&root);
    assert!(stats.failed.is_empty(), "{:?}", stats.failed);
    assert!(stats.scheduled.is_empty());
    assert!(!fsops::exists(&root));
}

#[test]
#[ignore = "HKCU 레지스트리 사용 — CI에서 실행"]
fn registry_export_and_delete() {
    let path = format!(r"Software\AppDeleterRegTest{}", std::process::id());
    let (key, _) = winreg::RegKey::predef(winreg::enums::HKEY_CURRENT_USER)
        .create_subkey(format!(r"{path}\Child"))
        .expect("키 생성");
    key.set_value("Name", &"값 \"따옴표\" C:\\경로")
        .expect("값 쓰기");
    key.set_value("Count", &42u32).expect("DWORD 쓰기");

    let mut text = String::new();
    reg::export_key(Hive::CurrentUser, &path, &mut text).expect("내보내기");
    assert!(text.contains(&format!(r"[HKEY_CURRENT_USER\{path}\Child]")));
    assert!(text.contains("\"Count\"=dword:0000002a"));
    assert!(text.contains(r#""Name"="값 \"따옴표\" C:\\경로""#));

    reg::delete_key(Hive::CurrentUser, &path).expect("삭제");
    assert!(!reg::key_exists(Hive::CurrentUser, &path));
    // 이미 없는 키 삭제는 성공으로 본다.
    reg::delete_key(Hive::CurrentUser, &path).expect("없는 키 삭제");
}

#[test]
#[ignore = "실제 프로세스 실행 — CI에서 실행"]
fn job_waits_for_child_processes() {
    let skip = AtomicBool::new(false);
    // 루트 cmd는 즉시 끝나고, 백그라운드 자식이 약 2초 더 실행된다.
    let started = Instant::now();
    let r = run_and_wait(
        r#"cmd.exe /c start "" /b cmd.exe /c ping -n 3 127.0.0.1 > nul"#,
        &skip,
        |_| {},
    );
    assert!(matches!(r, Ok(RunEnd::Completed { .. })), "{r:?}");
    assert!(
        started.elapsed() >= Duration::from_millis(1500),
        "자식 프로세스를 기다리지 않음: {:?}",
        started.elapsed()
    );

    match run_and_wait("cmd.exe /c exit 3", &skip, |_| {}) {
        Ok(RunEnd::Completed { exit_code }) => assert_eq!(exit_code, Some(3)),
        other => panic!("{other:?}"),
    }
    assert!(matches!(
        run_and_wait(r#""C:\does-not-exist\uninstall.exe" /S"#, &skip, |_| {}),
        Err(RunError::NotFound(_))
    ));
}

#[test]
#[ignore = "Windows 셸 리소스 사용 — CI에서 실행"]
fn extracts_icons_from_system_binaries() {
    let windir = std::env::var("SystemRoot").unwrap_or_else(|_| r"C:\Windows".into());
    let img = icons::load(
        &icons::IconSource::Resource(format!(r"{windir}\explorer.exe,0")),
        32,
    )
    .expect("explorer.exe 아이콘");
    assert_eq!((img.width, img.height), (32, 32));
    assert_eq!(img.pixels.len(), 32 * 32 * 4);
    assert!(
        img.pixels.chunks_exact(4).any(|p| p[3] > 0),
        "완전히 투명한 아이콘"
    );
}
