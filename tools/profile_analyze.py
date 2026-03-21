#!/usr/bin/env python3
"""Analyze molt_profile_json output from benchmark runs.

Usage:
    python3 tools/profile_analyze.py bench/results/profiles_20260320/*.json
    python3 tools/profile_analyze.py bench/results/profiles_20260320/*.log
    python3 tools/profile_analyze.py --raw-line 'molt_profile_json {...}'

Reads JSON profile files or log files containing ``molt_profile_json`` lines
and prints a ranked summary of hot-path counters, allocation pressure, and
optimization targets.
"""

from __future__ import annotations

import argparse
import json
import sys
from pathlib import Path


def _extract_profile_from_log(text: str) -> dict | None:
    """Extract the molt_profile_json payload from a log file."""
    for line in text.splitlines():
        if line.startswith("molt_profile_json "):
            payload = line[len("molt_profile_json ") :].strip()
            try:
                return json.loads(payload)
            except json.JSONDecodeError:
                continue
    return None


def _load_profile(path: Path) -> dict | None:
    """Load a profile from a .json file or a .log file."""
    text = path.read_text(encoding="utf-8", errors="replace")
    if path.suffix == ".json":
        try:
            return json.loads(text)
        except json.JSONDecodeError:
            return _extract_profile_from_log(text)
    return _extract_profile_from_log(text)


def _flatten_counters(profile: dict) -> dict[str, int]:
    """Flatten nested profile JSON into a single counter dict."""
    flat: dict[str, int] = {}
    for section in ("profile", "hot_paths", "deopt_reasons"):
        sub = profile.get(section, {})
        if isinstance(sub, dict):
            for key, value in sub.items():
                if isinstance(value, (int, float)):
                    flat[key] = int(value)
    return flat


def _print_section(title: str, items: list[tuple[str, int]], width: int = 50) -> None:
    print(f"\n{'=' * width}")
    print(f"  {title}")
    print(f"{'=' * width}")
    if not items:
        print("  (none)")
        return
    max_val_len = max(len(f"{v:,}") for _, v in items)
    for name, value in items:
        bar_len = 0
        if items[0][1] > 0:
            bar_len = int(30 * value / items[0][1])
        bar = "#" * bar_len
        print(f"  {name:<40s} {value:>{max_val_len},d}  {bar}")


