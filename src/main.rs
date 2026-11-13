#![cfg_attr(all(windows, not(debug_assertions)), windows_subsystem = "windows")]

#[cfg(windows)]
mod app;
#[cfg(windows)]
mod domain;
#[cfg(windows)]
mod platform;
#[cfg(windows)]
mod policy;
#[cfg(windows)]
mod router;
#[cfg(windows)]
mod storage;
#[cfg(windows)]
mod tasks;
#[cfg(windows)]
mod ui;

#[cfg(windows)]
fn main() {
    if let Err(error) = app::run() {
        eprintln!("MIMO: {error}");
        platform::windows::show_startup_error();
        std::process::exit(1);
    }
}

#[cfg(not(windows))]
compile_error!("MIMO targets Windows 10 and Windows 11");
