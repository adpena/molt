#!/usr/bin/env python3
"""Fail-closed parity guard for runtime stdlib in-tree <-> satellite
module pairs.

Background (the P0 this guard exists to keep extinct)
---------------------------------------------
Molt used to ship feature-gated stdlib modules in two physical copies:

  * an IN-TREE copy under runtime/molt-runtime/src/builtins/<mod>.rs, gated
    `#[cfg(not(feature = "stdlib_X"))]`, which was the SOLE compiled source for
    reduced build tiers, and
  * a SATELLITE copy under runtime/molt-runtime-X/src/<mod>.rs, which is the
    compiled source for full native builds.

Those copies were not one logic in two namespaces. They were two runtime-access
implementations of the same behavior, so a behavioral fix could land in only one
copy and make SHIPPED BEHAVIOR DIFFER BY BUILD TIER. The decomposition program
has now removed every tracked dual-authority pair; PAIRS is intentionally empty
and the committed ratchet ceiling is zero. Keeping this guard alive makes any
future reintroduction of a two-copy fallback an explicit, fail-closed authority
decision instead of silent drift.

What this guard does
--------------------
For each listed pair it NORMALIZES away the by-design access-layer differences
(imports, doc comments, the GIL macro/token, bridge path prefixes, single-line
`unsafe {}` wrappers, trailing comments) and then compares the residual
line-MULTISET (sorted, so pure reordering is ignored). The residual is the
genuine semantic-drift surface.

It is a CONTRACT, not a sync script: it never edits source. The committed
baseline records the allowed residual size and content hash per pair. With the
current empty PAIRS table, the expected total is zero. The guard FAILS (exit 1)
when, for any pair:

  * the residual count EXCEEDS the baselined count (NEW drift), or
  * the residual content HASH differs from the baseline while the count is
    unchanged (drift that swapped one divergence for another), or
  * a pair is missing/unreadable, or
  * the baseline total exceeds the committed ratchet ceiling RATCHET_CEILING.

Reconciling a pair shrinks its residual; regenerating the baseline with
`--update-baseline` can only lower RATCHET_CEILING (the guard refuses to raise
it). Now that the ceiling is zero, any new drift is a hard test failure.

Usage
-----
  python3 tools/check_satellite_parity.py            # check against baseline
  python3 tools/check_satellite_parity.py --verbose  # + per-pair residual sizes
  python3 tools/check_satellite_parity.py --show PAIR # print a pair's residual
  python3 tools/check_satellite_parity.py --update-baseline   # regenerate
"""

from __future__ import annotations

import argparse
import hashlib
import json
import re
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
RUNTIME = ROOT / "runtime"
INTREE_DIR = RUNTIME / "molt-runtime" / "src"

# The remaining feature-gated in-tree <-> satellite module pairs. This is empty
# by design: reduced builds now either compile leaf-owned satellite source by
# direct include or have no fallback lane. Adding a pair here is an explicit
# declaration that a second physical authority exists and must be ratcheted back
# to zero.
PAIRS: dict[str, tuple[str, str]] = {}

# --- access-layer normalization (must stay byte-for-byte in sync with the
#     reconciliation audit normalizer; this is the committed source of truth) ---

GIL_MACROS = [
    "crate::with_gil_entry_nopanic!",
    "with_gil_entry_nopanic!",
    "with_core_gil!",
    "molt_runtime_core::with_gil_entry!",
    "with_gil_entry!",
]
# Longest/most-specific path prefixes first.
PREFIXES = [
    "crate::bridge::",
    "bridge::",
    "molt_runtime_core::ffi::",
    "molt_runtime_core::",
    "object::ops_hash::",
    "builtins::attr::",
    "object::type_ids::",
    "crate::",
]
TOKEN_TYPES = ["CoreGilToken", "PyToken<'_>", "PyToken<'a>", "PyToken"]

