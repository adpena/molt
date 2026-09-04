"""Typed, exact-schema authority for E3/E4 release criterion receipts."""

from __future__ import annotations

import argparse
import datetime as dt
import json
import math
import re
import subprocess
from collections.abc import Mapping, Sequence
from dataclasses import dataclass
from pathlib import Path, PurePosixPath
from typing import Any, TypedDict

from molt.exact_json import ExactJsonError, loads_exact, write_exact
from molt.portable_paths import portable_path_identity, portable_relative_path
from molt.toolchain_identity import stable_file_sha256
from tools.git_identity import is_git_object_id

SCHEMA_VERSION = 1
STATUS_PASS = "PASS"
STATUS_FAIL = "FAIL"

KIND_VERIFIED_SUBSET = "verified_subset"
KIND_CANONICALIZATION_CONTRACT = "canonicalization_contract"
KIND_STRUCTURAL_AUDIT = "structural_audit"
KIND_DEGRADE_TO_SLOW_GATE = "degrade_to_slow_gate"
KIND_FAIL_CLOSED_GATE = "fail_closed_gate"

KINDS = frozenset(
    {
        KIND_VERIFIED_SUBSET,
        KIND_CANONICALIZATION_CONTRACT,
        KIND_STRUCTURAL_AUDIT,
        KIND_DEGRADE_TO_SLOW_GATE,
        KIND_FAIL_CLOSED_GATE,
    }
)

KIND_TO_TOOL = {
    KIND_VERIFIED_SUBSET: "tools/verified_subset.py",
    KIND_CANONICALIZATION_CONTRACT: "tools/canonicalization_contract.py",
    KIND_STRUCTURAL_AUDIT: "tools/structural_audit.py",
    KIND_DEGRADE_TO_SLOW_GATE: "tools/degrade_to_slow_gate.py",
    KIND_FAIL_CLOSED_GATE: "tools/fail_closed_gate.py",
}

CANONICALIZATION_METRICS = frozenset(
    {
        "critical_layer_violations",
        "duplicate_authority_domains",
        "duplicate_authority_recoverable_lines",
        "layer_dependency_violations",
        "misplaced_module_lines",
        "non_member_layer_crates",
    }
)
STRUCTURAL_AUDIT_METRICS = frozenset(
    {
        "critical_hand_classifications",
        "debt_markers_total",
        "duplicate_authorities",
        "hand_classified_matches",
        "handset_classifications",
        "kitchen_sink_files",
        "kitchen_sink_large_regions",
        "max_kitchen_sink_structural_score",
        "max_undecomposed_file_lines",
        "native_scalar_plan_authority_violations",
        "python_stub_surfaces_total",
        "repr_name_scalar_authority_violations",
        "rust_backend_lowering_gaps_total",
        "rust_stub_surfaces_total",
        "undecomposed_god_files",
    }
)
FAIL_CLOSED_CLASSES = frozenset(
    {
        "duplicate_authority",
        "ecosystem_baked",
        "ecosystem_build_crutch",
        "ecosystem_reimplementation",
        "fail_open_backend_dispatch",
        "fail_open_stub",
        "todo_as_plan",
    }
)

_ROOT_KEYS = frozenset(
    {
        "schema_version",
        "kind",
        "source_sha",
        "generated_at",
        "status",
        "producer",
        "facts",
        "inputs",
    }
)
_INPUT_KEYS = frozenset({"path", "sha256", "size"})
_PRODUCER_KEYS = frozenset({"argv", "tool"})
_HEX = frozenset("0123456789abcdef")
_UTC_TIMESTAMP_RE = re.compile(
    r"^[0-9]{4}-[0-9]{2}-[0-9]{2}T[0-9]{2}:[0-9]{2}:[0-9]{2}"
    r"(?:\.[0-9]{1,6})?Z$"
)
_MAX_FUTURE_SKEW = dt.timedelta(minutes=5)
_VERIFIED_COUNT_KEYS = frozenset(
    {
        "backend_failed",
        "backend_missing",
        "backend_passed",
        "errors",
        "executed",
        "expected_failures",
        "oom",
        "raw_failed",
        "raw_passed",
        "resolved_failed",
        "resolved_passed",
        "skipped",
        "uncalibrated",
    }
)
_VERIFIED_OUTCOME_KEYS = frozenset(
    {
        "backend",
        "backend_returncode",
        "backend_status",
        "backend_stderr_sha256",
        "backend_stdout_sha256",
        "comparison_law",
        "compiler_target_python",
        "cpython_returncode",
        "cpython_stderr_sha256",
        "cpython_stdout_sha256",
        "expect_molt_fail",
        "expected_failure_reason",
        "path",
        "raw_status",
        "reason_tag",
        "resolved_status",
    }
)


class InputRecord(TypedDict):
    path: str
    sha256: str
    size: int


class ProducerRecord(TypedDict):
    argv: list[str]
    tool: InputRecord


class Receipt(TypedDict):
    schema_version: int
    kind: str
    source_sha: str
    generated_at: str
    status: str
    producer: ProducerRecord
    facts: dict[str, Any]
    inputs: list[InputRecord]


@dataclass(frozen=True)
class ReceiptDestination:
    repo_root: Path
    output_path: Path
    source_sha: str


