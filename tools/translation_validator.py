#!/usr/bin/env python3
"""Consume Rust TIR pass-delta validation and representation records."""

from __future__ import annotations

import json
import sys
from collections.abc import Mapping
from dataclasses import dataclass, field
from pathlib import Path
from typing import Any


# ---------------------------------------------------------------------------
# Serialised-IR helpers
# ---------------------------------------------------------------------------


_REPR_FAMILY: dict[str, str] = {
    "Never": "bottom",
    "DynBox": "dyn",
    "MaybeBigInt": "int",
    "RawI64Safe": "int",
    "RawI64FullDeopt": "int",
    "Bool": "bool",
    "FloatUnboxed": "float",
}

_REPR_RANK: dict[str, int] = {
    "Never": 0,
    "DynBox": 1,
    "MaybeBigInt": 2,
    "Bool": 2,
    "FloatUnboxed": 2,
    "RawI64Safe": 3,
    "RawI64FullDeopt": 4,
}


# ---------------------------------------------------------------------------
# Check result
# ---------------------------------------------------------------------------


@dataclass
class CheckResult:
    """Outcome of a single structural check."""

    check: str
    passed: bool
    detail: str = ""

    def to_dict(self) -> dict[str, Any]:
        d: dict[str, Any] = {"check": self.check, "passed": self.passed}
        if self.detail:
            d["detail"] = self.detail
        return d


@dataclass
class PassReport:
    """Aggregated report for one pass on one function."""

    function: str
    pass_name: str
    checks: list[CheckResult] = field(default_factory=list)

    @property
    def passed(self) -> bool:
        return all(c.passed for c in self.checks)

    def to_dict(self) -> dict[str, Any]:
        return {
            "function": self.function,
            "pass": self.pass_name,
            "passed": self.passed,
            "checks": [c.to_dict() for c in self.checks],
        }


# ---------------------------------------------------------------------------
# TranslationValidator
# ---------------------------------------------------------------------------


class TranslationValidator:
    """Structural invariant checker for Molt mid-end passes."""

    # ------------------------------------------------------------------
    # Generic structural invariants
    # ------------------------------------------------------------------

    @staticmethod
    def check_repr_lattice_monotonic(
        before_profile: Mapping[str, Any],
        after_profile: Mapping[str, Any],
    ) -> CheckResult:
        """Validate value-keyed Repr facts across one TIR pass.

        The pass-delta emitter records `value_reprs` from the shared
        representation-facts authority. For a surviving `ValueId`, a pass may
        add proof and move upward in that carrier lattice; it may not silently
        lose proof or cross scalar families. Removed values and newly created
        values are checked by the structural IR validators and are ignored here.
        """
        before = _value_repr_map(before_profile, "before")
        after = _value_repr_map(after_profile, "after")
        if isinstance(before, str):
            return CheckResult(
                check="repr_lattice_monotonic",
                passed=False,
                detail=before,
            )
        if isinstance(after, str):
            return CheckResult(
                check="repr_lattice_monotonic",
                passed=False,
                detail=after,
            )

        violations: list[str] = []
        for value_id in sorted(set(before) & set(after), key=_value_id_sort_key):
            before_repr = before[value_id]
            after_repr = after[value_id]
            violation = _repr_transition_violation(before_repr, after_repr)
            if violation:
                violations.append(f"{value_id}: {violation}")

        if violations:
            return CheckResult(
                check="repr_lattice_monotonic",
                passed=False,
                detail=(
                    f"{len(violations)} Repr downgrade(s) or family drift: "
                    f"{violations[:10]}"
                ),
            )
        return CheckResult(check="repr_lattice_monotonic", passed=True)

    @staticmethod
    def check_rust_translation_validation(
        validation: Mapping[str, Any],
    ) -> CheckResult:
        """Consume the Rust-owned TIR translation-validation verdict.

        New pass-delta rows carry this verdict directly. The Python checker
        still has a legacy fallback for older rows, but it is no longer the
        authority for the R3a proof-order rule when Rust emitted a verdict.
        """
        check = validation.get("check")
        if not isinstance(check, str) or not check:
            return CheckResult(
                check="translation_validation",
                passed=False,
                detail="translation_validation.check missing or not a string",
            )
        passed = validation.get("passed")
        if not isinstance(passed, bool):
            return CheckResult(
                check=check,
                passed=False,
                detail="translation_validation.passed missing or not a bool",
            )
        violations = validation.get("violations")
        if not isinstance(violations, list):
            return CheckResult(
                check=check,
                passed=False,
                detail="translation_validation.violations missing or not a list",
            )
        if passed:
            return CheckResult(check=check, passed=True)

        details: list[str] = []
        for violation in violations[:10]:
            if not isinstance(violation, Mapping):
                details.append(repr(violation))
                continue
            value_id = violation.get("value_id", "?")
            before = violation.get("before", "?")
            after = violation.get("after", "?")
            reason = violation.get("reason", "")
            details.append(f"{value_id}: {before}->{after} {reason}".strip())
        return CheckResult(
            check=check,
            passed=False,
            detail=(f"{len(violations)} Repr downgrade(s) or family drift: {details}"),
        )

    # ------------------------------------------------------------------
    # Per-pass validators
    # ------------------------------------------------------------------

    # ------------------------------------------------------------------
    # Convenience dispatcher
    # ------------------------------------------------------------------

    def validate_pass_delta_record(self, record: Mapping[str, Any]) -> PassReport:
        """Validate one `MOLT_EMIT_PASS_DELTA=1` JSONL record."""
        function_name = str(record.get("function") or "<unknown>")
        pass_name = str(record.get("pass") or "<unknown>")
        report = PassReport(function=function_name, pass_name=pass_name)
        validation = record.get("translation_validation")
        if isinstance(validation, Mapping):
            report.checks.append(self.check_rust_translation_validation(validation))
            return report
        before = record.get("before")
        after = record.get("after")
        if not isinstance(before, Mapping) or not isinstance(after, Mapping):
            report.checks.append(
                CheckResult(
                    check="repr_lattice_monotonic",
                    passed=False,
                    detail="pass-delta record must contain before/after objects",
                )
            )
            return report
        report.checks.append(self.check_repr_lattice_monotonic(before, after))
        return report

    # ------------------------------------------------------------------
    # Batch validation from a TV dump directory
    # ------------------------------------------------------------------


