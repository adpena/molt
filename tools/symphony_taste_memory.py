from __future__ import annotations

import argparse
import json
import sys
from pathlib import Path

from molt.symphony.paths import (
    symphony_taste_memory_distillations_dir,
    symphony_taste_memory_events_file,
)
from molt.symphony.taste_memory import TasteMemoryStore


def build_parser() -> argparse.ArgumentParser:
    parser = argparse.ArgumentParser(
        description="Inspect and distill Symphony taste-memory signals."
    )
    parser.add_argument(
        "--events-path",
        default=str(symphony_taste_memory_events_file()),
        help="Taste-memory events JSONL path.",
    )
    parser.add_argument(
        "--distillations-dir",
        default=str(symphony_taste_memory_distillations_dir()),
        help="Taste-memory distillation output directory.",
    )
    sub = parser.add_subparsers(dest="cmd", required=True)

    summary = sub.add_parser("summary")
    summary.add_argument("--limit", type=int, default=50)

    distill = sub.add_parser("distill")
    distill.add_argument("--limit", type=int, default=200)
    return parser


def main(argv: list[str] | None = None) -> int:
    args = build_parser().parse_args(argv)
    store = TasteMemoryStore(
        events_path=Path(str(args.events_path)).expanduser().resolve(),
        distillations_dir=Path(str(args.distillations_dir)).expanduser().resolve(),
    )
    if args.cmd == "summary":
        print(json.dumps({"events": store.load(limit=max(int(args.limit), 0))}, indent=2, sort_keys=True))
        return 0
    print(json.dumps(store.distill_recent(limit=max(int(args.limit), 0)), indent=2, sort_keys=True))
    return 0


if __name__ == "__main__":
    raise SystemExit(main(sys.argv[1:]))