def _portable_relative_path(path: Path, repo_root: Path) -> str:
    root = repo_root.resolve(strict=True)
    resolved = path.resolve(strict=True)
    if not resolved.is_file():
        raise ValueError(f"release receipt input is not a file: {resolved}")
    try:
        relative = resolved.relative_to(root)
    except ValueError as exc:
        raise ValueError(
            f"release receipt input escapes source checkout: {resolved}"
        ) from exc
    return portable_relative_path(relative.as_posix()).as_posix()


def input_record(path: Path, *, repo_root: Path) -> InputRecord:
    resolved = path.resolve(strict=True)
    return {
        "path": _portable_relative_path(resolved, repo_root),
        "sha256": stable_file_sha256(
            resolved,
            label="release criterion receipt input",
        ),
        "size": resolved.stat().st_size,
    }


def sorted_input_records(
    paths: Sequence[Path], *, repo_root: Path
) -> list[InputRecord]:
    records = [input_record(path, repo_root=repo_root) for path in paths]
    records.sort(key=lambda item: item["path"])
    names = [item["path"] for item in records]
    identities = [portable_path_identity(name) for name in names]
    if len(identities) != len(set(identities)):
        raise ValueError(
            "release receipt inputs must be unique under the portable filesystem identity"
        )
    return records


def add_receipt_arguments(parser: argparse.ArgumentParser) -> None:
    parser.add_argument(
        "--receipt",
        type=Path,
        help="write an exact source-addressed release criterion receipt",
    )
    parser.add_argument(
        "--source-sha",
        help="exact clean Git HEAD the release receipt must address",
    )


def _git(repo_root: Path, *args: str) -> str:
    completed = subprocess.run(
        ["git", "-C", str(repo_root), *args],
        check=False,
        capture_output=True,
        text=True,
        encoding="utf-8",
    )
    if completed.returncode != 0:
        detail = completed.stderr.strip() or completed.stdout.strip()
        raise ValueError(f"git {' '.join(args)} failed: {detail}")
    return completed.stdout


def _valid_source_sha(value: object) -> bool:
    return is_git_object_id(value)


def assert_clean_source(
    *, repo_root: Path, source_sha: str, output_path: Path
) -> ReceiptDestination:
    root = repo_root.resolve(strict=True)
    output = output_path.expanduser().resolve()
    if output.exists():
        raise ValueError(f"release receipt output already exists: {output}")
    if not _valid_source_sha(source_sha):
        raise ValueError("release receipt source_sha must be lowercase Git object hex")
    head = _git(root, "rev-parse", "--verify", "HEAD").strip().lower()
    if head != source_sha:
        raise ValueError(
            f"release receipt source mismatch: requested {source_sha}, HEAD is {head}"
        )
    dirty = _git(
        root,
        "status",
        "--porcelain=v1",
        "--untracked-files=all",
        "--ignore-submodules=none",
    )
    if dirty:
        entries = [line for line in dirty.splitlines() if line]
        preview = ", ".join(entries[:5])
        suffix = f" (+{len(entries) - 5} more)" if len(entries) > 5 else ""
        raise ValueError(
            "release receipt requires a clean source checkout; dirty entries: "
            f"{preview}{suffix}"
        )
    return ReceiptDestination(root, output, source_sha)


def prepare_receipt_destination(
    *,
    repo_root: Path,
    receipt_path: Path | None,
    source_sha: str | None,
) -> ReceiptDestination | None:
    if receipt_path is None and source_sha is None:
        return None
    if receipt_path is None or source_sha is None:
        raise ValueError("--receipt and --source-sha must be provided together")
    return assert_clean_source(
        repo_root=repo_root,
        source_sha=source_sha,
        output_path=receipt_path,
    )


def _utc_now() -> str:
    return (
        dt.datetime.now(dt.timezone.utc)
        .isoformat(timespec="seconds")
        .replace("+00:00", "Z")
    )


def build_receipt(
    *,
    kind: str,
    source_sha: str,
    status: str,
    argv: Sequence[str],
    tool_path: Path,
    facts: Mapping[str, Any],
    input_paths: Sequence[Path],
    repo_root: Path,
    generated_at: str | None = None,
) -> Receipt:
    tool = input_record(tool_path, repo_root=repo_root)
    canonical_argv = [tool["path"], *argv]
    receipt: Receipt = {
        "schema_version": SCHEMA_VERSION,
        "kind": kind,
        "source_sha": source_sha,
        "generated_at": generated_at or _utc_now(),
        "status": status,
        "producer": {"argv": canonical_argv, "tool": tool},
        "facts": dict(facts),
        "inputs": sorted_input_records(input_paths, repo_root=repo_root),
    }
    problems = validate_receipt(
        receipt,
        expected_kind=kind,
        expected_source_sha=source_sha,
        repo_root=repo_root,
    )
    if problems:
        raise ValueError("invalid release criterion receipt: " + "; ".join(problems))
    return receipt


