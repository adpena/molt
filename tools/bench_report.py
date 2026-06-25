#!/usr/bin/env python3
"""Generate a combined native+WASM benchmark report in Markdown."""

from __future__ import annotations

import argparse
import json
import statistics
import sys
from datetime import datetime
from pathlib import Path
from typing import Any

ROOT = Path(__file__).resolve().parents[1]
TOOLS_ROOT = Path(__file__).resolve().parent
DEFAULT_MANIFEST_PATH = ROOT / "bench" / "results" / "docs_manifest.json"
if str(TOOLS_ROOT) not in sys.path:
    sys.path.insert(0, str(TOOLS_ROOT))

import perf_authority  # noqa: E402
from bench_evidence import (  # noqa: E402
    comparator_time,
    native_molt_speedup,
    native_molt_time,
    valid_positive_number,
    wasm_molt_time,
)


def _load_json(path: Path) -> dict[str, Any]:
    if not path.exists():
        raise SystemExit(f"missing benchmark file: {path}")
    return json.loads(path.read_text())


def _normalize_name(name: str) -> str:
    return name[:-3] if name.endswith(".py") else name


def _display_name(name: str) -> str:
    return name if name.endswith(".py") else f"{name}.py"


def _safe_div(num: float | None, den: float | None) -> float | None:
    # Route every display ratio through the SINGLE guarded authority
    # (perf_authority.signed_ratio) so a None/0/NaN/negative operand can never
    # render a finite ratio. This is a display tool over mixed time/size
    # operands, so the generic RATIO direction applies; the column header names
    # the operand order. No raw `time / time` division lives here (audit
    # meta-bug item 2: ratio-direction canonicalization).
    return perf_authority.signed_ratio_value(
        num, den, direction=perf_authority.RatioDirection.RATIO
    )


def _valid_positive_time(value: Any) -> float | None:
    return valid_positive_number(value)


def _wasm_time_if_ok(entry: dict[str, Any]) -> float | None:
    return wasm_molt_time(entry)


def _format_time(value: float | None) -> str:
    if value is None:
        return "-"
    return f"{value:.6f}"


def _format_ratio(value: float | None) -> str:
    if value is None:
        return "-"
    return f"{value:.2f}x"


def _format_system(system: dict[str, Any] | None) -> str:
    if not system:
        return "-"
    parts = [f"{key}={system[key]}" for key in sorted(system.keys())]
    return ", ".join(parts)


def _format_run_settings(payload: dict[str, Any]) -> str:
    timing_mode = payload.get("timing_mode", "legacy-unknown")
    warmup = payload.get("warmup", "legacy-unknown")
    samples = payload.get("samples", "legacy-unknown")
    return f"timing_mode={timing_mode}, warmup={warmup}, samples={samples}"


def _display_path(path: Path) -> str:
    try:
        return path.resolve().relative_to(ROOT.resolve()).as_posix()
    except ValueError:
        return str(path)


def _median(values: list[float]) -> float | None:
    if not values:
        return None
    return statistics.median(values)


def _collect_benchmarks(
    native: dict[str, Any], wasm: dict[str, Any]
) -> tuple[list[str], dict[str, Any], dict[str, Any]]:
    native_bench = {
        _normalize_name(name): entry
        for name, entry in native.get("benchmarks", {}).items()
    }
    wasm_bench = {
        _normalize_name(name): entry
        for name, entry in wasm.get("benchmarks", {}).items()
    }
    names = sorted(set(native_bench) | set(wasm_bench))
    return names, native_bench, wasm_bench


def _extract_date(value: str | None) -> str:
    if not value:
        return "-"
    try:
        parsed = datetime.fromisoformat(value.replace("Z", "+00:00"))
    except ValueError:
        return value
    return parsed.date().isoformat()


def _summarize_system(system: dict[str, Any] | None) -> str:
    if not system:
        return "-"
    platform = system.get("platform", "-")
    machine = system.get("machine", "-")
    python_ver = system.get("python", "-")
    platform_name = platform.split("-", maxsplit=1)[0] if platform else "-"
    return f"{platform_name} {machine}, CPython {python_ver}"


def _format_name_list(names: list[str], limit: int = 8) -> str:
    if len(names) <= limit:
        return ", ".join(f"`{name}`" for name in names)
    return (
        f"{', '.join(f'`{name}`' for name in names[:limit])}, "
        f"and {len(names) - limit} more"
    )


