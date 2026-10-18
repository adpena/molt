#!/usr/bin/env python3
from __future__ import annotations

import argparse
import os
import shutil
import stat
import time
from collections.abc import Mapping
from dataclasses import dataclass, field
from functools import lru_cache
from pathlib import Path
from typing import Any, Literal, TypedDict

from molt.default_paths import (
    DEFAULT_MOLT_CACHE_MAX_AGE_DAYS,
    DEFAULT_MOLT_CACHE_MAX_GB,
    NATIVE_STDLIB_OBJECT_CACHE_DIRECTORY,
    SHARED_STDLIB_CACHE_DIRECTORY,
    WASM_LINK_CACHE_DIRECTORY,
    WASM_LINK_CACHE_FAMILIES,
    _default_molt_cache,
    cache_control_root,
    protected_cache_roots,
    is_backend_cache_artifact_name,
    native_stdlib_object_cache_entry,
    shared_stdlib_cache_entry,
    wasm_link_cache_paths,
)

from molt.file_publication import is_staged_file_path

from molt.file_locks import (
    _file_lock_owned_operation,
    _release_file_lock,
    _try_acquire_file_lock,
)


def _default_cache_root() -> Path:
    return _default_molt_cache()


@dataclass
class CacheEntry:
    path: Path
    size_bytes: int
    metadata: os.stat_result
    lock_path: Path | None = None

    @property
    def mtime(self) -> float:
        return self.metadata.st_mtime


class PruneCompleted(TypedDict):
    status: Literal["completed"]
    cache_root: str
    entries_before: int
    inventory_elapsed_seconds: float
    elapsed_seconds: float
    entries_removed: int
    entries_remaining: int
    entries_busy: int
    entries_failed: int
    bytes_before: int
    bytes_remaining: int
    bytes_removed: int
    bytes_removed_human: str
    protected_state: list[str]
    legacy_native_state: list[str]


class PruneSkipped(TypedDict):
    status: Literal["busy", "recent"]
    cache_root: str


def _format_bytes(size: int) -> str:
    units = ["B", "KiB", "MiB", "GiB", "TiB"]
    value = float(size)
    for unit in units:
        if value < 1024.0 or unit == units[-1]:
            return f"{value:.1f}{unit}"
        value /= 1024.0
    return f"{size}B"


def _entry_size_bytes(path: Path) -> int:
    if path.is_symlink():
        return 0
    if path.is_file():
        try:
            return path.stat().st_size
        except OSError:
            return 0
    total = 0
    for child in path.rglob("*"):
        if child.is_symlink() or not child.is_file():
            continue
        try:
            total += child.stat().st_size
        except OSError:
            continue
    return total


def _children(path: Path) -> list[Path]:
    # Two cleanup consumers may legitimately remove the same entry between
    # discovery steps. Other I/O failures remain operational failures.
    try:
        return list(path.iterdir())
    except FileNotFoundError:
        return []


def _is_direct_directory(path: Path) -> bool:
    try:
        return stat.S_ISDIR(path.lstat().st_mode)
    except FileNotFoundError:
        return False


def _legacy_native_entry(path: Path) -> bool:
    # Old compilation scratch is not covered by its final publication lock.
    # Never use age/PID/open-file absence as permission to remove a live writer.
    # These names are no longer produced or read by the current cache protocol.
    name = path.name
    return (
        name.startswith(("stdlib_shared_", ".stdlib_shared_"))
        or name == "native-stdlib-objects"
        or name.endswith(".publish.lock")
        or name.endswith(".objects.tmp")
    )


