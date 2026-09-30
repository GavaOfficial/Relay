#![cfg_attr(windows, windows_subsystem = "windows")]
#[cfg(windows)]
mod win;
fn main() {
    #[cfg(windows)]
    if let Err(e) = win::run() {
        eprintln!("{e:#}");
        std::process::exit(1);
    }
}