def _format_speedup_list(items: list[tuple[str, float]], limit: int) -> str:
    if not items:
        return "none"
    return ", ".join(
        f"`{_display_name(name)}` {speedup:.2f}x" for name, speedup in items[:limit]
    )


def _format_time_list(items: list[tuple[str, float]], limit: int) -> str:
    if not items:
        return "none"
    return ", ".join(
        f"`{_display_name(name)}` {value:.2f}s" for name, value in items[:limit]
    )


def _format_size_list(items: list[tuple[str, float]], limit: int) -> str:
    if not items:
        return "none"
    return ", ".join(
        f"`{_display_name(name)}` {value:.1f} KB" for name, value in items[:limit]
    )


def _format_ratio_list(items: list[tuple[str, float]], limit: int) -> str:
    if not items:
        return "none"
    return ", ".join(
        f"`{_display_name(name)}` {value:.2f}x" for name, value in items[:limit]
    )


def _baseline_summary(native_bench: dict[str, Any]) -> str:
    parts: list[str] = []
    lane_labels = {
        "pypy": "PyPy",
        "codon": "Codon",
        "nuitka": "Nuitka",
        "pyodide": "Pyodide",
    }
    for lane, label in lane_labels.items():
        ok_key = f"{lane}_ok"
        available = any(entry.get(ok_key) for entry in native_bench.values())
        if not available:
            parts.append(f"{label} baseline unavailable")
            continue
        missing = sorted(
            _display_name(name)
            for name, entry in native_bench.items()
            if not entry.get(ok_key)
        )
        if missing:
            parts.append(f"{label} skipped for {_format_name_list(missing)}")

    if not parts:
        return "none"
    return "; ".join(parts)


def _molt_failure_summary(native_bench: dict[str, Any]) -> str:
    failed = sorted(
        _display_name(name)
        for name, entry in native_bench.items()
        if entry.get("molt_ok") is False
    )
    if not failed:
        return "none"
    return _format_name_list(failed, limit=8)


def _wasm_failure_summary(wasm_bench: dict[str, Any]) -> str:
    failed = sorted(
        _display_name(name)
        for name, entry in wasm_bench.items()
        if entry.get("molt_wasm_ok") is False
    )
    if not failed:
        return "none"
    return _format_name_list(failed, limit=8)


def _startup_stat_ms(section: Any, stat: str = "median_s") -> float | None:
    """Pull a startup metric (in ms) from an audit startup-mode section."""
    if not isinstance(section, dict):
        return None
    stats = section.get("stats")
    if not isinstance(stats, dict):
        return None
    value = stats.get(stat)
    if not isinstance(value, int | float):
        return None
    return float(value) * 1000.0


def _format_ms(value: float | None) -> str:
    if value is None:
        return "-"
    return f"{value:.1f}"


def _startup_audit_rows(startup_audit: dict[str, Any]) -> list[dict[str, Any]]:
    """Extract per-case cold-start/size rows from the audit JSON."""
    rows: list[dict[str, Any]] = []
    for case in startup_audit.get("cases", []):
        if not isinstance(case, dict):
            continue
        meta = case.get("case", {})
        artifact = case.get("artifact")
        startup = case.get("startup")
        budgets = case.get("budgets", {})
        rows.append(
            {
                "target": meta.get("target", "-"),
                "profile": meta.get("build_profile", "-"),
                "backend": meta.get("backend", "-"),
                "status": case.get("status", "-"),
                "bytes": artifact.get("bytes") if isinstance(artifact, dict) else None,
                "same_warm_min": _startup_stat_ms(
                    startup.get("same_path") if isinstance(startup, dict) else None,
                    "min_s",
                ),
                "same_warm_median": _startup_stat_ms(
                    startup.get("same_path") if isinstance(startup, dict) else None
                ),
                "page_cache_cold": _startup_stat_ms(
                    startup.get("page_cache_cold")
                    if isinstance(startup, dict)
                    else None
                ),
                "cold_first_sighting": _startup_stat_ms(
                    startup.get("cold_first_sighting")
                    if isinstance(startup, dict)
                    else None
                ),
                "skipped": (
                    startup.get("skipped") if isinstance(startup, dict) else None
                ),
                "budget_ok": bool(budgets.get("passed")) if budgets else True,
            }
        )
    return rows


