//! App Deleter — Windows 11 전용 고성능 앱 완전 삭제 도구 (Rust + Slint).
//!
//! 순수 로직(경로·이름 매칭·명령줄·백업 포맷·안전 가드)은 모든 OS에서 컴파일·테스트되고,
//! Win32/WinRT를 쓰는 부분과 UI는 Windows에서만 빌드된다.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]
#![cfg_attr(not(windows), allow(dead_code))]

mod cmdline;
mod format;
mod installdir;
mod pathutil;
mod regfile;
mod safety;
mod textmatch;

#[cfg(windows)]
mod app;
#[cfg(windows)]
mod cleaner;
#[cfg(all(windows, test))]
mod e2e_tests;
#[cfg(windows)]
mod inventory;
#[cfg(windows)]
mod leftovers;
#[cfg(windows)]
mod win;

#[cfg(windows)]
fn main() -> Result<(), slint::PlatformError> {
    app::run()
}

#[cfg(not(windows))]
fn main() {
    eprintln!("App Deleter는 Windows 11 전용 앱입니다.");
    std::process::exit(1);
}
