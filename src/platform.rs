//! Platform layer: handler state, log writer, and — on Windows — the
//! unhandled-exception filter + MiniDumpWriteDump capture.
//!
//! Crash-path rule: code running inside the exception filter never
//! panics and never blocks on a lock the crashing thread might hold —
//! it uses try_lock and proceeds with an empty context on contention.

#[cfg(not(windows))]
use crate::ERR_NOT_SUPPORTED;
#[cfg(windows)]
use crate::{dump_filename, dump_type_flags, sidecar_json};
use crate::{CResult, Timestamp, ERR_NOT_INSTALLED, ERR_PLATFORM, LOG_ROLL_BYTES};
use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::{Mutex, OnceLock};

pub struct State {
    pub dir: PathBuf,
    pub app: String,
    pub preset: i32,
    pub context: BTreeMap<String, String>,
    pub last_dump: Option<String>,
    /// Previous top-level filter (Windows), as a raw pointer value.
    pub prev_filter: usize,
}

static STATE: OnceLock<Mutex<State>> = OnceLock::new();

fn with_state<T>(f: impl FnOnce(&mut State) -> CResult<T>) -> CResult<T> {
    let m = STATE.get().ok_or((
        ERR_NOT_INSTALLED,
        "CrashKit handler is not installed (call ck_install first)".to_string(),
    ))?;
    let mut g = m
        .lock()
        .map_err(|_| (ERR_PLATFORM, "state lock poisoned".to_string()))?;
    f(&mut g)
}

pub fn is_installed() -> bool {
    STATE.get().is_some()
}

pub fn set_context(key: &str, value: &str) -> CResult<()> {
    with_state(|s| {
        s.context.insert(key.to_string(), value.to_string());
        Ok(())
    })
}

pub fn remove_context(key: &str) -> CResult<()> {
    with_state(|s| {
        s.context.remove(key);
        Ok(())
    })
}

pub fn last_dump_path() -> CResult<Option<String>> {
    with_state(|s| Ok(s.last_dump.clone()))
}

fn now_unix_ms() -> i64 {
    #[cfg(windows)]
    {
        win::now_unix_ms()
    }
    #[cfg(not(windows))]
    {
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_millis() as i64)
            .unwrap_or(0)
    }
}

fn process_id() -> u32 {
    std::process::id()
}

fn thread_id() -> u32 {
    #[cfg(windows)]
    {
        win::thread_id()
    }
    #[cfg(not(windows))]
    {
        0
    }
}

/// Append one line to <dir>/<app>.log, rolling at LOG_ROLL_BYTES.
pub fn log_line_write(line: &str) -> CResult<()> {
    with_state(|s| {
        let path = s.dir.join(format!("{}.log", s.app));
        if let Ok(meta) = std::fs::metadata(&path) {
            if meta.len() > LOG_ROLL_BYTES {
                let rolled = s.dir.join(format!("{}.log.1", s.app));
                let _ = std::fs::rename(&path, rolled);
            }
        }
        use std::io::Write;
        let mut f = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&path)
            .map_err(|e| (ERR_PLATFORM, format!("cannot open log file: {e}")))?;
        writeln!(f, "{line}").map_err(|e| (ERR_PLATFORM, format!("cannot write log file: {e}")))?;
        Ok(())
    })
}

pub fn log(level: i32, tag: &str, msg: &str) -> CResult<()> {
    let name = crate::level_name(level).ok_or((
        crate::ERR_INVALID_ARG,
        format!("level must be 0..=4, got {level}"),
    ))?;
    let line = crate::log_line(
        Timestamp {
            unix_ms: now_unix_ms(),
        },
        name,
        tag,
        msg,
        process_id(),
        thread_id(),
    );
    log_line_write(&line)
}

#[cfg(not(windows))]
pub fn install(_dir: &str, _app: &str, _preset: i32) -> CResult<()> {
    Err((
        ERR_NOT_SUPPORTED,
        "crash handling is only available on Windows".to_string(),
    ))
}

#[cfg(not(windows))]
pub fn write_minidump(_preset: i32) -> CResult<String> {
    Err((
        ERR_NOT_SUPPORTED,
        "minidumps are only available on Windows".to_string(),
    ))
}

