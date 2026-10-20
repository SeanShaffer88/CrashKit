# CrashKit Roadmap

## v0.1 (current)
Unhandled-exception minidump capture (3 presets) + JSON sidecar, structured JSON-lines log with single-roll rotation, context entries, on-demand dumps, chained-filter etiquette. Full crash path proven natively on Windows CI via a crashing child process.

## Candidates
- **User stream in the dump itself**: embed the sidecar JSON as a minidump user stream so it travels with the .dmp when separated from its file pair.
- **WER-friendly mode**: optionally register as a WerRegisterRuntimeExceptionModule for managed/mixed processes.
- **Symbol-server helper**: a tiny CLI that pairs dumps with PDBs and prints the faulting stack via dbghelp, for developers without WinDbg handy.
- **Retention helper**: `ck_prune(dir, keep_n, max_age_days)` so apps can bound the crash folder without writing their own sweeper.
- **Previous-dump detection**: `ck_previous_crash_json()` at startup — "we crashed last time, here is the sidecar" for support flows.

## Explicitly not planned
- Any upload/telemetry component. CrashKit is local-only by design; what an app does with its dump folder is the app's (and its user's) decision.
