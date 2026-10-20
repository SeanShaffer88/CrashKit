//! C ABI surface — everything declared in include/crashkit.h.
//! Conventions (house standard): string inputs are const char* UTF-8;
//! `*mut c_char` results are caller-owned (free with ck_free_string);
//! failures return an ERR_* code (or null) + thread-local last error;
//! nothing unwinds across the boundary.

// The exported functions ARE the raw-pointer C ABI declared in
// include/crashkit.h; pointer validity is the documented contract there.
#![allow(clippy::not_unsafe_ptr_arg_deref)]

use std::cell::RefCell;
use std::ffi::{CStr, CString};
use std::os::raw::{c_char, c_int};
use std::panic::catch_unwind;

use crate::{CK_OK, ERR_INTERNAL, ERR_INVALID_ARG};

thread_local! {
    static LAST_ERROR: RefCell<(i32, CString)> =
        RefCell::new((CK_OK, CString::new("").unwrap()));
}

fn set_error(code: i32, msg: &str) {
    let clean: String = msg.chars().filter(|&c| c != '\0').collect();
    LAST_ERROR.with(|e| {
        *e.borrow_mut() = (
            code,
            CString::new(clean).unwrap_or_else(|_| CString::new("error").unwrap()),
        );
    });
}

fn clear_error() {
    set_error(CK_OK, "");
}

fn guard<T, F: FnOnce() -> T>(f: F) -> std::thread::Result<T> {
    catch_unwind(std::panic::AssertUnwindSafe(f))
}

fn input(ptr: *const c_char, what: &str) -> Option<String> {
    if ptr.is_null() {
        set_error(ERR_INVALID_ARG, &format!("{what} must not be null"));
        return None;
    }
    match unsafe { CStr::from_ptr(ptr) }.to_str() {
        Ok(s) => Some(s.to_string()),
        Err(_) => {
            set_error(ERR_INVALID_ARG, &format!("{what} is not valid UTF-8"));
            None
        }
    }
}

fn status(r: crate::CResult<()>) -> c_int {
    match r {
        Ok(()) => {
            clear_error();
            CK_OK
        }
        Err((code, msg)) => {
            set_error(code, &msg);
            code
        }
    }
}

fn panic_code() -> c_int {
    set_error(ERR_INTERNAL, "internal panic caught at the FFI boundary");
    ERR_INTERNAL
}

#[no_mangle]
pub extern "C" fn ck_version() -> *const c_char {
    concat!(env!("CARGO_PKG_VERSION"), "\0").as_ptr() as *const c_char
}

/// Install the crash handler: dumps and the log go to `dir`
/// (created if missing); filenames are prefixed with `app`.
/// preset: 0 mini, 1 standard, 2 full. Re-installing replaces the
/// configuration. Returns CK_OK or an ERR_* code.
#[no_mangle]
pub extern "C" fn ck_install(dir: *const c_char, app: *const c_char, preset: c_int) -> c_int {
    match guard(|| {
        let Some(dir) = input(dir, "dir") else {
            return ERR_INVALID_ARG;
        };
        let Some(app) = input(app, "app") else {
            return ERR_INVALID_ARG;
        };
        if let Err((c, m)) = crate::validate_app_name(&app) {
            set_error(c, &m);
            return c;
        }
        if let Err((c, m)) = crate::dump_type_flags(preset) {
            set_error(c, &m);
            return c;
        }
        status(crate::platform::install(&dir, &app, preset))
    }) {
        Ok(c) => c,
        Err(_) => panic_code(),
    }
}

#[no_mangle]
pub extern "C" fn ck_is_installed() -> c_int {
    if crate::platform::is_installed() {
        1
    } else {
        0
    }
}

/// Write a minidump of the current process right now (same naming and
/// sidecar as a crash dump, with null exception fields). Returns the
/// dump path (free with ck_free_string), or null on failure.
#[no_mangle]
pub extern "C" fn ck_write_minidump(preset: c_int) -> *mut c_char {
    match guard(|| {
        if let Err((c, m)) = crate::dump_type_flags(preset) {
            set_error(c, &m);
            return std::ptr::null_mut();
        }
        match crate::platform::write_minidump(preset) {
            Ok(p) => match CString::new(p) {
                Ok(c) => {
                    clear_error();
                    c.into_raw()
                }
                Err(_) => {
                    set_error(ERR_INTERNAL, "dump path contained a NUL byte");
                    std::ptr::null_mut()
                }
            },
            Err((c, m)) => {
                set_error(c, &m);
                std::ptr::null_mut()
            }
        }
    }) {
        Ok(p) => p,
        Err(_) => {
            let _ = panic_code();
            std::ptr::null_mut()
        }
    }
}