def write_receipt(receipt: Receipt, destination: ReceiptDestination) -> None:
    # Recheck immediately before byte creation so a long-running producer cannot
    # attest a checkout that changed after its initial preflight.
    assert_clean_source(
        repo_root=destination.repo_root,
        source_sha=destination.source_sha,
        output_path=destination.output_path,
    )
    problems = validate_receipt(
        receipt,
        expected_kind=receipt["kind"],
        expected_source_sha=destination.source_sha,
        repo_root=destination.repo_root,
    )
    if problems:
        raise ValueError(
            "release criterion receipt changed before publication: "
            + "; ".join(problems)
        )
    write_exact(destination.output_path, receipt, exclusive=True)


def _is_exact_object(value: object, keys: frozenset[str]) -> bool:
    return isinstance(value, Mapping) and set(value) == keys


def _is_sha256(value: object) -> bool:
    return (
        isinstance(value, str)
        and len(value) == 64
        and all(character in _HEX for character in value)
    )


def _portable_posix(value: object) -> PurePosixPath | None:
    try:
        return portable_relative_path(value)
    except ValueError:
        return None


def _nonnegative_int(value: object) -> bool:
    return isinstance(value, int) and not isinstance(value, bool) and value >= 0


def _metric(value: object) -> bool:
    return (
        isinstance(value, (int, float))
        and not isinstance(value, bool)
        and math.isfinite(value)
        and value >= 0
    )


def _string_list(value: object) -> bool:
    return isinstance(value, list) and all(
        isinstance(item, str) and item for item in value
    )


def _validate_input_record(
    value: object,
    *,
    label: str,
    repo_root: Path | None,
    verify_bytes: bool,
) -> list[str]:
    if not _is_exact_object(value, _INPUT_KEYS):
        return [f"{label} must contain exactly path, sha256, and size"]
    assert isinstance(value, Mapping)
    problems: list[str] = []
    relative = _portable_posix(value.get("path"))
    if relative is None:
        problems.append(f"{label}.path must be a canonical relative POSIX path")
    if not _is_sha256(value.get("sha256")):
        problems.append(f"{label}.sha256 must be lowercase SHA-256")
    if not _nonnegative_int(value.get("size")):
        problems.append(f"{label}.size must be a non-negative integer")
    if verify_bytes and repo_root is not None and relative is not None:
        root = repo_root.resolve(strict=True)
        path = root.joinpath(*relative.parts).resolve()
        if not path.is_relative_to(root):
            problems.append(f"{label}.path escapes the source checkout")
        elif not path.is_file():
            problems.append(f"{label} input does not exist: {relative.as_posix()}")
        else:
            if (
                _nonnegative_int(value.get("size"))
                and path.stat().st_size != value["size"]
            ):
                problems.append(f"{label} input size mismatch: {relative.as_posix()}")
            if (
                _is_sha256(value.get("sha256"))
                and stable_file_sha256(
                    path,
                    label="release criterion receipt artifact",
                )
                != value["sha256"]
            ):
                problems.append(
                    f"{label} input checksum mismatch: {relative.as_posix()}"
                )
    return problems


def _validate_metric_facts(
    facts: Mapping[str, Any],
    *,
    expected_keys: frozenset[str],
    count_field: str,
    count_value_label: str,
    status: object,
) -> list[str]:
    required = frozenset(
        {
            "baseline_path",
            "baseline_metrics",
            "metrics",
            "regressed_metrics",
            "improved_metrics",
            count_field,
        }
    )
    if set(facts) != required:
        return [
            f"metric receipt facts differ from schema: "
            f"missing={sorted(required - set(facts), key=str)!r}, "
            f"unknown={sorted(set(facts) - required, key=str)!r}"
        ]
    problems: list[str] = []
    if _portable_posix(facts.get("baseline_path")) is None:
        problems.append("facts.baseline_path must be a canonical relative POSIX path")
    if not _nonnegative_int(facts.get(count_field)):
        problems.append(f"facts.{count_field} must be a non-negative integer")
    for field in ("metrics", "baseline_metrics"):
        metrics = facts.get(field)
        if not isinstance(metrics, Mapping) or set(metrics) != expected_keys:
            problems.append(
                f"facts.{field} must contain exactly the {count_value_label} metrics"
            )
        elif not all(_metric(value) for value in metrics.values()):
            problems.append(f"facts.{field} values must be finite non-negative numbers")
    metrics = facts.get("metrics")
    baseline = facts.get("baseline_metrics")
    if isinstance(metrics, Mapping) and isinstance(baseline, Mapping):
        if (
            set(metrics) == expected_keys
            and set(baseline) == expected_keys
            and all(_metric(value) for value in metrics.values())
            and all(_metric(value) for value in baseline.values())
        ):
            regressed = sorted(
                key for key in expected_keys if metrics[key] > baseline[key]
            )
            improved = sorted(
                key for key in expected_keys if metrics[key] < baseline[key]
            )
            if facts.get("regressed_metrics") != regressed:
                problems.append("facts.regressed_metrics is not derived from metrics")
            if facts.get("improved_metrics") != improved:
                problems.append("facts.improved_metrics is not derived from metrics")
            expected_status = STATUS_PASS if not regressed else STATUS_FAIL
            if status != expected_status:
                problems.append(
                    f"receipt status must be {expected_status} for its metric facts"
                )
    for field in ("regressed_metrics", "improved_metrics"):
        value = facts.get(field)
        if not _string_list(value) or value != sorted(set(value)):
            problems.append(f"facts.{field} must be a sorted unique string list")
        elif not set(value).issubset(expected_keys):
            problems.append(f"facts.{field} contains an unknown metric")
    return problems