def analyze_profiles(profiles: dict[str, dict]) -> None:
    """Print ranked analysis of profile data."""
    if not profiles:
        print("No profiles to analyze.")
        return

    print(f"\nMolt Profile Analysis  ({len(profiles)} benchmark(s))")
    print("=" * 60)

    # Per-benchmark summaries
    all_counters: dict[str, dict[str, int]] = {}
    for name, profile in profiles.items():
        all_counters[name] = _flatten_counters(profile)

    # --- Hot-path counters (aggregated across benchmarks) ---
    key_counters = [
        "call_bind_ic_hit",
        "call_bind_ic_miss",
        "attr_site_name_hit",
        "attr_site_name_miss",
        "split_ws_ascii",
        "split_ws_unicode",
        "dict_str_int_prehash_hit",
        "dict_str_int_prehash_miss",
        "dict_str_int_prehash_deopt",
        "ascii_i64_parse_fail",
    ]

    print("\n--- Hot-Path IC / Cache Counters (per benchmark) ---")
    for bench_name, counters in sorted(all_counters.items()):
        print(f"\n  [{bench_name}]")
        for key in key_counters:
            val = counters.get(key, 0)
            if val > 0:
                print(f"    {key:<42s} {val:>12,d}")
        # IC hit rate
        ic_hit = counters.get("call_bind_ic_hit", 0)
        ic_miss = counters.get("call_bind_ic_miss", 0)
        ic_total = ic_hit + ic_miss
        if ic_total > 0:
            rate = 100.0 * ic_hit / ic_total
            print(f"    {'call_bind_ic_hit_rate':<42s} {rate:>11.1f}%")
        attr_hit = counters.get("attr_site_name_hit", 0)
        attr_miss = counters.get("attr_site_name_miss", 0)
        attr_total = attr_hit + attr_miss
        if attr_total > 0:
            rate = 100.0 * attr_hit / attr_total
            print(f"    {'attr_site_name_hit_rate':<42s} {rate:>11.1f}%")

    # --- Allocation pressure ---
    alloc_keys = [
        "alloc_count",
        "alloc_string",
        "alloc_tuple",
        "alloc_dict",
        "alloc_callargs",
        "alloc_object",
        "alloc_exception",
    ]

    print("\n--- Allocation Pressure (per benchmark) ---")
    for bench_name, counters in sorted(all_counters.items()):
        print(f"\n  [{bench_name}]")
        for key in alloc_keys:
            val = counters.get(key, 0)
            if val > 0:
                pct = ""
                if key != "alloc_count":
                    total = counters.get("alloc_count", 1)
                    if total > 0:
                        pct = f"  ({100.0 * val / total:.1f}%)"
                print(f"    {key:<42s} {val:>12,d}{pct}")

    # --- Ranked: top allocation sources across all benchmarks ---
    agg_allocs: dict[str, int] = {}
    for counters in all_counters.values():
        for key in alloc_keys:
            agg_allocs[key] = agg_allocs.get(key, 0) + counters.get(key, 0)

    ranked_allocs = sorted(
        [(k, v) for k, v in agg_allocs.items() if v > 0],
        key=lambda x: x[1],
        reverse=True,
    )
    _print_section("Aggregate Allocation Ranking (all benchmarks)", ranked_allocs)

    # --- Ranked: top hot-path counters across all benchmarks ---
    agg_hot: dict[str, int] = {}
    for counters in all_counters.values():
        for key in key_counters:
            agg_hot[key] = agg_hot.get(key, 0) + counters.get(key, 0)

    ranked_hot = sorted(
        [(k, v) for k, v in agg_hot.items() if v > 0],
        key=lambda x: x[1],
        reverse=True,
    )
    _print_section("Aggregate Hot-Path Ranking (all benchmarks)", ranked_hot)

    # --- Deopt reasons ---
    agg_deopt: dict[str, int] = {}
    for counters in all_counters.values():
        deopt_keys = [
            k
            for k in counters
            if k.startswith("guard_") or k.startswith("call_indirect") or k.startswith("invoke_ffi")
        ]
        for key in deopt_keys:
            agg_deopt[key] = agg_deopt.get(key, 0) + counters.get(key, 0)

    ranked_deopt = sorted(
        [(k, v) for k, v in agg_deopt.items() if v > 0],
        key=lambda x: x[1],
        reverse=True,
    )
    _print_section("Deoptimization Events (all benchmarks)", ranked_deopt)

    # --- Optimization recommendations ---
    print(f"\n{'=' * 50}")
    print("  Optimization Recommendations")
    print(f"{'=' * 50}")

    recommendations: list[tuple[int, str]] = []

    # Check IC miss rate
    total_ic_hit = agg_hot.get("call_bind_ic_hit", 0)
    total_ic_miss = agg_hot.get("call_bind_ic_miss", 0)
    total_ic = total_ic_hit + total_ic_miss
    if total_ic > 0:
        miss_rate = 100.0 * total_ic_miss / total_ic
        if miss_rate > 10:
            recommendations.append(
                (
                    total_ic_miss,
                    f"Call bind IC miss rate is {miss_rate:.0f}% "
                    f"({total_ic_miss:,} misses / {total_ic:,} total). "
                    "Improve call-site inline caching or monomorphic call stubs.",
                )
            )

    # Check attr cache miss rate
    total_attr_hit = agg_hot.get("attr_site_name_hit", 0)
    total_attr_miss = agg_hot.get("attr_site_name_miss", 0)
    total_attr = total_attr_hit + total_attr_miss
    if total_attr > 0:
        miss_rate = 100.0 * total_attr_miss / total_attr
        if miss_rate > 10:
            recommendations.append(
                (
                    total_attr_miss,
                    f"Attr site-name cache miss rate is {miss_rate:.0f}% "
                    f"({total_attr_miss:,} misses / {total_attr:,} total). "
                    "Warm up attribute caches or use shape-based lookups.",
                )
            )

    # Check string allocation dominance
    total_alloc = agg_allocs.get("alloc_count", 0)
    total_str = agg_allocs.get("alloc_string", 0)
    if total_alloc > 0 and total_str > 0:
        str_pct = 100.0 * total_str / total_alloc
        if str_pct > 30:
            recommendations.append(
                (
                    total_str,
                    f"String allocations are {str_pct:.0f}% of all allocations "
                    f"({total_str:,} / {total_alloc:,}). "
                    "Consider string interning, SSO (small string optimization), "
                    "or arena allocation for short-lived strings.",
                )
            )

    # Check dict allocation
    total_dict = agg_allocs.get("alloc_dict", 0)
    if total_alloc > 0 and total_dict > 0:
        dict_pct = 100.0 * total_dict / total_alloc
        if dict_pct > 10:
            recommendations.append(
                (
                    total_dict,
                    f"Dict allocations are {dict_pct:.0f}% of all allocations "
                    f"({total_dict:,} / {total_alloc:,}). "
                    "Consider dict pooling or compact representations for "
                    "small fixed-key dicts (e.g., locals frames).",
                )
            )

    # Check tuple allocation
    total_tuple = agg_allocs.get("alloc_tuple", 0)
    if total_alloc > 0 and total_tuple > 0:
        tuple_pct = 100.0 * total_tuple / total_alloc
        if tuple_pct > 10:
            recommendations.append(
                (
                    total_tuple,
                    f"Tuple allocations are {tuple_pct:.0f}% of all allocations "
                    f"({total_tuple:,} / {total_alloc:,}). "
                    "Consider tuple caching for common arities (0-3 elements) "
                    "or stack-allocating short-lived tuples.",
                )
            )

    # Check callargs allocation
    total_callargs = agg_allocs.get("alloc_callargs", 0)
    if total_alloc > 0 and total_callargs > 0:
        recommendations.append(
            (
                total_callargs,
                f"CallArgs allocations: {total_callargs:,}. "
                "Consider pre-allocated argument buffers or "
                "inline call conventions for known-arity calls.",
            )
        )

    # Overall allocation pressure
    if total_alloc > 100_000:
        recommendations.append(
            (
                total_alloc,
                f"Total allocation count is {total_alloc:,} across benchmarks. "
                "A bump allocator or arena for per-iteration temporaries "
                "could reduce GC pressure significantly.",
            )
        )

    recommendations.sort(key=lambda x: x[0], reverse=True)
    for idx, (_, rec) in enumerate(recommendations[:10], 1):
        print(f"\n  {idx}. {rec}")

    if not recommendations:
        print("\n  No significant optimization targets detected.")

    print()


