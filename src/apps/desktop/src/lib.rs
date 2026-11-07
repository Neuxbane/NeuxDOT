//! Tauri application entry point.
//!
//! Desktop and mobile share this `run()`; `main.rs` calls it on desktop, and the
//! Android/iOS generated project calls it through `mobile_entry_point`.

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
