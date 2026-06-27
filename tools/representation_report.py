#!/usr/bin/env python3
"""Report backend-proven TIR/LIR representation coverage.

The report compiles source to SimpleIR, delegates representation accounting to
the `molt-backend` `typed_repr_report` binary, and fails if backend lowering or
LIR representation verification fails. It does not count legacy frontend
transport hints such as `fast_int`, `raw_int`, or `type_hint` as optimization
evidence.

Example:
  python3 tools/representation_report.py examples/hello.py
"""

from __future__ import annotations

import argparse
import json
import os
import sys
from collections import Counter
from dataclasses import dataclass
from pathlib import Path
from typing import Any

from molt.frontend import compile_to_tir

REPO_ROOT = Path(__file__).resolve().parents[1]
if str(REPO_ROOT) not in sys.path:
    sys.path.insert(0, str(REPO_ROOT))

from tools import harness_memory_guard  # noqa: E402
from molt.dx import development_artifact_env  # noqa: E402

BACKEND_SCHEMA = "molt.typed_repr_report.v1"
DEFAULT_MARKDOWN_PATH = (
    REPO_ROOT
    / "docs"
    / "spec"
    / "areas"
    / "compiler"
    / "backend_lir_representation.generated.md"
)
DEFAULT_REPORT_PATHS = (REPO_ROOT / "examples" / "hello.py",)


class BackendReportError(RuntimeError):
    """Raised when backend representation evidence cannot be produced."""


@dataclass(frozen=True)
class FileReport:
    path: Path
    report: dict[str, Any]


def _canonical_env() -> dict[str, str]:
    return development_artifact_env(
        REPO_ROOT,
        os.environ,
        session_prefix="representation-report",
        session_id=os.environ.get("MOLT_SESSION_ID") or "representation-report",
        create_dirs=True,
    )


def _display_path(path: Path) -> str:
    try:
        return str(path.relative_to(REPO_ROOT))
    except ValueError:
        return str(path)


def backend_report_command() -> list[str]:
    return [
        "cargo",
        "run",
        "--quiet",
        "--profile",
        "release-fast",
        "-p",
        "molt-backend",
        "--bin",
        "typed_repr_report",
        "--",
        "--stdin",
        "--json",
    ]


def run_backend_report(ir: dict[str, Any]) -> dict[str, Any]:
    payload = json.dumps(ir, sort_keys=True, separators=(",", ":"))
    env = _canonical_env()
    limits = harness_memory_guard.limits_from_env("MOLT_TEST_SUITE", env)
    completed = harness_memory_guard.guarded_completed_process(
        backend_report_command(),
        prefix="MOLT_TEST_SUITE",
        input=payload,
        text=True,
        capture_output=True,
        cwd=REPO_ROOT,
        env=env,
        limits=limits,
    )
    if completed.returncode != 0:
        detail = completed.stderr.strip() or completed.stdout.strip()
        raise BackendReportError(
            f"typed_repr_report failed with exit code {completed.returncode}: {detail}"
        )
    try:
        report = json.loads(completed.stdout)
    except json.JSONDecodeError as exc:
        raise BackendReportError(
            f"typed_repr_report emitted invalid JSON: {exc}"
        ) from exc
    if report.get("schema") != BACKEND_SCHEMA:
        raise BackendReportError(
            f"typed_repr_report schema mismatch: {report.get('schema')!r}"
        )
    if report.get("verified") is not True:
        raise BackendReportError("typed_repr_report returned unverified LIR")
    return report


def analyze_file(path: Path, type_hints: str) -> FileReport:
    source = path.read_text(encoding="utf-8")
    ir = compile_to_tir(source, type_hint_policy=type_hints)
    return FileReport(path=path, report=run_backend_report(ir))


def _counter_from_json(value: object) -> Counter[str]:
    counter: Counter[str] = Counter()
    if isinstance(value, dict):
        for key, raw_count in value.items():
            if isinstance(raw_count, int):
                counter[str(key)] += raw_count
    return counter


