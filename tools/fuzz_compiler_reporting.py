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
    source_file.write_text(result.source)
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
    report_file.write_text("\n".join(report_lines))
    return source_file


def _print_diff_snippet(result: FuzzResult, max_lines: int = 15) -> None:
    cp_lines = result.cpython_stdout.splitlines()
    molt_lines = result.molt_stdout.splitlines()
    printed = 0
    max_len = max(len(cp_lines), len(molt_lines))
    for i in range(min(max_len, max_lines)):
        cp_line = cp_lines[i] if i < len(cp_lines) else "<missing>"
        molt_line = molt_lines[i] if i < len(molt_lines) else "<missing>"
        if cp_line != molt_line:
            _log(f"    line {i + 1}:")
            _log(f"      CPython: {cp_line!r}")
            _log(f"      Molt:    {molt_line!r}")
            printed += 1
            if printed >= 5:
                remaining = sum(
                    1
                    for j in range(i + 1, max_len)
                    if (cp_lines[j] if j < len(cp_lines) else "")
                    != (molt_lines[j] if j < len(molt_lines) else "")
                )
                if remaining > 0:
                    _log(f"    ... and {remaining} more differing lines")
                break