def _startup_baseline_ms(startup_audit: dict[str, Any], key: str) -> float | None:
    baselines = startup_audit.get("baselines", {})
    if not isinstance(baselines, dict):
        return None
    return _startup_stat_ms(baselines.get(key))


def _format_bytes(value: int | None) -> str:
    if value is None:
        return "-"
    return f"{value} ({value / 1024 / 1024:.2f} MB)"


def _status_summary(
    native: dict[str, Any],
    wasm: dict[str, Any],
    native_bench: dict[str, Any],
    wasm_bench: dict[str, Any],
    startup_audit: dict[str, Any] | None = None,
) -> str:
    speedups = [
        (name, speedup)
        for name, entry in native_bench.items()
        if (speedup := native_molt_speedup(entry)) is not None
    ]
    speedups.sort(key=lambda item: item[1], reverse=True)

    regressions = [
        (name, speedup)
        for name, entry in native_bench.items()
        if (speedup := native_molt_speedup(entry)) is not None and speedup < 1.0
    ]
    regressions.sort(key=lambda item: item[1])

    slowest = sorted(speedups, key=lambda item: item[1])

    wasm_times = [
        (name, wasm_time)
        for name, entry in wasm_bench.items()
        if (wasm_time := _wasm_time_if_ok(entry)) is not None
    ]
    wasm_times.sort(key=lambda item: item[1], reverse=True)

    wasm_sizes = [
        (name, entry["molt_wasm_size_kb"])
        for name, entry in wasm_bench.items()
        if entry.get("molt_wasm_size_kb") is not None
    ]
    wasm_sizes.sort(key=lambda item: item[1], reverse=True)

    wasm_ratios = []
    for name, entry in wasm_bench.items():
        wasm_time = _wasm_time_if_ok(entry)
        cpython_time = native_bench.get(name, {}).get("cpython_time_s")
        ratio = _safe_div(wasm_time, cpython_time)
        if ratio is not None:
            wasm_ratios.append((name, ratio))
    wasm_ratios.sort(key=lambda item: item[1], reverse=True)

    native_date = _extract_date(native.get("created_at"))
    wasm_date = _extract_date(wasm.get("created_at"))

    native_system = _summarize_system(native.get("system"))
    wasm_system = _summarize_system(wasm.get("system"))
    wasm_ok = sum(1 for entry in wasm_bench.values() if entry.get("molt_wasm_ok"))

    summary_lines = [
        f"Latest run: {native_date} ({native_system}).",
        f"Top speedups: {_format_speedup_list(speedups, 5)}.",
        f"Regressions: {_format_speedup_list(regressions, len(regressions))}.",
        f"Slowest: {_format_speedup_list(slowest, 3)}.",
        f"Molt build/run failures: {_molt_failure_summary(native_bench)}.",
        f"Comparator baseline coverage: {_baseline_summary(native_bench)}.",
        (
            f"WASM run: {wasm_date} ({wasm_system}); "
            f"ok {wasm_ok}/{len(wasm_bench)}, failures: {_wasm_failure_summary(wasm_bench)}. "
            f"Slowest: {_format_time_list(wasm_times, 3)}; "
            f"largest sizes: {_format_size_list(wasm_sizes, 3)}; "
            f"WASM vs CPython slowest ratios: {_format_ratio_list(wasm_ratios, 3)}."
        ),
    ]
    startup_line = _startup_status_line(startup_audit or {})
    if startup_line is not None:
        summary_lines.append(startup_line)
    return "\n".join(summary_lines)


def _startup_status_line(startup_audit: dict[str, Any]) -> str | None:
    """One-line cold-start/size summary for the STATUS.md generated block."""
    rows = _startup_audit_rows(startup_audit)
    native_rows = [row for row in rows if row["target"] == "native" and row["bytes"]]
    if not native_rows:
        return None
    row = native_rows[0]
    c_ms = _startup_baseline_ms(startup_audit, "c")
    cpy_ms = _startup_baseline_ms(startup_audit, "cpython")
    size_ok = all(r["budget_ok"] for r in rows)
    return (
        f"Startup/size: native hello-world {_format_bytes(row['bytes'])}, "
        f"warm {_format_ms(row['same_warm_median'])}ms / "
        f"cold(first-sighting) {_format_ms(row['cold_first_sighting'])}ms; "
        f"C baseline {_format_ms(c_ms)}ms, CPython {_format_ms(cpy_ms)}ms; "
        f"budget {'OK' if size_ok else 'REGRESSED'}."
    )


