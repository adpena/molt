#!/usr/bin/env python3
"""Transport and presentation for the canonical Rust SimpleIR verifier.

Control-flow reconstruction, dominance, PHI ordering, generated field roles,
and structural validation live in ``runtime/molt-ir``. This module owns only
typed transport and CLI presentation; it contains no second verifier API.
"""

from __future__ import annotations

import argparse
import json
import sys
from dataclasses import dataclass, field
from pathlib import Path
from typing import Any

ROOT = Path(__file__).resolve().parents[1]
if str(ROOT) not in sys.path:
    sys.path.insert(0, str(ROOT))

from tools.rust_ir_verifier import (  # noqa: E402
    close_process_local_verifier,
    verify_ir,
)


@dataclass
class Diagnostic:
    function: str
    op_index: int
    kind: str
    message: str

    def __str__(self) -> str:
        return (
            f"  [{self.kind}] function {self.function!r}, "
            f"op #{self.op_index}: {self.message}"
        )


@dataclass
class VerificationResult:
    errors: list[Diagnostic] = field(default_factory=list)
    warnings: list[Diagnostic] = field(default_factory=list)
    functions_checked: int = 0
    ops_checked: int = 0
    verifier_pid: int | None = None
    verifier_cpu_seconds: float = 0.0
    verifier_lifetime_peak_rss_bytes: int = 0

    @property
    def ok(self) -> bool:
        return not self.errors


def _diagnostics(payload: object) -> list[Diagnostic]:
    if not isinstance(payload, list):
        raise RuntimeError("Rust IR verifier diagnostics are not a list")
    diagnostics: list[Diagnostic] = []
    for item in payload:
        if not isinstance(item, dict):
            raise RuntimeError("Rust IR verifier diagnostic is not an object")
        diagnostics.append(
            Diagnostic(
                function=str(item.get("function", "<unknown>")),
                op_index=int(item.get("op_index", -1)),
                kind=str(item.get("kind", "invalid-diagnostic")),
                message=str(item.get("message", "")),
            )
        )
    return diagnostics


def _verification_result(report: dict[str, Any]) -> VerificationResult:
    process = report.get("verifier_process")
    process_payload = process if isinstance(process, dict) else {}
    return VerificationResult(
        errors=_diagnostics(report.get("errors")),
        warnings=_diagnostics(report.get("warnings")),
        functions_checked=int(report.get("functions_checked", 0)),
        ops_checked=int(report.get("ops_checked", 0)),
        verifier_pid=(
            int(process_payload["pid"])
            if isinstance(process_payload.get("pid"), int)
            else None
        ),
        verifier_cpu_seconds=float(process_payload.get("cpu_seconds", 0.0)),
        verifier_lifetime_peak_rss_bytes=int(
            process_payload.get("lifetime_peak_rss_bytes", 0)
        ),
    )


def verify_tir(
    tir: dict[str, Any],
    *,
    request_id: int | None = None,
    timeout_seconds: float | None = None,
) -> VerificationResult:
    """Verify a complete SimpleIR document with the canonical Rust oracle."""
    if not isinstance(tir, dict):
        return VerificationResult(
            errors=[
                Diagnostic(
                    function="<top-level>",
                    op_index=-1,
                    kind="invalid-format",
                    message="TIR JSON root must be an object",
                )
            ]
        )
    try:
        report = verify_ir(
            tir,
            request_id=request_id,
            timeout_seconds=timeout_seconds,
        )
    except (TypeError, ValueError) as exc:
        return VerificationResult(
            errors=[
                Diagnostic(
                    function="<top-level>",
                    op_index=-1,
                    kind="invalid-format",
                    message=str(exc),
                )
            ]
        )
    return _verification_result(report)


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(
        description="Verify structural well-formedness of SimpleIR JSON.",
    )
    parser.add_argument("file", nargs="?", help="SimpleIR JSON file")
    parser.add_argument("--stdin", action="store_true", help="Read JSON from stdin")
    parser.add_argument("--quiet", "-q", action="store_true")
    parser.add_argument("--warn-as-error", action="store_true")
    args = parser.parse_args(argv)

    if args.stdin:
        raw = sys.stdin.read()
        source_label = "<stdin>"
    elif args.file:
        path = Path(args.file)
        if not path.exists():
            print(f"error: file not found: {path}", file=sys.stderr)
            return 2
        raw = path.read_text(encoding="utf-8")
        source_label = str(path)
    else:
        parser.print_help(sys.stderr)
        return 2

    try:
        tir = json.loads(raw)
    except json.JSONDecodeError as exc:
        print(f"error: invalid JSON from {source_label}: {exc}", file=sys.stderr)
        return 2
    if not isinstance(tir, dict):
        print(
            f"error: TIR JSON root must be an object, got {type(tir).__name__}",
            file=sys.stderr,
        )
        return 2

    try:
        try:
            result = verify_tir(tir)
        except (RuntimeError, TimeoutError) as exc:
            print(f"error: Rust IR verifier transport failed: {exc}", file=sys.stderr)
            return 2
    finally:
        close_process_local_verifier()

    has_issues = bool(result.errors or (args.warn_as_error and result.warnings))
    if result.errors:
        print(f"ERRORS ({len(result.errors)}):")
        for diagnostic in result.errors:
            print(diagnostic)
    if result.warnings:
        print(f"WARNINGS ({len(result.warnings)}):")
        for diagnostic in result.warnings:
            print(diagnostic)
    if not args.quiet:
        status = "PASS" if result.ok else "FAIL"
        print(
            f"\nIR structure check: {status}"
            f" | {result.functions_checked} functions"
            f" | {result.ops_checked} ops"
            f" | {len(result.errors)} errors"
            f" | {len(result.warnings)} warnings"
        )
    return 1 if has_issues else 0


if __name__ == "__main__":
    raise SystemExit(main())
