#!/usr/bin/env python3
"""Generate a combined native+WASM benchmark report in Markdown."""

from __future__ import annotations

import argparse
import json
import statistics
from datetime import datetime, timezone
from pathlib import Path
from typing import Any


def _load_json(path: Path) -> dict[str, Any]:
    if not path.exists():
        raise SystemExit(f"missing benchmark file: {path}")
    return json.loads(path.read_text())


def _normalize_name(name: str) -> str:
    return name[:-3] if name.endswith(".py") else name


def _display_name(name: str) -> str:
    return name if name.endswith(".py") else f"{name}.py"


def _safe_div(num: float | None, den: float | None) -> float | None:
    if num is None or den is None or den == 0:
        return None
    return num / den


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
    cython_available = any(entry.get("cython_ok") for entry in native_bench.values())
    numba_available = any(entry.get("numba_ok") for entry in native_bench.values())
    codon_available = any(entry.get("codon_ok") for entry in native_bench.values())

    parts: list[str] = []

    if not cython_available and not numba_available:
        parts.append("Cython/Numba baselines unavailable")
    else:
        if not cython_available:
            parts.append("Cython baseline unavailable")
        else:
            cython_missing = sorted(
                _display_name(name)
                for name, entry in native_bench.items()
                if not entry.get("cython_ok")
            )
            if cython_missing:
                parts.append(f"Cython skipped for {_format_name_list(cython_missing)}")
        if not numba_available:
            parts.append("Numba baseline unavailable")
        else:
            numba_missing = sorted(
                _display_name(name)
                for name, entry in native_bench.items()
                if not entry.get("numba_ok")
            )
            if numba_missing:
                parts.append(f"Numba skipped for {_format_name_list(numba_missing)}")

    if not codon_available:
        parts.append("Codon baseline unavailable")
    else:
        codon_missing = sorted(
            _display_name(name)
            for name, entry in native_bench.items()
            if not entry.get("codon_ok")
        )
        if codon_missing:
            parts.append(f"Codon skipped for {_format_name_list(codon_missing)}")

    if not parts:
        return "none"
    return "; ".join(parts)


