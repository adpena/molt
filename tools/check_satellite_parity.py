#!/usr/bin/env python3
"""Fail-closed parity guard for runtime stdlib in-tree <-> satellite
module pairs.

Background (the P0 this guard exists to kill)
---------------------------------------------
molt still has several feature-gated stdlib modules in TWO physical copies:

  * an IN-TREE copy under runtime/molt-runtime/src/builtins/<mod>.rs, gated
    `#[cfg(not(feature = "stdlib_X"))]`, which is the SOLE compiled source for
    the reduced build tiers (`--stdlib-profile micro`, `stdlib_edge`, and the
    WASM feature set — see src/molt/cli.py), and
  * a SATELLITE copy under runtime/molt-runtime-X/src/<mod>.rs, which is the
    compiled source for the DEFAULT native build (`stdlib_full`).

The two copies are NOT one logic in two namespaces. They are two runtime-access
implementations of the same behavior: the in-tree copy calls molt-runtime
internals DIRECTLY (`use crate::{...}`, the `PyToken` GIL token, the
`crate::with_gil_entry_nopanic!` macro); the satellite reaches the same
internals through an `extern "C"` FFI BRIDGE (`use crate::bridge::*` +
`molt_runtime_core::prelude::*`, the `CoreGilToken` token, `with_core_gil!`).

Because there is no single source of truth, a behavioral fix landed in only one
copy makes SHIPPED BEHAVIOR DIFFER BY BUILD TIER — exactly the silent-miscompile
bug-class the decomposition program (docs/design/foundation/21) set out to kill.
All original pairs had bidirectionally drifted before this guard existed; see
memory/recovery/baton_move_R_satellite_drift.md for the full inventory.

What this guard does
--------------------
For each pair it NORMALIZES away the by-design access-layer differences
(imports, doc comments, the GIL macro/token, bridge path prefixes, single-line
`unsafe {}` wrappers, trailing comments) and then compares the residual
line-MULTISET (sorted, so pure reordering is ignored). The residual is the
genuine semantic-drift surface.

It is a CONTRACT, not a sync script: it never edits source. It loads a committed
baseline (this file's SATELLITE_PARITY_BASELINE) recording, per pair, the
allowed residual size and a SHA-256 of the sorted residual content. The guard
FAILS (exit 1) when, for any pair:

  * the residual count EXCEEDS the baselined count (NEW drift), or
  * the residual content HASH differs from the baseline while the count is
    unchanged (drift that swapped one divergence for another), or
  * a pair is missing/unreadable, or
  * the baseline total exceeds the committed ratchet ceiling RATCHET_CEILING.

Reconciling a pair (porting a one-sided fix so both copies embody the same
behavior) SHRINKS its residual; you then regenerate the baseline with
`--update-baseline`, which can only lower RATCHET_CEILING (the guard refuses to
raise it). This makes the baseline a one-way ratchet toward zero drift, and
makes any NEW drift a hard test failure.

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

# The remaining feature-gated in-tree <-> satellite module pairs. The key is a stable
# short name; values are the in-tree path (relative to molt-runtime/src) and the
# satellite path (relative to runtime/). Derived from the
# `#[cfg(not(feature = "stdlib_*"))]` gates in builtins/mod.rs and verified
# against the on-disk crates. Leaf-owned modules with no in-tree fallback are
# deliberately absent; adding them back would reintroduce a second authority.
PAIRS: dict[str, tuple[str, str]] = {
    "functions_http": (
        "builtins/functions_http.rs",
        "molt-runtime-http/src/functions_http.rs",
    ),
    "functions_logging": (
        "builtins/functions_logging.rs",
        "molt-runtime-http/src/functions_logging.rs",
    ),
    "itertools": ("builtins/itertools.rs", "molt-runtime-itertools/src/itertools.rs"),
    "difflib": ("builtins/difflib.rs", "molt-runtime-difflib/src/difflib.rs"),
    "ipaddress": ("builtins/ipaddress.rs", "molt-runtime-ipaddress/src/ipaddress.rs"),
    "cmath_mod": ("builtins/cmath_mod.rs", "molt-runtime-math/src/cmath_mod.rs"),
    "colorsys": ("builtins/colorsys.rs", "molt-runtime-math/src/colorsys.rs"),
    "fractions": ("builtins/fractions.rs", "molt-runtime-math/src/fractions.rs"),
    "math": ("builtins/math.rs", "molt-runtime-math/src/math.rs"),
    "random_mod": ("builtins/random_mod.rs", "molt-runtime-math/src/random_mod.rs"),
    "os_ext": ("builtins/os_ext.rs", "molt-runtime-path/src/os_ext.rs"),
    "pathlib": ("builtins/pathlib.rs", "molt-runtime-path/src/pathlib.rs"),
    "regex": ("builtins/regex.rs", "molt-runtime-regex/src/regex.rs"),
    "base64_mod": ("builtins/base64_mod.rs", "molt-runtime-serial/src/base64_mod.rs"),
    "binascii": ("builtins/binascii.rs", "molt-runtime-serial/src/binascii.rs"),
    "configparser": (
        "builtins/configparser.rs",
        "molt-runtime-serial/src/configparser.rs",
    ),
    "csv": ("builtins/csv.rs", "molt-runtime-serial/src/csv.rs"),
    "datetime": ("builtins/datetime.rs", "molt-runtime-serial/src/datetime.rs"),
    "decimal": ("builtins/decimal.rs", "molt-runtime-serial/src/decimal.rs"),
    "structs": ("builtins/structs.rs", "molt-runtime-serial/src/structs.rs"),
    "functions_zipfile": (
        "builtins/functions_zipfile.rs",
        "molt-runtime-serial/src/zipfile.rs",
    ),
    "functions_email": (
        "builtins/functions_email.rs",
        "molt-runtime-serial/src/email.rs",
    ),
    "xml_etree": ("builtins/xml_etree.rs", "molt-runtime-xml/src/xml_etree.rs"),
    "xml_sax": ("builtins/xml_sax.rs", "molt-runtime-xml/src/xml_sax.rs"),
}

# decimal is architecturally different on the in-tree side: the in-tree
# `builtins/decimal.rs` is a 13-line dispatcher to decimal_with_mpdec.rs /
# decimal_without_mpdec.rs, whereas the satellite is a single self-contained
# file. A line-multiset residual is meaningless for that shape, so the guard
# compares the satellite against the in-tree `decimal_without_mpdec.rs`
# implementation file instead of the dispatcher stub.
DECIMAL_INTREE_IMPL = "builtins/decimal_without_mpdec.rs"

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
            if (saw_brace and depth <= 0) or (not saw_brace and line.strip().endswith(";")):
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
    if name == "decimal":
        return INTREE_DIR / DECIMAL_INTREE_IMPL
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
