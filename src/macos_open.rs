//! macOS "open with Haven" support.
//!
//! Finder does not pass double-clicked files as command-line arguments; it sends
//! an Apple Event that AppKit turns into `application:openFiles:` on the app
//! delegate. winit's delegate does not implement that method, so we add it at
//! runtime and hand the paths to the same loopback channel that a second launch
//! uses on Windows (see `spawn_ipc` in app.rs). No extra crates are needed.
//!
//! The handler has to exist before AppKit delivers the open event, which on a
//! cold launch happens before the window is created. So `install_early` hooks
//! `-[NSApplication setDelegate:]` and adds `application:openFiles:` to whatever
//! delegate winit installs, the moment it is installed.

use std::ffi::{c_char, c_void, CStr};
use std::io::Write;
use std::net::{SocketAddr, TcpStream};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Once;
use std::time::Duration;

type Id = *mut c_void;
type Sel = *const c_void;

#[link(name = "objc")]
extern "C" {
    fn objc_getClass(name: *const c_char) -> Id;
    fn sel_registerName(name: *const c_char) -> Sel;
    fn object_getClass(obj: Id) -> Id;
    fn class_addMethod(cls: Id, name: Sel, imp: *const c_void, types: *const c_char) -> u8;
    fn objc_msgSend();
    fn class_getInstanceMethod(cls: Id, name: Sel) -> *mut c_void;
    fn method_setImplementation(m: *mut c_void, imp: *const c_void) -> *const c_void;
}

static ORIG_SET_DELEGATE: AtomicUsize = AtomicUsize::new(0);
static HOOK: Once = Once::new();

unsafe fn sel(name: &CStr) -> Sel {
    sel_registerName(name.as_ptr())
}

unsafe fn msg_id(obj: Id, name: &CStr) -> Id {
    let f: unsafe extern "C" fn(Id, Sel) -> Id =
        std::mem::transmute(objc_msgSend as unsafe extern "C" fn());
    f(obj, sel(name))
}

unsafe fn msg_usize(obj: Id, name: &CStr) -> usize {
    let f: unsafe extern "C" fn(Id, Sel) -> usize =
        std::mem::transmute(objc_msgSend as unsafe extern "C" fn());
    f(obj, sel(name))
}

unsafe fn msg_id_usize(obj: Id, name: &CStr, arg: usize) -> Id {
    let f: unsafe extern "C" fn(Id, Sel, usize) -> Id =
        std::mem::transmute(objc_msgSend as unsafe extern "C" fn());
    f(obj, sel(name), arg)
}

unsafe fn msg_cstr(obj: Id, name: &CStr) -> *const c_char {
    let f: unsafe extern "C" fn(Id, Sel) -> *const c_char =
        std::mem::transmute(objc_msgSend as unsafe extern "C" fn());
    f(obj, sel(name))
}

unsafe fn msg_void_isize(obj: Id, name: &CStr, arg: isize) {
    let f: unsafe extern "C" fn(Id, Sel, isize) =
        std::mem::transmute(objc_msgSend as unsafe extern "C" fn());
    f(obj, sel(name), arg)
}

/// `- (void)application:(NSApplication *)app openFiles:(NSArray<NSString *> *)files`
extern "C" fn open_files(_this: Id, _cmd: Sel, app: Id, files: Id) {
    let mut msg = String::new();
    unsafe {
        if !files.is_null() {
            for i in 0..msg_usize(files, c"count") {
                let s = msg_id_usize(files, c"objectAtIndex:", i);
                let c = msg_cstr(s, c"UTF8String");
                if !c.is_null() {
                    msg.push_str(&CStr::from_ptr(c).to_string_lossy());
                    msg.push('\n');
                }
            }
        }
        // AppKit requires a reply once openFiles: is implemented (0 = success).
        msg_void_isize(app, c"replyToOpenOrPrint:", 0);
    }
    if msg.is_empty() {
        return;
    }
    // Same channel a second launch uses; the listener thread queues and plays.
    std::thread::spawn(move || {
        let addr: SocketAddr = "127.0.0.1:45831".parse().unwrap();
        if let Ok(mut s) = TcpStream::connect_timeout(&addr, Duration::from_secs(2)) {
            let _ = s.write_all(msg.as_bytes());
        }
    });
}

unsafe fn add_open_method(cls: Id) {
    if cls.is_null() {
        return;
    }
    // Returns NO (harmlessly) when the class already has the method.
    class_addMethod(
        cls,
        sel(c"application:openFiles:"),
        open_files as *const c_void,
        c"v@:@@".as_ptr(),
    );
}

/// Replacement for `-[NSApplication setDelegate:]`: run the original, then give
/// the new delegate's class the open-files method.
extern "C" fn set_delegate(this: Id, cmd: Sel, delegate: Id) {
    unsafe {
        let orig = ORIG_SET_DELEGATE.load(Ordering::SeqCst);
        if orig != 0 {
            let f: extern "C" fn(Id, Sel, Id) = std::mem::transmute(orig);
            f(this, cmd, delegate);
        }
        if !delegate.is_null() {
            add_open_method(object_getClass(delegate));
        }
    }
}

/// Call once at the very start of `main`, before the event loop is created.
pub fn install_early() {
    HOOK.call_once(|| unsafe {
        let cls = objc_getClass(c"NSApplication".as_ptr());
        if cls.is_null() {
            return;
        }
        let m = class_getInstanceMethod(cls, sel(c"setDelegate:"));
        if m.is_null() {
            return;
        }
        let old = method_setImplementation(m, set_delegate as *const c_void);
        ORIG_SET_DELEGATE.store(old as usize, Ordering::SeqCst);
    });
}

/// Fallback: adds the method to the current delegate if the early hook missed
/// it (call from the eframe creation callback).
pub fn install() {
    unsafe {
        let app = msg_id(
            objc_getClass(c"NSApplication".as_ptr()),
            c"sharedApplication",
        );
        if app.is_null() {
            return;
        }
        let delegate = msg_id(app, c"delegate");
        if !delegate.is_null() {
            add_open_method(object_getClass(delegate));
        }
    }
}
