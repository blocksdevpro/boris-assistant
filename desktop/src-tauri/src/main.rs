//! Binary entry for the Boris desktop host.
//!
//! All product logic lives in the `boris_desktop_lib` library crate so tests
//! and the mobile entry can share the same surface.

// Prevents an extra console window on Windows in release — do not remove.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

fn main() {
    boris_desktop_lib::run()
}
