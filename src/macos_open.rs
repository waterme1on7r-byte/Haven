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
    fn objc_allocateClassPair(superclass: Id, name: *const c_char, extra: usize) -> Id;
    fn objc_registerClassPair(cls: Id);
    fn objc_msgSend();
    fn class_getInstanceMethod(cls: Id, name: Sel) -> *mut c_void;
    fn method_setImplementation(m: *mut c_void, imp: *const c_void) -> *const c_void;
}

static ORIG_SET_DELEGATE: AtomicUsize = AtomicUsize::new(0);
static STATUS_ITEM: AtomicUsize = AtomicUsize::new(0);
static STATUS_TARGET: AtomicUsize = AtomicUsize::new(0);
static MENU_READY: Once = Once::new();
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

unsafe fn msg_void_bool(obj: Id, name: &CStr, arg: i8) {
    let f: unsafe extern "C" fn(Id, Sel, i8) =
        std::mem::transmute(objc_msgSend as unsafe extern "C" fn());
    f(obj, sel(name), arg)
}

unsafe fn msg_void_id(obj: Id, name: &CStr, arg: Id) {
    let f: unsafe extern "C" fn(Id, Sel, Id) =
        std::mem::transmute(objc_msgSend as unsafe extern "C" fn());
    f(obj, sel(name), arg)
}

unsafe fn ns_string(text: &str) -> Id {
    let Ok(text) = std::ffi::CString::new(text) else {
        return std::ptr::null_mut();
    };
    let f: unsafe extern "C" fn(Id, Sel, *const c_char) -> Id =
        std::mem::transmute(objc_msgSend as unsafe extern "C" fn());
    f(
        objc_getClass(c"NSString".as_ptr()),
        sel(c"stringWithUTF8String:"),
        text.as_ptr(),
    )
}

unsafe fn status_command(name: &str) {
    let addr: SocketAddr = "127.0.0.1:45831".parse().unwrap();
    if let Ok(mut stream) = TcpStream::connect_timeout(&addr, Duration::from_millis(250)) {
        let _ = stream.write_all(format!("@HAVEN:{name}\n").as_bytes());
    }
}

extern "C" fn status_play(_this: Id, _cmd: Sel, _sender: Id) {
    unsafe { status_command("PLAY") }
}
extern "C" fn status_prev(_this: Id, _cmd: Sel, _sender: Id) {
    unsafe { status_command("PREV") }
}
extern "C" fn status_next(_this: Id, _cmd: Sel, _sender: Id) {
    unsafe { status_command("NEXT") }
}
extern "C" fn status_show(_this: Id, _cmd: Sel, _sender: Id) {
    unsafe { status_command("SHOW") }
}
extern "C" fn status_quit(_this: Id, _cmd: Sel, _sender: Id) {
    unsafe { status_command("QUIT") }
}

unsafe fn status_target() -> Id {
    MENU_READY.call_once(|| unsafe {
        let superclass = objc_getClass(c"NSObject".as_ptr());
        let cls = objc_allocateClassPair(superclass, c"HavenStatusBarTarget".as_ptr(), 0);
        if cls.is_null() {
            return;
        }
        for (selector, callback) in [
            (c"play:", status_play as *const c_void),
            (c"previous:", status_prev as *const c_void),
            (c"next:", status_next as *const c_void),
            (c"showHaven:", status_show as *const c_void),
            (c"quitHaven:", status_quit as *const c_void),
        ] {
            class_addMethod(cls, sel(selector), callback, c"v@:@".as_ptr());
        }
        objc_registerClassPair(cls);
    });
    let existing = STATUS_TARGET.load(Ordering::SeqCst) as Id;
    if !existing.is_null() {
        return existing;
    }
    let cls = objc_getClass(c"HavenStatusBarTarget".as_ptr());
    if cls.is_null() {
        return std::ptr::null_mut();
    }
    let target = msg_id(cls, c"new");
    STATUS_TARGET.store(target as usize, Ordering::SeqCst);
    target
}

