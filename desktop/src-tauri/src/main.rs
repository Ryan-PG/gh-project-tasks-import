// Hide the console window on Windows release builds. Without this the app
// opens a terminal alongside the window, which looks broken.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

fn main() {
    github_importer_lib::run()
}