def _validate_verified_subset_facts(
    facts: Mapping[str, Any], status: object
) -> list[str]:
    from molt.verified_subset import (
        ROOT as VERIFIED_SUBSET_ROOT,
        load_verified_subset_policy,
        verified_subset_coordinate_by_id,
    )
    from tools import verified_subset as verified
    from tools.compat import comparison, test_policy

    keys = frozenset(
        {
            "authority_inputs",
            "coordinate",
            "counts",
            "execution",
            "fallback_policy",
            "outcomes",
            "projection",
            "suites",
        }
    )
    if set(facts) != keys:
        return ["verified_subset facts schema is invalid"]
    problems: list[str] = []
    policy = load_verified_subset_policy()
    if facts.get("fallback_policy") != policy.fallback_policy:
        problems.append("facts.fallback_policy differs from verified-subset policy")
    suites = facts.get("suites")
    if suites != [suite.as_record() for suite in policy.suites]:
        problems.append("facts.suites differs from verified-subset policy")
    authority_inputs = facts.get("authority_inputs")
    if not _string_list(authority_inputs) or authority_inputs != sorted(
        set(authority_inputs)
    ):
        problems.append("facts.authority_inputs must be a sorted unique string list")
    elif any(_portable_posix(path) is None for path in authority_inputs):
        problems.append("facts.authority_inputs contains a non-portable path")
    else:
        expected_authority_inputs = [
            path.relative_to(VERIFIED_SUBSET_ROOT).as_posix()
            for path in verified.verified_subset_authority_files(policy)
        ]
        if authority_inputs != expected_authority_inputs:
            problems.append(
                "facts.authority_inputs differs from verified-subset authority closure"
            )
    coordinate = facts.get("coordinate")
    expected_coordinate = None
    coordinate_keys = frozenset(
        {
            "id",
            "python",
            "reference_python",
            "abi",
            "backend",
            "concurrency",
            "platform",
            "arch",
            "rust_target",
            "runner",
        }
    )
    if not _is_exact_object(coordinate, coordinate_keys):
        problems.append("facts.coordinate schema is invalid")
    else:
        assert isinstance(coordinate, Mapping)
        coordinate_id = coordinate.get("id")
        try:
            expected_coordinate = verified_subset_coordinate_by_id(str(coordinate_id))
        except ValueError as exc:
            problems.append(str(exc))
        else:
            if dict(coordinate) != expected_coordinate.as_record():
                problems.append(
                    "facts.coordinate differs from verified-subset authority"
                )
    projection = facts.get("projection")
    projection_keys = frozenset(
        {"applicable", "excluded", "expected_failures", "sha256", "source_tests"}
    )
    expected_projection = None
    if expected_coordinate is not None:
        expected_projection = verified.verified_subset_projection(
            policy, expected_coordinate
        )
    if not _is_exact_object(projection, projection_keys):
        problems.append("facts.projection schema is invalid")
    elif expected_projection is not None:
        if projection != expected_projection.closure_record():
            problems.append("facts.projection differs from verified-subset sources")

    outcomes = facts.get("outcomes")
    normalized_outcomes: list[Mapping[str, object]] = []
    if not isinstance(outcomes, list):
        problems.append("facts.outcomes must be a list")
    else:
        if not outcomes:
            problems.append("facts.outcomes must not be empty")
        for index, outcome in enumerate(outcomes):
            if not _is_exact_object(outcome, _VERIFIED_OUTCOME_KEYS):
                problems.append(f"facts.outcomes[{index}] schema is invalid")
                continue
            assert isinstance(outcome, Mapping)
            normalized_outcomes.append(outcome)
            path = outcome.get("path")
            if _portable_posix(path) is None:
                problems.append(f"facts.outcomes[{index}].path is not portable")
            for field in ("raw_status", "resolved_status"):
                if outcome.get(field) not in verified.RESULT_STATUSES:
                    problems.append(f"facts.outcomes[{index}].{field} is invalid")
            if outcome.get("backend") != (
                expected_coordinate.backend if expected_coordinate is not None else None
            ):
                problems.append(f"facts.outcomes[{index}].backend differs")
            if outcome.get("compiler_target_python") != (
                expected_coordinate.python if expected_coordinate is not None else None
            ):
                problems.append(
                    f"facts.outcomes[{index}].compiler_target_python differs"
                )
            if outcome.get("backend_status") not in {*verified.RESULT_STATUSES, None}:
                problems.append(f"facts.outcomes[{index}].backend_status is invalid")
            if not isinstance(outcome.get("expect_molt_fail"), bool):
                problems.append(
                    f"facts.outcomes[{index}].expect_molt_fail must be boolean"
                )
            if outcome.get("reason_tag") not in {None, "xfail", "xpass"}:
                problems.append(f"facts.outcomes[{index}].reason_tag is invalid")
            for field in ("cpython_returncode", "backend_returncode"):
                if outcome.get(field) is not None and not isinstance(
                    outcome.get(field), int
                ):
                    problems.append(f"facts.outcomes[{index}].{field} is invalid")
            for field in (
                "cpython_stderr_sha256",
                "cpython_stdout_sha256",
                "backend_stderr_sha256",
                "backend_stdout_sha256",
            ):
                value = outcome.get(field)
                if value is not None and not _is_sha256(value):
                    problems.append(f"facts.outcomes[{index}].{field} is invalid")

    if expected_projection is not None and normalized_outcomes:
        expected_tests = {test.path: test for test in expected_projection.applicable}
        actual_paths = [str(outcome.get("path")) for outcome in normalized_outcomes]
        if actual_paths != sorted(expected_tests):
            problems.append("facts.outcomes do not exactly cover projected tests")
        for outcome in normalized_outcomes:
            test = expected_tests.get(str(outcome.get("path")))
            if test is None:
                continue
            if outcome.get("expect_molt_fail") is not test.expect_molt_fail:
                problems.append(
                    f"facts.outcomes[{test.path}] expected-failure classification differs"
                )
            if outcome.get("expected_failure_reason") != test.expected_failure_reason:
                problems.append(
                    f"facts.outcomes[{test.path}] expected-failure reason differs"
                )
            cpython_returncode = outcome.get("cpython_returncode")
            if isinstance(cpython_returncode, int):
                expected_resolved, expected_reason_tag = (
                    test_policy.resolve_expected_failure_status(
                        expect_molt_fail=test.expect_molt_fail,
                        raw_status=str(outcome.get("raw_status")),
                        cpython_returncode=cpython_returncode,
                    )
                )
                if outcome.get("resolved_status") != expected_resolved:
                    problems.append(
                        f"facts.outcomes[{test.path}] resolved status is not derived"
                    )
                if outcome.get("reason_tag") != expected_reason_tag:
                    problems.append(
                        f"facts.outcomes[{test.path}] reason tag is not derived"
                    )
            if (
                outcome.get("raw_status") == "pass"
                and outcome.get("comparison_law") != comparison.COMPARISON_LAW_VERSION
            ):
                problems.append(f"facts.outcomes[{test.path}] comparison law differs")

    counts = facts.get("counts")
    if not _is_exact_object(counts, _VERIFIED_COUNT_KEYS):
        problems.append("facts.counts schema is invalid")
        counts = {}
    elif not all(_nonnegative_int(value) for value in counts.values()):
        problems.append("facts.counts values must be non-negative integers")
    if normalized_outcomes:
        expected_counts = verified.outcome_counts(normalized_outcomes)
        if counts != expected_counts:
            problems.append("facts.counts are not derived from outcomes")
        expected_status = (
            STATUS_PASS if verified.outcomes_pass(normalized_outcomes) else STATUS_FAIL
        )
        if status != expected_status:
            problems.append(
                f"receipt status must be {expected_status} for verified-subset facts"
            )

    execution = facts.get("execution")
    execution_keys = frozenset({"backend", "ci", "host", "python", "rust"})
    if not _is_exact_object(execution, execution_keys):
        problems.append("facts.execution schema is invalid")
    else:
        assert isinstance(execution, Mapping)
        ci = execution.get("ci")
        ci_keys = frozenset(
            {
                "job",
                "provider",
                "run_attempt",
                "run_id",
                "runner_arch",
                "runner_label",
                "runner_os",
                "source_sha",
                "workflow_ref",
            }
        )
        if not _is_exact_object(ci, ci_keys) or not all(
            isinstance(value, str) and value
            for value in ci.values()  # type: ignore[union-attr]
        ):
            problems.append("facts.execution.ci schema is invalid")
        elif ci.get("provider") != "github-actions":  # type: ignore[union-attr]
            problems.append("facts.execution.ci provider is not github-actions")
        elif expected_coordinate is not None:
            assert isinstance(ci, Mapping)
            expected_runner_os = {
                "linux": "Linux",
                "macos": "macOS",
                "windows": "Windows",
            }[expected_coordinate.platform]
            expected_runner_arch = (
                "ARM64" if expected_coordinate.arch in {"aarch64", "arm64"} else "X64"
            )
            if (
                ci.get("runner_label") != expected_coordinate.runner
                or ci.get("runner_os") != expected_runner_os
                or ci.get("runner_arch") != expected_runner_arch
            ):
                problems.append("facts.execution.ci runner differs from coordinate")

        host = execution.get("host")
        host_keys = frozenset({"arch", "platform", "pointer_bits"})
        if not _is_exact_object(host, host_keys):
            problems.append("facts.execution.host schema is invalid")
        elif expected_coordinate is not None:
            assert isinstance(host, Mapping)
            if (
                host.get("arch") != expected_coordinate.arch
                or host.get("platform") != expected_coordinate.platform
            ):
                problems.append("facts.execution.host differs from coordinate")
            if host.get("pointer_bits") != 64:
                problems.append("facts.execution.host pointer width is not 64-bit")

        python = execution.get("python")
        python_keys = frozenset(
            {
                "abi_flags",
                "cache_tag",
                "command_executable",
                "executable_name",
                "executable_sha256",
                "gil_disabled",
                "hexversion",
                "implementation",
                "pointer_bits",
                "version",
                "version_info",
            }
        )
        if not _is_exact_object(python, python_keys):
            problems.append("facts.execution.python schema is invalid")
        else:
            assert isinstance(python, Mapping)
            version_info = python.get("version_info")
            if (
                python.get("implementation") != "CPython"
                or python.get("gil_disabled") is not False
                or python.get("pointer_bits") != 64
                or not _is_sha256(python.get("executable_sha256"))
                or not isinstance(version_info, list)
                or len(version_info) != 5
                or expected_coordinate is None
                or f"{version_info[0]}.{version_info[1]}" != expected_coordinate.python
                or python.get("version") != expected_coordinate.reference_python
            ):
                problems.append("facts.execution.python differs from coordinate")

        rust = execution.get("rust")
        rust_keys = frozenset(
            {
                "binary_name",
                "binary_sha256",
                "commit_date",
                "commit_hash",
                "host",
                "llvm_version",
                "release",
            }
        )
        if not _is_exact_object(rust, rust_keys):
            problems.append("facts.execution.rust schema is invalid")
        elif expected_coordinate is not None:
            assert isinstance(rust, Mapping)
            if (
                rust.get("host") != expected_coordinate.rust_target
                or not _is_sha256(rust.get("binary_sha256"))
                or not all(
                    isinstance(rust.get(field), str) and rust.get(field)
                    for field in (
                        "commit_date",
                        "commit_hash",
                        "llvm_version",
                        "release",
                    )
                )
            ):
                problems.append("facts.execution.rust differs from coordinate")

        backend = execution.get("backend")
        backend_keys = (
            frozenset({"backend", "runner"})
            if expected_coordinate is not None
            and expected_coordinate.backend == "native"
            else frozenset(
                {"backend", "binary_name", "binary_sha256", "runner", "version"}
            )
        )
        if not _is_exact_object(backend, backend_keys):
            problems.append("facts.execution.backend schema is invalid")
        elif expected_coordinate is not None:
            assert isinstance(backend, Mapping)
            expected_backend_runner = (
                "node-wasi" if expected_coordinate.backend == "wasm" else "process"
            )
            if (
                backend.get("backend") != expected_coordinate.backend
                or backend.get("runner") != expected_backend_runner
            ):
                problems.append("facts.execution.backend differs from coordinate")
            if expected_coordinate.backend == "wasm" and not _is_sha256(
                backend.get("binary_sha256")
            ):
                problems.append("facts.execution.backend binary hash is invalid")
    return problems