@dataclass(slots=True)
class _LegacyStandaloneNames:
    """Ephemeral dot-component index of adjacent legacy publication owners."""

    children: dict[str, _LegacyStandaloneNames] = field(default_factory=dict)
    terminal: bool = False

    def add(self, stem: str) -> None:
        node = self
        for component in stem.split("."):
            child = node.children.get(component)
            if child is None:
                child = _LegacyStandaloneNames()
                node.children[component] = child
            node = child
        node.terminal = True

    def _matches(self, components: list[str], *, exact: bool) -> bool:
        node = self
        for component in components:
            if node.terminal:
                return True
            child = node.children.get(component)
            if child is None:
                return False
            node = child
        return exact and node.terminal

    def protects(self, name: str) -> bool:
        if not self.children:
            return False
        components = name.split(".")
        return self._matches(components, exact=True) or (
            name.startswith(".") and self._matches(components[1:], exact=False)
        )


def _collect_entries(
    cache_root: Path, *, environ: Mapping[str, str] | None = None
) -> list[CacheEntry]:
    cache_root = cache_root.resolve(strict=False)
    entries: list[CacheEntry] = []
    protected = protected_cache_roots(cache_root, environ=environ)
    if cache_root in protected:
        return entries

    def append(path: Path, lock: Path | None = None) -> None:
        try:
            metadata = path.lstat()
        except FileNotFoundError:
            return
        entries.append(CacheEntry(path, _entry_size_bytes(path), metadata, lock))

    children = _children(cache_root)
    standalone_names = _LegacyStandaloneNames()
    for child in children:
        if child.name.endswith(".publish.lock"):
            standalone_names.add(Path(child.name.removesuffix(".publish.lock")).stem)
    for child in children:
        if (
            child.absolute() in protected
            or is_staged_file_path(child)
            or _legacy_native_entry(child)
            or standalone_names.protects(child.name)
        ):
            continue
        if child.name in {
            "module_analysis",
            "module_lowering",
        } and _is_direct_directory(child):
            # Published JSON leaves are immutable; readers copy on hydration and
            # treat disappearance as a cache miss. Staging files remain owned.
            for leaf in _children(child):
                if leaf.suffix == ".json" and leaf.is_file() and not leaf.is_symlink():
                    append(leaf)
            continue
        if child.name in {
            SHARED_STDLIB_CACHE_DIRECTORY,
            NATIVE_STDLIB_OBJECT_CACHE_DIRECTORY,
        } and _is_direct_directory(child):
            for entry_root in _children(child):
                if (
                    child.name == NATIVE_STDLIB_OBJECT_CACHE_DIRECTORY
                    and len(entry_root.name) == 2
                    and all(c in "0123456789abcdef" for c in entry_root.name)
                ):
                    # Previous producers used flat files plus adjacent locks in
                    # a two-hex shard. They do not share current entry stripes.
                    continue
                try:
                    if child.name == SHARED_STDLIB_CACHE_DIRECTORY:
                        _root, lock = shared_stdlib_cache_entry(
                            cache_root, entry_root.name
                        )
                    else:
                        _root, lock = native_stdlib_object_cache_entry(
                            cache_root, entry_root.name
                        )
                except ValueError:
                    # No immutable-entry protocol owns unknown names.
                    continue
                append(entry_root, lock)
            continue
        if child.name == WASM_LINK_CACHE_DIRECTORY and _is_direct_directory(child):
            for family_root in _children(child):
                if (
                    family_root.name not in WASM_LINK_CACHE_FAMILIES
                    or not _is_direct_directory(family_root)
                ):
                    continue
                for schema_root in _children(family_root):
                    if schema_root.name == ".locks":
                        continue
                    try:
                        wasm_link_cache_paths(
                            child, family_root.name, schema_root.name, "0" * 64
                        )
                    except ValueError:
                        continue
                    if not _is_direct_directory(schema_root):
                        continue
                    for entry_root in _children(schema_root):
                        try:
                            _root, lock = wasm_link_cache_paths(
                                child,
                                family_root.name,
                                schema_root.name,
                                entry_root.name,
                            )
                        except ValueError:
                            continue
                        append(entry_root, lock)
            continue
        if (
            is_backend_cache_artifact_name(child.name)
            and child.is_file()
            and not child.is_symlink()
        ):
            append(child)
        # Unknown namespaces may own live mutable stages or persistent controls.
        # Their root is never an inferred recursive eviction unit.
    return entries


