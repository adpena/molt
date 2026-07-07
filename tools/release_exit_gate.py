#!/usr/bin/env python3
"""Fail-closed release exit gate for the swarm E1-E4 contract.

The live orchestration board names four exit criteria:

E1 witness green, E2 performance above CPython, E3 parity, and E4 structural
floor. Those lanes already have their own proof tools. This module is the
composition authority: it consumes a small manifest of evidence receipts and
fails unless all four criteria are present, passing, and backed by concrete
artifacts.

It deliberately does not run the heavy proofs. Release readiness is a pure fact
over receipts, so the final "done" decision cannot be replaced by prose.
"""

from __future__ import annotations

import argparse
import json
import sys
from collections.abc import Mapping, Sequence
from dataclasses import asdict, dataclass
from pathlib import Path
from typing import Any

SCHEMA_VERSION = 1

STATUS_PASS = "pass"
STATUS_FAIL = "fail"
STATUS_BLOCKED = "blocked"
STATUS_MISSING = "missing"
STATUS_ADVISORY = "advisory"

VALID_STATUSES = frozenset(
    {
        STATUS_PASS,
        STATUS_FAIL,
        STATUS_BLOCKED,
        STATUS_MISSING,
        STATUS_ADVISORY,
    }
)

CRITERIA: dict[str, str] = {
    "E1": (
        "witness green: pact WASM witness builds, runs, writes "
        "candidate_outputs.npz, and passes parity"
    ),
    "E2": (
        "performance: canonical perf plane proves release-fast CPython floor "
        "and regressions"
    ),
    "E3": "parity: CPython >=3.12 verified subset/conformance receipts are green",
    "E4": "structural floor: god-crate/god-file and poison ratchets are green",
}

REQUIRED_EVIDENCE_FIELDS = frozenset({"path", "command", "summary"})


@dataclass(frozen=True)
class CriterionResult:
    criterion: str
    status: str
    passed: bool
    problems: tuple[str, ...]
    evidence_paths: tuple[str, ...]


@dataclass(frozen=True)
class ReleaseGateReport:
    passed: bool
    criteria: tuple[CriterionResult, ...]
    problems: tuple[str, ...]

    def to_jsonable(self) -> dict[str, Any]:
        return {
            "passed": self.passed,
            "problems": list(self.problems),
            "criteria": [asdict(c) for c in self.criteria],
        }


def _manifest_criteria(doc: Mapping[str, Any]) -> Mapping[str, Any] | None:
    raw = doc.get("criteria")
    return raw if isinstance(raw, Mapping) else None


def _resolve_evidence_path(raw: object, *, base_dir: Path) -> Path | None:
    if not isinstance(raw, str) or not raw.strip():
        return None
    path = Path(raw)
    if not path.is_absolute():
        path = base_dir / path
    return path


def _validate_evidence(
    evidence: object,
    *,
    criterion: str,
    base_dir: Path,
    require_existing_evidence: bool,
) -> tuple[list[str], list[str]]:
    problems: list[str] = []
    paths: list[str] = []
    if not isinstance(evidence, list) or not evidence:
        return [
            f"{criterion}: passing criteria require at least one evidence receipt"
        ], []

    for index, item in enumerate(evidence):
        entry = item if isinstance(item, Mapping) else None
        if entry is None:
            problems.append(f"{criterion}: evidence[{index}] must be an object")
            continue
        missing = sorted(REQUIRED_EVIDENCE_FIELDS - set(entry))
        if missing:
            problems.append(
                f"{criterion}: evidence[{index}] missing required fields "
                f"{', '.join(missing)}"
            )
        resolved = _resolve_evidence_path(entry.get("path"), base_dir=base_dir)
        if resolved is None:
            problems.append(
                f"{criterion}: evidence[{index}].path must be a non-empty path"
            )
            continue
        paths.append(str(resolved))
        if require_existing_evidence and not resolved.exists():
            problems.append(
                f"{criterion}: evidence artifact does not exist: {resolved}"
            )
    return problems, paths


