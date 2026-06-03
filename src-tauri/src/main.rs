// Windows에서 콘솔 창이 뜨지 않도록 한다(릴리스 빌드).
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

fn main() {
    app_deleter_lib::run();
}
