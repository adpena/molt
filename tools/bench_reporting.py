from __future__ import annotations

from collections.abc import Callable
import json
from pathlib import Path

from bench_evidence import comparable_run_metadata_errors


MAX_FAILURE_DETAIL_RECORDS = 32


def write_json(path: Path, payload: dict) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(json.dumps(payload, indent=2, sort_keys=True) + "\n")


def load_json(path: Path) -> dict:
    return json.loads(path.read_text())


def compare_baseline(
    current: dict,
    baseline: dict,
    max_regression: float,
) -> list[str]:
    metadata_errors = comparable_run_metadata_errors(current, baseline)
    if metadata_errors:
        return [
            "incompatible benchmark baseline: "
            + "; ".join(metadata_errors)
            + "; regenerate the baseline with matching benchmark timing settings"
        ]

    regressions = []
    baseline_bench = baseline.get("benchmarks", {})
    for name, stats in current.get("benchmarks", {}).items():
        current_ratio = stats.get("molt_cpython_ratio")
        base_ratio = baseline_bench.get(name, {}).get("molt_cpython_ratio")
        if current_ratio is None or base_ratio is None:
            continue
        if current_ratio > base_ratio * (1 + max_regression):
            regressions.append(
                f"{name}: ratio {current_ratio:.4f} > "
                f"{base_ratio:.4f} * {1 + max_regression:.2f}"
            )
    return regressions


def summary_path_for_json(json_out: Path, explicit: Path | None) -> Path:
    if explicit is not None:
        return explicit
    if json_out.name == "results.json":
        return json_out.with_name("summary.md")
    return json_out.with_name(f"{json_out.stem}_summary.md")


def failure_details_path_for_json(json_out: Path) -> Path:
    if json_out.name == "results.json":
        return json_out.with_name("molt_failure_details.jsonl")
    return json_out.with_name(f"{json_out.stem}_molt_failure_details.jsonl")


def bench_custody_artifacts(
    *,
    json_out: Path,
    summary_out: Path,
    artifact_root: Path,
    failure_details_path: Path,
) -> dict[str, str]:
    memory_guard_root = artifact_root / "memory_guard"
    return {
        "results_json": str(json_out),
        "summary_md": str(summary_out),
        "molt_failure_details_jsonl": str(failure_details_path),
        "harness_command_profile_jsonl": str(memory_guard_root / "commands.jsonl"),
        "repo_process_sentinel_jsonl": str(memory_guard_root / "bench_sentinel.jsonl"),
        "backend_daemon_cleanup_jsonl": str(
            memory_guard_root / "backend_daemon_cleanup.jsonl"
        ),
    }


def molt_failure_detail_records(
    benchmarks: dict[str, object],
    *,
    bounded_failure_text: Callable[[object], str | None],
    max_records: int = MAX_FAILURE_DETAIL_RECORDS,
) -> dict[str, object]:
    records: list[dict[str, object]] = []
    total = 0
    for benchmark_name, raw_stats in sorted(benchmarks.items()):
        if not isinstance(raw_stats, dict):
            continue
        raw_failure = raw_stats.get("molt_failure")
        if not isinstance(raw_failure, dict):
            continue
        total += 1
        if len(records) >= max_records:
            continue
        records.append(
            {
                "benchmark": benchmark_name,
                "phase": raw_failure.get("phase"),
                "status": raw_failure.get("status"),
                "detail": raw_failure.get("detail"),
                "returncode": raw_failure.get("returncode"),
                "timed_out": raw_failure.get("timed_out"),
                "elapsed_s": raw_failure.get("elapsed_s"),
                "message": bounded_failure_text(raw_failure.get("message")),
                "guard_violation": raw_failure.get("guard_violation"),
                "signal": raw_failure.get("signal"),
                "orphaned_process_groups": raw_failure.get("orphaned_process_groups"),
                "log_refs": raw_failure.get("log_refs", []),
            }
        )
    return {
        "schema_version": 1,
        "total": total,
        "truncated": total > len(records),
        "max_records": max_records,
        "records": records,
    }


