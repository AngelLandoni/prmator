//! macOS window chrome: the toolbar doubles as the titlebar (the window's titlebar is
//! transparent so content fills the window). gpui 0.2 doesn't implement window dragging
//! on macOS, so dragging the toolbar asks AppKit directly, like Zed does.

// objc 0.2's macros test `feature = "cargo-clippy"`, which newer compilers flag.
#![allow(unexpected_cfgs)]

use gpui::Window;
use objc::runtime::Object;
use objc::{class, msg_send, sel, sel_impl};
use raw_window_handle::{HasWindowHandle, RawWindowHandle};

/// Starts a native window drag from the event being handled (a mouse down or drag).
pub fn start_window_drag(window: &Window) {
    let Ok(handle) = HasWindowHandle::window_handle(window) else { return };
    let RawWindowHandle::AppKit(handle) = handle.as_raw() else { return };
    unsafe {
        let view = handle.ns_view.as_ptr() as *mut Object;
        let ns_window: *mut Object = msg_send![view, window];
        let app: *mut Object = msg_send![class!(NSApplication), sharedApplication];
        let event: *mut Object = msg_send![app, currentEvent];
        if !ns_window.is_null() && !event.is_null() {
            let _: () = msg_send![ns_window, performWindowDragWithEvent: event];
        }
    }
}
