from __future__ import annotations

import argparse
import json
import os
import shutil
import sys
from pathlib import Path
from typing import Any

from molt.symphony.durable_memory import DurableMemoryStore


def _default_root() -> Path:
    ext_root = Path(
        str(os.environ.get("MOLT_EXT_ROOT") or "/Volumes/APDataStore/Molt")
    ).expanduser()
    return Path(
        str(
            os.environ.get("MOLT_SYMPHONY_DURABLE_ROOT")
            or (ext_root / "logs" / "symphony" / "durable_memory")
        )
    ).expanduser()


def _store(root: Path) -> DurableMemoryStore:
    return DurableMemoryStore(root=root, sync_interval_seconds=3600, max_queue=1024)


def _restore_backup(*, root: Path, backup_dir: Path) -> dict[str, Any]:
    if not backup_dir.exists() or not backup_dir.is_dir():
        return {"ok": False, "error": "backup_missing", "backup_dir": str(backup_dir)}
    restored: list[str] = []
    for name in ("events.jsonl", "events.duckdb", "events.parquet"):
        source = backup_dir / name
        if not source.exists():
            continue
        target = root / name
        target.parent.mkdir(parents=True, exist_ok=True)
        shutil.copy2(source, target)
        restored.append(name)
    return {"ok": True, "backup_dir": str(backup_dir), "restored": restored}


def build_parser() -> argparse.ArgumentParser:
    parser = argparse.ArgumentParser(
        description=(
            "Durable memory admin tool for Symphony backups, integrity checks, and "
            "restore/prune operations."
        )
    )
    parser.add_argument(
        "--root",
        default=str(_default_root()),
        help="Durable memory root path (default from MOLT_SYMPHONY_DURABLE_ROOT).",
    )
    sub = parser.add_subparsers(dest="cmd", required=True)

    backup = sub.add_parser("backup")
    backup.add_argument("--reason", default="manual")

    sub.add_parser("check")

    restore = sub.add_parser("restore")
    restore.add_argument(
        "--backup-dir",
        default=None,
        help="Backup directory to restore; defaults to latest backup in root/backups.",
    )

    prune = sub.add_parser("prune")
    prune.add_argument("--keep-latest", type=int, default=20)
    prune.add_argument("--max-age-days", type=int, default=30)

    sub.add_parser("summary")
    return parser


def main(argv: list[str] | None = None) -> int:
    args = build_parser().parse_args(argv)
    root = Path(args.root).expanduser()
    root.mkdir(parents=True, exist_ok=True)

    if args.cmd == "restore":
        backup_dir = args.backup_dir
        if backup_dir:
            selected = Path(backup_dir).expanduser()
            if not selected.is_absolute():
                selected = (Path.cwd() / selected).resolve()
        else:
            backups_root = root / "backups"
            candidates = (
                sorted(
                    [entry for entry in backups_root.iterdir() if entry.is_dir()],
                    reverse=True,
                )
                if backups_root.exists()
                else []
            )
            if not candidates:
                print(
                    json.dumps(
                        {
                            "ok": False,
                            "error": "no_backups",
                            "root": str(root),
                        },
                        indent=2,
                        sort_keys=True,
                    )
                )
                return 2
            selected = candidates[0]
        result = _restore_backup(root=root, backup_dir=selected)
        print(json.dumps(result, indent=2, sort_keys=True))
        return 0 if bool(result.get("ok")) else 2

    store = _store(root)
    try:
        if args.cmd == "backup":
            result = store.create_backup(reason=str(args.reason))
            print(json.dumps(result, indent=2, sort_keys=True))
            return 0 if bool(result.get("ok")) else 1
        if args.cmd == "check":
            result = store.run_integrity_check()
            print(json.dumps(result, indent=2, sort_keys=True))
            return 0 if bool(result.get("ok")) else 2
        if args.cmd == "prune":
            result = store.prune_backups(
                keep_latest=int(args.keep_latest),
                max_age_days=int(args.max_age_days),
            )
            print(json.dumps(result, indent=2, sort_keys=True))
            return 0 if bool(result.get("ok")) else 1
        if args.cmd == "summary":
            result = store.summary(limit=120)
            print(json.dumps(result, indent=2, sort_keys=True))
            return 0
        raise RuntimeError(f"unsupported command: {args.cmd}")
    finally:
        store.close()


if __name__ == "__main__":
    raise SystemExit(main(sys.argv[1:]))
