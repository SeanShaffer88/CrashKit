//! Windows-only end-to-end proof through the public C ABI. Covers an
//! install + manual minidump (MDMP signature, sidecar, log line), and a
//! real crash: a child copy of this test installs the handler and
//! dereferences null; the parent verifies dump + sidecar appeared.
//! Compiles to nothing off Windows; runs natively on Windows CI.

#![cfg(windows)]

use crashkit::ffi::*;
use std::ffi::{CStr, CString};
use std::os::raw::c_char;
use std::path::PathBuf;

fn cs(s: &str) -> CString {
    CString::new(s).unwrap()
}

fn take(ptr: *mut c_char) -> String {
    assert!(!ptr.is_null(), "null result");
    let s = unsafe { CStr::from_ptr(ptr) }.to_str().unwrap().to_string();
    ck_free_string(ptr);
    s
}

fn temp_dir(tag: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("ck-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

#[test]
fn manual_dump_and_log() {
    let dir = temp_dir("manual");
    let dir_s = dir.to_string_lossy().to_string();
    assert_eq!(ck_install(cs(&dir_s).as_ptr(), cs("CkTest").as_ptr(), 0), 0);
    assert_eq!(ck_is_installed(), 1);
    assert_eq!(ck_set_context(cs("build").as_ptr(), cs("ci").as_ptr()), 0);
    assert_eq!(
        ck_log(2, cs("test").as_ptr(), cs("hello crashkit").as_ptr()),
        0
    );

    let path = take(ck_write_minidump(0));
    assert_eq!(take(ck_last_dump_path()), path);
    let bytes = std::fs::read(&path).unwrap();
    assert!(
        bytes.len() > 100,
        "dump suspiciously small: {}",
        bytes.len()
    );
    assert_eq!(&bytes[0..4], b"MDMP", "minidump signature");

    let sidecar = std::fs::read_to_string(PathBuf::from(&path).with_extension("json")).unwrap();
    assert!(sidecar.contains("\"exception_code\":null"), "{sidecar}");
    assert!(sidecar.contains("\"build\":\"ci\""), "{sidecar}");

    let log = std::fs::read_to_string(dir.join("CkTest.log")).unwrap();
    assert!(log.contains("hello crashkit"), "{log}");
    assert!(log.contains("\"level\":\"INFO\""), "{log}");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn crash_child_writes_dump() {
    if std::env::var("CK_CHILD").is_ok() {
        // Child mode: install, then crash for real (volatile null read —
        // an access violation the optimizer cannot fold away).
        let dir = std::env::var("CK_DIR").unwrap();
        assert_eq!(ck_install(cs(&dir).as_ptr(), cs("CkChild").as_ptr(), 0), 0);
        assert_eq!(ck_set_context(cs("mode").as_ptr(), cs("child").as_ptr()), 0);
        unsafe {
            let p: *const u8 = std::ptr::null();
            std::hint::black_box(std::ptr::read_volatile(p));
        }
        std::process::exit(99); // unreachable if the crash fires
    }
    // Parent mode: spawn ourselves in child mode and inspect the wreckage.
    let dir = temp_dir("crash");
    let dir_s = dir.to_string_lossy().to_string();
    let exe = std::env::current_exe().unwrap();
    let status = std::process::Command::new(exe)
        .args(["crash_child_writes_dump", "--exact", "--nocapture"])
        .env("CK_CHILD", "1")
        .env("CK_DIR", &dir_s)
        .status()
        .unwrap();
    assert!(!status.success(), "child should not exit cleanly");

    let mut dumps = Vec::new();
    for e in std::fs::read_dir(&dir).unwrap() {
        let p = e.unwrap().path();
        if p.extension().map(|x| x == "dmp").unwrap_or(false) {
            dumps.push(p);
        }
    }
    assert_eq!(dumps.len(), 1, "expected exactly one dump in {dir:?}");
    let bytes = std::fs::read(&dumps[0]).unwrap();
    assert!(
        bytes.len() > 100,
        "dump too small ({} bytes); dir contents: {:?}; crashkit-error.txt: {}",
        bytes.len(),
        std::fs::read_dir(&dir)
            .unwrap()
            .filter_map(|e| e.ok().map(|x| x.file_name()))
            .collect::<Vec<_>>(),
        std::fs::read_to_string(dir.join("crashkit-error.txt"))
            .unwrap_or_else(|_| "<none>".to_string())
    );
    assert_eq!(&bytes[0..4], b"MDMP");
    let sidecar = std::fs::read_to_string(dumps[0].with_extension("json")).unwrap();
    assert!(
        sidecar.contains("\"0xc0000005\""),
        "AV code in sidecar: {sidecar}"
    );
    assert!(sidecar.contains("\"mode\":\"child\""), "{sidecar}");
    let _ = std::fs::remove_dir_all(&dir);
}