def _validate_degrade_facts(facts: Mapping[str, Any], status: object) -> list[str]:
    keys = frozenset(
        {
            "discovered_site_count",
            "errors",
            "metabug_fix_pending_baseline",
            "metabug_fix_pending_count",
            "registry_path",
            "registry_row_count",
            "warnings",
        }
    )
    if set(facts) != keys:
        return ["degrade_to_slow_gate facts schema is invalid"]
    problems: list[str] = []
    if _portable_posix(facts.get("registry_path")) is None:
        problems.append("facts.registry_path must be a canonical relative POSIX path")
    for field in (
        "discovered_site_count",
        "metabug_fix_pending_baseline",
        "metabug_fix_pending_count",
        "registry_row_count",
    ):
        if not _nonnegative_int(facts.get(field)):
            problems.append(f"facts.{field} must be a non-negative integer")
    for field in ("errors", "warnings"):
        if not isinstance(facts.get(field), list) or not all(
            isinstance(item, str) for item in facts.get(field, [])
        ):
            problems.append(f"facts.{field} must be a string list")
    errors = facts.get("errors")
    pending = facts.get("metabug_fix_pending_count")
    baseline = facts.get("metabug_fix_pending_baseline")
    if (
        isinstance(errors, list)
        and _nonnegative_int(pending)
        and _nonnegative_int(baseline)
    ):
        expected_status = (
            STATUS_PASS if not errors and pending <= baseline else STATUS_FAIL
        )
        if status != expected_status:
            problems.append(
                f"receipt status must be {expected_status} for degrade-to-slow facts"
            )
    return problems


