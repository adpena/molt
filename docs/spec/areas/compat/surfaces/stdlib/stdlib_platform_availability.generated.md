# Stdlib Platform Availability (CPython 3.12-3.14)

**Status:** Generated
**Source:** `docs/python_documentation/python-<version>-docs-text/library/*.txt`
**Generated on (UTC):** 2026-02-23 20:37:26Z

## Summary
- Modules with explicit Availability metadata: `66`
- WASI blocked (any lane): `41`
- Emscripten blocked (any lane): `37`

## Matrix

| Module | py312 | py313 | py314 | wasm_wasi | wasm_emscripten | os_hint |
| --- | --- | --- | --- | --- | --- | --- |
| `_thread` | Windows, FreeBSD, Linux, macOS, OpenBSD, NetBSD, AIX, | Windows, FreeBSD, Linux, macOS, OpenBSD, NetBSD, AIX, | Windows, FreeBSD, Linux, macOS, OpenBSD, NetBSD, AIX, | allowed_or_unspecified | allowed_or_unspecified | windows-biased |
| `asyncio` | not Emscripten, not WASI. | not WASI. | not WASI. | blocked | blocked | unspecified |
| `asyncio-eventloop` | Unix. | Unix. | Unix. | allowed_or_unspecified | allowed_or_unspecified | unix-biased |
| `asyncio-policy` | Windows. | Windows. | Windows. | allowed_or_unspecified | allowed_or_unspecified | windows-biased |
| `asyncio-stream` | Unix. | Unix. | Unix. | allowed_or_unspecified | allowed_or_unspecified | unix-biased |
| `cgi` | not Emscripten, not WASI. | - | - | blocked | blocked | unspecified |
| `codecs` | Windows. | Windows. | Windows. | allowed_or_unspecified | allowed_or_unspecified | windows-biased |
| `compileall` | not Emscripten, not WASI. | not WASI. | not WASI. | blocked | blocked | unspecified |
| `concurrent.futures` | not Emscripten, not WASI. | not WASI. | not WASI. | blocked | blocked | unspecified |
| `concurrent.interpreters` | - | - | not WASI. | blocked | allowed_or_unspecified | unspecified |
| `crypt` | Unix, not VxWorks. | - | - | allowed_or_unspecified | allowed_or_unspecified | unix-biased |
| `ctypes` | Windows | Windows | Windows | allowed_or_unspecified | allowed_or_unspecified | windows-biased |
| `curses` | if the ncurses library is used. | not Android, not iOS, not WASI. | not Android, not iOS, not WASI. | blocked | allowed_or_unspecified | unspecified |
| `dbm` | - | not WASI. | not WASI. | blocked | allowed_or_unspecified | unspecified |
| `ensurepip` | not Emscripten, not WASI. | not Android, not iOS, not WASI. | not Android, not iOS, not WASI. | blocked | blocked | unspecified |
| `errno` | WASI, FreeBSD | WASI, FreeBSD | WASI, FreeBSD | allowed_or_unspecified | allowed_or_unspecified | unspecified |
| `fcntl` | Unix, not Emscripten, not WASI. | Unix, not WASI. | Unix, not WASI. | blocked | blocked | unix-biased |
| `ftplib` | not Emscripten, not WASI. | not WASI. | not WASI. | blocked | blocked | unspecified |
| `getpass` | not Emscripten, not WASI. | not WASI. | not WASI. | blocked | blocked | unspecified |
| `grp` | Unix, not Emscripten, not WASI. | Unix, not WASI, not Android, not iOS. | Unix, not WASI, not Android, not iOS. | blocked | blocked | unix-biased |
| `http.client` | not Emscripten, not WASI. | not WASI. | not WASI. | blocked | blocked | unspecified |
| `http.server` | not Emscripten, not WASI. | not WASI. | not WASI. | blocked | blocked | unspecified |
| `imaplib` | not Emscripten, not WASI. | not WASI. | not WASI. | blocked | blocked | unspecified |
| `importlib` | - | iOS. | iOS. | allowed_or_unspecified | allowed_or_unspecified | unspecified |
| `mimetypes` | Windows. | Windows. | Windows. | allowed_or_unspecified | allowed_or_unspecified | windows-biased |
| `mmap` | not Emscripten, not WASI. | not WASI. | not WASI. | blocked | blocked | unspecified |
| `msvcrt` | - | Windows. | Windows. | allowed_or_unspecified | allowed_or_unspecified | windows-biased |
| `multiprocessing` | not Emscripten, not WASI. | not Android, not iOS, not WASI. | not Android, not iOS, not WASI. | blocked | blocked | unspecified |
| `nis` | not Emscripten, not WASI. | - | - | blocked | blocked | unspecified |
| `nntplib` | not Emscripten, not WASI. | - | - | blocked | blocked | unspecified |
| `os` | Unix, not Emscripten, not WASI. | Unix, not WASI. | Unix, not WASI. | blocked | blocked | unix-biased |
| `os.path` | Windows. | Windows. | Windows. | allowed_or_unspecified | allowed_or_unspecified | windows-biased |
| `pipes` | Unix, not VxWorks. | - | - | allowed_or_unspecified | allowed_or_unspecified | unix-biased |
| `poplib` | not Emscripten, not WASI. | not WASI. | not WASI. | blocked | blocked | unspecified |
| `posix` | Unix. | Unix. | Unix. | allowed_or_unspecified | allowed_or_unspecified | unix-biased |
| `pty` | Unix. | Unix. | Unix. | allowed_or_unspecified | allowed_or_unspecified | unix-biased |
| `pwd` | Unix, not Emscripten, not WASI. | Unix, not WASI, not iOS. | Unix, not WASI, not iOS. | blocked | blocked | unix-biased |
| `readline` | - | not Android, not iOS, not WASI. | not Android, not iOS, not WASI. | blocked | allowed_or_unspecified | unspecified |
| `resource` | Unix, not Emscripten, not WASI. | Unix, not WASI. | Unix, not WASI. | blocked | blocked | unix-biased |
| `select` | not Emscripten, not WASI. | not WASI. | not WASI. | blocked | blocked | unspecified |
| `selectors` | not Emscripten, not WASI. | not WASI. | not WASI. | blocked | blocked | unspecified |
| `shutil` | Unix, Windows. | Unix, Windows. | Unix, Windows. | allowed_or_unspecified | allowed_or_unspecified | multi-os |
| `signal` | Unix. | Unix. | Unix. | allowed_or_unspecified | allowed_or_unspecified | unix-biased |
| `smtplib` | not Emscripten, not WASI. | not WASI. | not WASI. | blocked | blocked | unspecified |
| `socket` | not Emscripten, not WASI. | not WASI. | not WASI. | blocked | blocked | unspecified |
| `socketserver` | not Emscripten, not WASI. | not WASI. | not WASI. | blocked | blocked | unspecified |
| `spwd` | not Emscripten, not WASI. | - | - | blocked | blocked | unspecified |
| `ssl` | not Emscripten, not WASI. | not WASI. | not WASI. | blocked | blocked | unspecified |
| `stat` | - | macOS | macOS | allowed_or_unspecified | allowed_or_unspecified | unspecified |
| `subprocess` | not Emscripten, not WASI. | not Android, not iOS, not WASI. | not Android, not iOS, not WASI. | blocked | blocked | unspecified |
| `sys` | Unix. | Unix. | Unix. | allowed_or_unspecified | allowed_or_unspecified | unix-biased |
| `syslog` | Unix, not Emscripten, not WASI. | Unix, not WASI, not iOS. | Unix, not WASI, not iOS. | blocked | blocked | unix-biased |
| `telnetlib` | not Emscripten, not WASI. | - | - | blocked | blocked | unspecified |
| `termios` | Unix. | Unix. | Unix. | allowed_or_unspecified | allowed_or_unspecified | unix-biased |
| `threading` | not Emscripten, not WASI. | not WASI. | not WASI. | blocked | blocked | unspecified |
| `time` | Unix | Unix | Unix | allowed_or_unspecified | allowed_or_unspecified | unix-biased |
| `tkinter.ttk` | Tk 8.6. | Tk 8.6. | Tk 8.6. | allowed_or_unspecified | allowed_or_unspecified | unspecified |
| `tty` | Unix. | Unix. | Unix. | allowed_or_unspecified | allowed_or_unspecified | unix-biased |
| `urllib.request` | not Emscripten, not WASI. | not WASI. | not WASI. | blocked | blocked | unspecified |
| `venv` | not Emscripten, not WASI. | not Android, not iOS, not WASI. | not Android, not iOS, not WASI. | blocked | blocked | unspecified |
| `webbrowser` | not Emscripten, not WASI. | not WASI, not Android. | not WASI, not Android. | blocked | blocked | unspecified |
| `winreg` | - | Windows. | Windows. | allowed_or_unspecified | allowed_or_unspecified | windows-biased |
| `winsound` | - | Windows. | Windows. | allowed_or_unspecified | allowed_or_unspecified | windows-biased |
| `xmlrpc.client` | not Emscripten, not WASI. | not WASI. | not WASI. | blocked | blocked | unspecified |
| `xmlrpc.server` | not Emscripten, not WASI. | not WASI. | not WASI. | blocked | blocked | unspecified |
| `zoneinfo` | not Emscripten, not WASI. | not WASI. | not WASI. | blocked | blocked | unspecified |

## Notes
- `allowed_or_unspecified` means CPython docs did not explicitly ban that platform in the Availability line.
- This matrix is a reference input for Molt capability and cross-platform planning, not an automatic parity claim.
