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
import datetime as dt
import json
import sys
from collections.abc import Mapping, Sequence
from dataclasses import asdict, dataclass
from pathlib import Path
from typing import Any

TOOLS_ROOT = Path(__file__).resolve().parent
if str(TOOLS_ROOT) not in sys.path:
    sys.path.insert(0, str(TOOLS_ROOT))

import perf_authority as pa  # noqa: E402

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
E2_SCOREBOARD_KIND = "cpython_floor_scoreboard"
E4_REQUIRED_RECEIPT_KINDS = frozenset(
    {
        "canonicalization_contract",
        "structural_audit",
        "degrade_to_slow_gate",
        "fail_closed_gate",
    }
)


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


def _load_json_evidence(path: Path) -> tuple[Mapping[str, Any] | None, str | None]:
    try:
        raw = json.loads(path.read_text(encoding="utf-8"))
    except OSError as exc:
        return None, f"cannot read {path}: {exc}"
    except json.JSONDecodeError as exc:
        return None, f"invalid JSON in {path}: {exc.msg}"
    if not isinstance(raw, Mapping):
        return None, f"JSON evidence root must be an object: {path}"
    return raw, None


def _load_text_evidence(path: Path) -> tuple[str | None, str | None]:
    try:
        return path.read_text(encoding="utf-8"), None
    except OSError as exc:
        return None, f"cannot read {path}: {exc}"
    except UnicodeDecodeError as exc:
        return None, f"invalid UTF-8 text in {path}: {exc}"


def _evidence_items(evidence: object) -> Sequence[Mapping[str, Any]]:
    if not isinstance(evidence, list):
        return ()
    return tuple(item for item in evidence if isinstance(item, Mapping))


def _entry_command_text(entry: Mapping[str, Any]) -> str:
    command = entry.get("command")
    return command if isinstance(command, str) else ""


def _candidate_output_path_from_log(text: str, *, log_path: Path) -> Path | None:
    for line in text.splitlines():
        key, sep, value = line.partition("=")
        if sep and key.strip() == "candidate_outputs":
            raw = value.strip()
            if not raw:
                return None
            path = Path(raw)
            if not path.is_absolute():
                path = log_path.parent / path
            return path
    return None


def _validate_e1_witness_acceptance(
    evidence: object,
    *,
    base_dir: Path,
) -> list[str]:
    problems: list[str] = []
    saw_witness_receipt = False
    for index, entry in enumerate(_evidence_items(evidence)):
        resolved = _resolve_evidence_path(entry.get("path"), base_dir=base_dir)
        if resolved is None or not resolved.exists():
            continue
        command_text = _entry_command_text(entry)
        if (
            "tools/pact_witness_acceptance.py" not in command_text
            and "pact-witness-acceptance" not in command_text
        ):
            continue
        text, text_error = _load_text_evidence(resolved)
        if text_error is not None:
            problems.append(f"E1: cannot read witness receipt[{index}]: {text_error}")
            continue
        assert text is not None
        saw_witness_receipt = True
        if "pact witness acceptance PASS" not in text:
            problems.append(
                f"E1: pact witness receipt lacks PASS verdict: {resolved}"
            )
        candidate = _candidate_output_path_from_log(text, log_path=resolved)
        if candidate is None:
            problems.append(
                f"E1: pact witness receipt lacks candidate_outputs path: {resolved}"
            )
        elif not candidate.is_file():
            problems.append(
                f"E1: candidate_outputs artifact does not exist: {candidate}"
            )

    if not saw_witness_receipt:
        problems.append(
            "E1: witness green requires a pact-witness-acceptance receipt "
            "with candidate_outputs and PASS verdict"
        )
    return problems


