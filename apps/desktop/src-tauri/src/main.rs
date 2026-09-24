//! Pulpit desktop editor binary: everything lives in the library so the
//! Tauri mobile targets and tests can reuse it.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

fn main() {
    pulpit_desktop::run()
}
