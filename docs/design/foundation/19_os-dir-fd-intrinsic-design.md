<!-- Design recon (background architect agent, 2026-06-04). All anchors verified against live code. -->

# os.* dir_fd Intrinsic-Variant Design

**Status:** Design doc — no implementation landed. All anchors verified on 2026-06-04.

---

## Background and Existing State

`src/molt/stdlib/os.py` implements the Python `os` module. Twelve functions currently raise `NotImplementedError` when the `dir_fd` or `src_dir_fd`/`dst_dir_fd` keyword arguments are not `None`:

- `readlink` at os.py:1084: `dir_fd`
- `symlink` at os.py:1097: `dir_fd`
- `stat` at os.py:1204: `dir_fd`
- `lstat` at os.py:1215: `dir_fd`
- `rename` at os.py:1235,1237: `src_dir_fd`, `dst_dir_fd`
- `replace` at os.py:1251,1253: `src_dir_fd`, `dst_dir_fd`
- `link` at os.py:1288,1290: `src_dir_fd`, `dst_dir_fd`
- `utime` at os.py:1398: `dir_fd`

The corresponding base (no `dir_fd`) intrinsics exist:

- `molt_os_stat` / `molt_os_lstat` / `molt_os_fstat` — `runtime/molt-runtime/src/builtins/io_path.rs:1734,1748,1762`
- `molt_os_rename` / `molt_os_replace` — `io_path.rs:1798,1816`
- `molt_os_readlink` — `os_ext.rs:673`
- `molt_os_symlink` — `os_ext.rs:619`
- `molt_os_link` — `os_ext.rs:586`
- `molt_os_utime` — `os_ext.rs:2031`

The intrinsics generator is `tools/gen_intrinsics.py`, which reads `runtime/molt-runtime/src/intrinsics/manifest.pyi` (the canonical type-signature source) and `runtime/molt-runtime/src/intrinsics/categories.toml` (the Cargo feature gates), then emits `runtime/molt-runtime/src/intrinsics/generated.rs` and `src/molt/_intrinsics.pyi` (paths at gen_intrinsics.py:17-20; `SYMBOL_OVERRIDES` at 22-23).

The capability system at `runtime/molt-runtime/src/intrinsics/capabilities.rs` maps `"fs.read"` to read-only ops and `"fs.write"` to write ops. All dir_fd variants follow the same capability mapping as their base variants.

---

## POSIX API Family

The POSIX `*at` family (Linux 2.6.16+, macOS 10.10+, all BSDs) implements directory-relative operations via an open directory fd. When `dir_fd` is not `AT_FDCWD`, the path resolves relative to the open directory:

```c
int fstatat(int dirfd, const char *path, struct stat *buf, int flags);
ssize_t readlinkat(int dirfd, const char *path, char *buf, size_t bufsiz);
int symlinkat(const char *target, int newdirfd, const char *linkpath);
int renameat(int olddirfd, const char *oldpath, int newdirfd, const char *newpath);
int linkat(int olddirfd, const char *oldpath, int newdirfd, const char *newpath, int flags);
int utimensat(int dirfd, const char *path, const struct timespec times[2], int flags);
```

NOTE: `symlinkat` takes the new directory fd as its SECOND argument — care required in the Rust wrapper.

CPython exposes dir_fd as `int | None`. `None` triggers the path-only variant (the existing base intrinsic). The fd must be an open directory descriptor; the kernel returns `ENOTDIR`/`EBADF` when wrong and CPython propagates the `OSError`.

---

## Intrinsic Signature Pattern

NaN-boxed `u64` arguments, `u64` return, named `molt_os_{op}_at`:

```
molt_os_stat_at(path: Any, dir_fd: Any, follow_symlinks: bool) -> tuple[...]
molt_os_lstat_at(path: Any, dir_fd: Any) -> tuple[...]
molt_os_readlink_at(path: Any, dir_fd: Any) -> str
molt_os_symlink_at(src: Any, dst: Any, dir_fd: Any) -> None
molt_os_rename_at(src: Any, src_dir_fd: Any, dst: Any, dst_dir_fd: Any) -> None
molt_os_replace_at(src: Any, src_dir_fd: Any, dst: Any, dst_dir_fd: Any) -> None
molt_os_link_at(src: Any, src_dir_fd: Any, dst: Any, dst_dir_fd: Any, follow_symlinks: bool) -> None
molt_os_utime_at(path: Any, dir_fd: Any, ns_atime: int, ns_mtime: int, follow_symlinks: bool) -> None
```