def aggregate_reports(file_reports: list[FileReport]) -> dict[str, Any]:
    values_by_repr: Counter[str] = Counter()
    values_by_type: Counter[str] = Counter()
    opcodes: dict[str, dict[str, Any]] = {}
    scalar_values = 0
    reference_values = 0
    boxed_values = 0
    lir_errors = 0
    repr_violations = 0
    functions = 0

    for file_report in file_reports:
        aggregate = file_report.report["aggregate"]
        functions += int(aggregate.get("functions", 0))
        scalar_values += int(aggregate.get("scalar_values", 0))
        reference_values += int(aggregate.get("reference_values", 0))
        boxed_values += int(aggregate.get("boxed_values", 0))
        lir_errors += int(aggregate.get("lir_errors", 0))
        repr_violations += int(aggregate.get("repr_violations", 0))
        values_by_repr.update(_counter_from_json(aggregate.get("values_by_repr")))
        values_by_type.update(_counter_from_json(aggregate.get("values_by_type")))

        for opcode, raw_stats in aggregate.get("opcodes", {}).items():
            if not isinstance(raw_stats, dict):
                continue
            stats = opcodes.setdefault(
                str(opcode),
                {
                    "total": 0,
                    "result_reprs": Counter(),
                    "operand_repr_tuples": Counter(),
                    "boxed_result_values": 0,
                },
            )
            stats["total"] += int(raw_stats.get("total", 0))
            stats["boxed_result_values"] += int(raw_stats.get("boxed_result_values", 0))
            stats["result_reprs"].update(
                _counter_from_json(raw_stats.get("result_reprs"))
            )
            stats["operand_repr_tuples"].update(
                _counter_from_json(raw_stats.get("operand_repr_tuples"))
            )

    return {
        "functions": functions,
        "values_by_repr": dict(sorted(values_by_repr.items())),
        "values_by_type": dict(sorted(values_by_type.items())),
        "scalar_values": scalar_values,
        "reference_values": reference_values,
        "boxed_values": boxed_values,
        "lir_errors": lir_errors,
        "repr_violations": repr_violations,
        "opcodes": {
            opcode: {
                "total": stats["total"],
                "result_reprs": dict(sorted(stats["result_reprs"].items())),
                "operand_repr_tuples": dict(
                    sorted(stats["operand_repr_tuples"].items())
                ),
                "boxed_result_values": stats["boxed_result_values"],
            }
            for opcode, stats in sorted(opcodes.items())
        },
    }


def report_payload(file_reports: list[FileReport], type_hints: str) -> dict[str, Any]:
    return {
        "schema": BACKEND_SCHEMA,
        "type_hints": type_hints,
        "files": {
            str(file_report.path): file_report.report for file_report in file_reports
        },
        "aggregate": aggregate_reports(file_reports),
    }


def _format_count_map(title: str, counts: dict[str, int]) -> list[str]:
    lines = [title]
    if not counts:
        return lines + ["  none"]
    width = max(len(key) for key in counts)
    return lines + [f"  {key:<{width}} {count:6d}" for key, count in counts.items()]


def format_report(payload: dict[str, Any]) -> str:
    aggregate = payload["aggregate"]
    lines = [
        f"type_hints={payload['type_hints']}",
        "== Backend LIR Representation Coverage ==",
        f"functions={aggregate['functions']}",
        f"scalar_values={aggregate['scalar_values']}",
        f"reference_values={aggregate['reference_values']}",
        f"boxed_values={aggregate['boxed_values']}",
        f"lir_errors={aggregate['lir_errors']}",
        f"repr_violations={aggregate['repr_violations']}",
        "",
    ]
    lines.extend(_format_count_map("== Values By Repr ==", aggregate["values_by_repr"]))
    lines.append("")
    lines.extend(_format_count_map("== Values By Type ==", aggregate["values_by_type"]))
    lines.append("")
    lines.append("== Opcode Result Reprs ==")
    if not aggregate["opcodes"]:
        lines.append("  none")
    for opcode, stats in aggregate["opcodes"].items():
        result_reprs = ", ".join(
            f"{repr_name}:{count}" for repr_name, count in stats["result_reprs"].items()
        )
        lines.append(
            f"  {opcode:<22} total={stats['total']:5d} "
            f"boxed_results={stats['boxed_result_values']:5d} "
            f"results=[{result_reprs}]"
        )
    return "\n".join(lines)