def write_failure_details_jsonl(
    path: Path,
    failure_details: dict[str, object],
) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    records = failure_details.get("records", [])
    if not isinstance(records, list):
        records = []
    with path.open("w", encoding="utf-8") as handle:
        for record in records:
            if isinstance(record, dict):
                handle.write(json.dumps(record, sort_keys=True) + "\n")


def format_summary_seconds(value: object) -> str:
    if not isinstance(value, (int, float)):
        return "-"
    return f"{float(value):.4f}"


def render_bench_summary_markdown(
    payload: dict[str, object],
    *,
    max_failure_detail_records: int = MAX_FAILURE_DETAIL_RECORDS,
) -> str:
    custody_artifacts = payload.get("custody_artifacts")
    if not isinstance(custody_artifacts, dict):
        custody_artifacts = {}
    failure_details = payload.get("molt_failure_details")
    if not isinstance(failure_details, dict):
        failure_details = {"records": [], "total": 0, "truncated": False}
    records = failure_details.get("records", [])
    if not isinstance(records, list):
        records = []

    lines: list[str] = [
        "# Molt Benchmark Summary",
        "",
        f"Generated: {payload.get('created_at', '')}",
    ]
    if custody_artifacts.get("results_json"):
        lines.append(f"JSON: `{custody_artifacts['results_json']}`")
    lines.extend(
        [
            "",
            "| Benchmark | Molt Status | CPython s | Molt s | Molt/CPython | Failure |",
            "| --- | --- | ---: | ---: | ---: | --- |",
        ]
    )
    benchmarks = payload.get("benchmarks")
    if isinstance(benchmarks, dict):
        for name, raw_stats in sorted(benchmarks.items()):
            if not isinstance(raw_stats, dict):
                continue
            failure = raw_stats.get("molt_failure")
            failure_text = "-"
            if isinstance(failure, dict):
                detail = failure.get("detail")
                failure_text = str(failure.get("status", "failed"))
                if detail:
                    failure_text = f"{failure_text} ({detail})"
            lines.append(
                "| "
                f"{name} | {raw_stats.get('molt_status', 'unknown')} | "
                f"{format_summary_seconds(raw_stats.get('cpython_time_s'))} | "
                f"{format_summary_seconds(raw_stats.get('molt_time_s'))} | "
                f"{format_summary_seconds(raw_stats.get('molt_cpython_ratio'))} | "
                f"{failure_text} |"
            )

    lines.extend(["", "## Custody Artifacts"])
    for key in (
        "molt_failure_details_jsonl",
        "harness_command_profile_jsonl",
        "repo_process_sentinel_jsonl",
        "backend_daemon_cleanup_jsonl",
    ):
        value = custody_artifacts.get(key)
        if value:
            lines.append(f"- `{key}`: `{value}`")

    if records:
        lines.extend(["", "## Molt Failure Details"])
        for record in records:
            if not isinstance(record, dict):
                continue
            detail = record.get("detail")
            detail_text = f" detail=`{detail}`" if detail else ""
            lines.append(
                f"- `{record.get('benchmark')}` phase=`{record.get('phase')}` "
                f"status=`{record.get('status')}`{detail_text}"
            )
            log_refs = record.get("log_refs")
            if isinstance(log_refs, list):
                for ref in log_refs[:4]:
                    if isinstance(ref, dict) and ref.get("path"):
                        lines.append(
                            f"  - {ref.get('kind', 'log')}: `{ref.get('path')}`"
                        )
        if failure_details.get("truncated"):
            lines.append(
                "- Failure detail list truncated at "
                f"{max_failure_detail_records} records."
            )

    lines.extend(["", "Generated by `tools/bench.py`."])
    return "\n".join(lines) + "\n"