unsafe fn make_status_menu_item(menu: Id, target: Id, title: &str, action: &CStr) {
    let alloc = msg_id(objc_getClass(c"NSMenuItem".as_ptr()), c"alloc");
    let init: unsafe extern "C" fn(Id, Sel, Id, Sel, Id) -> Id =
        std::mem::transmute(objc_msgSend as unsafe extern "C" fn());
    let item = init(
        alloc,
        sel(c"initWithTitle:action:keyEquivalent:"),
        ns_string(title),
        sel(action),
        ns_string(""),
    );
    if item.is_null() {
        return;
    }
    msg_void_id(item, c"setTarget:", target);
    let add: unsafe extern "C" fn(Id, Sel, Id) =
        std::mem::transmute(objc_msgSend as unsafe extern "C" fn());
    add(menu, sel(c"addItem:"), item);
}

unsafe fn create_status_item() -> bool {
    if !(STATUS_ITEM.load(Ordering::SeqCst) as Id).is_null() {
        return true;
    }
    let bar = msg_id(objc_getClass(c"NSStatusBar".as_ptr()), c"systemStatusBar");
    if bar.is_null() {
        return false;
    }
    let create: unsafe extern "C" fn(Id, Sel, f64) -> Id =
        std::mem::transmute(objc_msgSend as unsafe extern "C" fn());
    let item = create(bar, sel(c"statusItemWithLength:"), -1.0);
    if item.is_null() {
        return false;
    }
    STATUS_ITEM.store(item as usize, Ordering::SeqCst);
    let button = msg_id(item, c"button");
    if !button.is_null() {
        // Keep a visible glyph even if SF Symbols lookup/rendering differs by OS.
        msg_void_id(button, c"setTitle:", ns_string("♫"));
        msg_void_id(button, c"setToolTip:", ns_string("Haven"));
    } else {
        let remove: unsafe extern "C" fn(Id, Sel, Id) =
            std::mem::transmute(objc_msgSend as unsafe extern "C" fn());
        remove(bar, sel(c"removeStatusItem:"), item);
        STATUS_ITEM.store(0, Ordering::SeqCst);
        return false;
    }
    let menu = msg_id(objc_getClass(c"NSMenu".as_ptr()), c"new");
    let target = status_target();
    if menu.is_null() || target.is_null() {
        let remove: unsafe extern "C" fn(Id, Sel, Id) =
            std::mem::transmute(objc_msgSend as unsafe extern "C" fn());
        remove(bar, sel(c"removeStatusItem:"), item);
        STATUS_ITEM.store(0, Ordering::SeqCst);
        return false;
    }
    make_status_menu_item(menu, target, "Play / Pause", c"play:");
    make_status_menu_item(menu, target, "Previous", c"previous:");
    make_status_menu_item(menu, target, "Next", c"next:");
    let separator = msg_id(objc_getClass(c"NSMenuItem".as_ptr()), c"separatorItem");
    let add: unsafe extern "C" fn(Id, Sel, Id) =
        std::mem::transmute(objc_msgSend as unsafe extern "C" fn());
    add(menu, sel(c"addItem:"), separator);
    make_status_menu_item(menu, target, "Show Haven…", c"showHaven:");
    make_status_menu_item(menu, target, "Quit Haven", c"quitHaven:");
    msg_void_id(item, c"setMenu:", menu);
    true
}

/// Enable/disable the true macOS menu bar extra and hide Haven from the Dock.
pub fn set_menu_bar_mode(enabled: bool) -> bool {
    unsafe {
        let app = msg_id(
            objc_getClass(c"NSApplication".as_ptr()),
            c"sharedApplication",
        );
        if app.is_null() {
            return false;
        }
        if enabled {
            if !create_status_item() {
                msg_void_isize(app, c"setActivationPolicy:", 0);
                return false;
            }
            msg_void_isize(app, c"setActivationPolicy:", 1);
        } else {
            msg_void_isize(app, c"setActivationPolicy:", 0);
            let item = STATUS_ITEM.swap(0, Ordering::SeqCst) as Id;
            if !item.is_null() {
                let bar = msg_id(objc_getClass(c"NSStatusBar".as_ptr()), c"systemStatusBar");
                msg_void_id(bar, c"removeStatusItem:", item);
            }
        }
        true
    }
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

        // Haven draws its own shadow inside a transparent, borderless window.
        // AppKit's native shadow can leave ghost copies while the animated
        // waveform repaints the window, so disable it for the player window.
        let windows = msg_id(app, c"windows");
        if !windows.is_null() {
            for i in 0..msg_usize(windows, c"count") {
                let window = msg_id_usize(windows, c"objectAtIndex:", i);
                if !window.is_null() {
                    msg_void_bool(window, c"setHasShadow:", 0);
                }
            }
        }
    }
}
