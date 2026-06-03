mod uninstall;

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .invoke_handler(tauri::generate_handler![
            uninstall::app_info,
            uninstall::perform_uninstall,
            uninstall::exit_app
        ])
        .run(tauri::generate_context!())
        .expect("error while running uninstaller");
}
