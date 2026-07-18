#!/usr/bin/env python3
from __future__ import annotations

import argparse
import os
import shutil
import sys
import time
from dataclasses import dataclass
from pathlib import Path

TOOLS_ROOT = Path(__file__).resolve().parent
SRC_ROOT = TOOLS_ROOT.parent / "src"
if str(SRC_ROOT) not in sys.path:
    sys.path.insert(0, str(SRC_ROOT))

from molt.cli.build_locks import (  # noqa: E402
    _release_file_lock,
    _try_acquire_file_lock,
)
from molt.cli.default_paths import _default_molt_cache  # noqa: E402
from molt.cli.wasm_link_cache import (  # noqa: E402
    WASM_LINK_CACHE_DIRECTORY,
    WASM_LINK_CACHE_FAMILIES,
)


def _default_cache_root() -> Path:
    return _default_molt_cache()


def _is_external_volume(path: Path) -> bool:
    try:
        resolved = path.resolve()
    except OSError:
        resolved = path
    return str(resolved).startswith("/Volumes/APDataStore/")


@dataclass
class CacheEntry:
    path: Path
    size_bytes: int
    mtime: float
    lock_path: Path | None = None


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


def _collect_entries(cache_root: Path) -> list[CacheEntry]:
    entries: list[CacheEntry] = []
    if not cache_root.exists():
        return entries
    for child in cache_root.iterdir():
        if child.name == WASM_LINK_CACHE_DIRECTORY and child.is_dir():
            known_families: set[Path] = set()
            for family_name in sorted(WASM_LINK_CACHE_FAMILIES):
                family_root = child / family_name
                if not family_root.is_dir():
                    continue
                known_families.add(family_root)
                for schema_root in family_root.iterdir():
                    if not schema_root.is_dir() or schema_root.name == ".locks":
                        continue
                    lock_root = family_root / ".locks"
                    for entry_root in schema_root.iterdir():
                        if not entry_root.is_dir() or entry_root.name == ".locks":
                            continue
                        try:
                            stat = entry_root.stat()
                        except OSError:
                            continue
                        entries.append(
                            CacheEntry(
                                path=entry_root,
                                size_bytes=_entry_size_bytes(entry_root),
                                mtime=stat.st_mtime,
                                lock_path=lock_root / f"{entry_root.name[:2]}.lock",
                            )
                        )
            for unknown in child.iterdir():
                if unknown in known_families:
                    continue
                try:
                    stat = unknown.stat()
                except OSError:
                    continue
                entries.append(
                    CacheEntry(
                        path=unknown,
                        size_bytes=_entry_size_bytes(unknown),
                        mtime=stat.st_mtime,
                    )
                )
            continue
        try:
            stat = child.stat()
        except OSError:
            continue
        size = _entry_size_bytes(child)
        entries.append(CacheEntry(path=child, size_bytes=size, mtime=stat.st_mtime))
    return entries


def _remove_entry(entry: CacheEntry, dry_run: bool) -> bool:
    lock_handle = None
    if entry.lock_path is not None:
        lock_handle = _try_acquire_file_lock(entry.lock_path)
        if lock_handle is None:
            return False
    try:
        if dry_run:
            return True
        if entry.path.is_dir() and not entry.path.is_symlink():
            shutil.rmtree(entry.path, ignore_errors=False)
        else:
            entry.path.unlink(missing_ok=True)
        return not entry.path.exists()
    except OSError:
        return False
    finally:
        if lock_handle is not None:
            _release_file_lock(lock_handle)


def _eviction_order(entry: CacheEntry) -> tuple[float, str]:
    normalized_path = os.path.normcase(os.path.normpath(os.fspath(entry.path)))
    return entry.mtime, normalized_path


def _prune(
    cache_root: Path,
    *,
    max_bytes: int | None,
    max_age_days: int | None,
    dry_run: bool,
) -> dict[str, object]:
    entries = _collect_entries(cache_root)
    removed: list[CacheEntry] = []
    now = time.time()

    if max_age_days is not None and max_age_days >= 0:
        cutoff = now - (max_age_days * 86400)
        keep: list[CacheEntry] = []
        for entry in entries:
            if entry.mtime < cutoff:
                if _remove_entry(entry, dry_run):
                    removed.append(entry)
                else:
                    keep.append(entry)
            else:
                keep.append(entry)
        entries = keep

    total = sum(item.size_bytes for item in entries)
    if max_bytes is not None and max_bytes >= 0 and total > max_bytes:
        # Remove oldest entries first until total <= max_bytes.
        for entry in sorted(entries, key=_eviction_order):
            if total <= max_bytes:
                break
            if _remove_entry(entry, dry_run):
                removed.append(entry)
                total -= entry.size_bytes

    removed_bytes = sum(item.size_bytes for item in removed)
    return {
        "cache_root": str(cache_root),
        "entries_removed": len(removed),
        "bytes_removed": removed_bytes,
        "bytes_removed_human": _format_bytes(removed_bytes),
    }


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
        help="Delete top-level cache entries older than this age (days).",
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
    cache_root.mkdir(parents=True, exist_ok=True)

    max_gb = args.max_gb
    max_age_days = args.max_age_days
    if max_gb is None:
        max_gb = 200.0 if _is_external_volume(cache_root) else 30.0
    if max_age_days is None:
        max_age_days = 30

    max_bytes = int(max_gb * (1024**3))
    before_entries = _collect_entries(cache_root)
    before_bytes = sum(item.size_bytes for item in before_entries)
    result = _prune(
        cache_root,
        max_bytes=max_bytes,
        max_age_days=max_age_days,
        dry_run=args.dry_run,
    )
    after_entries = _collect_entries(cache_root)
    after_bytes = sum(item.size_bytes for item in after_entries)

    print(f"cache_root={cache_root}")
    print(f"policy.max_gb={max_gb}")
    print(f"policy.max_age_days={max_age_days}")
    print(f"before={_format_bytes(before_bytes)}")
    print(f"after={_format_bytes(after_bytes)}")
    print(f"removed.entries={result['entries_removed']}")
    print(f"removed.bytes={result['bytes_removed_human']}")
    print(f"dry_run={args.dry_run}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
