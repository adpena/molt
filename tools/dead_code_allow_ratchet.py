#!/usr/bin/env python3
"""Dead-code-allow ratchet — a metabug guard against silently-masked mechanisms.

THE METABUG CLASS THIS PREVENTS
-------------------------------
`cargo clippy -D warnings` already blocks *un-allowed* dead code. The remaining
hole is silencing a HALF-WIRED mechanism by adding `#[allow(dead_code)]` — a live
consumer reading state that only dead code produces, so a safety interlock ships
inert (real example: array_mod.rs `ArrayCell::exports` was READ by the live
`resize_blocked` but INCREMENTED only by `#[allow(dead_code)]` code, so
resize-while-exported UAF protection was silently off). An `#[allow(dead_code)]`
is a promise of "landed, wire later" that is easy to forget — this ratchet makes
ADDING one a reviewed event: the total may DECREASE freely (wire-or-delete is
always welcome) but may not INCREASE past the committed baseline without an
explicit, justified rebaseline. It forces the wire-or-delete decision instead of
a silent mask.

Usage:
    python tools/dead_code_allow_ratchet.py            # check against baseline
    python tools/dead_code_allow_ratchet.py --update   # rebaseline (review the drop/rise)

Exit: 0 = at-or-below baseline; 2 = INCREASED (new mask — wire/delete or justify);
3 = usage/IO error.
"""
from __future__ import annotations

import argparse
import json
from pathlib import Path
import re
import sys

ROOT = Path(__file__).resolve().parents[1]
BASELINE = Path(__file__).resolve().parent / "dead_code_allow_baseline.json"
# Roots scanned for Rust sources.
_SCAN_ROOTS = ("runtime", "src")
# Matches #[allow(dead_code)] and #![allow(dead_code)] and grouped allows that
# include dead_code, e.g. #[allow(dead_code, unused)].
_ALLOW_RE = re.compile(r"#!?\[\s*allow\s*\(([^)]*)\)\s*\]")


def _counts() -> dict[str, int]:
    """Per-crate-area dead_code-allow counts across the Rust tree."""
    counts: dict[str, int] = {}
    for root_name in _SCAN_ROOTS:
        root = ROOT / root_name
        if not root.is_dir():
            continue
        for rs in root.rglob("*.rs"):
            # skip vendored/target/generated trees
            parts = set(rs.parts)
            if "target" in parts or ".git" in parts:
                continue
            n = 0
            try:
                text = rs.read_text(encoding="utf-8", errors="replace")
            except OSError:
                continue
            for m in _ALLOW_RE.finditer(text):
                lints = {t.strip() for t in m.group(1).split(",")}
                if "dead_code" in lints:
                    n += 1
            if n:
                # bucket by the crate dir (first 3 path components under ROOT)
                rel = rs.relative_to(ROOT)
                area = "/".join(rel.parts[: min(2, len(rel.parts) - 1)]) or rel.parts[0]
                counts[area] = counts.get(area, 0) + n
    return counts


def _total(counts: dict[str, int]) -> int:
    return sum(counts.values())


def main(argv: list[str] | None = None) -> int:
    ap = argparse.ArgumentParser(description=__doc__)
    ap.add_argument("--update", action="store_true", help="rebaseline to the current count")
    args = ap.parse_args(argv)

    counts = _counts()
    total = _total(counts)

    if args.update:
        BASELINE.write_text(
            json.dumps({"total": total, "by_area": dict(sorted(counts.items()))}, indent=2)
            + "\n",
            encoding="utf-8",
        )
        print(f"dead_code_allow_ratchet: baseline updated to total={total}")
        return 0

    if not BASELINE.is_file():
        print(
            "dead_code_allow_ratchet: no baseline — run --update once to establish it.",
            file=sys.stderr,
        )
        return 3
    try:
        base = json.loads(BASELINE.read_text(encoding="utf-8"))
    except (OSError, ValueError) as exc:
        print(f"dead_code_allow_ratchet: cannot read baseline: {exc}", file=sys.stderr)
        return 3

    base_total = int(base.get("total", 0))
    if total <= base_total:
        note = "" if total == base_total else f" (down {base_total - total} — nice)"
        print(f"dead_code_allow_ratchet: PASS — {total} <= baseline {base_total}{note}")
        return 0

    print(
        f"dead_code_allow_ratchet: FAIL — #[allow(dead_code)] rose {base_total} -> {total} "
        f"(+{total - base_total}).",
        file=sys.stderr,
    )
    base_by = base.get("by_area", {})
    for area in sorted(set(counts) | set(base_by)):
        b, c = int(base_by.get(area, 0)), counts.get(area, 0)
        if c > b:
            print(f"    {area}: {b} -> {c}  (+{c - b})", file=sys.stderr)
    print(
        "\nA new #[allow(dead_code)] usually masks a HALF-WIRED mechanism (a live "
        "consumer of state only dead code produces). WIRE it into a live path or "
        "DELETE it. If the allow is genuinely justified (cfg-gated, trait-required), "
        "rebaseline with `python tools/dead_code_allow_ratchet.py --update` and "
        "explain in the commit.",
        file=sys.stderr,
    )
    return 2


if __name__ == "__main__":
    raise SystemExit(main())