def _validate_fail_closed_facts(facts: Mapping[str, Any], status: object) -> list[str]:
    keys = frozenset(
        {
            "baseline_counts",
            "class_counts",
            "registered_site_count",
            "registry_path",
            "violations",
        }
    )
    if set(facts) != keys:
        return ["fail_closed_gate facts schema is invalid"]
    problems: list[str] = []
    if _portable_posix(facts.get("registry_path")) is None:
        problems.append("facts.registry_path must be a canonical relative POSIX path")
    if not _nonnegative_int(facts.get("registered_site_count")):
        problems.append("facts.registered_site_count must be a non-negative integer")
    for field in ("class_counts", "baseline_counts"):
        counts = facts.get(field)
        if not isinstance(counts, Mapping) or set(counts) != FAIL_CLOSED_CLASSES:
            problems.append(f"facts.{field} must contain exactly the poison classes")
        elif not all(_nonnegative_int(value) for value in counts.values()):
            problems.append(f"facts.{field} values must be non-negative integers")
    class_counts = facts.get("class_counts")
    baseline_counts = facts.get("baseline_counts")
    registered = facts.get("registered_site_count")
    if (
        isinstance(class_counts, Mapping)
        and set(class_counts) == FAIL_CLOSED_CLASSES
        and all(_nonnegative_int(value) for value in class_counts.values())
    ):
        if _nonnegative_int(registered) and sum(class_counts.values()) != registered:
            problems.append(
                "facts.registered_site_count is not derived from class_counts"
            )
    violations = facts.get("violations")
    violation_keys = frozenset({"detail", "kind"})
    if not isinstance(violations, list):
        problems.append("facts.violations must be a list")
        violations = []
    for index, item in enumerate(violations):
        if not _is_exact_object(item, violation_keys) or not all(
            isinstance(item.get(field), str) and item.get(field)
            for field in violation_keys
        ):
            problems.append(f"facts.violations[{index}] schema is invalid")
    if isinstance(class_counts, Mapping) and isinstance(baseline_counts, Mapping):
        if (
            set(class_counts) == FAIL_CLOSED_CLASSES
            and set(baseline_counts) == FAIL_CLOSED_CLASSES
            and all(_nonnegative_int(value) for value in class_counts.values())
            and all(_nonnegative_int(value) for value in baseline_counts.values())
        ):
            regressed = any(
                class_counts[name] > baseline_counts[name]
                for name in FAIL_CLOSED_CLASSES
            )
            expected_status = (
                STATUS_PASS if not violations and not regressed else STATUS_FAIL
            )
            if status != expected_status:
                problems.append(
                    f"receipt status must be {expected_status} for fail-closed facts"
                )
    return problems


