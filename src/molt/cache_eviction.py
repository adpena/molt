#!/usr/bin/env python3
from __future__ import annotations

import argparse
import heapq
import os
import shutil
import stat
import time
from collections.abc import Mapping
from contextlib import ExitStack
from dataclasses import dataclass, field
from functools import lru_cache
from pathlib import Path
from typing import TYPE_CHECKING, Any, Literal, TypedDict

from molt.default_paths import (
    CACHE_MAINTENANCE_INTERVAL_SECONDS as AUTOMATIC_PRUNE_INTERVAL_SECONDS,
    NATIVE_STDLIB_ARCHIVE_CACHE_DIRECTORY,
    NATIVE_STDLIB_OBJECT_CACHE_DIRECTORY,
    SHARED_STDLIB_CACHE_DIRECTORY,
    SHARED_SOURCE_RECORDS_DIRECTORY,
    WASM_LINK_CACHE_DIRECTORY,
    WASM_LINK_CACHE_FAMILIES,
    TOOLCHAIN_SYMBOL_FACTS_CACHE_DIRECTORY,
    RUNTIME_WASM_CACHE_DIRECTORY,
    RUNTIME_WASM_CACHE_RETIREMENT_SCOPE,
    _default_molt_cache,
    protected_cache_roots,
    shared_cache_lock_path,
    is_backend_cache_artifact_name,
    backend_cache_sync_receipt_path,
    backend_cache_artifact_for_sync_receipt,
    backend_cache_publication_lock_name,
    native_stdlib_archive_cache_entry,
    native_stdlib_object_cache_entry,
    resolve_budget,
    shared_stdlib_cache_entry,
    toolchain_symbol_facts_cache_entry,
    runtime_wasm_cache_entry,
    wasm_link_cache_paths,
)

from molt.file_publication import (
    durable_remove_path,
    is_staged_file_path,
    reclaim_retired_paths,
)

from molt.file_locks import (
    _acquire_file_lock,
    _file_lock_owned_operation,
    _release_file_lock,
    _try_acquire_file_lock,
)


if TYPE_CHECKING:
    from molt.cli.source_record_store import SourceRecordMetadata


def _default_cache_root() -> Path:
    return _default_molt_cache()