# Match the maintenance cadence: hot readers need no metadata write per access.
AUTOMATIC_PRUNE_INTERVAL_SECONDS = 300.0


def _same_cache_generation(left: os.stat_result, right: os.stat_result) -> bool:
    return (
        os.path.samestat(left, right)
        and left.st_size == right.st_size
        and left.st_mtime_ns == right.st_mtime_ns
        and left.st_ctime_ns == right.st_ctime_ns
    )


@lru_cache(maxsize=1)
def _windows_timestamp_api() -> tuple[Any, Any, Any]:
    if os.name != "nt":
        raise OSError("Windows timestamp API is unavailable on this platform")
    import ctypes
    from ctypes import wintypes

    kernel32 = ctypes.WinDLL("kernel32", use_last_error=True)
    kernel32.CreateFileW.restype = wintypes.HANDLE
    kernel32.CreateFileW.argtypes = [
        wintypes.LPCWSTR,
        wintypes.DWORD,
        wintypes.DWORD,
        ctypes.c_void_p,
        wintypes.DWORD,
        wintypes.DWORD,
        wintypes.HANDLE,
    ]
    kernel32.SetFileTime.restype = wintypes.BOOL
    kernel32.SetFileTime.argtypes = [
        wintypes.HANDLE,
        ctypes.c_void_p,
        ctypes.c_void_p,
        ctypes.POINTER(wintypes.FILETIME),
    ]
    kernel32.CloseHandle.restype = wintypes.BOOL
    kernel32.CloseHandle.argtypes = [wintypes.HANDLE]
    return ctypes, kernel32, wintypes.FILETIME


def _refresh_cache_entry_mtime(path: Path, observed: os.stat_result, now: int) -> None:
    """Update the admitted generation through a pinned, no-follow descriptor.

    CPython 3.12's Windows utime supports neither descriptors nor no-follow
    paths. SetFileTime supplies the same operation on a no-follow Win32 handle;
    read-only sharing prevents replacement while its identity is qualified.
    POSIX uses futimens via os.utime. Neither path changes a replacement file.
    """
    if os.name == "nt":
        import msvcrt

        ctypes, kernel32, filetime = _windows_timestamp_api()
        spelling = os.fspath(path)
        if "\0" in spelling:
            raise ValueError("embedded null character")
        handle = kernel32.CreateFileW(
            spelling,
            0x0080 | 0x0100,  # FILE_READ_ATTRIBUTES | FILE_WRITE_ATTRIBUTES
            0x00000001,  # FILE_SHARE_READ; pin against writes and deletion.
            None,
            3,  # OPEN_EXISTING
            0x02000000 | 0x00200000,  # BACKUP_SEMANTICS | OPEN_REPARSE_POINT
            None,
        )
        if handle in (None, ctypes.c_void_p(-1).value):
            raise ctypes.WinError(ctypes.get_last_error())
        try:
            descriptor = msvcrt.open_osfhandle(handle, os.O_RDONLY | os.O_NOINHERIT)
        except BaseException:
            kernel32.CloseHandle(handle)
            raise
    else:
        descriptor = os.open(
            path, os.O_RDONLY | os.O_NOFOLLOW | os.O_NONBLOCK | os.O_CLOEXEC
        )
    try:
        metadata = os.fstat(descriptor)
        if (
            not (stat.S_ISREG(metadata.st_mode) or stat.S_ISDIR(metadata.st_mode))
            or getattr(metadata, "st_file_attributes", 0)
            & getattr(stat, "FILE_ATTRIBUTE_REPARSE_POINT", 0)
            or not _same_cache_generation(metadata, observed)
        ):
            return
        if os.name == "nt":
            ticks = now // 100 + 116444736000000000
            written = filetime(ticks & 0xFFFFFFFF, ticks >> 32)
            if not kernel32.SetFileTime(handle, None, None, ctypes.byref(written)):
                raise ctypes.WinError(ctypes.get_last_error())
        else:
            os.utime(descriptor, ns=(metadata.st_atime_ns, now))
    finally:
        os.close(descriptor)


