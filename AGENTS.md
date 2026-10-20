# CrashKit — agent notes

- Zero dependencies is a design rule: dbghelp/kernel32 are reached through hand-declared bindings in src/platform.rs (#[link] on every extern block: dbghelp for MiniDumpWriteDump, kernel32 for the rest). Do not add a windows-binding dependency without a written justification.
- The crash path (filter_entry) must never panic and never take a blocking lock — try_lock only, empty context on contention. A crash handler that can deadlock or abort inside a dying process is worse than none.
- CrashKit is local-only by design: no network code, ever. Dumps/sidecars/logs go only to the ck_install directory.
- Off-Windows builds keep the FFI contract testable (platform stubs return CK_ERR_NOT_SUPPORTED); the real crash proof is tests/roundtrip.rs (crashing child process), which runs natively on Windows CI.
- Gate: WinDevKit tools/gate.sh (fmt + clippy both targets + full tests). Check/clippy do not link — CI's real build is the link gate.
