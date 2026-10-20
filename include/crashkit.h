/* CrashKit.dll — crash capture + structured diagnostics for Windows apps.
 *
 * Install once at startup:
 *     ck_install("C:\\Users\\me\\AppData\\Local\\MyApp\\crashes", "MyApp", 1);
 * On an unhandled exception CrashKit writes a minidump
 * (MyApp-<yyyymmdd-hhmmss>-<pid>.dmp) plus a JSON sidecar with the
 * exception code/address and your context entries, then defers to the
 * previously installed filter, or the OS default when there was none.
 * Nothing is uploaded anywhere, ever.
 *
 * Strings are UTF-8. Returned char* are caller-owned: free with
 * ck_free_string(). Errors: thread-local ck_last_error() /
 * ck_last_error_code(). */
#ifndef CRASHKIT_H
#define CRASHKIT_H

#include <stdint.h>

#ifdef __cplusplus
extern "C" {
#endif

#if defined(_WIN32) && defined(CK_BUILD)
#define CK_API __declspec(dllexport)
#elif defined(_WIN32)
#define CK_API __declspec(dllimport)
#else
#define CK_API
#endif

enum {
    CK_OK = 0,
    CK_ERR_INVALID_ARG = 1,
    CK_ERR_NOT_SUPPORTED = 2,
    CK_ERR_NOT_INSTALLED = 3,
    CK_ERR_PLATFORM = 5,
    CK_ERR_INTERNAL = 6
};

/* Dump presets: 0 mini, 1 standard, 2 full. */
/* Log levels: 0 TRACE, 1 DEBUG, 2 INFO, 3 WARN, 4 ERROR. */

/* "0.1.0" — static string, do not free. */
CK_API const char *ck_version(void);

/* Install (or reconfigure) the handler. dir is created if missing.
 * app: 1-64 chars [A-Za-z0-9._-], used as the filename prefix. */
CK_API int ck_install(const char *dir, const char *app, int preset);
CK_API int ck_is_installed(void);

/* Write a minidump of this process now; returns its path or NULL. */
CK_API char *ck_write_minidump(int preset);

/* Append one JSON line to <dir>/<app>.log (rolls to .log.1 at 5 MiB). */
CK_API int ck_log(int level, const char *tag, const char *msg);

/* Context entries ride along in every crash sidecar. */
CK_API int ck_set_context(const char *key, const char *value);
CK_API int ck_remove_context(const char *key);

/* Most recent dump path (crash or manual), or NULL if none yet. */
CK_API char *ck_last_dump_path(void);

CK_API void ck_free_string(char *ptr);
CK_API const char *ck_last_error(void);
CK_API int ck_last_error_code(void);

#ifdef __cplusplus
}
#endif
#endif /* CRASHKIT_H */