def validate_manifest(
    doc: Mapping[str, Any],
    *,
    manifest_path: Path | None = None,
    require_existing_evidence: bool = True,
) -> ReleaseGateReport:
    """Validate a release-exit manifest and return a gate report."""
    problems: list[str] = []
    results: list[CriterionResult] = []
    base_dir = manifest_path.parent if manifest_path is not None else Path.cwd()

    if doc.get("schema_version") != SCHEMA_VERSION:
        problems.append(
            f"schema_version must be {SCHEMA_VERSION}, got {doc.get('schema_version')!r}"
        )

    criteria = _manifest_criteria(doc)
    if criteria is None:
        problems.append("manifest must contain a criteria object")
        criteria = {}

    for key in sorted(set(criteria) - set(CRITERIA)):
        problems.append(f"unknown release criterion {key!r}")

    for key, description in CRITERIA.items():
        raw = criteria.get(key)
        item = raw if isinstance(raw, Mapping) else None
        item_problems: list[str] = []
        evidence_paths: list[str] = []
        status = STATUS_MISSING
        if item is None:
            item_problems.append(f"{key}: missing criterion receipt ({description})")
        else:
            raw_status = item.get("status")
            if isinstance(raw_status, str):
                status = raw_status.strip().lower()
            else:
                item_problems.append(f"{key}: status must be a string")
            if status not in VALID_STATUSES:
                item_problems.append(f"{key}: invalid status {status!r}")
            if status != STATUS_PASS:
                item_problems.append(f"{key}: status is {status}, expected pass")
            if status == STATUS_PASS:
                evidence_problems, evidence_paths = _validate_evidence(
                    item.get("evidence"),
                    criterion=key,
                    base_dir=base_dir,
                    require_existing_evidence=require_existing_evidence,
                )
                item_problems.extend(evidence_problems)
        results.append(
            CriterionResult(
                criterion=key,
                status=status,
                passed=status == STATUS_PASS and not item_problems,
                problems=tuple(item_problems),
                evidence_paths=tuple(evidence_paths),
            )
        )
        problems.extend(item_problems)

    return ReleaseGateReport(
        passed=not problems and all(result.passed for result in results),
        criteria=tuple(results),
        problems=tuple(problems),
    )


def load_manifest(path: Path) -> Mapping[str, Any]:
    try:
        doc = json.loads(path.read_text(encoding="utf-8"))
    except OSError as exc:
        raise SystemExit(f"release-exit gate: cannot read {path}: {exc}") from exc
    except json.JSONDecodeError as exc:
        raise SystemExit(f"release-exit gate: invalid JSON in {path}: {exc}") from exc
    if not isinstance(doc, Mapping):
        raise SystemExit("release-exit gate: manifest root must be an object")
    return doc


def check_manifest_path(
    path: Path, *, require_existing_evidence: bool = True
) -> ReleaseGateReport:
    return validate_manifest(
        load_manifest(path),
        manifest_path=path,
        require_existing_evidence=require_existing_evidence,
    )


def _print_text_report(report: ReleaseGateReport) -> None:
    if report.passed:
        print("[release-exit-gate] PASS: E1-E4 receipts are present and passing")
        return
    print(
        "[release-exit-gate] FAIL: release exit criteria are not satisfied",
        file=sys.stderr,
    )
    for problem in report.problems:
        print(f"  - {problem}", file=sys.stderr)


def main(argv: Sequence[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("manifest", type=Path, help="release-exit JSON manifest")
    parser.add_argument(
        "--allow-missing-evidence",
        action="store_true",
        help="validate manifest shape without requiring evidence paths to exist",
    )
    parser.add_argument("--json", action="store_true", help="emit JSON report")
    args = parser.parse_args(argv)

    report = check_manifest_path(
        args.manifest,
        require_existing_evidence=not args.allow_missing_evidence,
    )
    if args.json:
        print(json.dumps(report.to_jsonable(), indent=2))
    else:
        _print_text_report(report)
    return 0 if report.passed else 1


if __name__ == "__main__":
    raise SystemExit(main())