def _update_status_doc(status_path: Path, summary_block: str) -> None:
    updated = _render_updated_status_doc(
        status_path.read_text(), status_path, summary_block
    )
    status_path.write_text(updated)


def _render_updated_status_doc(
    content: str, status_path: Path, summary_block: str
) -> str:
    marker_start = "<!-- GENERATED:bench-summary:start -->"
    marker_end = "<!-- GENERATED:bench-summary:end -->"
    if marker_start not in content or marker_end not in content:
        raise SystemExit(
            f"missing status markers {marker_start}/{marker_end} in {status_path}"
        )
    before, rest = content.split(marker_start, maxsplit=1)
    _, after = rest.split(marker_end, maxsplit=1)
    return f"{before}{marker_start}\n{summary_block}\n{marker_end}{after}"


def _render_report_markdown(
    native_path: Path,
    wasm_path: Path,
    native: dict[str, Any],
    wasm: dict[str, Any],
    startup_audit: dict[str, Any] | None = None,
) -> str:
    names, native_bench, wasm_bench = _collect_benchmarks(native, wasm)
    lane_labels = {
        "pypy": "PyPy",
        "codon": "Codon",
        "nuitka": "Nuitka",
        "pyodide": "Pyodide",
    }
    comparator_rows: dict[str, list[tuple[str, float, float, float]]] = {
        lane: [] for lane in lane_labels
    }

    native_ok = sum(1 for entry in native_bench.values() if entry.get("molt_ok"))
    lane_ok_counts = {
        lane: sum(1 for entry in native_bench.values() if entry.get(f"{lane}_ok"))
        for lane in lane_labels
    }
    wasm_ok = sum(1 for entry in wasm_bench.values() if entry.get("molt_wasm_ok"))

    native_speedups = [
        speedup
        for entry in native_bench.values()
        if (speedup := native_molt_speedup(entry)) is not None
    ]

    wasm_speedups: list[float] = []
    wasm_native_ratios: list[float] = []
    regressions: list[tuple[str, float, float | None, float | None]] = []
    wasm_slowest: list[tuple[str, float, float, float]] = []

    for name in names:
        n_entry = native_bench.get(name, {})
        w_entry = wasm_bench.get(name, {})
        molt_time = native_molt_time(n_entry)
        cpython_time = n_entry.get("cpython_time_s")
        speedup = native_molt_speedup(n_entry)
        wasm_time = _wasm_time_if_ok(w_entry)

        wasm_speedup = _safe_div(cpython_time, wasm_time)
        wasm_native_ratio = _safe_div(wasm_time, molt_time)
        if wasm_speedup is not None:
            wasm_speedups.append(wasm_speedup)
        if (
            wasm_native_ratio is not None
            and wasm_time is not None
            and molt_time is not None
        ):
            wasm_native_ratios.append(wasm_native_ratio)
            wasm_slowest.append((name, wasm_time, molt_time, wasm_native_ratio))
        if speedup is not None and speedup < 1.0:
            regressions.append((name, speedup, molt_time, cpython_time))

        if molt_time is None:
            continue
        for lane in lane_labels:
            lane_time = comparator_time(n_entry, lane)
            if lane_time is not None:
                ratio = _safe_div(molt_time, lane_time)
                if ratio is not None:
                    comparator_rows[lane].append((name, molt_time, lane_time, ratio))

    regressions.sort(key=lambda item: item[1])
    wasm_slowest.sort(key=lambda item: item[3], reverse=True)
    for lane in comparator_rows:
        comparator_rows[lane].sort(key=lambda item: item[3], reverse=True)

    missing_native = sorted(set(wasm_bench) - set(native_bench))
    missing_wasm = sorted(set(native_bench) - set(wasm_bench))

    native_rev = native.get("git_rev") or "-"
    wasm_rev = wasm.get("git_rev") or "-"
    native_created = native.get("created_at") or "-"
    wasm_created = wasm.get("created_at") or "-"
    native_system = _format_system(native.get("system"))
    wasm_system = _format_system(wasm.get("system"))
    native_run_settings = _format_run_settings(native)
    wasm_run_settings = _format_run_settings(wasm)

    lines: list[str] = []
    lines.append("# Molt Bench Summary")
    lines.append("")
    lines.append("## Inputs")
    lines.append(
        f"- Native: `{_display_path(native_path)}`; git_rev={native_rev}; created_at={native_created}; "
        f"{native_run_settings}; system={native_system}"
    )
    lines.append(
        f"- WASM: `{_display_path(wasm_path)}`; git_rev={wasm_rev}; created_at={wasm_created}; "
        f"{wasm_run_settings}; system={wasm_system}"
    )
    if native_rev != "-" and wasm_rev != "-" and native_rev != wasm_rev:
        lines.append(
            "- NOTE: native and wasm results come from different git revisions; "
            "interpret combined ratios cautiously."
        )
    lines.append("")
    lines.append("## Summary")
    lines.append(
        f"- Benchmarks: {len(names)} total; native ok {native_ok}/{len(native_bench)}; "
        f"wasm ok {wasm_ok}/{len(wasm_bench)}."
    )
    lines.append(
        "- Median native speedup vs CPython: "
        f"{_format_ratio(_median(native_speedups))}."
    )
    lines.append(
        f"- Median wasm speedup vs CPython: {_format_ratio(_median(wasm_speedups))}."
    )
    lines.append(
        f"- Median wasm/native ratio: {_format_ratio(_median(wasm_native_ratios))}."
    )
    lines.append(f"- Native regressions (< 1.0x): {len(regressions)}.")
    lines.append(
        "- Comparator coverage: "
        + ", ".join(
            f"{label} {lane_ok_counts[lane]}/{len(native_bench)}"
            for lane, label in lane_labels.items()
        )
        + "."
    )
    if missing_native:
        lines.append(f"- Missing native entries: {', '.join(missing_native)}.")
    if missing_wasm:
        lines.append(f"- Missing wasm entries: {', '.join(missing_wasm)}.")
    lines.append("")

    lines.append("## Regressions (Native < 1.0x)")
    lines.append("| Benchmark | Speedup | Molt s | CPython s |")
    lines.append("| --- | --- | --- | --- |")
    for name, speedup, molt_time, cpython_time in regressions[:10]:
        lines.append(
            "| "
            f"{name} | {_format_ratio(speedup)} | {_format_time(molt_time)} | "
            f"{_format_time(cpython_time)} |"
        )
    if not regressions:
        lines.append("| - | - | - | - |")
    lines.append("")

    lines.append("## WASM vs Native (Slowest)")
    lines.append("| Benchmark | WASM s | Native s | WASM/Native |")
    lines.append("| --- | --- | --- | --- |")
    for name, wasm_time, molt_time, ratio in wasm_slowest[:10]:
        lines.append(
            f"| {name} | {_format_time(wasm_time)} | {_format_time(molt_time)} | "
            f"{_format_ratio(ratio)} |"
        )
    if not wasm_slowest:
        lines.append("| - | - | - | - |")
    lines.append("")

    for lane, label in lane_labels.items():
        lines.append(f"## Molt vs {label} (Both OK)")
        lines.append("| Benchmark | Molt s | Comparator s | Molt/Comparator |")
        lines.append("| --- | --- | --- | --- |")
        rows = comparator_rows[lane]
        for name, molt_time, lane_time, ratio in rows[:10]:
            lines.append(
                f"| {name} | {_format_time(molt_time)} | {_format_time(lane_time)} | "
                f"{_format_ratio(ratio)} |"
            )
        if not rows:
            lines.append("| - | - | - | - |")
        lines.append("")

    lines.append("## Combined Table")
    lines.append(
        "| Benchmark | Molt OK | CPython s | PyPy s | Codon build s | Codon run s | "
        "Codon KB | Nuitka build s | Nuitka run s | Nuitka KB | Pyodide run s | "
        "Molt build s | Molt run s | Molt KB | Molt/CPython | Molt/PyPy | "
        "Molt/Codon | Molt/Nuitka | Molt/Pyodide | WASM OK | WASM s | "
        "WASM/Native | WASM/CPython |"
    )
    lines.append(
        "| --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | "
        "--- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- |"
    )
    for name in names:
        n_entry = native_bench.get(name, {})
        w_entry = wasm_bench.get(name, {})
        cpython_time = n_entry.get("cpython_time_s")
        pypy_time = comparator_time(n_entry, "pypy")
        codon_build = n_entry.get("codon_build_s")
        codon_time = comparator_time(n_entry, "codon")
        codon_size = n_entry.get("codon_size_kb")
        nuitka_build = n_entry.get("nuitka_build_s")
        nuitka_time = comparator_time(n_entry, "nuitka")
        nuitka_size = n_entry.get("nuitka_size_kb")
        pyodide_time = comparator_time(n_entry, "pyodide")
        molt_build = n_entry.get("molt_build_s")
        molt_time = native_molt_time(n_entry)
        molt_size = n_entry.get("molt_size_kb")
        wasm_time = _wasm_time_if_ok(w_entry)
        wasm_native_ratio = _safe_div(wasm_time, molt_time)
        wasm_cpython_ratio = _safe_div(wasm_time, cpython_time)
        lines.append(
            "| "
            f"{name} | {'yes' if n_entry.get('molt_ok') else 'no'} | "
            f"{_format_time(cpython_time)} | {_format_time(pypy_time)} | "
            f"{_format_time(codon_build)} | {_format_time(codon_time)} | "
            f"{_format_time(codon_size)} | {_format_time(nuitka_build)} | "
            f"{_format_time(nuitka_time)} | {_format_time(nuitka_size)} | "
            f"{_format_time(pyodide_time)} | {_format_time(molt_build)} | "
            f"{_format_time(molt_time)} | {_format_time(molt_size)} | "
            f"{_format_ratio(_safe_div(molt_time, cpython_time))} | "
            f"{_format_ratio(_safe_div(molt_time, pypy_time))} | "
            f"{_format_ratio(_safe_div(molt_time, codon_time))} | "
            f"{_format_ratio(_safe_div(molt_time, nuitka_time))} | "
            f"{_format_ratio(_safe_div(molt_time, pyodide_time))} | "
            f"{'yes' if w_entry.get('molt_wasm_ok') else 'no'} | "
            f"{_format_time(wasm_time)} | {_format_ratio(wasm_native_ratio)} | "
            f"{_format_ratio(wasm_cpython_ratio)} |"
        )

    lines.extend(_render_startup_section(startup_audit or {}))

    lines.append("")
    lines.append("Generated by `tools/bench_report.py`.")
    return "\n".join(lines) + "\n"