`follow_symlinks` rides along for `stat`/`link`/`utime` (where it composes with `dir_fd`); `lstat` is always nofollow. `utime_at` takes a single nanosecond encoding: the Python wrapper pre-computes ns from either `times=` (float seconds × 1e9) or `ns=`; `-1` is the "current time" sentinel mapping to a null timespec pointer.

**`dir_fd=None` handling:** the Python wrapper passes `dir_fd` verbatim (None or int); the Rust side maps `None` bits → `libc::AT_FDCWD` (which is platform-dependent: −100 Linux, −2 macOS — always use the `libc` constant, never a hardcoded integer):

```rust
let src_dir_fd = if obj_from_bits(src_dir_fd_bits).is_none() {
    libc::AT_FDCWD
} else {
    match to_i64(obj_from_bits(src_dir_fd_bits)) {
        Some(fd) => fd as libc::c_int,
        None => return raise_exception::<u64>(_py, "TypeError", "an integer is required"),
    }
};
```

---

## Rust Implementation Pattern

New intrinsics co-locate with their base variants: `readlink_at`/`symlink_at`/`utime_at`/`lstat_at` in `os_ext.rs` (after `molt_os_utime` at 2031); `stat_at`/`rename_at`/`replace_at`/`link_at` in `io_path.rs` (after `molt_os_replace` at 1816). Canonical shape:

```rust
#[cfg(not(target_arch = "wasm32"))]
#[unsafe(no_mangle)]
pub extern "C" fn molt_os_stat_at(path_bits: u64, dir_fd_bits: u64, follow_symlinks_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let allowed = has_capability(_py, "fs.read");
        audit_capability_decision("os.stat_at", "fs.read", AuditArgs::None, allowed);
        if !allowed {
            return raise_exception::<_>(_py, "PermissionError", "missing fs.read capability");
        }
        let path = match require_path(_py, path_bits, "path") { Ok(p) => p, Err(bits) => return bits };
        // dir_fd: None -> AT_FDCWD; non-int -> TypeError (CPython parity)
        // follow_symlinks=false -> AT_SYMLINK_NOFOLLOW
        #[cfg(unix)]
        {
            let c_path = match to_cstring(&path) { Ok(c) => c, Err(bits) => return bits };
            let mut stat_buf: libc::stat = unsafe { std::mem::zeroed() };
            let rc = unsafe { libc::fstatat(dir_fd, c_path.as_ptr(), &mut stat_buf, flags) };
            if rc < 0 {
                // PORTABILITY: route errno through std::io::Error::last_os_error()
                // (the established os_ext.rs:29-31 pattern) — never raw
                // __errno_location() (spelled differently on macOS).
                return raise_os_error(_py, std::io::Error::last_os_error(), "stat");
            }
            stat_tuple_from_libc_stat(_py, &stat_buf)
        }
        #[cfg(not(unix))]
        { raise_os_error_errno::<u64>(_py, libc::ENOSYS as i64, "stat") }
    })
}

#[cfg(target_arch = "wasm32")]
#[unsafe(no_mangle)]
pub extern "C" fn molt_os_stat_at(_p: u64, _d: u64, _f: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        raise_os_error_errno::<u64>(_py, libc::ENOSYS as i64, "stat")
    })
}
```

A `to_cstring` helper (PathBuf → CString, raising on interior NUL) joins the helpers at the top of `os_ext.rs` next to `require_path`/`str_bits`.

`rename_at`/`replace_at` both call `libc::renameat` (POSIX rename IS atomic-replace; the two intrinsic symbols exist so the Python wrapper uses the semantically correct name). `utime_at` uses `libc::utimensat` with the timespec pair (or null pointer when both ns are −1).

---

When this pattern is implemented in `molt-runtime-path`, do not reintroduce a
raw `has_capability` branch or a generic `path.has_capability` audit event. Use
the path leaf bridge helper instead:

```rust
if !audit_capability(_py, "os.stat_at", "fs.read", AuditArg::None) {
    return raise_exception::<_>(_py, "PermissionError", "missing fs.read capability");
}
```

## WASI Story

WASI preview-1 is *natively* dir-fd-relative: all FS ops are `*at`-style against preopened directory fds — structurally the same model. BUT a wasm app cannot dynamically `os.open(dir, O_DIRECTORY)` to mint general dir fds (preopens are start-time grants), and reaching the raw `path_filestat_get`-family calls means bypassing `std::fs`. **Decision:** WASM stubs raise `OSError(ENOSYS)` (not `NotImplementedError`) for v1. WASI preview-2's `wasi:filesystem` resource handles map cleanly to dir_fd ints — documented as the correct future path.

## Windows Policy

