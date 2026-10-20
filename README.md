# CrashKit

**When your app crashes at 2 a.m. on a user's machine, what do you get?** With CrashKit: a minidump and a JSON sidecar that tells you the exception, the build, and the context — written locally, in a folder you choose, never uploaded anywhere.

```c
ck_install(dir, "MyApp", 1);                 // once, at startup
ck_set_context("build", "1.4.2");            // rides along in sidecars
ck_log(2, "net", "connected");               // structured JSON-lines log
char *p = ck_write_minidump(1);              // or capture on demand
```

## What you get on a crash

- `MyApp-<yyyymmdd-hhmmss>-<pid>.dmp` — a real Windows minidump (preset 0 mini / 1 standard / 2 full), openable in WinDbg or Visual Studio.
- `MyApp-<...>.json` — the sidecar: exception code and address, PID/TID, timestamp, CrashKit version, and every `ck_set_context` entry.
- The handler then defers — to your previously installed filter if you had one, otherwise to the OS default handling. CrashKit never swallows a crash silently and never phones home.

The same directory holds `MyApp.log`, a JSON Lines structured log (`ts_unix_ms`, level, tag, message, pid, tid) that rolls to `.log.1` at 5 MiB.

## Honest limits

- The crash path is best-effort by nature: if the process heap is corrupt, minidump writing can fail — CrashKit catches what it can and never makes the crash worse (no panics, no blocking locks in the filter; it proceeds without context if the state lock is contended).
- Dumps can contain user data from process memory. They are written only to the directory *you* pass to `ck_install`, with default file permissions — choose the location and retention policy accordingly.
- Windows only: off Windows the library builds and its FFI contract tests run, with install/dump calls returning `CK_ERR_NOT_SUPPORTED`.

## Verification status

Local gate (fmt, clippy on host + windows-msvc targets, full test suite) green. Windows CI runs the roundtrip suite **natively**, including a real access-violation child process whose dump and sidecar (exception `0xc0000005`) are verified by the parent test. Manual WinDbg inspection of a produced dump remains a worthwhile on-machine check — see ROADMAP.md.

Building: `cargo build --release` → `crashkit.dll`; `cargo test --locked` runs everything.

License: MIT. No third-party code. See `AGENTS.md` for build discipline.
