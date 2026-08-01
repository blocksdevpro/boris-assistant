//! Legacy CLI entrypoint — retired.
//!
//! The product host is **boris-desktop** (`desktop/src-tauri`).
//! Voice runtime lives in `boris-pipeline`.

fn main() {
    eprintln!("boris-assistant CLI is retired.");
    eprintln!("Run the desktop app:  cd desktop && bun run tauri dev");
    std::process::exit(1);
}
