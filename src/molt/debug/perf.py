from __future__ import annotations

import json
from pathlib import Path
from typing import Any


HOT_COUNTER_KEYS = (
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
)

ALLOC_COUNTER_KEYS = (
    "alloc_count",
    "alloc_string",
    "alloc_tuple",
    "alloc_dict",
    "alloc_callargs",
    "alloc_object",
    "alloc_exception",
)


def extract_profile_from_log(text: str) -> dict[str, Any] | None:
    for line in text.splitlines():
        if line.startswith("molt_profile_json "):
            payload = line[len("molt_profile_json ") :].strip()
            try:
                return json.loads(payload)
            except json.JSONDecodeError:
                continue
    return None


def load_profile(path: Path) -> dict[str, Any] | None:
    text = path.read_text(encoding="utf-8", errors="replace")
    if path.suffix == ".json":
        try:
            return json.loads(text)
        except json.JSONDecodeError:
            return extract_profile_from_log(text)
    return extract_profile_from_log(text)


def flatten_counters(profile: dict[str, Any]) -> dict[str, int]:
    flat: dict[str, int] = {}
    for section in ("profile", "hot_paths", "deopt_reasons"):
        sub = profile.get(section, {})
        if isinstance(sub, dict):
            for key, value in sub.items():
                if isinstance(value, (int, float)):
                    flat[key] = int(value)
    return flat


def build_perf_summary_payload(profiles: dict[str, dict[str, Any]]) -> dict[str, Any]:
    counters_by_profile = {
        name: flatten_counters(profile) for name, profile in sorted(profiles.items())
    }
    aggregate_hot: dict[str, int] = {}
    aggregate_alloc: dict[str, int] = {}
    for counters in counters_by_profile.values():
        for key in HOT_COUNTER_KEYS:
            aggregate_hot[key] = aggregate_hot.get(key, 0) + counters.get(key, 0)
        for key in ALLOC_COUNTER_KEYS:
            aggregate_alloc[key] = aggregate_alloc.get(key, 0) + counters.get(key, 0)

    recommendations: list[str] = []
    total_ic_hit = aggregate_hot.get("call_bind_ic_hit", 0)
    total_ic_miss = aggregate_hot.get("call_bind_ic_miss", 0)
    total_ic = total_ic_hit + total_ic_miss
    if total_ic > 0:
        miss_rate = 100.0 * total_ic_miss / total_ic
        if miss_rate > 10.0:
            recommendations.append(
                "Improve call-site inline caching or monomorphic call stubs."
            )

    total_alloc = aggregate_alloc.get("alloc_count", 0)
    total_callargs = aggregate_alloc.get("alloc_callargs", 0)
    if total_alloc > 0 and total_callargs > 0:
        recommendations.append(
            "Consider pre-allocated argument buffers or inline known-arity call conventions."
        )

    return {
        "profile_count": len(profiles),
        "profiles": counters_by_profile,
        "aggregate": {
            "hot_paths": {k: v for k, v in aggregate_hot.items() if v > 0},
            "allocations": {k: v for k, v in aggregate_alloc.items() if v > 0},
        },
        "recommendations": recommendations,
    }


def render_perf_text(summary: dict[str, Any]) -> str:
    aggregate = summary.get("aggregate", {})
    hot_paths = aggregate.get("hot_paths", {}) if isinstance(aggregate, dict) else {}
    allocations = (
        aggregate.get("allocations", {}) if isinstance(aggregate, dict) else {}
    )
    lines = [
        "Molt Debug Perf",
        f"Profiles: {summary.get('profile_count', 0)}",
    ]
    if isinstance(hot_paths, dict) and hot_paths:
        hot_bits = [f"{key}={hot_paths[key]}" for key in sorted(hot_paths)]
        lines.append("Hot Paths: " + ", ".join(hot_bits))
    if isinstance(allocations, dict) and allocations:
        alloc_bits = [f"{key}={allocations[key]}" for key in sorted(allocations)]
        lines.append("Allocations: " + ", ".join(alloc_bits))
    recommendations = summary.get("recommendations", [])
    if isinstance(recommendations, list) and recommendations:
        lines.append(f"Recommendations: {len(recommendations)}")
        lines.extend(f"- {item}" for item in recommendations if isinstance(item, str))
    return "\n".join(lines) + "\n"
