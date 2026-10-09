from __future__ import annotations

import sys
from pathlib import Path

from tools.fuzz_compiler_types import FuzzResult

# ---------------------------------------------------------------------------
# Logging and reporting
# ---------------------------------------------------------------------------


def _log(msg: str) -> None:
    print(msg, file=sys.stderr, flush=True)


def _save_failure(result: FuzzResult, output_dir: Path) -> Path:
    output_dir.mkdir(parents=True, exist_ok=True)
    source_file = output_dir / f"fuzz_{result.program_id:06d}.py"
    source_file.write_text(result.source, encoding="utf-8")
    report_file = output_dir / f"fuzz_{result.program_id:06d}.report.txt"
    report_lines = [
        f"Fuzz ID: {result.program_id}",
        f"Seed: {result.seed}",
        f"Status: {result.status}",
        f"Elapsed: {result.elapsed_sec:.2f}s",
        "",
        "=== CPython stdout ===",
        result.cpython_stdout,
        "=== Molt stdout ===",
        result.molt_stdout,
        "=== CPython stderr ===",
        result.cpython_stderr,
        "=== Molt stderr ===",
        result.molt_stderr,
    ]
    if result.error_detail:
        report_lines.extend(["", "=== Error Detail ===", result.error_detail])
    report_file.write_text("\n".join(report_lines), encoding="utf-8")
    return source_file


def _log_failure_detail(result: FuzzResult) -> None:
    """Print a failure's evidence when it happens.

    A campaign can be stopped before it writes its receipt, so each failure
    leaves its own diagnosis in the log.
    """
    detail = " ".join(result.error_detail.split())[:300]
    if detail:
        _log(f"         {detail}")
    for line in result.molt_stderr.strip().splitlines()[-3:]:
        _log(f"         molt stderr: {line[:200]}")
    if result.status in {"mismatch", "molt_run_error"}:
        _print_diff_snippet(result)


def _print_diff_snippet(result: FuzzResult, max_diffs: int = 5) -> None:
    cp_lines = result.cpython_stdout.splitlines()
    molt_lines = result.molt_stdout.splitlines()

    def line(lines: list[str], index: int) -> str:
        return lines[index] if index < len(lines) else "<missing>"

    differing = [
        i
        for i in range(max(len(cp_lines), len(molt_lines)))
        if line(cp_lines, i) != line(molt_lines, i)
    ]
    for i in differing[:max_diffs]:
        _log(f"    line {i + 1}:")
        _log(f"      CPython: {line(cp_lines, i)[:200]!r}")
        _log(f"      Molt:    {line(molt_lines, i)[:200]!r}")
    if len(differing) > max_diffs:
        _log(f"    ... and {len(differing) - max_diffs} more differing lines")
