#!/usr/bin/env python3
"""memory-graph-integrity check (APPARATUS A10).

The memory corpus is a ``[[wikilink]]`` graph with an ``M##`` hook index resolved
by ``POINTERS.md``. Two integrity invariants keep it navigable by machine:

  1. Every ``[[wikilink]]`` either resolves to a real node OR is an intentional
     DANGLING forward-reference. The memory system EXPLICITLY ALLOWS danglers
     ("worth writing later" -- MEMORY.md's own convention), so this check REPORTS
     them; it never fails on them.
  2. Every ``M##`` referenced by the ``MEMORY.md`` hook index resolves via
     ``POINTERS.md`` (a row -- a file-backed topic OR an ``(inline note)``). An
     ``M##`` in MEMORY.md with no POINTERS row is a real index break.

LIFECYCLE (A9). This lands WARN-ONLY: ``--check`` prints the report and ALWAYS
exits 0. The NAMED flip condition is recorded as a ``[[gate_flip]]`` entry in
``tools/molt_dev_gates.toml`` (``name = "memory_graph_integrity"``). Its
``strict_when`` is deliberately a *manual-review* (free-text) token, not a machine
``live_count == 0``: the live corpus lives in Claude's auto-memory dir, which CI
cannot see (``discover_memory_dir`` returns ``None`` in CI -> 0/0), so only the
operator, looking at the real corpus, can decide to tighten. ``--strict`` (the
post-flip mode) exits 1 when any MEMORY.md ``M##`` is unresolved (danglers stay
warn-only even then).

Run::

    python tools/check_memory_graph.py            # warn-only report, exit 0
    python tools/check_memory_graph.py --check    # same (ci_gate mode), exit 0
    python tools/check_memory_graph.py --strict   # exit 1 if unresolved M## > 0
    python tools/check_memory_graph.py --count     # integer unresolved-M## count
    python tools/check_memory_graph.py --json
"""

from __future__ import annotations

import argparse
import json
import sys
from pathlib import Path

_THIS = Path(__file__).resolve()
REPO_ROOT = _THIS.parents[1]
if str(REPO_ROOT) not in sys.path:
    sys.path.insert(0, str(REPO_ROOT))

from tools import memory_graph  # noqa: E402


def _force_utf8() -> None:
    try:
        from tools._io_utf8 import force_utf8_stdio

        force_utf8_stdio()
    except Exception:
        for stream in (sys.stdout, sys.stderr):
            rc = getattr(stream, "reconfigure", None)
            if rc:
                try:
                    rc(encoding="utf-8", errors="backslashreplace")
                except Exception:
                    pass


def analyze(*, memory_dir: Path | None = None, repo_root: Path | None = None) -> dict:
    """Build the graph and return the integrity report. Never raises."""
    try:
        graph = memory_graph.build_graph(
            memory_dir=memory_dir, repo_root=repo_root or REPO_ROOT
        )
    except Exception as exc:  # a broken build must degrade, not crash CI
        return {
            "corpus_found": False,
            "error": f"{type(exc).__name__}: {exc}",
            "dangling_count": 0,
            "unresolved_mref_count": 0,
            "dangling": [],
            "unresolved_mrefs": [],
            "counts": {},
        }
    dangling = graph.dangling_links()
    unresolved = graph.unresolved_mrefs()
    return {
        "corpus_found": graph.memory_dir is not None,
        "memory_dir": str(graph.memory_dir) if graph.memory_dir else None,
        "dangling_count": len(dangling),
        "unresolved_mref_count": len(unresolved),
        "dangling": [
            {"src": e.src, "dst": e.dst, "type": e.type, "source": e.source}
            for e in dangling
        ],
        "unresolved_mrefs": unresolved,
        "counts": graph.counts(),
        "warnings": graph.warnings[:50],
    }


def _print_report(report: dict) -> None:
    if not report.get("corpus_found"):
        err = report.get("error")
        if err:
            print(
                f"memory-graph-integrity: corpus build error ({err}); nothing to check."
            )
        else:
            print(
                "memory-graph-integrity: no memory corpus discovered "
                "(MOLT_MEMORY_DIR / <repo>/memory / ~/.claude auto-memory). "
                "Nothing to check -- OK (CI has no corpus)."
            )
        return
    c = report["counts"]
    print(
        f"memory-graph-integrity: {c.get('nodes_total', 0)} nodes / "
        f"{c.get('edges_total', 0)} edges from {report.get('memory_dir')}"
    )
    print(
        f"  dangling [[wikilinks]] (ALLOWED, worth-writing-later): "
        f"{report['dangling_count']}"
    )
    for e in report["dangling"][:40]:
        print(f"    {e['src']} -[{e['type']}]-> [[{e['dst']}]]  ({e['source']})")
    if report["dangling_count"] > 40:
        print(f"    ... and {report['dangling_count'] - 40} more")
    n = report["unresolved_mref_count"]
    if n == 0:
        print("  unresolved M## (MEMORY.md not resolved by POINTERS.md): 0  [OK]")
    else:
        print(
            f"  unresolved M## (MEMORY.md not resolved by POINTERS.md): {n}  "
            f"[BREAK] -> {', '.join(report['unresolved_mrefs'])}"
        )


def main(argv: list[str] | None = None) -> int:
    _force_utf8()
    ap = argparse.ArgumentParser(prog="check_memory_graph", description=__doc__)
    ap.add_argument("--memory-dir", default=None, help="override corpus dir (tests)")
    ap.add_argument(
        "--check",
        action="store_true",
        help="warn-only report (ci_gate mode); ALWAYS exits 0",
    )
    ap.add_argument(
        "--strict",
        action="store_true",
        help="post-flip mode: exit 1 if any MEMORY.md M## is unresolved",
    )
    ap.add_argument(
        "--count",
        action="store_true",
        help="print ONLY the unresolved-M## integer (the [[gate_flip]] live_count)",
    )
    ap.add_argument("--json", action="store_true")
    args = ap.parse_args(argv)

    report = analyze(memory_dir=Path(args.memory_dir) if args.memory_dir else None)

    if args.count:
        # Integer-only stdout for the [[gate_flip]] count_cmd (live_count).
        print(report["unresolved_mref_count"])
        return 0
    if args.json:
        print(json.dumps(report, indent=2))
    else:
        _print_report(report)

    # Danglers NEVER fail (explicitly allowed). Only --strict fails, and only on
    # a genuine index break (unresolved M##).
    if args.strict and report["unresolved_mref_count"] > 0:
        return 1
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
