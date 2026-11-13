use std::ffi::OsString;
use std::io;
use std::os::windows::ffi::OsStringExt;
use std::path::PathBuf;
use std::ptr::null_mut;

use raw_window_handle::{HasWindowHandle, RawWindowHandle};
use windows_sys::Win32::Foundation::{HWND, RECT};
use windows_sys::Win32::Graphics::Dwm::{DWMWA_EXTENDED_FRAME_BOUNDS, DwmGetWindowAttribute};
use windows_sys::Win32::Graphics::Gdi::{
    CreateRoundRectRgn, DeleteObject, GetMonitorInfoW, MONITOR_DEFAULTTONEAREST, MONITORINFO,
    MonitorFromWindow, SetWindowRgn,
};
use windows_sys::Win32::System::Com::CoTaskMemFree;
use windows_sys::Win32::UI::HiDpi::GetDpiForWindow;
use windows_sys::Win32::UI::Shell::{FOLDERID_LocalAppData, SHGetKnownFolderPath};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    GWL_EXSTYLE, GWL_STYLE, GetDesktopWindow, GetForegroundWindow, GetShellWindow, GetWindowLongW,
    GetWindowRect, HWND_BOTTOM, HWND_NOTOPMOST, HWND_TOPMOST, IsZoomed, MB_ICONERROR, MessageBoxW,
    SWP_NOACTIVATE, SWP_NOMOVE, SWP_NOSIZE, SWP_NOZORDER, SetWindowPos, WS_CAPTION, WS_EX_TOPMOST,
};

use crate::domain::Corner;

pub fn local_app_data() -> io::Result<PathBuf> {
    let mut path = null_mut();
    // The known-folder API allocates a NUL-terminated string for the current user.
    let status = unsafe { SHGetKnownFolderPath(&FOLDERID_LocalAppData, 0, null_mut(), &mut path) };
    let result = if status < 0 || path.is_null() {
        Err(io::Error::other(format!(
            "LocalAppData HRESULT {status:#x}"
        )))
    } else {
        let mut len = 0;
        // The allocation remains live until the string has been copied.
        unsafe {
            while *path.add(len) != 0 {
                len += 1;
            }
            Ok(PathBuf::from(OsString::from_wide(
                std::slice::from_raw_parts(path, len),
            )))
        }
    };
    unsafe { CoTaskMemFree(path.cast()) };
    result
}

pub fn place_window(window: &slint::Window, corner: Corner, expanded: bool) -> io::Result<()> {
    place_hwnd(window_hwnd(window)?, corner, expanded)
}

fn window_hwnd(window: &slint::Window) -> io::Result<HWND> {
    let handle = window.window_handle();
    let RawWindowHandle::Win32(handle) = handle.window_handle().map_err(io::Error::other)?.as_raw()
    else {
        return Err(io::Error::other("expected a Win32 window"));
    };
    Ok(handle.hwnd.get() as HWND)
}

pub fn update_window_pin(window: &slint::Window, pinned: bool) -> io::Result<()> {
    let hwnd = window_hwnd(window)?;
    let yield_to_fullscreen = pinned && foreground_covers_monitor(hwnd);
    let topmost = pinned && !yield_to_fullscreen;
    // This window belongs to the UI thread. Changing its Z order keeps foreground focus intact.
    unsafe {
        let is_topmost = GetWindowLongW(hwnd, GWL_EXSTYLE) as u32 & WS_EX_TOPMOST != 0;
        if is_topmost == topmost {
            return Ok(());
        }
        let after = if topmost {
            HWND_TOPMOST
        } else if yield_to_fullscreen {
            HWND_BOTTOM
        } else {
            HWND_NOTOPMOST
        };
        if SetWindowPos(
            hwnd,
            after,
            0,
            0,
            0,
            0,
            SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE,
        ) == 0
        {
            return Err(io::Error::last_os_error());
        }
    }
    Ok(())
}