# Access-layer-equivalent runtime calls: the in-tree copy calls a `molt-runtime`
# helper DIRECTLY (threading the `PyToken` GIL token); the satellite reaches the
# same behavior through a `molt-runtime-core` `rt_*` FFI-bridge wrapper that
# acquires the GIL internally and so takes no token. They are the same operation
# in the two access layers (exactly like the GIL-macro / token normalizations).
# Map both spellings to a common token so the residual reflects only genuine
# SEMANTIC drift, not the bridge shape. Order longest-first.
RT_WRAPPER_EQUIVALENTS = [
    ("object::ops_sys::runtime_target_at_least(_py, ", "__RT_TARGET_AT_LEAST__("),
    ("runtime_target_at_least(_py, ", "__RT_TARGET_AT_LEAST__("),
    ("rt_target_at_least(", "__RT_TARGET_AT_LEAST__("),
    ("py_hash_inf()", "__NUMERIC_HASH_INF__"),
    ("PY_HASH_INF", "__NUMERIC_HASH_INF__"),
]


def _strip_use_blocks(lines: list[str]) -> list[str]:
    """Drop `use ...;` items, including multi-line brace-balanced blocks."""
    out: list[str] = []
    i, n = 0, len(lines)
    while i < n:
        s = lines[i].strip()
        if s.startswith("use ") or s.startswith("pub use "):
            depth, j = 0, i
            while j < n:
                for ch in lines[j]:
                    if ch == "{":
                        depth += 1
                    elif ch == "}":
                        depth -= 1
                if depth <= 0 and lines[j].rstrip().endswith(";"):
                    break
                j += 1
            i = j + 1
            continue
        out.append(lines[i])
        i += 1
    return out


def _strip_trailing_comment(line: str) -> str:
    """Strip a trailing `// ...` comment that is not inside a string literal."""
    in_str = esc = False
    i, n = 0, len(line)
    while i < n:
        c = line[i]
        if in_str:
            if esc:
                esc = False
            elif c == "\\":
                esc = True
            elif c == '"':
                in_str = False
        else:
            if c == '"':
                in_str = True
            elif c == "/" and i + 1 < n and line[i + 1] == "/":
                return line[:i].rstrip()
        i += 1
    return line


def _strip_cfg_test_items(lines: list[str]) -> list[str]:
    """Drop Rust items guarded by `#[cfg(test)]`.

    The satellite parity guard compares shipped runtime implementation, not test
    code. Unit tests can differ between access layers without changing build-tier
    behavior, and Cargo remains the authority for compiling/executing them.
    """
    out: list[str] = []
    i, n = 0, len(lines)
    while i < n:
        if lines[i].strip() != "#[cfg(test)]":
            out.append(lines[i])
            i += 1
            continue

        i += 1
        while i < n and lines[i].strip().startswith("#["):
            i += 1
        if i >= n:
            break

        depth = 0
        saw_brace = False
        while i < n:
            line = lines[i]
            for ch in line:
                if ch == "{":
                    depth += 1
                    saw_brace = True
                elif ch == "}":
                    depth -= 1
            i += 1
            if (saw_brace and depth <= 0) or (
                not saw_brace and line.strip().endswith(";")
            ):
                break
    return out


def normalize(path: Path) -> list[str]:
    raw = path.read_text(encoding="utf-8").splitlines()
    raw = _strip_cfg_test_items(raw)
    raw = _strip_use_blocks(raw)
    out: list[str] = []
    for line in raw:
        line = line.rstrip()
        s = line.strip()
        if s == "" or s.startswith("//") or s.startswith("#!["):
            continue
        if s.startswith("/*") and s.endswith("*/"):
            continue
        for m in GIL_MACROS:
            line = line.replace(m, "__GIL__!")
        for t in TOKEN_TYPES:
            line = line.replace(t, "__TOK__")
        for p in PREFIXES:
            line = line.replace(p, "")
        for src, dst in RT_WRAPPER_EQUIVALENTS:
            line = line.replace(src, dst)
        line = re.sub(
            r"is_truthy\(_py,\s*obj_from_bits\(molt_is_callable\(([^)]*)\)\)\)",
            r"molt_is_callable(\1)",
            line,
        )
        line = _strip_trailing_comment(line)
        s2 = line.strip()
        # Collapse a single-line `unsafe { EXPR }` / `unsafe { EXPR };` wrapper:
        # the satellite must wrap each extern-C bridge call in `unsafe {}` while
        # the in-tree direct call is safe. Same EXPR, same effect.
        if s2.startswith("unsafe {") and s2.endswith("};"):
            inner = s2[len("unsafe {") : -2].strip()
            if inner and "{" not in inner:
                s2 = inner + ";"
        elif s2.startswith("unsafe {") and s2.endswith("}"):
            inner = s2[len("unsafe {") : -1].strip()
            if inner and "{" not in inner:
                s2 = inner
        if s2 == "":
            continue
        out.append(s2)
    return out