def main() -> None:
    parser = argparse.ArgumentParser(
        description="Analyze molt_profile_json output from benchmark runs."
    )
    parser.add_argument(
        "files",
        nargs="*",
        help="Profile JSON files or log files containing molt_profile_json lines.",
    )
    parser.add_argument(
        "--raw-line",
        action="append",
        default=[],
        help="Raw molt_profile_json line(s) to parse directly.",
    )
    args = parser.parse_args()

    profiles: dict[str, dict] = {}

    for raw in args.raw_line:
        line = raw.strip()
        if line.startswith("molt_profile_json "):
            line = line[len("molt_profile_json ") :]
        try:
            data = json.loads(line)
            label = f"raw_line_{len(profiles)}"
            profiles[label] = data
        except json.JSONDecodeError as exc:
            print(f"Warning: could not parse raw line: {exc}", file=sys.stderr)

    for file_arg in args.files:
        path = Path(file_arg)
        if not path.exists():
            print(f"Warning: file not found: {path}", file=sys.stderr)
            continue
        profile = _load_profile(path)
        if profile is None:
            print(f"Warning: no profile data found in {path}", file=sys.stderr)
            continue
        label = path.stem
        profiles[label] = profile

    if not profiles:
        print("No profile data provided. Use --help for usage.", file=sys.stderr)
        sys.exit(1)

    analyze_profiles(profiles)


if __name__ == "__main__":
    main()
