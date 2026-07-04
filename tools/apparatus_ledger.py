#!/usr/bin/env python3
"""Apparatus compression-progress ledger — foundation doc 72.

Measures how well Molt's proof-queue apparatus has COMPRESSED its recurring
failures into interpretable rules (Schmidhuber: compression progress as the
intrinsic-reward signal; Rudin: the compressed form is a transparent rule, not a
black-box score), and surfaces the highest-value UN-compressed surprises as the
"curiosity queue" — the next deterministic diagnosis rules / gates to write.

The pact/comma-lab apparatus (see memory pact-apparatus-reference) realizes this
at scale: its Catalog #N registry is the compression ledger and preflight.py's
~295 STRICT gates are the compressed rules. This is Molt's bounded first cut.

A failure that RECURS with the same signature but is not yet compressed into a
rule/fix is pure drag on the 100-year work — every recurrence is re-paid as
manual log archaeology. The share of failure-mass carried by recurring, still-
uncompressed signatures is the apparatus's compression DEBT; driving it down is
compression PROGRESS. Read-only; reads the proof-queue SQLite DB + run logs.
"""
from __future__ import annotations

import argparse
import re
import sqlite3
from collections import defaultdict
from dataclasses import dataclass, field
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
DEFAULT_DB = ROOT / "logs" / "proof_queue" / "proof_queue.sqlite3"

# Normalization rules: strip the volatile tokens so two runs of the SAME failure
# collapse to one signature. Order matters (longest/most-specific first).
_NORMALIZERS: tuple[tuple[re.Pattern[str], str], ...] = (
    (re.compile(r"\b\d{8}T\d{6}\b"), "<ts>"),                 # queue run stamps
    (re.compile(r"\b[0-9a-f]{12,64}\b"), "<hash>"),           # git/obj hashes, run suffixes
    (re.compile(r"[A-Za-z]:[\\/][^\s:'\"]+"), "<path>"),      # windows abs paths
    (re.compile(r"/(?:Users|home|mnt|tmp)/[^\s:'\"]+"), "<path>"),  # posix abs paths
    (re.compile(r"\b\d+\.\d+s\b"), "<dur>"),                  # durations
    (re.compile(r":\d+:\d+\b"), ":<lc>"),                     # line:col
    (re.compile(r"\bpid[= ]?\d+\b", re.I), "pid=<n>"),
    (re.compile(r"\belapsed=\d+\b"), "elapsed=<n>"),
    (re.compile(r"\b\d{3,}\b"), "<n>"),                       # long bare integers
)

_ERROR_HINT = re.compile(
    r"(?:^|\W)(error\[?[A-Z]?\d*\]?|panicked|assertion|failed|FAILED|exception|"
    r"could not compile|linker|undefined reference|timeout|OOM|killed)",
    re.I,
)


def _normalize(line: str) -> str:
    s = line.strip()
    for pat, repl in _NORMALIZERS:
        s = pat.sub(repl, s)
    return s[:200]


def _signature(log_path: str) -> str | None:
    """The most error-like normalized line near the tail of a run log."""
    p = Path(log_path)
    if not p.is_absolute():
        p = ROOT / log_path
    try:
        lines = [ln for ln in p.read_text("utf-8", errors="replace").splitlines() if ln.strip()]
    except OSError:
        return None
    if not lines:
        return None
    tail = lines[-40:]
    for ln in reversed(tail):
        if _ERROR_HINT.search(ln):
            return _normalize(ln)
    return _normalize(tail[-1])


@dataclass
class SigStat:
    signature: str
    count: int = 0
    cost_s: float = 0.0
    examples: list[str] = field(default_factory=list)

    @property
    def interestingness(self) -> float:
        # Schmidhuber: worth compressing ∝ recurrence × cost. A one-off is not
        # interesting; a cheap-but-frequent or costly-and-frequent class is.
        return self.count * (1.0 + self.cost_s)


def build_ledger(db: Path) -> tuple[list[SigStat], dict[str, int]]:
    conn = sqlite3.connect(str(db))
    conn.row_factory = sqlite3.Row
    rows = conn.execute(
        "SELECT run_id, status, elapsed_s, log_path FROM proof_runs "
        "WHERE status IN ('failed','stale')"
    ).fetchall()
    by_sig: dict[str, SigStat] = {}
    counts = {"failed_rows": 0, "no_signature": 0, "distinct_signatures": 0}
    for r in rows:
        counts["failed_rows"] += 1
        sig = _signature(r["log_path"]) if r["log_path"] else None
        if sig is None:
            counts["no_signature"] += 1
            continue
        st = by_sig.setdefault(sig, SigStat(sig))
        st.count += 1
        st.cost_s += float(r["elapsed_s"] or 0.0)
        if len(st.examples) < 3:
            st.examples.append(str(r["run_id"]))
    counts["distinct_signatures"] = len(by_sig)
    ranked = sorted(by_sig.values(), key=lambda s: -s.interestingness)
    return ranked, counts


def main(argv: list[str] | None = None) -> int:
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("--db", type=Path, default=DEFAULT_DB)
    ap.add_argument("--recurring-threshold", type=int, default=3,
                    help="a signature seen >= this many times is 'recurring' = compression debt")
    ap.add_argument("--top", type=int, default=15, help="curiosity-queue length")
    args = ap.parse_args(argv)

    if not args.db.exists():
        print(f"apparatus_ledger: no proof-queue DB at {args.db}")
        return 0

    ranked, counts = build_ledger(args.db)
    total_fail_mass = sum(s.count for s in ranked)
    recurring = [s for s in ranked if s.count >= args.recurring_threshold]
    recurring_mass = sum(s.count for s in recurring)
    # Compression PROGRESS proxy: 1 - (failure-mass in recurring-uncompressed
    # signatures / total failure-mass). Rising over time = the apparatus is
    # retiring its recurring drag into rules/fixes so it stops recurring.
    debt = (recurring_mass / total_fail_mass) if total_fail_mass else 0.0

    print("=== Molt apparatus compression-progress ledger (doc 72) ===")
    print(f"failed/stale rows scanned : {counts['failed_rows']}")
    print(f"  no extractable signature: {counts['no_signature']}")
    print(f"distinct failure signatures: {counts['distinct_signatures']}")
    print(f"recurring (>= {args.recurring_threshold}x) signatures : {len(recurring)}"
          f"  carrying {recurring_mass}/{total_fail_mass} of failure-mass")
    print(f"COMPRESSION DEBT (recurring-uncompressed share): {debt:.1%}   "
          f"(lower = more compressed; drive to 0 by writing rules/fixes)")
    print()
    print(f"=== CURIOSITY QUEUE — top {args.top} surprises to compress into rules/gates ===")
    print("(Schmidhuber: rank = recurrence × cost. Each recurring row is a diagnosis")
    print(" rule or fix waiting to be written — the pact Catalog #N move.)")
    for i, s in enumerate(ranked[: args.top], 1):
        flag = "RECURRING" if s.count >= args.recurring_threshold else "one-off  "
        print(f"{i:2d}. [{flag} x{s.count:<3d} cost={s.cost_s:6.0f}s] {s.signature}")
        print(f"      e.g. {', '.join(s.examples)}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