def _validate_e3_parity_receipt(
    evidence: object,
    *,
    base_dir: Path,
) -> list[str]:
    problems: list[str] = []
    saw_parity_receipt = False
    for index, entry in enumerate(_evidence_items(evidence)):
        resolved = _resolve_evidence_path(entry.get("path"), base_dir=base_dir)
        if resolved is None or not resolved.exists():
            continue
        command_text = _entry_command_text(entry)
        if "tools/parity_gate.py" not in command_text:
            continue
        text, text_error = _load_text_evidence(resolved)
        if text_error is not None:
            problems.append(f"E3: cannot read parity receipt[{index}]: {text_error}")
            continue
        assert text is not None
        saw_parity_receipt = True
        if "PASS: No Tier 1 violations." not in text:
            problems.append(f"E3: parity receipt lacks PASS verdict: {resolved}")

    if not saw_parity_receipt:
        problems.append(
            "E3: parity criteria require a tools/parity_gate.py receipt with "
            "the no-Tier-1-violations PASS verdict"
        )
    return problems


def _load_metric_baseline(rel_path: str) -> tuple[Mapping[str, Any] | None, str | None]:
    path = TOOLS_ROOT.parent / rel_path
    doc, error = _load_json_evidence(path)
    if error is not None:
        return None, error
    assert doc is not None
    return doc, None


def _validate_metrics_against_baseline(
    doc: Mapping[str, Any],
    *,
    path: Path,
    baseline_rel: str,
    label: str,
) -> list[str]:
    problems: list[str] = []
    metrics = doc.get("metrics")
    if not isinstance(metrics, Mapping):
        return [f"E4: {label} evidence lacks a metrics object: {path}"]

    baseline, error = _load_metric_baseline(baseline_rel)
    if error is not None:
        return [f"E4: cannot load {label} baseline {baseline_rel}: {error}"]
    assert baseline is not None

    for key, raw_baseline in sorted(baseline.items()):
        raw_current = metrics.get(key)
        if not isinstance(raw_current, int | float):
            problems.append(f"E4: {label} metric {key!r} missing/non-numeric: {path}")
            continue
        if not isinstance(raw_baseline, int | float):
            problems.append(
                f"E4: {label} baseline metric {key!r} is non-numeric in {baseline_rel}"
            )
            continue
        if float(raw_current) > float(raw_baseline):
            problems.append(
                f"E4: {label} metric {key!r} regressed "
                f"{float(raw_baseline):g} -> {float(raw_current):g}: {path}"
            )
    return problems


def _classify_e4_json_receipt(
    doc: Mapping[str, Any],
    *,
    path: Path,
) -> tuple[str | None, list[str]]:
    problems: list[str] = []
    if "violations" in doc and "metrics" in doc:
        problems.extend(
            _validate_metrics_against_baseline(
                doc,
                path=path,
                baseline_rel="tools/canonicalization_contract_baseline.json",
                label="canonicalization_contract",
            )
        )
        return "canonicalization_contract", problems

    if "findings" in doc and "metrics" in doc:
        problems.extend(
            _validate_metrics_against_baseline(
                doc,
                path=path,
                baseline_rel="tools/structural_audit_baseline.json",
                label="structural_audit",
            )
        )
        return "structural_audit", problems

    if {
        "ok",
        "errors",
        "metabug_fix_pending_count",
        "metabug_fix_pending_baseline",
    } <= set(doc):
        if doc.get("ok") is not True:
            problems.append(f"E4: degrade_to_slow_gate report is not ok: {path}")
        if doc.get("errors"):
            problems.append(f"E4: degrade_to_slow_gate report has errors: {path}")
        pending = doc.get("metabug_fix_pending_count")
        baseline = doc.get("metabug_fix_pending_baseline")
        if not isinstance(pending, int) or not isinstance(baseline, int):
            problems.append(
                "E4: degrade_to_slow_gate pending/baseline counts "
                f"must be integers: {path}"
            )
        elif pending > baseline:
            problems.append(
                "E4: degrade_to_slow_gate pending count regressed "
                f"{baseline} -> {pending}: {path}"
            )
        return "degrade_to_slow_gate", problems

    return None, problems


