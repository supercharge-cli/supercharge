//! Windows Terminal handles Ctrl+V itself. When the clipboard is an image and
//! has no text, it sends nothing into the PTY, so the TUI never reads the
//! picture. Inside Windows Terminal, a low-level key hook notices that chord
//! while this tab is focused. The input reader then runs the normal paste path.

use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Once, OnceLock};

use windows_sys::Win32::Foundation::{LPARAM, LRESULT, WPARAM};
use windows_sys::Win32::System::DataExchange::{
    CloseClipboard, GetClipboardData, IsClipboardFormatAvailable, OpenClipboard,
    RegisterClipboardFormatW,
};
use windows_sys::Win32::System::Memory::{GlobalLock, GlobalSize, GlobalUnlock};
use windows_sys::Win32::UI::Input::KeyboardAndMouse::{
    GetAsyncKeyState, VK_CONTROL, VK_MENU, VK_SHIFT, VK_V,
};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    CallNextHookEx, DispatchMessageW, GetClassNameW, GetForegroundWindow, GetMessageW,
    SetWindowsHookExW, TranslateMessage, HHOOK, KBDLLHOOKSTRUCT, MSG, WH_KEYBOARD_LL, WM_KEYDOWN,
};

const CF_DIB: u32 = 8;
const CF_UNICODETEXT: u32 = 13;
const CF_DIBV5: u32 = 17;
const WT_WINDOW_CLASS: &str = "CASCADIA_HOSTING_WINDOW_CLASS";

static REQUESTED: AtomicBool = AtomicBool::new(false);
static HOOK: AtomicUsize = AtomicUsize::new(0);
static FOCUSED: OnceLock<Arc<AtomicBool>> = OnceLock::new();

/// Pure gate shared by the hook and tests.
pub(crate) fn image_only_paste_requested(
    focused: bool,
    terminal_window_focused: bool,
    clipboard_image_only: bool,
) -> bool {
    focused && terminal_window_focused && clipboard_image_only
}

/// Installed once, and only when this process is inside Windows Terminal.
/// `focused` is updated by the input reader from focus reports.
pub(crate) fn install(focused: Arc<AtomicBool>) {
    static START: Once = Once::new();
    START.call_once(|| {
        if std::env::var_os("WT_SESSION").is_none() {
            return;
        }
        let _ = FOCUSED.set(focused);
        std::thread::Builder::new()
            .name("wt-image-paste".into())
            .spawn(|| {
                let hook = unsafe {
                    SetWindowsHookExW(WH_KEYBOARD_LL, Some(hook_proc), std::ptr::null_mut(), 0)
                };
                if hook.is_null() {
                    tracing::debug!("windows image-paste hook was not installed");
                    return;
                }
                HOOK.store(hook as usize, Ordering::Release);
                let mut msg = MSG::default();
                loop {
                    let got = unsafe { GetMessageW(&mut msg, std::ptr::null_mut(), 0, 0) };
                    if got == 0 || got == -1 {
                        break;
                    }
                    unsafe {
                        TranslateMessage(&msg);
                        DispatchMessageW(&msg);
                    }
                }
            })
            .ok();
    });
}

/// `true` once, when Ctrl+V landed in a focused Windows Terminal window and the
/// clipboard holds an image with no text. The caller turns that into a paste key.
pub(crate) fn take_paste_request() -> bool {
    if !REQUESTED.swap(false, Ordering::AcqRel) {
        return false;
    }
    clipboard_image_only()
}

unsafe extern "system" fn hook_proc(code: i32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    if code >= 0 && wparam == WM_KEYDOWN as usize {
        let info = unsafe { &*(lparam as *const KBDLLHOOKSTRUCT) };
        let focused = FOCUSED
            .get()
            .is_some_and(|flag| flag.load(Ordering::Acquire));
        // Clipboard is read on the input thread. Opening it here can stall every key.
        if info.vkCode == VK_V as u32
            && ctrl_only()
            && image_only_paste_requested(focused, foreground_is_windows_terminal(), true)
        {
            REQUESTED.store(true, Ordering::Release);
        }
    }
    let hook = HOOK.load(Ordering::Acquire) as HHOOK;
    unsafe { CallNextHookEx(hook, code, wparam, lparam) }
}

fn ctrl_only() -> bool {
    key_down(VK_CONTROL) && !key_down(VK_MENU) && !key_down(VK_SHIFT)
}

fn key_down(vk: u16) -> bool {
    unsafe { GetAsyncKeyState(vk as i32) as u16 & 0x8000 != 0 }
}

fn foreground_is_windows_terminal() -> bool {
    let hwnd = unsafe { GetForegroundWindow() };
    if hwnd.is_null() {
        return false;
    }
    let mut buf = [0u16; 64];
    let len = unsafe { GetClassNameW(hwnd, buf.as_mut_ptr(), buf.len() as i32) };
    if len <= 0 {
        return false;
    }
    let name = String::from_utf16_lossy(&buf[..len as usize]);
    name == WT_WINDOW_CLASS
}

fn clipboard_image_only() -> bool {
    struct Close;
    impl Drop for Close {
        fn drop(&mut self) {
            unsafe { CloseClipboard() };
        }
    }
    unsafe {
        if OpenClipboard(std::ptr::null_mut()) == 0 {
            return false;
        }
        let _close = Close;
        let image = IsClipboardFormatAvailable(CF_DIB) != 0
            || IsClipboardFormatAvailable(CF_DIBV5) != 0
            || png_format_available();
        image && !unicode_text_present()
    }
}

fn png_format_available() -> bool {
    let name: Vec<u16> = "PNG".encode_utf16().chain(std::iter::once(0)).collect();
    let format = unsafe { RegisterClipboardFormatW(name.as_ptr()) };
    format != 0 && unsafe { IsClipboardFormatAvailable(format) } != 0
}

fn unicode_text_present() -> bool {
    if unsafe { IsClipboardFormatAvailable(CF_UNICODETEXT) } == 0 {
        return false;
    }
    let handle = unsafe { GetClipboardData(CF_UNICODETEXT) };
    if handle.is_null() {
        return false;
    }
    let ptr = unsafe { GlobalLock(handle) } as *const u16;
    if ptr.is_null() {
        return false;
    }
    let count = unsafe { GlobalSize(handle) } / 2;
    let mut saw = false;
    for i in 0..count {
        let ch = unsafe { *ptr.add(i) };
        if ch == 0 {
            break;
        }
        if ch != ' ' as u16 && ch != '\t' as u16 && ch != '\r' as u16 && ch != '\n' as u16 {
            saw = true;
            break;
        }
    }
    unsafe { GlobalUnlock(handle) };
    saw
}

#[cfg(test)]
mod tests {
    use super::image_only_paste_requested;

    #[test]
    fn requests_only_a_focused_terminal_image() {
        assert!(image_only_paste_requested(true, true, true));
        assert!(!image_only_paste_requested(false, true, true));
        assert!(!image_only_paste_requested(true, false, true));
        assert!(!image_only_paste_requested(true, true, false));
    }
}