def _render_startup_section(startup_audit: dict[str, Any]) -> list[str]:
    """Render the cold-start + binary-size table from the audit JSON.

    Sources from tools/output_startup_size_audit.py. Columns separate the warm
    steady-state (same-path), the page-cache-cold copy, and the true cold
    first-sighting so cold-start is never conflated with throughput. The budget
    verdict column surfaces size/loader regressions.
    """
    lines: list[str] = ["", "## Cold Start & Binary Size"]
    rows = _startup_audit_rows(startup_audit)
    if not rows:
        lines.append(
            "Startup/size audit JSON not provided "
            "(`tools/output_startup_size_audit.py`); section empty."
        )
        return lines

    recorded = startup_audit.get("recorded_at") or startup_audit.get("created_at")
    if recorded:
        lines.append(f"- Audit recorded_at: {recorded}")
    c_ms = _startup_baseline_ms(startup_audit, "c")
    cpy_ms = _startup_baseline_ms(startup_audit, "cpython")
    lines.append(
        f"- Process baselines: C {_format_ms(c_ms)}ms, "
        f"CPython {_format_ms(cpy_ms)}ms (median)."
    )
    lines.append(
        "- Warm = same-path steady-state (provenance + page cache warm); "
        "page-cache-cold = fresh-copy load; cold(first-sighting) = the genuine "
        "first run of freshly built bytes (one-time macOS amfid tax)."
    )
    lines.append("")
    lines.append(
        "| Target | Profile | Backend | Size | Warm min/median ms | "
        "Page-cache-cold ms | Cold first-sighting ms | Budget |"
    )
    lines.append("| --- | --- | --- | --- | --- | --- | --- | --- |")
    for row in rows:
        if row["skipped"]:
            warm = page = cold = f"skipped: {row['skipped']}"
            size = _format_bytes(row["bytes"])
            lines.append(
                f"| {row['target']} | {row['profile']} | {row['backend']} | "
                f"{size} | {warm} | {page} | {cold} | "
                f"{'OK' if row['budget_ok'] else 'FAIL'} |"
            )
            continue
        warm = (
            f"{_format_ms(row['same_warm_min'])} / "
            f"{_format_ms(row['same_warm_median'])}"
        )
        lines.append(
            f"| {row['target']} | {row['profile']} | {row['backend']} | "
            f"{_format_bytes(row['bytes'])} | {warm} | "
            f"{_format_ms(row['page_cache_cold'])} | "
            f"{_format_ms(row['cold_first_sighting'])} | "
            f"{'OK' if row['budget_ok'] else 'FAIL'} |"
        )
    return lines