#[cfg(windows)]
pub fn install(dir: &str, app: &str, preset: i32) -> CResult<()> {
    crate::dump_type_flags(preset)?; // validate early
    let dir_pb = PathBuf::from(dir);
    std::fs::create_dir_all(&dir_pb)
        .map_err(|e| (ERR_PLATFORM, format!("cannot create dump directory: {e}")))?;
    // A second install replaces the configuration (documented) and
    // keeps the originally captured previous filter.
    if STATE.get().is_some() {
        return with_state(|s| {
            s.dir = dir_pb;
            s.app = app.to_string();
            s.preset = preset;
            Ok(())
        });
    }
    let prev = win::install_filter();
    let state = State {
        dir: dir_pb,
        app: app.to_string(),
        preset,
        context: BTreeMap::new(),
        last_dump: None,
        prev_filter: prev,
    };
    STATE.set(Mutex::new(state)).map_err(|_| {
        (
            ERR_PLATFORM,
            "handler install raced another install".to_string(),
        )
    })
}

#[cfg(windows)]
pub fn write_minidump(preset: i32) -> CResult<String> {
    let flags = dump_type_flags(preset)?;
    with_state(|s| {
        let ts = Timestamp {
            unix_ms: now_unix_ms(),
        };
        let name = dump_filename(&s.app, ts, process_id());
        let path = s.dir.join(&name);
        win::write_dump(&path, flags, None, process_id(), thread_id())?;
        let sidecar = sidecar_json(
            &s.app,
            ts,
            process_id(),
            thread_id(),
            None,
            None,
            preset,
            &s.context,
        );
        let sc_path = path.with_extension("json");
        std::fs::write(&sc_path, sidecar)
            .map_err(|e| (ERR_PLATFORM, format!("cannot write sidecar: {e}")))?;
        let p = path.to_string_lossy().to_string();
        s.last_dump = Some(p.clone());
        Ok(p)
    })
}

#[cfg(windows)]
#[allow(non_snake_case, clippy::upper_case_acronyms)]
mod win {
    use super::*;
    use std::ffi::c_void;
    use std::os::windows::io::AsRawHandle;

    pub type HANDLE = *mut c_void;

    #[repr(C)]
    pub struct EXCEPTION_RECORD {
        pub ExceptionCode: u32,
        pub ExceptionFlags: u32,
        pub ExceptionRecord: *mut EXCEPTION_RECORD,
        pub ExceptionAddress: *mut c_void,
        pub NumberParameters: u32,
        pub ExceptionInformation: [usize; 15],
    }

    #[repr(C)]
    pub struct EXCEPTION_POINTERS {
        pub ExceptionRecord: *mut EXCEPTION_RECORD,
        pub ContextRecord: *mut c_void,
    }

    #[repr(C)]
    pub struct MINIDUMP_EXCEPTION_INFORMATION {
        pub ThreadId: u32,
        pub ExceptionPointers: *mut EXCEPTION_POINTERS,
        pub ClientPointers: i32,
    }

    pub type UnhandledFilter = unsafe extern "system" fn(*mut EXCEPTION_POINTERS) -> i32;

    #[link(name = "kernel32")]
    extern "system" {
        pub fn SetUnhandledExceptionFilter(filter: UnhandledFilter) -> Option<UnhandledFilter>;
        pub fn GetCurrentProcess() -> HANDLE;
        pub fn GetCurrentThreadId() -> u32;
        pub fn GetSystemTimeAsFileTime(ft: *mut u64);
        pub fn GetLastError() -> u32;
    }

    #[link(name = "dbghelp")]
    extern "system" {
        pub fn MiniDumpWriteDump(
            process: HANDLE,
            pid: u32,
            file: HANDLE,
            dump_type: u32,
            exception_param: *const MINIDUMP_EXCEPTION_INFORMATION,
            user_stream_param: *const c_void,
            callback_param: *const c_void,
        ) -> i32;
    }

    pub fn now_unix_ms() -> i64 {
        let mut ft: u64 = 0;
        unsafe { GetSystemTimeAsFileTime(&mut ft) };
        const EPOCH_DIFF_100NS: u64 = 116_444_736_000_000_000;
        if ft < EPOCH_DIFF_100NS {
            return 0;
        }
        ((ft - EPOCH_DIFF_100NS) / 10_000) as i64
    }

    pub fn thread_id() -> u32 {
        unsafe { GetCurrentThreadId() }
    }

