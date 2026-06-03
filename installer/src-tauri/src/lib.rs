mod install;

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .invoke_handler(tauri::generate_handler![
            install::default_install_dir,
            install::install,
            install::launch_app,
            install::exit_installer
        ])
        .run(tauri::generate_context!())
        .expect("error while running installer");
}