def refresh_cache_entry_recency(
    path: Path,
    *,
    cache_root: Path | None = None,
    observed: os.stat_result | None = None,
) -> None:
    """Record admitted reuse on its optional eviction owner, never owned output.

    Directory entries already hold their canonical publication stripe. Lockless
    JSON/flat-file readers pass their cache root and the observation preceding
    admission; the existing prune lock serializes their metadata-only update.
    Recency is advisory: ordinary optional-cache I/O failure cannot fail a build.
    """
    handle = None
    try:
        metadata = observed if observed is not None else path.lstat()
        if not (stat.S_ISREG(metadata.st_mode) or stat.S_ISDIR(metadata.st_mode)):
            return
        now = time.time_ns()
        if now - metadata.st_mtime_ns < int(AUTOMATIC_PRUNE_INTERVAL_SECONDS * 1e9):
            return
        if cache_root is not None:
            if observed is None:
                return
            absolute = path.absolute()
            resolved = path.resolve(strict=False)
            if any(
                absolute.is_relative_to(root) or resolved.is_relative_to(root)
                for root in protected_cache_roots(cache_root)
            ):
                return
            lock_path = cache_control_root(cache_root) / "prune.lock"
            handle = _try_acquire_file_lock(lock_path)
            if handle is None:
                return
            with _file_lock_owned_operation(handle, expected_lock_path=lock_path):
                _refresh_cache_entry_mtime(path, observed, now)
        else:
            _refresh_cache_entry_mtime(path, metadata, now)
    except OSError:
        # The value has already been admitted and copied into reader custody.
        pass
    finally:
        if handle is not None:
            _release_file_lock(handle)


def _remove_entry(entry: CacheEntry, dry_run: bool) -> str:
    lock_handle = None
    try:
        if entry.lock_path is not None:
            try:
                lock_handle = _try_acquire_file_lock(
                    entry.lock_path, create=not dry_run
                )
            except FileNotFoundError:
                if not dry_run:
                    raise
                # No lock existed at observation time. A dry-run forecasts
                # eviction; it grants no capability for a later deletion.
            else:
                if lock_handle is None:
                    return "busy"
        try:
            current = entry.path.lstat()
        except FileNotFoundError:
            return "removed"
        if not _same_cache_generation(current, entry.metadata):
            # A reader refreshed this entry (or a publisher replaced it) after
            # inventory. The old observation no longer owns its eviction.
            return "busy"
        if dry_run:
            return "removed"
        if _is_direct_directory(entry.path):
            try:
                shutil.rmtree(entry.path, ignore_errors=False)
            except FileNotFoundError:
                pass
        else:
            entry.path.unlink(missing_ok=True)
        return "failed" if entry.path.exists() or entry.path.is_symlink() else "removed"
    except OSError:
        return "failed"
    finally:
        if lock_handle is not None:
            _release_file_lock(lock_handle)


def _eviction_order(entry: CacheEntry) -> tuple[float, str]:
    normalized_path = os.path.normcase(os.path.normpath(os.fspath(entry.path)))
    return entry.mtime, normalized_path