def _readme_summary(
    native: dict[str, Any],
    wasm: dict[str, Any],
    native_bench: dict[str, Any],
    wasm_bench: dict[str, Any],
) -> str:
    speedups = [
        (name, entry["molt_speedup"])
        for name, entry in native_bench.items()
        if entry.get("molt_ok") and entry.get("molt_speedup") is not None
    ]
    speedups.sort(key=lambda item: item[1], reverse=True)

    regressions = [
        (name, entry["molt_speedup"])
        for name, entry in native_bench.items()
        if entry.get("molt_ok")
        and entry.get("molt_speedup") is not None
        and entry["molt_speedup"] < 1.0
    ]
    regressions.sort(key=lambda item: item[1])

    slowest = sorted(speedups, key=lambda item: item[1])

    wasm_times = [
        (name, entry["molt_wasm_time_s"])
        for name, entry in wasm_bench.items()
        if entry.get("molt_wasm_time_s") is not None
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
        wasm_time = entry.get("molt_wasm_time_s")
        cpython_time = native_bench.get(name, {}).get("cpython_time_s")
        ratio = _safe_div(wasm_time, cpython_time)
        if ratio is not None:
            wasm_ratios.append((name, ratio))
    wasm_ratios.sort(key=lambda item: item[1], reverse=True)

    native_date = _extract_date(native.get("created_at"))
    wasm_date = _extract_date(wasm.get("created_at"))

    native_system = _summarize_system(native.get("system"))
    wasm_system = _summarize_system(wasm.get("system"))

    summary_lines = [
        f"Latest run: {native_date} ({native_system}).",
        f"Top speedups: {_format_speedup_list(speedups, 5)}.",
        f"Regressions: {_format_speedup_list(regressions, len(regressions))}.",
        f"Slowest: {_format_speedup_list(slowest, 3)}.",
        f"Build/run failures: {_baseline_summary(native_bench)}.",
        (
            f"WASM run: {wasm_date} ({wasm_system}). "
            f"Slowest: {_format_time_list(wasm_times, 3)}; "
            f"largest sizes: {_format_size_list(wasm_sizes, 3)}; "
            f"WASM vs CPython slowest ratios: {_format_ratio_list(wasm_ratios, 3)}."
        ),
    ]
    return "\n".join(summary_lines)


def _update_readme(readme_path: Path, summary_block: str) -> None:
    marker_start = "<!-- BENCH_SUMMARY_START -->"
    marker_end = "<!-- BENCH_SUMMARY_END -->"
    content = readme_path.read_text()
    if marker_start not in content or marker_end not in content:
        raise SystemExit(
            f"missing README markers {marker_start}/{marker_end} in {readme_path}"
        )
    before, rest = content.split(marker_start, maxsplit=1)
    _, after = rest.split(marker_end, maxsplit=1)
    updated = f"{before}{marker_start}\n{summary_block}\n{marker_end}{after}"
    readme_path.write_text(updated)


def _render_report(
    native_path: Path,
    wasm_path: Path,
    out_path: Path,
    native: dict[str, Any],
    wasm: dict[str, Any],
) -> None:
    names, native_bench, wasm_bench = _collect_benchmarks(native, wasm)

    native_ok = sum(1 for entry in native_bench.values() if entry.get("molt_ok"))
    wasm_ok = sum(1 for entry in wasm_bench.values() if entry.get("molt_wasm_ok"))

    native_speedups = [
        entry["molt_speedup"]
        for entry in native_bench.values()
        if entry.get("molt_ok") and entry.get("molt_speedup")
    ]

    wasm_speedups = []
    wasm_native_ratios = []
    regressions = []
    wasm_slowest = []

    for name in names:
        n_entry = native_bench.get(name, {})
        w_entry = wasm_bench.get(name, {})

        molt_time = n_entry.get("molt_time_s")
        cpython_time = n_entry.get("cpython_time_s")
        speedup = n_entry.get("molt_speedup")

        wasm_time = w_entry.get("molt_wasm_time_s")

        wasm_speedup = _safe_div(cpython_time, wasm_time)
        wasm_native_ratio = _safe_div(wasm_time, molt_time)

        if wasm_speedup is not None:
            wasm_speedups.append(wasm_speedup)

        if wasm_native_ratio is not None:
            wasm_native_ratios.append(wasm_native_ratio)
            wasm_slowest.append((name, wasm_time, molt_time, wasm_native_ratio))

        if speedup is not None and speedup < 1.0:
            regressions.append((name, speedup, molt_time, cpython_time))

    regressions.sort(key=lambda item: item[1])
    wasm_slowest.sort(key=lambda item: item[3], reverse=True)

    missing_native = sorted(set(wasm_bench) - set(native_bench))
    missing_wasm = sorted(set(native_bench) - set(wasm_bench))

    generated = datetime.now(timezone.utc).replace(microsecond=0).isoformat()
    if generated.endswith("+00:00"):
        generated = generated.replace("+00:00", "Z")

    native_rev = native.get("git_rev") or "-"
    wasm_rev = wasm.get("git_rev") or "-"

    native_created = native.get("created_at") or "-"
    wasm_created = wasm.get("created_at") or "-"

    native_system = _format_system(native.get("system"))
    wasm_system = _format_system(wasm.get("system"))

    lines: list[str] = []
    lines.append("# Molt Bench Summary")
    lines.append("")
    lines.append(f"Generated: {generated}")
    lines.append("")
    lines.append("## Inputs")
    lines.append(
        f"- Native: `{native_path}`; git_rev={native_rev}; created_at={native_created}; "
        f"system={native_system}"
    )
    lines.append(
        f"- WASM: `{wasm_path}`; git_rev={wasm_rev}; created_at={wasm_created}; "
        f"system={wasm_system}"
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

    lines.append("## Combined Table")
    lines.append(
        "| Benchmark | Native OK | CPython s | Molt s | Speedup | "
        "WASM OK | WASM s | WASM/Native | WASM/CPython |"
    )
    lines.append("| --- | --- | --- | --- | --- | --- | --- | --- | --- |")
    for name in names:
        n_entry = native_bench.get(name, {})
        w_entry = wasm_bench.get(name, {})

        cpython_time = n_entry.get("cpython_time_s")
        molt_time = n_entry.get("molt_time_s")
        speedup = n_entry.get("molt_speedup")

        wasm_time = w_entry.get("molt_wasm_time_s")
        wasm_native_ratio = _safe_div(wasm_time, molt_time)
        wasm_speedup = _safe_div(cpython_time, wasm_time)

        native_ok_label = "yes" if n_entry.get("molt_ok") else "no"
        wasm_ok_label = "yes" if w_entry.get("molt_wasm_ok") else "no"

        lines.append(
            "| "
            f"{name} | {native_ok_label} | {_format_time(cpython_time)} | "
            f"{_format_time(molt_time)} | {_format_ratio(speedup)} | "
            f"{wasm_ok_label} | {_format_time(wasm_time)} | "
            f"{_format_ratio(wasm_native_ratio)} | {_format_ratio(wasm_speedup)} |"
        )

    lines.append("")
    lines.append("Generated by `tools/bench_report.py`.")

    out_path.parent.mkdir(parents=True, exist_ok=True)
    out_path.write_text("\n".join(lines) + "\n")


def main() -> None:
    parser = argparse.ArgumentParser(
        description="Generate a combined native+WASM benchmark report."
    )
    parser.add_argument(
        "--native",
        type=Path,
        default=Path("bench/results/bench.json"),
        help="Path to the native benchmark JSON.",
    )
    parser.add_argument(
        "--wasm",
        type=Path,
        default=Path("bench/results/bench_wasm.json"),
        help="Path to the WASM benchmark JSON.",
    )
    parser.add_argument(
        "--out",
        type=Path,
        default=Path("docs/benchmarks/bench_summary.md"),
        help="Output Markdown report path.",
    )
    parser.add_argument(
        "--update-readme",
        action="store_true",
        help="Update README Performance & Comparisons summary block.",
    )
    parser.add_argument(
        "--readme",
        type=Path,
        default=Path("README.md"),
        help="Path to README for summary updates.",
    )
    args = parser.parse_args()

    native = _load_json(args.native)
    wasm = _load_json(args.wasm)
    _render_report(args.native, args.wasm, args.out, native, wasm)
    if args.update_readme:
        _, native_bench, wasm_bench = _collect_benchmarks(native, wasm)
        summary_block = _readme_summary(native, wasm, native_bench, wasm_bench)
        _update_readme(args.readme, summary_block)


if __name__ == "__main__":
    main()