fn foreground_covers_monitor(hwnd: HWND) -> bool {
    let mut monitor = MONITORINFO {
        cbSize: size_of::<MONITORINFO>() as u32,
        ..Default::default()
    };
    let mut bounds = RECT::default();
    // Win32 supplies window handles and geometry. No window titles or contents are read.
    unsafe {
        let foreground = GetForegroundWindow();
        if foreground.is_null()
            || foreground == hwnd
            || foreground == GetDesktopWindow()
            || foreground == GetShellWindow()
        {
            return false;
        }
        let screen = MonitorFromWindow(hwnd, MONITOR_DEFAULTTONEAREST);
        if GetMonitorInfoW(screen, &mut monitor) == 0 {
            return false;
        }
        let style = GetWindowLongW(foreground, GWL_STYLE) as u32;
        if IsZoomed(foreground) != 0 && style & WS_CAPTION == WS_CAPTION {
            return false;
        }
        if DwmGetWindowAttribute(
            foreground,
            DWMWA_EXTENDED_FRAME_BOUNDS as u32,
            (&mut bounds as *mut RECT).cast(),
            size_of::<RECT>() as u32,
        ) < 0
            && GetWindowRect(foreground, &mut bounds) == 0
        {
            return false;
        }
    }
    let screen = monitor.rcMonitor;
    bounds.left <= screen.left
        && bounds.top <= screen.top
        && bounds.right >= screen.right
        && bounds.bottom >= screen.bottom
}

fn place_hwnd(hwnd: HWND, corner: Corner, expanded: bool) -> io::Result<()> {
    let mut monitor = MONITORINFO {
        cbSize: size_of::<MONITORINFO>() as u32,
        ..Default::default()
    };
    // The borrowed HWND stays valid on the UI thread for these synchronous calls.
    unsafe {
        let nearest = MonitorFromWindow(hwnd, MONITOR_DEFAULTTONEAREST);
        if GetMonitorInfoW(nearest, &mut monitor) == 0 {
            return Err(io::Error::last_os_error());
        }
        let mut rect = Default::default();
        if GetWindowRect(hwnd, &mut rect) == 0 {
            return Err(io::Error::last_os_error());
        }
        let width = rect.right - rect.left;
        let height = rect.bottom - rect.top;
        let dpi = GetDpiForWindow(hwnd).max(96) as i32;
        let work = monitor.rcWork;
        let margin = 16 * dpi / 96;
        let x = match corner {
            Corner::TopLeft | Corner::BottomLeft => work.left + margin,
            Corner::TopRight | Corner::BottomRight => work.right - width - margin,
        }
        .clamp(work.left, (work.right - width).max(work.left));
        let y = match corner {
            Corner::TopLeft | Corner::TopRight => work.top + margin,
            Corner::BottomLeft | Corner::BottomRight => work.bottom - height - margin,
        }
        .clamp(work.top, (work.bottom - height).max(work.top));
        if (rect.left != x || rect.top != y)
            && SetWindowPos(
                hwnd,
                null_mut(),
                x,
                y,
                0,
                0,
                SWP_NOSIZE | SWP_NOZORDER | SWP_NOACTIVATE,
            ) == 0
        {
            return Err(io::Error::last_os_error());
        }

        // A native region clips the software surface and gives transparent corners no hit area.
        let diameter = (if expanded { 40 } else { 42 }) * dpi / 96;
        let region = CreateRoundRectRgn(0, 0, width + 1, height + 1, diameter, diameter);
        if region.is_null() {
            return Err(io::Error::last_os_error());
        }
        if SetWindowRgn(hwnd, region, 1) == 0 {
            let error = io::Error::last_os_error();
            DeleteObject(region);
            return Err(error);
        }
        // Windows owns the region after SetWindowRgn succeeds.
    }
    Ok(())
}

pub fn show_startup_error() {
    let text: Vec<u16> = "Не удалось запустить MIMO. Запустите cargo run для диагностики."
        .encode_utf16()
        .chain([0])
        .collect();
    let title: Vec<u16> = "MIMO".encode_utf16().chain([0]).collect();
    // Both strings are NUL-terminated and valid for the duration of the dialog.
    unsafe { MessageBoxW(null_mut(), text.as_ptr(), title.as_ptr(), MB_ICONERROR) };
}
