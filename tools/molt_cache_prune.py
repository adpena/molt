#!/usr/bin/env python3
from __future__ import annotations

import argparse
import shutil
import sys
import time
from dataclasses import dataclass
from pathlib import Path


def _default_cache_root() -> Path:
    import os

    raw = os.environ.get("MOLT_CACHE")
    if raw:
        path = Path(raw).expanduser()
        if not path.is_absolute():
            return (Path.cwd() / path).resolve()
        return path
    external = Path("/Volumes/APDataStore/Molt")
    if external.is_dir():
        return external / "molt_cache"
    if sys.platform == "darwin":
        return Path.home() / "Library" / "Caches" / "molt"
    xdg = os.environ.get("XDG_CACHE_HOME")
    if xdg:
        path = Path(xdg).expanduser()
        if not path.is_absolute():
            path = (Path.cwd() / path).resolve()
        return path / "molt"
    return Path.home() / ".cache" / "molt"


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
        try:
            stat = child.stat()
        except OSError:
            continue
        size = _entry_size_bytes(child)
        entries.append(CacheEntry(path=child, size_bytes=size, mtime=stat.st_mtime))
    return entries


def _remove_entry(path: Path, dry_run: bool) -> None:
    if dry_run:
        return
    if path.is_dir() and not path.is_symlink():
        shutil.rmtree(path, ignore_errors=True)
        return
    try:
        path.unlink(missing_ok=True)
    except OSError:
        pass


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
                _remove_entry(entry.path, dry_run)
                removed.append(entry)
            else:
                keep.append(entry)
        entries = keep

    total = sum(item.size_bytes for item in entries)
    if max_bytes is not None and max_bytes >= 0 and total > max_bytes:
        # Remove oldest entries first until total <= max_bytes.
        for entry in sorted(entries, key=lambda item: item.mtime):
            if total <= max_bytes:
                break
            _remove_entry(entry.path, dry_run)
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
