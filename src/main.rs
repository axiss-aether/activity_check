#![cfg_attr(target_os = "windows", windows_subsystem = "windows")]

#[cfg(not(target_os = "windows"))]
fn main() {
    eprintln!("Ритм работает только в Windows.");
}

#[cfg(target_os = "windows")]
mod app;

#[cfg(target_os = "windows")]
fn main() {
    use tauri::{Manager, RunEvent};

    let result = tauri::Builder::default()
        .setup(|app| {
            let tracker = std::sync::Arc::new(std::sync::Mutex::new(
                app::Tracker::new().map_err(std::io::Error::other)?,
            ));
            app.manage(tracker.clone());
            app::start_worker(tracker);
            app::attach_tray(app)?;
            Ok(())
        })
        .on_window_event(|window, event| app::handle_window_event(window, event))
        .invoke_handler(tauri::generate_handler![
            app::get_snapshot,
            app::set_preference
        ])
        .build(tauri::generate_context!());

    match result {
        Ok(app) => app.run(|app_handle, event| {
            if matches!(event, RunEvent::Exit) {
                app::close_session(&app_handle.state::<app::SharedTracker>());
            }
        }),
        Err(error) => {
            let _ = std::fs::write(
                std::env::temp_dir().join("ritm-startup-error.txt"),
                format!("Не удалось запустить Ритм: {error}"),
            );
        }
    }
}