def _check_expected_file(path: Path, expected: str, label: str) -> None:
    if not path.exists():
        raise SystemExit(f"missing generated {label}: {path}")
    actual = path.read_text()
    if actual != expected:
        raise SystemExit(f"stale generated {label}: {path}")


def _resolve_manifest_path(path: str | None) -> Path | None:
    if path is None:
        return None
    candidate = Path(path)
    if candidate.is_absolute():
        return candidate
    return ROOT / candidate


def _load_manifest(path: Path) -> dict[str, Path]:
    payload = json.loads(path.read_text())
    resolved: dict[str, Path] = {}
    for key in ("native", "wasm", "out", "status_doc", "startup_audit"):
        raw_value = payload.get(key)
        if raw_value is None:
            continue
        resolved[key] = _resolve_manifest_path(raw_value)
    return resolved


def _default_manifest() -> dict[str, Path]:
    if DEFAULT_MANIFEST_PATH.exists():
        return _load_manifest(DEFAULT_MANIFEST_PATH)
    return {}


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(
        description="Generate a combined native+WASM benchmark report."
    )
    parser.add_argument(
        "--manifest",
        type=Path,
        help="JSON manifest with repo-relative native/wasm/out/status_doc paths.",
    )
    parser.add_argument(
        "--native",
        type=Path,
        default=None,
        help="Path to the native benchmark JSON.",
    )
    parser.add_argument(
        "--wasm",
        type=Path,
        default=None,
        help="Path to the WASM benchmark JSON.",
    )
    parser.add_argument(
        "--startup-audit",
        type=Path,
        default=None,
        help=(
            "Path to the output_startup_size_audit JSON for the cold-start / "
            "binary-size section. Absent file renders an empty section."
        ),
    )
    parser.add_argument(
        "--out",
        type=Path,
        default=None,
        help="Output Markdown report path.",
    )
    parser.add_argument(
        "--update-status-doc",
        action="store_true",
        help="Update docs/spec/STATUS.md benchmark summary block.",
    )
    parser.add_argument(
        "--check",
        action="store_true",
        help="Check generated report/status outputs without rewriting them.",
    )
    parser.add_argument(
        "--status-doc-path",
        type=Path,
        default=None,
        help="Path to STATUS.md for benchmark summary updates.",
    )
    args = parser.parse_args(argv)

    manifest = _load_manifest(args.manifest) if args.manifest else _default_manifest()
    native_path = (
        args.native or manifest.get("native") or Path("bench/results/bench.json")
    )
    wasm_path = (
        args.wasm or manifest.get("wasm") or Path("bench/results/bench_wasm.json")
    )
    out_path = (
        args.out or manifest.get("out") or Path("docs/benchmarks/bench_summary.md")
    )
    status_doc_path = (
        args.status_doc_path
        or manifest.get("status_doc")
        or Path("docs/spec/STATUS.md")
    )
    startup_audit_path = (
        args.startup_audit
        or manifest.get("startup_audit")
        or Path("bench/results/output_startup_size_audit.json")
    )

    native = _load_json(native_path)
    wasm = _load_json(wasm_path)
    startup_audit = (
        _load_json(startup_audit_path) if startup_audit_path.exists() else {}
    )
    report = _render_report_markdown(
        native_path, wasm_path, native, wasm, startup_audit
    )

    try:
        if args.check:
            _check_expected_file(out_path, report, "benchmark report")
        else:
            out_path.parent.mkdir(parents=True, exist_ok=True)
            out_path.write_text(report)
    except SystemExit as exc:
        print(exc, file=sys.stderr)
        return 1

    if args.update_status_doc:
        _, native_bench, wasm_bench = _collect_benchmarks(native, wasm)
        summary_block = _status_summary(
            native, wasm, native_bench, wasm_bench, startup_audit
        )
        try:
            if args.check:
                if not status_doc_path.exists():
                    raise SystemExit(f"missing generated STATUS doc: {status_doc_path}")
                updated = _render_updated_status_doc(
                    status_doc_path.read_text(), status_doc_path, summary_block
                )
                _check_expected_file(status_doc_path, updated, "STATUS benchmark block")
            else:
                _update_status_doc(status_doc_path, summary_block)
        except SystemExit as exc:
            print(exc, file=sys.stderr)
            return 1
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