    pub fn install_filter() -> usize {
        let prev = unsafe { SetUnhandledExceptionFilter(filter_entry) };
        prev.map(|f| f as usize).unwrap_or(0)
    }

    pub fn write_dump(
        path: &std::path::Path,
        flags: u32,
        exception: Option<&MINIDUMP_EXCEPTION_INFORMATION>,
        pid: u32,
        _tid: u32,
    ) -> CResult<()> {
        let file = std::fs::File::create(path)
            .map_err(|e| (ERR_PLATFORM, format!("cannot create dump file: {e}")))?;
        let ok = unsafe {
            MiniDumpWriteDump(
                GetCurrentProcess(),
                pid,
                file.as_raw_handle() as HANDLE,
                flags,
                exception
                    .map(|e| e as *const MINIDUMP_EXCEPTION_INFORMATION)
                    .unwrap_or(std::ptr::null()),
                std::ptr::null(),
                std::ptr::null(),
            )
        };
        if ok == 0 {
            let gle = unsafe { GetLastError() };
            return Err((
                ERR_PLATFORM,
                format!("MiniDumpWriteDump failed (Win32 error {gle})"),
            ));
        }
        Ok(())
    }

    unsafe extern "system" fn filter_entry(info: *mut EXCEPTION_POINTERS) -> i32 {
        // The crash path: no panics, no lock held across the capture,
        // best effort only. The dump is written by a dedicated capture
        // thread: MiniDumpWriteDump called on the faulting thread from
        // inside its own filter proved unreliable (CI, 2026-10-20: the
        // call failed and left a 0-byte file), while a helper thread in
        // the still-alive process — the pattern production crash
        // handlers use — captures consistently. The exception records
        // stay valid because the filter waits for the capture to
        // finish before returning.
        const EXCEPTION_CONTINUE_SEARCH: i32 = 0;
        let mut prev: usize = 0;
        if let Some(m) = STATE.get() {
            // Snapshot under try_lock; never hold the lock during capture.
            let snap = m.try_lock().ok().map(|g| {
                prev = g.prev_filter;
                (g.dir.clone(), g.app.clone(), g.preset, g.context.clone())
            });
            if let Some((dir, app, preset, context)) = snap {
                let (code, addr) = if !info.is_null() && !(*info).ExceptionRecord.is_null() {
                    (
                        Some((*(*info).ExceptionRecord).ExceptionCode),
                        Some((*(*info).ExceptionRecord).ExceptionAddress as usize),
                    )
                } else {
                    (None, None)
                };
                let ts = Timestamp {
                    unix_ms: now_unix_ms(),
                };
                let pid = std::process::id();
                let tid = thread_id();
                let name = dump_filename(&app, ts, pid);
                let path = dir.join(&name);
                if let Ok(flags) = dump_type_flags(preset) {
                    let info_usize = info as usize;
                    let worker_path = path.clone();
                    let worker = std::thread::spawn(move || {
                        let mei = MINIDUMP_EXCEPTION_INFORMATION {
                            ThreadId: tid,
                            ExceptionPointers: info_usize as *mut EXCEPTION_POINTERS,
                            ClientPointers: 0,
                        };
                        write_dump(&worker_path, flags, Some(&mei), pid, tid)
                    });
                    match worker.join() {
                        Ok(Ok(())) => {
                            let sidecar =
                                sidecar_json(&app, ts, pid, tid, code, addr, preset, &context);
                            let _ = std::fs::write(path.with_extension("json"), sidecar);
                            if let Some(m2) = STATE.get() {
                                if let Ok(mut g) = m2.try_lock() {
                                    g.last_dump = Some(path.to_string_lossy().to_string());
                                }
                            }
                        }
                        Ok(Err((_, msg))) => {
                            let _ = std::fs::write(
                                dir.join("crashkit-error.txt"),
                                format!("crash capture failed: {msg}\n"),
                            );
                        }
                        Err(_) => {
                            let _ = std::fs::write(
                                dir.join("crashkit-error.txt"),
                                "crash capture thread panicked\n",
                            );
                        }
                    }
                }
            }
        }
        if prev != 0 {
            let f: UnhandledFilter = unsafe { std::mem::transmute(prev) };
            return unsafe { f(info) };
        }
        EXCEPTION_CONTINUE_SEARCH
    }
}