@dataclass
class CacheEntry:
    path: Path
    size_bytes: int
    metadata: os.stat_result
    lock_path: Path | None = None
    retirement_scope: str | None = None
    companions: tuple[tuple[Path, os.stat_result], ...] = ()
    absent_peer: Path | None = None
    minimum_idle_seconds: float = 0.0

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
    bytes_basis: Literal["file-bytes-plus-source-record-payload-bytes"]
    source_records_before: int
    source_records_removed: int
    source_record_payload_bytes_before: int
    source_record_payload_bytes_removed: int
    source_record_physical_before: dict[str, int | None] | None
    source_record_physical_after: dict[str, int | None] | None
    source_record_maintenance_failed: bool
    source_record_maintenance_busy: bool
    source_record_store_unmigrated: bool
    source_record_checkpoint_busy: bool | None
    protected_state: list[str]
    legacy_native_state: list[str]
    legacy_source_record_state: list[str]


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
    # Pre-e9b0b9be8 writers may keep required outputs and compilation scratch
    # here; the final publication lock does not cover their whole lifetime.
    # Current standalone publishers still create *.publish.lock. Keep that
    # inode/name even when unheld: an already-open waiter may acquire it later.
    # Age or one successful lock attempt cannot retire this shared protocol.
    # HF-282 requires a versioned namespace or an explicitly quiescent purge.
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

    def append(
        path: Path,
        lock: Path | None = None,
        *,
        retirement_scope: str | None = None,
        companions: tuple[Path, ...] = (),
        absent_peer: Path | None = None,
        minimum_idle_seconds: float = 0.0,
    ) -> None:
        try:
            metadata = path.lstat()
        except FileNotFoundError:
            return
        observed_companions = []
        for companion in companions:
            try:
                companion_metadata = companion.lstat()
            except FileNotFoundError:
                continue
            if not stat.S_ISREG(companion_metadata.st_mode):
                return
            observed_companions.append((companion, companion_metadata))
        entries.append(
            CacheEntry(
                path,
                _entry_size_bytes(path)
                + sum(row.st_size for _, row in observed_companions),
                metadata,
                lock,
                retirement_scope,
                tuple(observed_companions),
                absent_peer,
                minimum_idle_seconds,
            )
        )

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
            # Retired file tiers remain cleanup-only namespaces. Older processes
            # admit immutable leaves and tolerate a miss; stages remain owned.
            for leaf in _children(child):
                if leaf.suffix == ".json" and leaf.is_file() and not leaf.is_symlink():
                    append(leaf)
            continue
        if child.name == RUNTIME_WASM_CACHE_DIRECTORY and _is_direct_directory(child):
            from molt.cli.generation_store import GENERATION_STORE_IDLE_SECONDS

            for family in _children(child):
                if not _is_direct_directory(family):
                    continue
                try:
                    _root, lock = runtime_wasm_cache_entry(cache_root, family.name)
                except ValueError:
                    continue
                append(
                    family,
                    lock,
                    retirement_scope=RUNTIME_WASM_CACHE_RETIREMENT_SCOPE,
                    minimum_idle_seconds=GENERATION_STORE_IDLE_SECONDS,
                )
            continue
        if (
            child.name == TOOLCHAIN_SYMBOL_FACTS_CACHE_DIRECTORY
            and _is_direct_directory(child)
        ):
            # Completed facts are eagerly decoded into reader custody. A miss
            # recomputes them; stages, future schemas and unknown names have no
            # eviction authority here.
            for schema_root in _children(child):
                if not _is_direct_directory(schema_root):
                    continue
                try:
                    version = int(schema_root.name.removeprefix("v"))
                    canonical_schema = toolchain_symbol_facts_cache_entry(
                        cache_root, version, "0" * 64
                    ).parent
                except ValueError:
                    continue
                if canonical_schema != schema_root:
                    continue
                for leaf in _children(schema_root):
                    try:
                        expected = toolchain_symbol_facts_cache_entry(
                            cache_root, version, leaf.stem
                        )
                    except ValueError:
                        continue
                    if expected == leaf and leaf.is_file() and not leaf.is_symlink():
                        append(leaf)
            continue
        if child.name in {
            SHARED_STDLIB_CACHE_DIRECTORY,
            NATIVE_STDLIB_ARCHIVE_CACHE_DIRECTORY,
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
                    elif child.name == NATIVE_STDLIB_ARCHIVE_CACHE_DIRECTORY:
                        _root, lock = native_stdlib_archive_cache_entry(
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
        if child.name == SHARED_SOURCE_RECORDS_DIRECTORY and _is_direct_directory(
            child
        ):
            # A namespace is one SQLite/WAL identity. Only older namespaces
            # speaking the lifetime protocol can retire, under their exclusive
            # reader lock; current/newer and legacy flat stores stay untouched.
            from molt.cli.source_record_store import (
                SOURCE_RECORD_RETIREMENT_SCOPE,
                source_record_namespace_paths,
                superseded_store_namespace,
            )

            for namespace in _children(child):
                if superseded_store_namespace(namespace.name) and _is_direct_directory(
                    namespace
                ):
                    _, lock = source_record_namespace_paths(cache_root, namespace.name)
                    append(
                        namespace, lock, retirement_scope=SOURCE_RECORD_RETIREMENT_SCOPE
                    )
            continue
        if (
            is_backend_cache_artifact_name(child.name)
            and child.is_file()
            and not child.is_symlink()
        ):
            append(
                child,
                shared_cache_lock_path(
                    cache_root, backend_cache_publication_lock_name(child)
                ),
                companions=(backend_cache_sync_receipt_path(child),),
            )
        elif (artifact := backend_cache_artifact_for_sync_receipt(child)) is not None:
            if not artifact.exists() and child.is_file() and not child.is_symlink():
                append(
                    child,
                    shared_cache_lock_path(
                        cache_root, backend_cache_publication_lock_name(artifact)
                    ),
                    absent_peer=artifact,
                )
        # Unknown namespaces may own live mutable stages or persistent controls.
        # Their root is never an inferred recursive eviction unit.
    return entries


def _same_cache_generation(left: os.stat_result, right: os.stat_result) -> bool:
    # CPython's Windows pathname stat exposes creation time as legacy ctime,
    # while fstat may expose ChangeTime. Compare the native creation field
    # explicitly; a pathname/descriptor pair must describe the same clock.
    if os.name == "nt":
        same_generation_time = left.st_birthtime_ns == right.st_birthtime_ns
    else:
        same_generation_time = left.st_ctime_ns == right.st_ctime_ns
    return (
        os.path.samestat(left, right)
        and left.st_size == right.st_size
        and left.st_mtime_ns == right.st_mtime_ns
        and same_generation_time
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
            lock_path = shared_cache_lock_path(cache_root, "prune")
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


def _remove_entry(
    entry: CacheEntry, dry_run: bool, *, retirement_prepared: bool = False
) -> str:
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
        if (
            entry.minimum_idle_seconds > 0
            and time.time() - current.st_mtime < entry.minimum_idle_seconds
        ):
            # A byte budget cannot revoke the generation store's reader grace.
            # Publication and successful renewal change this directory identity
            # under the same persistent stripe held by this removal.
            return "busy"
        if entry.absent_peer is not None and (
            entry.absent_peer.exists() or entry.absent_peer.is_symlink()
        ):
            return "busy"
        for companion, observed in entry.companions:
            try:
                companion_current = companion.lstat()
            except FileNotFoundError:
                return "busy"
            if not _same_cache_generation(companion_current, observed):
                return "busy"
        if dry_run:
            return "removed"
        if entry.retirement_scope is not None:
            try:
                durable_remove_path(
                    entry.path,
                    retirement_scope=entry.retirement_scope,
                    reclaim_existing=not retirement_prepared,
                )
            except ValueError:
                # Malformed/changed retirement residue stays as evidence. It
                # must fail this entry without abandoning unrelated eviction.
                return "failed"
        elif _is_direct_directory(entry.path):
            try:
                shutil.rmtree(entry.path, ignore_errors=False)
            except FileNotFoundError:
                pass
        else:
            entry.path.unlink(missing_ok=True)
            for companion, _ in entry.companions:
                companion.unlink(missing_ok=True)
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
    before_bytes = sum(item.size_bytes for item in entries)
    before_count = len(entries)
    total = before_bytes
    removed_count = removed_bytes = busy = failures = 0
    row_count = row_bytes = removed_rows = removed_row_bytes = 0
    store_failed = store_busy = store_unmigrated = False
    legacy_records: list[str] = []
    row_enabled = True
    record_batch_rows = record_batch_bytes = 0
    physical_before = physical_after = None
    checkpoint_busy = None
    store = maintenance = inventory = None
    store_errors: tuple[type[Exception], ...] = ()
    store_busy_errors: tuple[type[Exception], ...] = ()
    prepared_retirement_scopes: set[str] = set()
    with ExitStack() as stack:
        wasm_root = cache_root / RUNTIME_WASM_CACHE_DIRECTORY
        if not dry_run and _is_direct_directory(wasm_root):
            try:
                # Only the global prune lock owns this retirement scope;
                # interrupted payload reclamation is never a live family.
                reclaim_retired_paths(
                    wasm_root, retirement_scope=RUNTIME_WASM_CACHE_RETIREMENT_SCOPE
                )
                prepared_retirement_scopes.add(RUNTIME_WASM_CACHE_RETIREMENT_SCOPE)
            except (OSError, ValueError):
                failures += 1
        record_root = cache_root / SHARED_SOURCE_RECORDS_DIRECTORY
        if _is_direct_directory(record_root):
            # Keep the ordinary file-only pruner lightweight and never create a
            # database just to discover that there are no source records.
            from molt.cli.source_record_store import (
                SOURCE_RECORD_RETIREMENT_SCOPE,
                STORE_RELATIVE_PATH,
                legacy_source_record_file,
                SOURCE_RECORD_BATCH_MAX_RECORDS,
                SOURCE_RECORD_BATCH_MAX_BYTES,
                SourceRecordUnavailable,
                SourceRecordUnmigrated,
                SourceRecordBusy,
                source_record_store,
            )

            legacy_records = [
                str(leaf)
                for leaf in _children(record_root)
                if legacy_source_record_file(leaf.name)
                and leaf.is_file()
                and not leaf.is_symlink()
            ]
            if not dry_run:
                try:
                    # The global prune lock owns this retirement scope. A prior
                    # partial reclamation never leaves WAL at a new live name.
                    reclaim_retired_paths(
                        record_root, retirement_scope=SOURCE_RECORD_RETIREMENT_SCOPE
                    )
                    prepared_retirement_scopes.add(SOURCE_RECORD_RETIREMENT_SCOPE)
                except (OSError, ValueError):
                    store_failed = True
                    failures += 1
            database = cache_root / STORE_RELATIVE_PATH
            if database.is_file() and not database.is_symlink():
                record_batch_rows = SOURCE_RECORD_BATCH_MAX_RECORDS
                record_batch_bytes = SOURCE_RECORD_BATCH_MAX_BYTES
                store_errors = (SourceRecordUnavailable,)
                store_busy_errors = (SourceRecordBusy,)
                store = source_record_store(cache_root)
                physical_before = store.physical_bytes()
                try:
                    maintenance = stack.enter_context(
                        store.maintenance(dry_run=dry_run)
                    )
                    inventory = maintenance.inventory()
                    row_count, row_bytes = inventory.entries, inventory.size_bytes
                except SourceRecordBusy:
                    store_busy = True
                    maintenance = None
                except SourceRecordUnmigrated:
                    # A dry run cannot publish a virgin schema; report it rather
                    # than count a failure. A writing sweep initializes it.
                    store_unmigrated = True
                    maintenance = None
                except SourceRecordUnavailable:
                    store_failed = True
                    failures += 1
                    maintenance = None
        total += row_bytes
        before_bytes += row_bytes
        before_count += row_count
        inventory_elapsed = time.monotonic() - started

        def rows():
            nonlocal store_failed, store_busy, failures
            if maintenance is None or inventory is None:
                return
            try:
                for observed in maintenance.iter_lru(inventory):
                    if not row_enabled:
                        return
                    yield observed
            except store_busy_errors:
                store_busy = True
            except store_errors:
                store_failed = True
                failures += 1

        def order(entry):
            if isinstance(entry, CacheEntry):
                return _eviction_order(entry)
            assert store is not None
            return (
                entry.used_ns / 1_000_000_000,
                os.path.normcase(os.fspath(store.path / entry.kind / entry.key)),
            )

        cutoff = (
            time.time() - max_age_days * 86400
            if max_age_days is not None and max_age_days >= 0
            else None
        )
        # SQL metadata and pending CAS decisions stay bounded; no write lock
        # crosses a yield or filesystem operation. Only committed removals
        # reduce the budget. A refused observation leaves later candidates live.
        pending: list[SourceRecordMetadata] = []
        pending_bytes = 0

        def flush_rows() -> None:
            nonlocal pending_bytes, total, removed_count, removed_bytes
            nonlocal removed_rows, removed_row_bytes, busy, failures
            nonlocal store_failed, row_enabled
            if not pending:
                return
            assert maintenance is not None
            batch = tuple(pending)
            pending.clear()
            pending_bytes = 0
            try:
                outcomes = maintenance.remove(batch)
            except store_errors:
                store_failed = True
                row_enabled = False
                failures += len(batch)
                return
            for observed, outcome in zip(batch, outcomes, strict=True):
                if outcome == "removed":
                    removed_count += 1
                    removed_rows += 1
                    removed_bytes += observed.size_bytes
                    removed_row_bytes += observed.size_bytes
                    total -= observed.size_bytes
                else:
                    busy += 1

        budget_limit = max_bytes if max_bytes is not None and max_bytes >= 0 else None

        def over_budget(size: int) -> bool:
            return budget_limit is not None and size > budget_limit

        for entry in heapq.merge(sorted(entries, key=order), rows(), key=order):
            is_file = isinstance(entry, CacheEntry)
            if is_file:
                flush_rows()
            elif not row_enabled:
                continue
            mtime = entry.mtime if is_file else entry.used_ns / 1e9
            expired = cutoff is not None and mtime < cutoff
            if not expired and not over_budget(total - pending_bytes):
                flush_rows()
                if not over_budget(total):
                    break
            if is_file:
                outcome = _remove_entry(
                    entry,
                    dry_run,
                    retirement_prepared=entry.retirement_scope
                    in prepared_retirement_scopes,
                )
                if outcome == "removed":
                    removed_count += 1
                    removed_bytes += entry.size_bytes
                    total -= entry.size_bytes
                else:
                    busy += outcome == "busy"
                    failures += outcome == "failed"
                continue
            if pending and pending_bytes + entry.size_bytes > record_batch_bytes:
                flush_rows()
                if not row_enabled:
                    continue
            pending.append(entry)
            pending_bytes += entry.size_bytes
            if (
                len(pending) >= record_batch_rows
                or pending_bytes >= record_batch_bytes
                or (budget_limit is not None and not over_budget(total - pending_bytes))
            ):
                flush_rows()
        flush_rows()
        if maintenance is not None:
            try:
                # Later sweeps must continue bounded freelist reclamation even
                # when this sweep selected no new rows for removal.
                maintenance.reclaim()
                checkpoint_busy = maintenance.checkpoint_busy
            except store_errors:
                store_failed = True
                failures += 1
    if store is not None:
        physical_after = store.physical_bytes()
    return {
        "status": "completed",
        "cache_root": str(cache_root),
        "entries_before": before_count,
        "inventory_elapsed_seconds": inventory_elapsed,
        "elapsed_seconds": time.monotonic() - started,
        "entries_removed": removed_count,
        "entries_remaining": before_count - removed_count,
        "entries_busy": busy,
        "entries_failed": failures,
        "bytes_before": before_bytes,
        "bytes_remaining": total,
        "bytes_removed": removed_bytes,
        "bytes_removed_human": _format_bytes(removed_bytes),
        "bytes_basis": "file-bytes-plus-source-record-payload-bytes",
        "source_records_before": row_count,
        "source_records_removed": removed_rows,
        "source_record_payload_bytes_before": row_bytes,
        "source_record_payload_bytes_removed": removed_row_bytes,
        "source_record_physical_before": physical_before,
        "source_record_physical_after": physical_after,
        "source_record_maintenance_failed": store_failed,
        "source_record_maintenance_busy": store_busy,
        "source_record_store_unmigrated": store_unmigrated,
        "source_record_checkpoint_busy": checkpoint_busy,
        "protected_state": [
            str(path) for path in protected_cache_roots(cache_root, environ=environ)
        ],
        "legacy_source_record_state": legacy_records,
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


def _prune(
    cache_root: Path,
    *,
    max_bytes: int | None,
    max_age_days: int | None,
    dry_run: bool,
    automatic: bool = False,
    automatic_interval_seconds: float = AUTOMATIC_PRUNE_INTERVAL_SECONDS,
    wait_for_lock: bool = False,
    environ: Mapping[str, str] | None = None,
) -> PruneCompleted | PruneSkipped:
    """Serialize inventory/eviction; foreground callers never wait.

    The persistent control lock records automatic attempts and completed sweeps.
    Automatic callers coalesce here before inventory, including failed attempts,
    and skip a sweep that any caller ran within ``automatic_interval_seconds``.
    Explicit sweeps are unthrottled; dry runs do not write retention control
    or payload state. SQLite may use its own WAL read-coordination sidecars.
    One retained background worker may wait for the same lock and recheck
    cadence after admission; it cannot leave a terminal busy epoch stranded.
    """
    if automatic and dry_run:
        raise ValueError("automatic cache retention cannot be a dry run")
    if wait_for_lock and not automatic:
        raise ValueError(
            "only a supervised automatic worker may wait for the prune lock"
        )
    lock_path = shared_cache_lock_path(cache_root, "prune")
    try:
        handle = (
            _acquire_file_lock(
                lock_path,
                timeout_s=None,
                timeout_message="cache retention lock stayed busy",
            )
            if wait_for_lock
            else _try_acquire_file_lock(lock_path, create=not dry_run)
        )
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

    def stamp_attempt() -> None:
        handle.file.seek(0)
        handle.file.write(str(time.time()).encode("ascii"))
        handle.file.truncate()
        handle.file.flush()

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
                        elapsed = automatic_interval_seconds
                    if 0 <= elapsed < automatic_interval_seconds:
                        return {"status": "recent", "cache_root": str(cache_root)}
                # Persist admission before inventory so a failed or interrupted
                # sweep is not immediately repeated by another completed proof.
                stamp_attempt()
            try:
                result = _prune_locked(
                    cache_root,
                    max_bytes=max_bytes,
                    max_age_days=max_age_days,
                    dry_run=dry_run,
                    environ=environ,
                )
            except BaseException as error:
                if automatic:
                    try:
                        stamp_attempt()
                    except BaseException as stamp_error:
                        error.add_note(
                            f"cache retention completion stamp failed: {stamp_error!r}"
                        )
                raise
            if not dry_run:
                stamp_attempt()
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
        help="Maximum evictable file and source-payload budget in GiB.",
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
        help="Compute removals without changing cached payloads.",
    )
    return parser.parse_args()


def automatic_retention(
    environ: Mapping[str, str],
    *,
    cache_root: Path | None = None,
    min_interval_seconds: float = AUTOMATIC_PRUNE_INTERVAL_SECONDS,
    wait_for_lock: bool = False,
) -> dict[str, object] | None:
    """Apply the budget to one cache root once, as an automatic caller.

    ``cache_root`` defaults to the environment's cache (``MOLT_CACHE`` or the
    platform default); a build passes the root it actually used.

    Proof payloads and CLI builds call this after they publish their output. It
    starts no process; the canonical prune lock coalesces callers across
    checkouts, and a sweep any caller ran within ``min_interval_seconds`` skips
    this one. ``MOLT_CACHE_PRUNE=0`` disables it. Ordinary retention errors are
    returned as diagnostics, never raised into the caller's result.
    A retained background worker passes ``wait_for_lock`` so contention keeps
    its one owned attempt pending instead of consuming an unstamped epoch.
    """
    if environ.get("MOLT_CACHE_PRUNE", "1") == "0":
        return None
    try:
        root = (cache_root or _default_molt_cache(environ=environ)).resolve(
            strict=False
        )
        if not root.is_dir():
            return None
        max_gb, max_age = resolve_budget(None, None, environ)
        report = _prune(
            root,
            max_bytes=int(max_gb * 1024**3),
            max_age_days=max_age,
            dry_run=False,
            automatic=True,
            automatic_interval_seconds=min_interval_seconds,
            wait_for_lock=wait_for_lock,
            environ=environ,
        )
        # Callers keep bounded diagnostics, not a second cache inventory; the
        # canonical pruner still owns all protected-state rules.
        return {
            name: value
            for name, value in report.items()
            if name
            not in {
                "protected_state",
                "legacy_native_state",
                "legacy_source_record_state",
            }
        }
    except (OSError, ValueError, RuntimeError) as exc:
        return {"status": "failed", "error": f"{type(exc).__name__}: {exc}"}


def prune_report_lines(
    cache_root: Path,
    result: PruneCompleted | PruneSkipped,
    *,
    max_gb: float,
    max_age_days: int,
    dry_run: bool,
) -> list[str]:
    """The ``key=value`` report every explicit prune entry point prints."""
    lines = [f"cache_root={cache_root}"]
    if result["status"] != "completed":
        lines.append(f"prune.status={result['status']}")
        return lines
    lines += [
        f"policy.max_gb={max_gb}",
        f"policy.max_age_days={max_age_days}",
        f"evictable.before={_format_bytes(result['bytes_before'])}",
        f"evictable.after={_format_bytes(result['bytes_remaining'])}",
        f"protected_state.roots={len(result['protected_state'])}",
        f"legacy_native.protected={len(result['legacy_native_state'])}",
        f"legacy_source_records.protected={len(result['legacy_source_record_state'])}",
        f"inventory.elapsed_s={result['inventory_elapsed_seconds']:.6f}",
        f"prune.elapsed_s={result['elapsed_seconds']:.6f}",
        f"remaining.entries={result['entries_remaining']}",
        f"busy.entries={result['entries_busy']}",
        f"failed.entries={result['entries_failed']}",
        f"removed.entries={result['entries_removed']}",
        f"removed.bytes={result['bytes_removed_human']}",
    ]
    if (
        result["source_records_before"]
        or result["source_record_maintenance_failed"]
        or result["source_record_maintenance_busy"]
        or result["source_record_store_unmigrated"]
    ):
        lines += [
            f"policy.bytes_basis={result['bytes_basis']}",
            f"source_records.maintenance_busy={result['source_record_maintenance_busy']}",
            f"source_records.unmigrated={result['source_record_store_unmigrated']}",
            f"source_records.removed={result['source_records_removed']}",
            "source_records.removed_payload_bytes="
            f"{result['source_record_payload_bytes_removed']}",
            "source_records.maintenance_failed="
            f"{result['source_record_maintenance_failed']}",
            f"source_records.checkpoint_busy={result['source_record_checkpoint_busy']}",
        ]
        for phase, measurements in (
            ("before", result["source_record_physical_before"]),
            ("after", result["source_record_physical_after"]),
        ):
            for name, size in (measurements or {}).items():
                lines.append(f"source_records.physical_{phase}.{name}={size}")
    lines.append(f"dry_run={dry_run}")
    return lines


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
    for line in prune_report_lines(
        cache_root,
        result,
        max_gb=max_gb,
        max_age_days=max_age_days,
        dry_run=args.dry_run,
    ):
        print(line)
    if result["status"] != "completed":
        return 0
    return 1 if result["entries_failed"] else 0


if __name__ == "__main__":
    raise SystemExit(main())