def _validate_facts(kind: object, facts: object, status: object) -> list[str]:
    if not isinstance(facts, Mapping):
        return ["receipt facts must be an object"]
    if kind == KIND_VERIFIED_SUBSET:
        return _validate_verified_subset_facts(facts, status)
    if kind == KIND_CANONICALIZATION_CONTRACT:
        return _validate_metric_facts(
            facts,
            expected_keys=CANONICALIZATION_METRICS,
            count_field="open_violations",
            count_value_label="canonicalization",
            status=status,
        )
    if kind == KIND_STRUCTURAL_AUDIT:
        return _validate_metric_facts(
            facts,
            expected_keys=STRUCTURAL_AUDIT_METRICS,
            count_field="findings_count",
            count_value_label="structural audit",
            status=status,
        )
    if kind == KIND_DEGRADE_TO_SLOW_GATE:
        return _validate_degrade_facts(facts, status)
    if kind == KIND_FAIL_CLOSED_GATE:
        return _validate_fail_closed_facts(facts, status)
    return []


def validate_receipt(
    payload: object,
    *,
    expected_kind: str,
    expected_source_sha: str,
    repo_root: Path,
    verify_inputs: bool = True,
    now: dt.datetime | None = None,
) -> tuple[str, ...]:
    """Return every violation of the source-addressed receipt contract."""

    if not isinstance(payload, Mapping):
        return ("release criterion receipt root must be an object",)
    problems: list[str] = []
    if set(payload) != _ROOT_KEYS:
        problems.append(
            "release criterion receipt keys differ from schema: "
            f"missing={sorted(_ROOT_KEYS - set(payload), key=str)!r}, "
            f"unknown={sorted(set(payload) - _ROOT_KEYS, key=str)!r}"
        )
    if payload.get("schema_version") != SCHEMA_VERSION:
        problems.append(f"receipt schema_version must be {SCHEMA_VERSION}")
    kind = payload.get("kind")
    if not isinstance(kind, str) or kind not in KINDS:
        problems.append("receipt kind is unknown")
    if kind != expected_kind:
        problems.append(
            f"receipt kind mismatch: expected {expected_kind!r}, got {kind!r}"
        )
    source_sha = payload.get("source_sha")
    if not _valid_source_sha(source_sha):
        problems.append("receipt source_sha must be lowercase Git object hex")
    if source_sha != expected_source_sha:
        problems.append("receipt source_sha differs from the expected release source")
    generated_at = payload.get("generated_at")
    parsed_at: dt.datetime | None = None
    if not isinstance(generated_at, str) or not _UTC_TIMESTAMP_RE.fullmatch(
        generated_at
    ):
        problems.append(
            "receipt generated_at must be a strict UTC timestamp ending in Z"
        )
    else:
        try:
            parsed_at = dt.datetime.fromisoformat(generated_at[:-1] + "+00:00")
        except ValueError:
            problems.append("receipt generated_at is not a valid UTC timestamp")
    if parsed_at is not None:
        current = now or dt.datetime.now(dt.timezone.utc)
        if current.tzinfo is None or current.utcoffset() is None:
            raise ValueError("receipt validation 'now' must be timezone-aware")
        if parsed_at > current.astimezone(dt.timezone.utc) + _MAX_FUTURE_SKEW:
            problems.append("receipt generated_at is in the future")
    status = payload.get("status")
    if not isinstance(status, str) or status not in {STATUS_PASS, STATUS_FAIL}:
        problems.append("receipt status must be PASS or FAIL")

    producer = payload.get("producer")
    if not _is_exact_object(producer, _PRODUCER_KEYS):
        problems.append("receipt producer must contain exactly argv and tool")
    else:
        assert isinstance(producer, Mapping)
        argv = producer.get("argv")
        if not _string_list(argv) or not argv:
            problems.append("receipt producer.argv must be a non-empty string list")
        problems.extend(
            _validate_input_record(
                producer.get("tool"),
                label="receipt producer.tool",
                repo_root=repo_root,
                verify_bytes=verify_inputs,
            )
        )
        tool = producer.get("tool")
        expected_tool = KIND_TO_TOOL.get(kind) if isinstance(kind, str) else None
        if isinstance(tool, Mapping) and tool.get("path") != expected_tool:
            problems.append(
                f"receipt producer.tool.path must be {expected_tool!r} for {kind!r}"
            )
        if isinstance(argv, list) and argv and isinstance(tool, Mapping):
            if argv[0] != tool.get("path"):
                problems.append(
                    "receipt producer.argv[0] must equal producer.tool.path"
                )
            if kind == KIND_VERIFIED_SUBSET:
                expected_options = {"--coordinate", "--receipt", "--source-sha"}
                option_names = {
                    value
                    for value in argv[2::2]
                    if isinstance(value, str) and value.startswith("--")
                }
                if (
                    len(argv) != 8
                    or argv[1] != "run"
                    or option_names != expected_options
                    or any(argv.count(option) != 1 for option in expected_options)
                ):
                    problems.append(
                        "verified-subset producer argv must be one exact run invocation"
                    )

    inputs = payload.get("inputs")
    input_paths: list[str] = []
    if not isinstance(inputs, list):
        problems.append("receipt inputs must be a list")
        inputs = []
    for index, item in enumerate(inputs):
        problems.extend(
            _validate_input_record(
                item,
                label=f"receipt inputs[{index}]",
                repo_root=repo_root,
                verify_bytes=verify_inputs,
            )
        )
        if isinstance(item, Mapping) and isinstance(item.get("path"), str):
            input_paths.append(item["path"])
    if input_paths != sorted(set(input_paths)):
        problems.append("receipt inputs must be sorted and unique by path")

    facts = payload.get("facts")
    problems.extend(_validate_facts(kind, facts, status))
    if isinstance(facts, Mapping):
        if kind == KIND_VERIFIED_SUBSET:
            execution = facts.get("execution")
            ci = execution.get("ci") if isinstance(execution, Mapping) else None
            if not isinstance(ci, Mapping) or ci.get("source_sha") != payload.get(
                "source_sha"
            ):
                problems.append(
                    "verified-subset CI execution identity differs from source_sha"
                )
            coordinate = facts.get("coordinate")
            producer = payload.get("producer")
            argv = producer.get("argv") if isinstance(producer, Mapping) else None
            if isinstance(coordinate, Mapping) and isinstance(argv, list):
                try:
                    coordinate_value = argv[argv.index("--coordinate") + 1]
                    source_value = argv[argv.index("--source-sha") + 1]
                except (ValueError, IndexError):
                    pass
                else:
                    if coordinate_value != coordinate.get("id"):
                        problems.append(
                            "verified-subset producer coordinate differs from facts"
                        )
                    if source_value != payload.get("source_sha"):
                        problems.append(
                            "verified-subset producer source differs from receipt"
                        )
        required_input: object | None = None
        if isinstance(kind, str) and kind in {
            KIND_CANONICALIZATION_CONTRACT,
            KIND_STRUCTURAL_AUDIT,
        }:
            required_input = facts.get("baseline_path")
        elif isinstance(kind, str) and kind in {
            KIND_DEGRADE_TO_SLOW_GATE,
            KIND_FAIL_CLOSED_GATE,
        }:
            required_input = facts.get("registry_path")
        if isinstance(required_input, str) and required_input not in input_paths:
            problems.append(
                f"receipt inputs do not bind required file {required_input!r}"
            )
        if kind == KIND_VERIFIED_SUBSET:
            authority_inputs = facts.get("authority_inputs")
            expected_inputs = (
                list(authority_inputs) if isinstance(authority_inputs, list) else []
            )
            if input_paths != expected_inputs:
                problems.append(
                    "verified-subset receipt inputs must exactly match authorities"
                )

    if (
        verify_inputs
        and isinstance(facts, Mapping)
        and isinstance(kind, str)
        and kind
        in {
            KIND_CANONICALIZATION_CONTRACT,
            KIND_STRUCTURAL_AUDIT,
        }
    ):
        baseline_path = facts.get("baseline_path")
        if (
            isinstance(baseline_path, str)
            and _portable_posix(baseline_path) is not None
        ):
            path = repo_root.resolve(strict=True).joinpath(
                *_portable_posix(baseline_path).parts  # type: ignore[union-attr]
            )
            try:
                baseline_payload = loads_exact(path.read_text(encoding="utf-8"))
            except (OSError, UnicodeError, json.JSONDecodeError, ExactJsonError) as exc:
                problems.append(f"receipt baseline is invalid: {exc}")
            else:
                if baseline_payload != facts.get("baseline_metrics"):
                    problems.append(
                        "facts.baseline_metrics differs from bound baseline"
                    )

    return tuple(problems)
