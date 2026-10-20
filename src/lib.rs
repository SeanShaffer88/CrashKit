//! CrashKit — pure logic core: civil time, dump planning, JSON lines.
//! Windows crash machinery lives in [`platform`]; the C ABI in [`ffi`].
//!
//! Design: install one handler with (output directory, app name). On an
//! unhandled exception the handler writes a minidump plus a JSON sidecar
//! (exception code/address, context entries, timestamps) next to it.
//! A structured JSON-lines log shares the directory. Nothing is ever
//! uploaded anywhere — the app (and its user) decide what to do with
//! the files.

pub mod ffi;
pub mod platform;

pub const VERSION: &str = "0.1.0";
pub const VERSION_MAJOR: u32 = 0;
pub const VERSION_MINOR: u32 = 1;
pub const VERSION_PATCH: u32 = 0;

pub const CK_OK: i32 = 0;
pub const ERR_INVALID_ARG: i32 = 1;
pub const ERR_NOT_SUPPORTED: i32 = 2;
pub const ERR_NOT_INSTALLED: i32 = 3;
pub const ERR_PLATFORM: i32 = 5;
pub const ERR_INTERNAL: i32 = 6;

/// Log rotation threshold: at this size the log rolls to `<name>.1`.
pub const LOG_ROLL_BYTES: u64 = 5 * 1024 * 1024;

pub type CResult<T> = Result<T, (i32, String)>;

pub const LEVELS: [&str; 5] = ["TRACE", "DEBUG", "INFO", "WARN", "ERROR"];

pub fn level_name(level: i32) -> Option<&'static str> {
    if level < 0 {
        return None;
    }
    LEVELS.get(level as usize).copied()
}

pub fn json_escape(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 8);
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if (c as u32) < 0x20 => out.push_str(&format!("\\u{:04x}", c as u32)),
            c => out.push(c),
        }
    }
    out
}

/// Days since 1970-01-01 → (year, month, day). Howard Hinnant's
/// civil-from-days algorithm; exact across the whole realistic range.
pub fn civil_from_days(z: i64) -> (i64, u32, u32) {
    let z = z + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = (z - era * 146_097) as u64; // [0, 146096]
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146_096) / 365;
    let y = yoe as i64 + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = (if mp < 10 { mp + 3 } else { mp - 9 }) as u32;
    (if m <= 2 { y + 1 } else { y }, m, d)
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Timestamp {
    pub unix_ms: i64,
}

impl Timestamp {
    /// "20261020-151530" (UTC) for filenames.
    pub fn filename_part(&self) -> String {
        let secs = self.unix_ms.div_euclid(1000);
        let days = secs.div_euclid(86_400);
        let tod = secs.rem_euclid(86_400);
        let (y, m, d) = civil_from_days(days);
        format!(
            "{:04}{:02}{:02}-{:02}{:02}{:02}",
            y,
            m,
            d,
            tod / 3600,
            (tod % 3600) / 60,
            tod % 60
        )
    }
}

/// Minidump filename: <app>-<yyyymmdd-hhmmss>-<pid>.dmp
pub fn dump_filename(app: &str, ts: Timestamp, pid: u32) -> String {
    format!("{}-{}-{}.dmp", app, ts.filename_part(), pid)
}

/// Dump type presets → MINIDUMP_TYPE flag words (dbghelp values).
/// 0 = mini (Normal | ThreadInfo)
/// 1 = standard (WithDataSegs | WithProcessThreadData | ThreadInfo)
/// 2 = full (WithFullMemory | WithDataSegs | WithHandleData | ThreadInfo | ProcessThreadData)
pub fn dump_type_flags(preset: i32) -> CResult<u32> {
    match preset {
        0 => Ok(0x0000_1000),
        1 => Ok(0x0000_0001 | 0x0000_0004 | 0x0000_1000),
        2 => Ok(0x0000_0002 | 0x0000_0001 | 0x0000_0004 | 0x0000_1000 | 0x0000_2000),
        _ => Err((
            ERR_INVALID_ARG,
            format!("dump type preset must be 0, 1 or 2, got {preset}"),
        )),
    }
}

fn validate_name(what: &str, value: &str) -> CResult<()> {
    if value.is_empty() || value.chars().count() > 64 {
        return Err((ERR_INVALID_ARG, format!("{what} must be 1-64 characters")));
    }
    for c in value.chars() {
        if c.is_control() || !(c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '_')) {
            return Err((
                ERR_INVALID_ARG,
                format!("{what} contains a disallowed character: {c:?}"),
            ));
        }
    }
    Ok(())
}

/// App names become filename prefixes: conservative charset.
pub fn validate_app_name(app: &str) -> CResult<()> {
    validate_name("app name", app)
}

