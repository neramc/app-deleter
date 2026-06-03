mod apps;
#[cfg(windows)]
mod fastsize;
mod uninstall;

pub use apps::{list_installed_apps, InstalledApp};
pub use uninstall::uninstall_app;

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .invoke_handler(tauri::generate_handler![
            list_installed_apps,
            uninstall_app
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