# ---------------------------------------------------------------------------
# Utility functions
# ---------------------------------------------------------------------------


def _value_repr_map(profile: Mapping[str, Any], label: str) -> dict[str, str] | str:
    raw = profile.get("value_reprs")
    if not isinstance(raw, Mapping):
        return f"{label}.value_reprs missing or not an object"
    out: dict[str, str] = {}
    for value_id, repr_name in raw.items():
        if not isinstance(value_id, str) or not value_id:
            return f"{label}.value_reprs keys must be non-empty strings"
        if not isinstance(repr_name, str) or repr_name not in _REPR_RANK:
            return f"{label}.value_reprs[{value_id!r}] has unknown Repr {repr_name!r}"
        out[value_id] = repr_name
    return out


def _value_id_sort_key(value_id: str) -> tuple[int, int | str]:
    try:
        return (0, int(value_id))
    except ValueError:
        return (1, value_id)


def _repr_transition_violation(before: str, after: str) -> str | None:
    if before == after or before == "Never":
        return None
    before_family = _REPR_FAMILY[before]
    after_family = _REPR_FAMILY[after]
    if before == "DynBox":
        return None
    if after == "DynBox":
        return f"{before}->{after} moves to dynbox"
    if before_family != after_family:
        return f"{before}->{after} crosses {before_family}->{after_family}"
    if _REPR_RANK[after] < _REPR_RANK[before]:
        return f"{before}->{after}"
    return None


def _load_pass_delta_records(path: Path) -> list[dict[str, Any]]:
    records: list[dict[str, Any]] = []
    with path.open("r", encoding="utf-8") as handle:
        for line_no, raw_line in enumerate(handle, start=1):
            line = raw_line.strip()
            if not line:
                continue
            try:
                row = json.loads(line)
            except json.JSONDecodeError as exc:
                raise ValueError(f"{path}:{line_no}: invalid JSONL row: {exc}") from exc
            if not isinstance(row, dict):
                raise ValueError(f"{path}:{line_no}: row must be a JSON object")
            records.append(row)
    return records


# ---------------------------------------------------------------------------
# CLI
# ---------------------------------------------------------------------------


def main(argv: list[str] | None = None) -> int:
    """Validate actual Rust pass-delta records and print their verdicts."""
    import argparse

    parser = argparse.ArgumentParser(
        description="Validate Rust TIR pass-delta records."
    )
    parser.add_argument("--pass-delta-jsonl", type=Path, required=True)
    args = parser.parse_args(argv)
    try:
        records = _load_pass_delta_records(args.pass_delta_jsonl)
    except ValueError as exc:
        print(str(exc), file=sys.stderr)
        return 2
    validator = TranslationValidator()
    reports = [validator.validate_pass_delta_record(record) for record in records]
    total_failures = sum(1 for report in reports if not report.passed)
    print(json.dumps([report.to_dict() for report in reports], indent=2))
    print(
        f"TIR validation: {len(reports)} pass(es) checked, {total_failures} failure(s).",
        file=sys.stderr,
    )
    return 1 if total_failures else 0


if __name__ == "__main__":
    raise SystemExit(main())