/// Context keys: same conservative shape as app names.
pub fn validate_context_key(key: &str) -> CResult<()> {
    validate_name("context key", key)
}

/// One structured log line (JSON Lines format).
pub fn log_line(ts: Timestamp, level: &str, tag: &str, msg: &str, pid: u32, tid: u32) -> String {
    format!(
        "{{\"ts_unix_ms\":{},\"level\":\"{}\",\"tag\":\"{}\",\"msg\":\"{}\",\"pid\":{},\"tid\":{}}}",
        ts.unix_ms,
        level,
        json_escape(tag),
        json_escape(msg),
        pid,
        tid
    )
}

/// The crash sidecar JSON that accompanies every dump.
/// (A flat argument list keeps this pure builder trivially testable;
/// it is an internal record constructor, not a public API surface.)
#[allow(clippy::too_many_arguments)]
pub fn sidecar_json(
    app: &str,
    ts: Timestamp,
    pid: u32,
    tid: u32,
    exception_code: Option<u32>,
    exception_address: Option<usize>,
    dump_preset: i32,
    context: &std::collections::BTreeMap<String, String>,
) -> String {
    let code = match exception_code {
        Some(c) => format!("\"0x{c:08x}\""),
        None => "null".to_string(),
    };
    let addr = match exception_address {
        Some(a) => format!("\"0x{a:x}\""),
        None => "null".to_string(),
    };
    let mut ctx = String::from("{");
    for (i, (k, v)) in context.iter().enumerate() {
        if i > 0 {
            ctx.push(',');
        }
        ctx.push_str(&format!("\"{}\":\"{}\"", json_escape(k), json_escape(v)));
    }
    ctx.push('}');
    format!(
        "{{\"app\":\"{}\",\"crashkit\":\"{}\",\"ts_unix_ms\":{},\"pid\":{},\"tid\":{},\"exception_code\":{},\"exception_address\":{},\"dump_preset\":{},\"context\":{}}}",
        json_escape(app),
        VERSION,
        ts.unix_ms,
        pid,
        tid,
        code,
        addr,
        dump_preset,
        ctx
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn civil_dates() {
        assert_eq!(civil_from_days(0), (1970, 1, 1));
        assert_eq!(civil_from_days(20_000), (2024, 10, 4));
        assert_eq!(civil_from_days(20_746), (2026, 10, 20));
        assert_eq!(civil_from_days(-1), (1969, 12, 31));
        // Leap day:
        assert_eq!(civil_from_days(19_782), (2024, 2, 29));
    }

    #[test]
    fn timestamp_and_filenames() {
        // 2026-10-20T15:15:30Z = 1792509330 s
        let ts = Timestamp {
            unix_ms: 1_792_509_330_000,
        };
        assert_eq!(ts.filename_part(), "20261020-151530");
        assert_eq!(
            dump_filename("MyApp", ts, 4242),
            "MyApp-20261020-151530-4242.dmp"
        );
    }

    #[test]
    fn dump_presets() {
        assert!(dump_type_flags(0).is_ok());
        assert!(dump_type_flags(1).is_ok());
        assert!(dump_type_flags(2).is_ok());
        assert!(dump_type_flags(3).is_err());
        assert_eq!(level_name(2), Some("INFO"));
        assert_eq!(level_name(5), None);
        assert_eq!(level_name(-1), None);
    }

    #[test]
    fn log_and_sidecar_json() {
        let line = log_line(
            Timestamp { unix_ms: 5 },
            "INFO",
            "net",
            "say \"hi\"\n",
            1,
            2,
        );
        assert!(line.contains("\\\"hi\\\"\\n"), "{line}");
        let mut ctx = std::collections::BTreeMap::new();
        ctx.insert("build".to_string(), "debug".to_string());
        let sc = sidecar_json(
            "MyApp",
            Timestamp { unix_ms: 5 },
            1,
            2,
            Some(0xc000_0005),
            Some(0x7ff6_1234),
            1,
            &ctx,
        );
        assert!(sc.contains("\"0xc0000005\""), "{sc}");
        assert!(sc.contains("\"build\":\"debug\""), "{sc}");
        let sc2 = sidecar_json("A", Timestamp { unix_ms: 0 }, 1, 1, None, None, 0, &ctx);
        assert!(sc2.contains("\"exception_code\":null"), "{sc2}");
    }

    #[test]
    fn validation() {
        assert!(validate_app_name("My-App_2.0").is_ok());
        assert!(validate_app_name("has space").is_err());
        assert!(validate_app_name("").is_err());
        assert!(validate_context_key("gpu.model").is_ok());
        assert!(validate_context_key("a/b").is_err());
    }
}
