use std::io;
use std::ptr::null_mut;
use std::sync::mpsc;
use std::thread::{self, JoinHandle};

use windows_sys::Win32::System::Threading::GetCurrentThreadId;
use windows_sys::Win32::UI::Input::KeyboardAndMouse::{
    MOD_ALT, MOD_CONTROL, MOD_NOREPEAT, RegisterHotKey, UnregisterHotKey,
};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    GetMessageW, MSG, PM_NOREMOVE, PeekMessageW, PostThreadMessageW, WM_HOTKEY, WM_QUIT,
};

pub struct Hotkey {
    id: u32,
    worker: Option<JoinHandle<()>>,
}

impl Hotkey {
    pub fn start(activate: impl Fn() + Send + 'static) -> io::Result<Self> {
        let (send, receive) = mpsc::sync_channel(1);
        let worker = thread::Builder::new()
            .name("mimo-hotkey".into())
            .spawn(move || {
                // This thread owns the registration and its Win32 message queue.
                unsafe {
                    let mut message = MSG::default();
                    PeekMessageW(&mut message, null_mut(), 0, 0, PM_NOREMOVE);
                    if RegisterHotKey(
                        null_mut(),
                        1,
                        MOD_CONTROL | MOD_ALT | MOD_NOREPEAT,
                        u32::from(b'M'),
                    ) == 0
                    {
                        let _ = send.send(Err(io::Error::last_os_error()));
                        return;
                    }
                    let _ = send.send(Ok(GetCurrentThreadId()));
                    while GetMessageW(&mut message, null_mut(), 0, 0) > 0 {
                        if message.message == WM_HOTKEY && message.wParam == 1 {
                            activate();
                        }
                    }
                    UnregisterHotKey(null_mut(), 1);
                }
            })?;
        let id = receive.recv().map_err(io::Error::other)??;
        Ok(Self {
            id,
            worker: Some(worker),
        })
    }
}

impl Drop for Hotkey {
    fn drop(&mut self) {
        // Wake the message loop so it unregisters the shortcut before the process exits.
        if unsafe { PostThreadMessageW(self.id, WM_QUIT, 0, 0) } != 0
            && let Some(worker) = self.worker.take()
        {
            let _ = worker.join();
        }
    }
}
