from __future__ import annotations

import argparse
import json
import sys
from pathlib import Path

from molt.symphony.paths import (
    symphony_taste_memory_distillations_dir,
    symphony_taste_memory_events_file,
    symphony_tool_promotion_distillations_dir,
    symphony_tool_promotion_events_file,
)
from molt.symphony.taste_memory import TasteMemoryStore
from molt.symphony.tool_promotion import ToolPromotionStore


def build_parser() -> argparse.ArgumentParser:
    parser = argparse.ArgumentParser(
        description="Inspect and distill Symphony tool-promotion candidates."
    )
    parser.add_argument(
        "--events-path",
        default=str(symphony_tool_promotion_events_file()),
        help="Tool-promotion events JSONL path.",
    )
    parser.add_argument(
        "--distillations-dir",
        default=str(symphony_tool_promotion_distillations_dir()),
        help="Tool-promotion distillation output directory.",
    )
    parser.add_argument(
        "--taste-events-path",
        default=str(symphony_taste_memory_events_file()),
        help="Taste-memory events JSONL path used to derive candidates.",
    )
    parser.add_argument(
        "--taste-distillations-dir",
        default=str(symphony_taste_memory_distillations_dir()),
        help="Taste-memory distillation directory.",
    )
    sub = parser.add_subparsers(dest="cmd", required=True)

    summary = sub.add_parser("summary")
    summary.add_argument("--limit", type=int, default=50)

    distill = sub.add_parser("distill")
    distill.add_argument("--limit", type=int, default=200)
    distill.add_argument("--min-success-count", type=int, default=3)
    return parser


def main(argv: list[str] | None = None) -> int:
    args = build_parser().parse_args(argv)
    promotion_store = ToolPromotionStore(
        events_path=Path(str(args.events_path)).expanduser().resolve(),
        distillations_dir=Path(str(args.distillations_dir)).expanduser().resolve(),
    )
    if args.cmd == "summary":
        print(
            json.dumps(
                {"events": promotion_store.load(limit=max(int(args.limit), 0))},
                indent=2,
                sort_keys=True,
            )
        )
        return 0

    taste_store = TasteMemoryStore(
        events_path=Path(str(args.taste_events_path)).expanduser().resolve(),
        distillations_dir=Path(str(args.taste_distillations_dir))
        .expanduser()
        .resolve(),
    )
    payload = promotion_store.distill_candidates(
        taste_rows=taste_store.load(limit=max(int(args.limit), 0)),
        limit=max(int(args.limit), 0),
        min_success_count=max(1, int(args.min_success_count)),
    )
    promotion_store.record(
        {
            "kind": "tool_promotion_distillation",
            "samples": payload["samples"],
            "candidate_count": payload["candidate_count"],
            "ready_candidate_count": payload["ready_candidate_count"],
            "manifest_count": (
                (payload.get("manifest_batch") or {}).get("manifest_count")
                if isinstance(payload.get("manifest_batch"), dict)
                else 0
            ),
            "path": payload["path"],
        }
    )
    print(json.dumps(payload, indent=2, sort_keys=True))
    return 0


if __name__ == "__main__":
    raise SystemExit(main(sys.argv[1:]))