Mirror CPython 3.12 exactly: every dir_fd operation raises `NotImplementedError` on Windows (CPython implements none of these there via the dir_fd parameter). The `#[cfg(not(unix))]` ENOSYS arm covers it at the Rust level; the Python wrapper surfaces CPython's message shape.

---

## Python Wrapper Changes (`src/molt/stdlib/os.py`)

Replace all 12 `NotImplementedError` raises with `_at` intrinsic dispatch. Pattern for `stat` (os.py:1199-1209):

```python
def stat(path, *, dir_fd=None, follow_symlinks=True):
    _require_cap("fs.read")
    if dir_fd is not None:
        intrinsic = _require_os_intrinsic("molt_os_stat_at")
        return _expect_stat_result(intrinsic(path, dir_fd, bool(follow_symlinks)), "stat")
    if bool(follow_symlinks):
        return _expect_stat_result(_require_os_intrinsic("molt_os_stat")(path), "stat")
    return _expect_stat_result(_require_os_intrinsic("molt_os_lstat")(path), "lstat")
```

`rename`/`replace`/`link`: pass `src_dir_fd`/`dst_dir_fd` verbatim (None → AT_FDCWD on the Rust side). `utime`: pre-compute the ns pair from `times=`/`ns=` (−1,−1 = current time) — this also closes the separate `utime(ns=...)` gap at os.py:1399-1400.

Update the `supports_dir_fd` set to add: `stat`, `lstat`, `rename`, `replace`, `link`, `symlink`, `readlink`, `utime`.

---

## Error Parity with CPython

1. Non-integer dir_fd → `TypeError: an integer is required`.
2. Invalid fd → exact-errno `OSError` subclass mapping (`ENOENT`→FileNotFoundError, `EPERM/EACCES`→PermissionError, `EEXIST`→FileExistsError, `ENOTDIR`→NotADirectoryError, `EBADF`→OSError) — the existing `raise_os_error` mapping already does this.
3. `supports_dir_fd` membership must match CPython for implemented functions.
4. `link(follow_symlinks=False, src_dir_fd=...)` on Linux: `linkat(AT_EMPTY_PATH)` needs `CAP_DAC_READ_SEARCH` — attempt the syscall and propagate the kernel's `EPERM` as `PermissionError` (CPython 3.12 Linux behavior), do NOT pre-raise `NotImplementedError`.

---

## Implementation Order

**Step 1 (Rust, easiest→hardest):** `readlink_at` → `symlink_at` (arg-order trap) → `utime_at` → `lstat_at` (os_ext.rs); then `stat_at` → `rename_at` → `replace_at` → `link_at` (io_path.rs). All with wasm32 ENOSYS stub pairs.

**Step 2:** `manifest.pyi` — add the 8 signatures after `molt_os_replace` (line ~1141).

**Step 3:** `python3 tools/gen_intrinsics.py` → regenerates `generated.rs` + `_intrinsics.pyi` (both CHECKED IN — git add with the manifest in one atomic commit; never hand-edit).

**Step 4:** os.py wrapper dispatch + `supports_dir_fd` + `utime(ns=)`.

**Step 5 (differential tests, tests/differential/stdlib/):** `os_dir_fd_stat.py`, `os_dir_fd_lstat.py`, `os_dir_fd_readlink.py`, `os_dir_fd_symlink.py`, `os_dir_fd_rename.py`, `os_dir_fd_replace.py`, `os_dir_fd_link.py`, `os_dir_fd_utime.py` (times= AND ns=), `os_dir_fd_error_badf.py` (dir_fd=-1 → EBADF), `os_dir_fd_error_notdir.py` (file fd → ENOTDIR), `os_dir_fd_follow_symlinks_false.py`, `os_utime_ns_only.py`, `os_supports_dir_fd.py`. Each: mkdtemp → `os.open(tmpdir, os.O_RDONLY)` → op under test → diff vs CPython 3.12 → close fd in finally.

---

## Critical Implementation Notes

1. **`stat_tuple_from_libc_stat` helper** (io_path.rs): converts `libc::stat` to the same 10-tuple as the existing `stat_tuple_from_metadata`. macOS spells times `st_atimespec` (timespec) vs Linux `st_atime` (seconds) — `#[cfg(target_os = ...)]` inside the helper.
2. **`libc::AT_FDCWD`** differs per platform (−2 macOS, −100 Linux) — always the libc constant.
3. **errno portability**: route through `std::io::Error::last_os_error()` (established os_ext.rs pattern), never `__errno_location()`.
4. **`replace_at` = `rename_at`** at the syscall level; separate symbols for wrapper-name correctness.
5. **`generated.rs` is checked in** (24K+ lines) — regenerate + commit atomically with manifest.pyi and _intrinsics.pyi.