def _intree_path(name: str) -> Path:
    intree_rel, _sat_rel = PAIRS[name]
    return INTREE_DIR / intree_rel


def residual(name: str) -> tuple[list[str], str]:
    """Return (residual_lines, sha256) for a pair. The residual is the sorted
    `diff`-style symmetric difference of the two normalized line multisets.
    """
    a_path = _intree_path(name)
    _intree_rel, sat_rel = PAIRS[name]
    b_path = RUNTIME / sat_rel
    if not a_path.exists():
        raise FileNotFoundError(f"in-tree copy missing: {a_path}")
    if not b_path.exists():
        raise FileNotFoundError(f"satellite copy missing: {b_path}")
    a = normalize(a_path)
    b = normalize(b_path)
    # Multiset symmetric difference: count occurrences, emit the surplus on each
    # side. Sorting makes the result order-independent and stable.
    from collections import Counter

    ca, cb = Counter(a), Counter(b)
    only_a = sorted((ca - cb).elements())
    only_b = sorted((cb - ca).elements())
    lines = [f"< {ln}" for ln in only_a] + [f"> {ln}" for ln in only_b]
    digest = hashlib.sha256("\n".join(lines).encode("utf-8")).hexdigest()
    return lines, digest


BASELINE_PATH = Path(__file__).resolve().parent / "satellite_parity_baseline.json"


def load_baseline() -> dict:
    if not BASELINE_PATH.exists():
        return {"ratchet_ceiling": None, "pairs": {}}
    return json.loads(BASELINE_PATH.read_text(encoding="utf-8"))


def compute_all() -> dict[str, tuple[list[str], str]]:
    return {name: residual(name) for name in PAIRS}


def cmd_update_baseline() -> int:
    results = compute_all()
    pairs = {
        name: {"count": len(lines), "sha256": digest}
        for name, (lines, digest) in sorted(results.items())
    }
    total = sum(p["count"] for p in pairs.values())
    prev = load_baseline()
    prev_ceiling = prev.get("ratchet_ceiling")
    # The ceiling is a one-way ratchet: it may only decrease (or be set the
    # first time). Refuse to raise it — raising it would re-admit drift.
    if prev_ceiling is not None and total > prev_ceiling:
        print(
            f"REFUSING to raise ratchet ceiling: new total {total} > "
            f"committed ceiling {prev_ceiling}.\n"
            "New drift was introduced. Reconcile the pair instead of widening "
            "the baseline (the baseline is a one-way ratchet toward zero).",
            file=sys.stderr,
        )
        return 1
    baseline = {
        "_comment": (
            "Fail-closed parity baseline for the in-tree<->satellite stdlib "
            "pairs. Generated by tools/check_satellite_parity.py "
            "--update-baseline. ratchet_ceiling may only DECREASE: reconciling "
            "a pair shrinks its residual and lowers the ceiling; the guard "
            "refuses to raise it. See the script docstring and "
            "memory/recovery/baton_move_R_satellite_drift.md."
        ),
        "ratchet_ceiling": total,
        "pairs": pairs,
    }
    BASELINE_PATH.write_text(
        json.dumps(baseline, indent=2, sort_keys=False) + "\n", encoding="utf-8"
    )
    print(
        f"baseline updated: {len(pairs)} pairs, total residual {total}, ceiling {total}"
    )
    return 0


