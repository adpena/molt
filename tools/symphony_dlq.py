from __future__ import annotations

import argparse
import json
import os
import subprocess
import sys
from pathlib import Path

from molt.symphony.dlq import DeadLetterQueue
from molt.symphony.paths import symphony_dlq_events_file


def build_parser() -> argparse.ArgumentParser:
    parser = argparse.ArgumentParser(
        description="Inspect and replay Symphony recursive-loop dead-letter items."
    )
    parser.add_argument(
        "--path",
        default=str(symphony_dlq_events_file()),
        help="Dead-letter queue JSONL path.",
    )
    sub = parser.add_subparsers(dest="cmd", required=True)

    summary = sub.add_parser("summary")
    summary.add_argument("--limit", type=int, default=200)

    replay = sub.add_parser("replay")
    replay.add_argument("--fingerprint", required=True)
    replay.add_argument("--dry-run", action="store_true")
    return parser


def _load_latest_by_fingerprint(
    queue: DeadLetterQueue, fingerprint: str
) -> dict[str, object] | None:
    rows = queue.load(limit=0)
    matches = [
        row for row in rows if str(row.get("fingerprint") or "") == fingerprint.strip()
    ]
    return matches[-1] if matches else None


def main(argv: list[str] | None = None) -> int:
    args = build_parser().parse_args(argv)
    path = Path(str(args.path)).expanduser().resolve()
    queue = DeadLetterQueue(path)
    if args.cmd == "summary":
        print(
            json.dumps(
                queue.summary(limit=max(int(args.limit), 0)), indent=2, sort_keys=True
            )
        )
        return 0
    row = _load_latest_by_fingerprint(queue, str(args.fingerprint))
    if row is None:
        print(
            json.dumps(
                {
                    "ok": False,
                    "error": "fingerprint_not_found",
                    "fingerprint": args.fingerprint,
                },
                indent=2,
                sort_keys=True,
            )
        )
        return 2
    command = row.get("command")
    if not isinstance(command, list) or not command:
        print(
            json.dumps(
                {"ok": False, "error": "missing_command", "row": row},
                indent=2,
                sort_keys=True,
            )
        )
        return 2
    if args.dry_run:
        replay_row = queue.append_replay_result(
            target_fingerprint=str(args.fingerprint),
            command=[str(part) for part in command],
            returncode=0,
            dry_run=True,
        )
        print(
            json.dumps(
                {"ok": True, "dry_run": True, "row": row, "replay_row": replay_row},
                indent=2,
                sort_keys=True,
            )
        )
        return 0
    proc = subprocess.run(
        [str(part) for part in command],
        cwd=Path.cwd(),
        env=os.environ.copy(),
        check=False,
    )
    replay_row = queue.append_replay_result(
        target_fingerprint=str(args.fingerprint),
        command=[str(part) for part in command],
        returncode=int(proc.returncode),
    )
    print(
        json.dumps(
            {
                "ok": proc.returncode == 0,
                "fingerprint": args.fingerprint,
                "returncode": int(proc.returncode),
                "command": command,
                "replay_row": replay_row,
            },
            indent=2,
            sort_keys=True,
        )
    )
    return int(proc.returncode)


if __name__ == "__main__":
    raise SystemExit(main(sys.argv[1:]))