def _prune_locked(
    cache_root: Path,
    *,
    max_bytes: int | None,
    max_age_days: int | None,
    dry_run: bool,
    environ: Mapping[str, str] | None = None,
) -> PruneCompleted:
    started = time.monotonic()
    entries = _collect_entries(cache_root, environ=environ)
    inventory_elapsed = time.monotonic() - started
    before_bytes = sum(item.size_bytes for item in entries)
    remaining = {entry.path: entry for entry in entries}
    removed: list[CacheEntry] = []
    attempted: set[Path] = set()
    busy = failures = 0

    def remove(entry: CacheEntry) -> bool:
        nonlocal busy, failures
        attempted.add(entry.path)
        outcome = _remove_entry(entry, dry_run)
        if outcome == "removed":
            removed.append(entry)
            remaining.pop(entry.path)
            return True
        busy += outcome == "busy"
        failures += outcome == "failed"
        return False

    if max_age_days is not None and max_age_days >= 0:
        cutoff = time.time() - (max_age_days * 86400)
        for entry in entries:
            if entry.mtime < cutoff:
                remove(entry)

    total = sum(item.size_bytes for item in remaining.values())
    if max_bytes is not None and max_bytes >= 0 and total > max_bytes:
        for entry in sorted(remaining.values(), key=_eviction_order):
            if total <= max_bytes:
                break
            if entry.path not in attempted and remove(entry):
                total -= entry.size_bytes

    removed_bytes = sum(item.size_bytes for item in removed)
    return {
        "status": "completed",
        "cache_root": str(cache_root),
        "entries_before": len(entries),
        "inventory_elapsed_seconds": inventory_elapsed,
        "elapsed_seconds": time.monotonic() - started,
        "entries_removed": len(removed),
        "entries_remaining": len(remaining),
        "entries_busy": busy,
        "entries_failed": failures,
        "bytes_before": before_bytes,
        "bytes_remaining": sum(item.size_bytes for item in remaining.values()),
        "bytes_removed": removed_bytes,
        "bytes_removed_human": _format_bytes(removed_bytes),
        "protected_state": [
            str(path) for path in protected_cache_roots(cache_root, environ=environ)
        ],
        "legacy_native_state": [
            str(path) for path in _children(cache_root) if _legacy_native_entry(path)
        ]
        + [
            str(path)
            for path in (
                _children(cache_root / NATIVE_STDLIB_OBJECT_CACHE_DIRECTORY)
                if _is_direct_directory(
                    cache_root / NATIVE_STDLIB_OBJECT_CACHE_DIRECTORY
                )
                else []
            )
            if len(path.name) == 2 and all(c in "0123456789abcdef" for c in path.name)
        ],
    }


def resolve_budget(
    max_gb: float | None,
    max_age_days: int | None,
    environ: Mapping[str, str],
) -> tuple[float, int]:
    """Return the cache budget: flags, then the projected env, then canonical user-state defaults.

    RunContext projects MOLT_CACHE_MAX_GB and MOLT_CACHE_MAX_AGE_DAYS from the
    default_paths, so the pruner and every DX entry point share one budget.
    """
    gb = (
        max_gb
        if max_gb is not None
        else float(environ.get("MOLT_CACHE_MAX_GB") or DEFAULT_MOLT_CACHE_MAX_GB)
    )
    days = (
        max_age_days
        if max_age_days is not None
        else int(
            environ.get("MOLT_CACHE_MAX_AGE_DAYS") or DEFAULT_MOLT_CACHE_MAX_AGE_DAYS
        )
    )
    return gb, days