/// Append one structured line to <dir>/<app>.log (JSON Lines; rolls to
/// .log.1 past 5 MiB). level: 0 TRACE, 1 DEBUG, 2 INFO, 3 WARN, 4 ERROR.
#[no_mangle]
pub extern "C" fn ck_log(level: c_int, tag: *const c_char, msg: *const c_char) -> c_int {
    match guard(|| {
        let Some(tag) = input(tag, "tag") else {
            return ERR_INVALID_ARG;
        };
        let Some(msg) = input(msg, "msg") else {
            return ERR_INVALID_ARG;
        };
        status(crate::platform::log(level, &tag, &msg))
    }) {
        Ok(c) => c,
        Err(_) => panic_code(),
    }
}

/// Attach a context entry included in every crash sidecar (build id,
/// GPU model, session kind — whatever helps triage).
#[no_mangle]
pub extern "C" fn ck_set_context(key: *const c_char, value: *const c_char) -> c_int {
    match guard(|| {
        let Some(key) = input(key, "key") else {
            return ERR_INVALID_ARG;
        };
        if let Err((c, m)) = crate::validate_context_key(&key) {
            set_error(c, &m);
            return c;
        }
        let Some(value) = input(value, "value") else {
            return ERR_INVALID_ARG;
        };
        status(crate::platform::set_context(&key, &value))
    }) {
        Ok(c) => c,
        Err(_) => panic_code(),
    }
}

#[no_mangle]
pub extern "C" fn ck_remove_context(key: *const c_char) -> c_int {
    match guard(|| {
        let Some(key) = input(key, "key") else {
            return ERR_INVALID_ARG;
        };
        status(crate::platform::remove_context(&key))
    }) {
        Ok(c) => c,
        Err(_) => panic_code(),
    }
}

/// Path of the most recent dump written (crash or manual), or null.
#[no_mangle]
pub extern "C" fn ck_last_dump_path() -> *mut c_char {
    match guard(|| match crate::platform::last_dump_path() {
        Ok(Some(p)) => match CString::new(p) {
            Ok(c) => {
                clear_error();
                c.into_raw()
            }
            Err(_) => {
                set_error(ERR_INTERNAL, "dump path contained a NUL byte");
                std::ptr::null_mut()
            }
        },
        Ok(None) => {
            clear_error();
            std::ptr::null_mut()
        }
        Err((c, m)) => {
            set_error(c, &m);
            std::ptr::null_mut()
        }
    }) {
        Ok(p) => p,
        Err(_) => {
            let _ = panic_code();
            std::ptr::null_mut()
        }
    }
}

#[no_mangle]
pub extern "C" fn ck_free_string(ptr: *mut c_char) {
    if !ptr.is_null() {
        let _ = guard(|| unsafe {
            drop(CString::from_raw(ptr));
        });
    }
}

#[no_mangle]
pub extern "C" fn ck_last_error() -> *const c_char {
    LAST_ERROR.with(|e| e.borrow().1.as_ptr())
}

#[no_mangle]
pub extern "C" fn ck_last_error_code() -> c_int {
    LAST_ERROR.with(|e| e.borrow().0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::ptr;

    fn cs(s: &str) -> CString {
        CString::new(s).unwrap()
    }

    #[test]
    fn ffi_contract() {
        assert!(!ck_version().is_null());
        assert_eq!(ck_is_installed(), 0);
        assert_eq!(
            ck_install(ptr::null(), cs("App").as_ptr(), 1),
            ERR_INVALID_ARG
        );
        assert_eq!(
            ck_install(cs("x").as_ptr(), cs("bad app").as_ptr(), 1),
            ERR_INVALID_ARG
        );
        assert_eq!(
            ck_install(cs("x").as_ptr(), cs("App").as_ptr(), 9),
            ERR_INVALID_ARG
        );
        #[cfg(not(windows))]
        {
            assert_eq!(
                ck_install(cs("x").as_ptr(), cs("App").as_ptr(), 1),
                crate::ERR_NOT_SUPPORTED
            );
            assert!(ck_write_minidump(1).is_null());
            assert_eq!(ck_last_error_code(), crate::ERR_NOT_SUPPORTED);
            // Not installed: log/context report NOT_INSTALLED.
            assert_eq!(
                ck_log(2, cs("t").as_ptr(), cs("m").as_ptr()),
                crate::ERR_NOT_INSTALLED
            );
            assert_eq!(
                ck_set_context(cs("k").as_ptr(), cs("v").as_ptr()),
                crate::ERR_NOT_INSTALLED
            );
        }
        assert_eq!(ck_log(9, cs("t").as_ptr(), cs("m").as_ptr()), {
            // level is validated inside platform::log before install check?
            // No: platform::log validates the level first, so this is INVALID_ARG.
            ERR_INVALID_ARG
        });
        ck_free_string(ptr::null_mut());
    }
}