def _validate_e4_structural_floor(
    evidence: object,
    *,
    base_dir: Path,
) -> list[str]:
    problems: list[str] = []
    seen: set[str] = set()
    for index, entry in enumerate(_evidence_items(evidence)):
        resolved = _resolve_evidence_path(entry.get("path"), base_dir=base_dir)
        if resolved is None or not resolved.exists():
            continue
        command_text = _entry_command_text(entry)

        doc, json_error = _load_json_evidence(resolved)
        if doc is not None:
            kind, receipt_problems = _classify_e4_json_receipt(doc, path=resolved)
            if kind is not None:
                seen.add(kind)
                problems.extend(receipt_problems)
                continue
            # It may still be the text-only fail-closed receipt; fall through.
        elif json_error is not None and resolved.suffix.lower() == ".json":
            problems.append(f"E4: invalid JSON evidence[{index}]: {json_error}")
            continue

        text, text_error = _load_text_evidence(resolved)
        if text_error is not None:
            problems.append(f"E4: cannot classify evidence[{index}]: {text_error}")
            continue
        assert text is not None
        if "tools/fail_closed_gate.py" in command_text:
            if "fail-closed gate: OK" in text:
                seen.add("fail_closed_gate")
            else:
                problems.append(
                    f"E4: fail_closed_gate receipt does not contain OK verdict: {resolved}"
                )
        elif "tools/degrade_to_slow_gate.py" in command_text:
            if "degrade-to-slow gate: PASS" in text:
                seen.add("degrade_to_slow_gate")
            else:
                problems.append(
                    "E4: degrade_to_slow_gate text receipt does not contain "
                    f"PASS verdict: {resolved}"
                )

    missing = sorted(E4_REQUIRED_RECEIPT_KINDS - seen)
    if missing:
        problems.append(
            "E4: structural floor requires canonicalization_contract, "
            "structural_audit, degrade_to_slow_gate, and fail_closed_gate "
            f"receipts; missing: {', '.join(missing)}"
        )
    return problems


def _validate_e2_perf_scoreboard(
    evidence: object,
    *,
    base_dir: Path,
    now: dt.datetime | None = None,
) -> list[str]:
    problems: list[str] = []
    scoreboard_count = 0
    current = now or dt.datetime.now(dt.timezone.utc)

    for index, entry in enumerate(_evidence_items(evidence)):
        path = _resolve_evidence_path(entry.get("path"), base_dir=base_dir)
        if path is None or not path.exists():
            continue
        doc, error = _load_json_evidence(path)
        if error is not None:
            problems.append(f"E2: {error}")
            continue
        assert doc is not None
        if doc.get("kind") != E2_SCOREBOARD_KIND:
            continue
        scoreboard_count += 1
        problems.extend(
            f"E2: evidence[{index}] {problem}"
            for problem in pa.canonical_scoreboard_command_problems(
                _entry_command_text(entry),
                label="canonical scoreboard command",
            )
        )

        problems.extend(
            f"E2: {problem}: {path}"
            for problem in pa.current_scoreboard_problems(
                doc,
                label="scoreboard",
                shape_label="canonical scoreboard",
                now=current,
                max_age_days=pa.DEFAULT_STALE_DAYS,
                require_canonical_shape=True,
            )
        )

    if scoreboard_count == 0:
        problems.append(
            "E2: passing performance criteria require at least one canonical "
            f"{E2_SCOREBOARD_KIND} evidence artifact"
        )
    return problems


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
                if key == "E1" and not evidence_problems:
                    item_problems.extend(
                        _validate_e1_witness_acceptance(
                            item.get("evidence"),
                            base_dir=base_dir,
                        )
                    )
                if key == "E2" and not evidence_problems:
                    item_problems.extend(
                        _validate_e2_perf_scoreboard(
                            item.get("evidence"),
                            base_dir=base_dir,
                        )
                    )
                if key == "E3" and not evidence_problems:
                    item_problems.extend(
                        _validate_e3_parity_receipt(
                            item.get("evidence"),
                            base_dir=base_dir,
                        )
                    )
                if key == "E4" and not evidence_problems:
                    item_problems.extend(
                        _validate_e4_structural_floor(
                            item.get("evidence"),
                            base_dir=base_dir,
                        )
                    )
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