def _prune(
    cache_root: Path,
    *,
    max_bytes: int | None,
    max_age_days: int | None,
    dry_run: bool,
    automatic: bool = False,
    environ: Mapping[str, str] | None = None,
) -> PruneCompleted | PruneSkipped:
    """Serialize inventory/eviction across every entry point without waiting.

    The persistent control lock records automatic attempts and completed sweeps.
    Automatic callers coalesce here before inventory, including failed attempts.
    Explicit sweeps are unthrottled; dry runs do not change control state.
    """
    if automatic and dry_run:
        raise ValueError("automatic cache retention cannot be a dry run")
    lock_path = cache_control_root(cache_root) / "prune.lock"
    try:
        handle = _try_acquire_file_lock(lock_path, create=not dry_run)
    except FileNotFoundError:
        if not dry_run:
            raise
        return _prune_locked(
            cache_root,
            max_bytes=max_bytes,
            max_age_days=max_age_days,
            dry_run=True,
            environ=environ,
        )
    if handle is None:
        return {"status": "busy", "cache_root": str(cache_root)}
    try:
        with _file_lock_owned_operation(handle, expected_lock_path=lock_path):
            if automatic:
                now = time.time()
                handle.file.seek(0)
                stamp = handle.file.read(64)
                if stamp:
                    try:
                        elapsed = now - float(stamp)
                    except ValueError:
                        elapsed = AUTOMATIC_PRUNE_INTERVAL_SECONDS
                    if 0 <= elapsed < AUTOMATIC_PRUNE_INTERVAL_SECONDS:
                        return {"status": "recent", "cache_root": str(cache_root)}
                # Persist admission before inventory so a failed or interrupted
                # sweep is not immediately repeated by another completed proof.
                handle.file.seek(0)
                handle.file.write(str(now).encode("ascii"))
                handle.file.truncate()
                handle.file.flush()
            result = _prune_locked(
                cache_root,
                max_bytes=max_bytes,
                max_age_days=max_age_days,
                dry_run=dry_run,
                environ=environ,
            )
            if not dry_run:
                handle.file.seek(0)
                handle.file.write(str(time.time()).encode("ascii"))
                handle.file.truncate()
                handle.file.flush()
            return result
    finally:
        _release_file_lock(handle)


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser(
        description="Prune Molt cache entries by age and/or total size."
    )
    parser.add_argument(
        "--cache-dir",
        default=None,
        help="Cache root to prune (default: MOLT_CACHE or platform default).",
    )
    parser.add_argument(
        "--max-gb",
        type=float,
        default=None,
        help="Maximum cache size in GiB after pruning.",
    )
    parser.add_argument(
        "--max-age-days",
        type=int,
        default=None,
        help="Delete inactive cache entries older than this age (days).",
    )
    parser.add_argument(
        "--dry-run",
        action="store_true",
        help="Compute what would be removed without deleting files.",
    )
    return parser.parse_args()


def main() -> int:
    args = parse_args()
    cache_root = (
        Path(args.cache_dir).expanduser().resolve()
        if args.cache_dir
        else _default_cache_root()
    )

    max_gb, max_age_days = resolve_budget(args.max_gb, args.max_age_days, os.environ)

    max_bytes = int(max_gb * (1024**3))
    result = _prune(
        cache_root,
        max_bytes=max_bytes,
        max_age_days=max_age_days,
        dry_run=args.dry_run,
    )

    print(f"cache_root={cache_root}")
    if result["status"] != "completed":
        print(f"prune.status={result['status']}")
        return 0
    print(f"policy.max_gb={max_gb}")
    print(f"policy.max_age_days={max_age_days}")
    print(f"evictable.before={_format_bytes(result['bytes_before'])}")
    print(f"evictable.after={_format_bytes(result['bytes_remaining'])}")
    print(f"protected_state.roots={len(result['protected_state'])}")
    print(f"legacy_native.protected={len(result['legacy_native_state'])}")
    print(f"inventory.elapsed_s={result['inventory_elapsed_seconds']:.6f}")
    print(f"prune.elapsed_s={result['elapsed_seconds']:.6f}")
    print(f"remaining.entries={result['entries_remaining']}")
    print(f"busy.entries={result['entries_busy']}")
    print(f"failed.entries={result['entries_failed']}")
    print(f"removed.entries={result['entries_removed']}")
    print(f"removed.bytes={result['bytes_removed_human']}")
    print(f"dry_run={args.dry_run}")
    return 1 if result["entries_failed"] else 0


if __name__ == "__main__":
    raise SystemExit(main())