def cmd_check(verbose: bool) -> int:
    baseline = load_baseline()
    base_pairs = baseline.get("pairs", {})
    ceiling = baseline.get("ratchet_ceiling")
    failures: list[str] = []
    total = 0
    rows: list[tuple[str, int, int]] = []
    for name in PAIRS:
        try:
            lines, digest = residual(name)
        except FileNotFoundError as exc:
            failures.append(f"[{name}] {exc}")
            continue
        total += len(lines)
        base = base_pairs.get(name)
        if base is None:
            failures.append(
                f"[{name}] pair has no baseline entry (residual={len(lines)}). "
                "Run --update-baseline after confirming the pair is intentional."
            )
            rows.append((name, len(lines), -1))
            continue
        rows.append((name, len(lines), base["count"]))
        if len(lines) > base["count"]:
            failures.append(
                f"[{name}] NEW DRIFT: normalized residual grew "
                f"{base['count']} -> {len(lines)}. A behavioral change landed in "
                f"only one of the two copies. Port it to the other copy (with a "
                f"differential test), or run --show {name} to inspect."
            )
        elif digest != base["sha256"]:
            # Same count, different content: a divergence was swapped for a new
            # one. Treat as drift (fail-closed); reconcile + regenerate baseline.
            failures.append(
                f"[{name}] DRIFT (content changed, count unchanged at "
                f"{len(lines)}): the set of divergent lines changed. "
                f"Run --show {name}; reconcile and --update-baseline."
            )
    if ceiling is not None and total > ceiling:
        failures.append(
            f"baseline total residual {total} exceeds committed ratchet ceiling "
            f"{ceiling}. New drift was introduced."
        )
    if verbose:
        print(f"{'pair':<22} {'residual':>9} {'baseline':>9}")
        for name, cur, base in rows:
            flag = "" if base >= 0 and cur <= base else "  <-- CHECK"
            print(f"{name:<22} {cur:>9} {base:>9}{flag}")
        print(f"{'TOTAL':<22} {total:>9} {ceiling if ceiling is not None else '-':>9}")
    if failures:
        print("\nSATELLITE PARITY GUARD FAILED:\n", file=sys.stderr)
        for f in failures:
            print(f"  - {f}", file=sys.stderr)
        print(
            "\nThe in-tree and satellite copies of a stdlib module diverged "
            "beyond the committed baseline. This means shipped behavior now "
            "DIFFERS BY BUILD TIER (reduced tiers compile the in-tree copy; the "
            "default native build compiles the satellite). Reconcile the drift "
            "in BOTH copies and re-run --update-baseline.\n",
            file=sys.stderr,
        )
        return 1
    print(
        f"satellite parity OK: {len(PAIRS)} pairs within baseline "
        f"(total normalized residual {total}, ceiling {ceiling})."
    )
    return 0


def cmd_show(name: str) -> int:
    if name not in PAIRS:
        print(f"unknown pair '{name}'. Known: {', '.join(PAIRS)}", file=sys.stderr)
        return 2
    lines, digest = residual(name)
    intree, sat = PAIRS[name]
    print(f"# pair {name}")
    print(
        f"#   in-tree:   runtime/molt-runtime/src/{_intree_path(name).relative_to(INTREE_DIR)}"
    )
    print(f"#   satellite: runtime/{sat}")
    print(f"#   residual lines: {len(lines)}  sha256: {digest}")
    print("#   ('<' = only in-tree, '>' = only satellite, both normalized)")
    for ln in lines:
        print(ln)
    return 0


def main(argv: list[str]) -> int:
    ap = argparse.ArgumentParser(description=__doc__)
    ap.add_argument(
        "--update-baseline",
        action="store_true",
        help="regenerate the committed baseline (ratchet ceiling may only decrease)",
    )
    ap.add_argument(
        "--verbose", action="store_true", help="print per-pair residual sizes"
    )
    ap.add_argument(
        "--show", metavar="PAIR", help="print one pair's normalized residual diff"
    )
    args = ap.parse_args(argv)
    if args.show:
        return cmd_show(args.show)
    if args.update_baseline:
        return cmd_update_baseline()
    return cmd_check(args.verbose)


if __name__ == "__main__":
    raise SystemExit(main(sys.argv[1:]))