def format_markdown_report(payload: dict[str, Any]) -> str:
    aggregate = payload["aggregate"]
    lines = [
        "# Backend LIR Representation Report",
        "",
        "<!-- GENERATED by tools/representation_report.py --update-doc. Do not hand-edit. -->",
        "",
        "This generated report records backend-proven LIR representation evidence.",
        "The backend reporter verifies LIR before admitting counts, so values here are",
        "lowering evidence rather than frontend transport-hint evidence.",
        "",
        "## Representation Contract",
        "",
        "| LIR repr | Accounting lane | Contract |",
        "| --- | --- | --- |",
        "| `i64` | scalar | Semantic signed integer machine scalar. |",
        "| `f64` | scalar | Semantic floating-point machine scalar. |",
        "| `bool1` | scalar | Semantic boolean predicate scalar. |",
        "| `ref64` | reference | Runtime reference-word carrier for typed object references; not a semantic `i64`. |",
        "| `dynbox` | reference + boxed | Runtime boxed object reference. |",
        "",
        "## Aggregate",
        "",
        f"- type_hints: `{payload['type_hints']}`",
        f"- functions: `{aggregate['functions']}`",
        f"- scalar_values: `{aggregate['scalar_values']}`",
        f"- reference_values: `{aggregate['reference_values']}`",
        f"- boxed_values: `{aggregate['boxed_values']}`",
        f"- lir_errors: `{aggregate['lir_errors']}`",
        f"- repr_violations: `{aggregate['repr_violations']}`",
        "",
    ]
    lines.extend(_markdown_count_table("Values By Repr", aggregate["values_by_repr"]))
    lines.append("")
    lines.extend(_markdown_count_table("Values By Type", aggregate["values_by_type"]))
    lines.append("")
    lines.extend(_markdown_opcode_table(aggregate["opcodes"]))
    lines.append("")
    return "\n".join(lines)


def _markdown_count_table(title: str, counts: dict[str, int]) -> list[str]:
    lines = [f"## {title}", "", "| Name | Count |", "| --- | ---: |"]
    if not counts:
        return lines + ["| none | 0 |"]
    return lines + [f"| `{name}` | {count} |" for name, count in counts.items()]


def _markdown_opcode_table(opcodes: dict[str, Any]) -> list[str]:
    lines = [
        "## Opcode Result Representations",
        "",
        "| Opcode | Total | Boxed results | Result reprs |",
        "| --- | ---: | ---: | --- |",
    ]
    if not opcodes:
        return lines + ["| none | 0 | 0 |  |"]
    for opcode, stats in opcodes.items():
        result_reprs = ", ".join(
            f"`{repr_name}`:{count}"
            for repr_name, count in stats["result_reprs"].items()
        )
        lines.append(
            f"| `{opcode}` | {stats['total']} | {stats['boxed_result_values']} | {result_reprs} |"
        )
    return lines


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("paths", nargs="*", type=Path, help="Python source files")
    parser.add_argument(
        "--type-hints",
        choices=("ignore", "trust", "check"),
        default="check",
        help="Frontend type hint policy (default: check)",
    )
    parser.add_argument("--json", action="store_true", help="Emit JSON report")
    parser.add_argument(
        "--markdown-out",
        type=Path,
        help="Write generated Markdown report to the given path",
    )
    parser.add_argument(
        "--update-doc",
        action="store_true",
        help=f"Refresh {_display_path(DEFAULT_MARKDOWN_PATH)}",
    )
    args = parser.parse_args(argv)

    paths = args.paths or list(DEFAULT_REPORT_PATHS)
    file_reports = [analyze_file(path, args.type_hints) for path in paths]
    payload = report_payload(file_reports, args.type_hints)
    markdown_path = DEFAULT_MARKDOWN_PATH if args.update_doc else args.markdown_out
    if markdown_path is not None:
        markdown_path.parent.mkdir(parents=True, exist_ok=True)
        markdown_path.write_text(format_markdown_report(payload), encoding="utf-8")
    if args.json:
        json.dump(payload, sys.stdout, indent=2, sort_keys=True)
        print()
    else:
        print(format_report(payload))
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
