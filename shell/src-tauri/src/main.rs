// Skeleton stage: no Windows console (tauri convention)
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

fn main() {
    superlive_shell_lib::run()
}
