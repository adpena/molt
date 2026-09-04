#!/usr/bin/env python3
"""Validate and execute the exact cross-platform verified-subset matrix."""

from __future__ import annotations

import argparse
import hashlib
import json
import os
import shutil
import struct
import subprocess
import sys
import tempfile
from collections.abc import Mapping, Sequence
from enum import StrEnum
from functools import lru_cache
from pathlib import Path
from typing import Any, Final

from molt.exact_json import ExactJsonError, loads_exact
from molt.file_publication import is_link_like
from molt import python_interpreter
from molt.verified_subset import (
    VerifiedSubsetCoordinate,
    VerifiedSubsetPolicy,
    load_verified_subset_policy,
    require_current_host,
    verified_subset_coordinate_by_id,
    verified_subset_coordinates,
)

try:
    from tools import harness_memory_guard
    from tools import release_criterion_receipt as release_receipt
    from tools.compat import comparison as compat_comparison
    from tools.compat import test_policy
except ModuleNotFoundError:  # pragma: no cover - direct script import from tools/
    import harness_memory_guard  # type: ignore[no-redef]
    import release_criterion_receipt as release_receipt  # type: ignore[no-redef]
    from compat import comparison as compat_comparison  # type: ignore[no-redef]
    from compat import test_policy  # type: ignore[no-redef]

ROOT = Path(__file__).resolve().parents[1]
_MAX_RECEIPT_BYTES = 4 * 1024 * 1024


class ResultStatus(StrEnum):
    """Canonical status vocabulary shared by result producers and validators."""

    ERROR = "error"
    FAIL = "fail"
    OOM = "oom"
    PASS = "pass"
    SKIP = "skip"
    UNCALIBRATED = "uncalibrated"


RESULT_STATUSES: Final[frozenset[ResultStatus]] = frozenset(ResultStatus)


@lru_cache(maxsize=4)
def verified_subset_test_files(
    policy: VerifiedSubsetPolicy,
) -> tuple[Path, ...]:
    return test_policy.collect_physical_test_files(
        policy.suite_selectors,
        repo_root=ROOT,
    )


@lru_cache(maxsize=4)
def _verified_subset_test_sources(
    policy: VerifiedSubsetPolicy,
) -> tuple[test_policy.TestPolicySource, ...]:
    return test_policy.load_test_sources(
        verified_subset_test_files(policy), repo_root=ROOT
    )


def verified_subset_projection(
    policy: VerifiedSubsetPolicy,
    coordinate: VerifiedSubsetCoordinate,
    *,
    sources: Sequence[test_policy.TestPolicySource] | None = None,
) -> test_policy.CoordinateProjection:
    if sources is None:
        return _cached_verified_subset_projection(policy, coordinate)
    prepared = tuple(sources)
    return test_policy.project_prepared_coordinate(
        prepared,
        python=coordinate.python,
        platform=coordinate.platform,
        arch=coordinate.arch,
        backend=coordinate.backend,
        excluded_verification_scopes=frozenset(policy.excluded_verification_scopes),
    )


@lru_cache(maxsize=128)
def _cached_verified_subset_projection(
    policy: VerifiedSubsetPolicy,
    coordinate: VerifiedSubsetCoordinate,
) -> test_policy.CoordinateProjection:
    return test_policy.project_prepared_coordinate(
        _verified_subset_test_sources(policy),
        python=coordinate.python,
        platform=coordinate.platform,
        arch=coordinate.arch,
        backend=coordinate.backend,
        excluded_verification_scopes=frozenset(policy.excluded_verification_scopes),
    )


def verified_subset_authority_files(
    policy: VerifiedSubsetPolicy,
) -> tuple[Path, ...]:
    paths = {
        (ROOT / "config" / "verified_subset.toml").resolve(strict=True),
        (ROOT / "config" / "release_targets.toml").resolve(strict=True),
        (ROOT / "src" / "molt" / "release_matrix.py").resolve(strict=True),
        (ROOT / "src" / "molt" / "target_python.py").resolve(strict=True),
        (ROOT / "src" / "molt" / "verified_subset.py").resolve(strict=True),
        (ROOT / "tests" / "molt_diff.py").resolve(strict=True),
        (ROOT / "tools" / "compat" / "backends.py").resolve(strict=True),
        (ROOT / "tools" / "compat" / "comparison.py").resolve(strict=True),
        (ROOT / "tools" / "compat" / "test_policy.py").resolve(strict=True),
    }
    return tuple(sorted(paths, key=lambda path: path.relative_to(ROOT).as_posix()))


def load_manifest() -> VerifiedSubsetPolicy:
    return load_verified_subset_policy()


def validate_suite_equivalence_floors(policy: VerifiedSubsetPolicy) -> None:
    for suite in policy.suites:
        files = test_policy.collect_physical_test_files(
            ((suite.path, suite.recursive),), repo_root=ROOT
        )
        sources = test_policy.load_test_sources(files, repo_root=ROOT)
        actual = sum(
            source.metadata.verification_scope == test_policy.CPYTHON_EQUIVALENCE_SCOPE
            for source in sources
        )
        if actual < suite.cpython_equivalence_floor:
            raise ValueError(
                "verified-subset suite "
                f"{suite.path} contracted below its CPython-equivalence floor: "
                f"actual={actual} floor={suite.cpython_equivalence_floor}"
            )


def validate_manifest(
    policy: VerifiedSubsetPolicy,
) -> tuple[test_policy.CoordinateProjection, ...]:
    coordinates = verified_subset_coordinates(policy)
    tests = verified_subset_test_files(policy)
    expected = len(policy.python_versions) * len(policy.backends)
    if not coordinates or len(coordinates) % expected:
        raise ValueError("verified-subset coordinate cross-product is incomplete")
    if not tests:
        raise ValueError("verified-subset test closure is empty")
    validate_suite_equivalence_floors(policy)
    sources = _verified_subset_test_sources(policy)
    projections: list[test_policy.CoordinateProjection] = []
    for coordinate in coordinates:
        projection = verified_subset_projection(policy, coordinate, sources=sources)
        if not projection.applicable:
            raise ValueError(
                f"verified-subset coordinate has no applicable tests: {coordinate.id}"
            )
        projections.append(projection)
    return tuple(projections)


def matrix_payload(policy: VerifiedSubsetPolicy | None = None) -> dict[str, object]:
    resolved_policy = policy or load_manifest()
    validate_manifest(resolved_policy)
    return {
        "include": [
            coordinate.as_record()
            for coordinate in verified_subset_coordinates(resolved_policy)
        ]
    }


def _load_summary(path: Path) -> Mapping[str, Any]:
    try:
        payload = loads_exact(path.read_text(encoding="utf-8"))
    except (OSError, UnicodeError, json.JSONDecodeError, ExactJsonError) as exc:
        raise ValueError(f"verified-subset harness summary is invalid: {exc}") from exc
    if not isinstance(payload, Mapping):
        raise ValueError("verified-subset harness summary root must be an object")
    return payload


def _summary_statuses(summary: Mapping[str, Any]) -> dict[str, str]:
    raw_results = summary.get("item_results")
    if not isinstance(raw_results, list):
        raise ValueError("verified-subset harness summary has no item_results list")
    results: dict[str, str] = {}
    for index, item in enumerate(raw_results):
        if not isinstance(item, Mapping):
            raise ValueError(
                f"verified-subset harness item_results[{index}] is not an object"
            )
        path = item.get("path")
        status = item.get("status")
        if not isinstance(path, str) or status not in RESULT_STATUSES:
            raise ValueError(
                f"verified-subset harness item_results[{index}] is invalid"
            )
        normalized = test_policy.normalize_repo_relative(path, repo_root=ROOT)
        if normalized in results:
            raise ValueError("verified-subset harness returned duplicate result paths")
        results[normalized] = str(status)
    return results


def _load_result_rows(path: Path) -> tuple[Mapping[str, Any], ...]:
    try:
        lines = path.read_text(encoding="utf-8").splitlines()
    except (OSError, UnicodeError) as exc:
        raise ValueError(f"verified-subset raw results are unreadable: {exc}") from exc
    rows: list[Mapping[str, Any]] = []
    for line_number, line in enumerate(lines, 1):
        if not line.strip():
            continue
        try:
            row = loads_exact(line)
        except (json.JSONDecodeError, ExactJsonError) as exc:
            raise ValueError(
                f"verified-subset raw result line {line_number} is invalid: {exc}"
            ) from exc
        if not isinstance(row, Mapping):
            raise ValueError(
                f"verified-subset raw result line {line_number} is not an object"
            )
        rows.append(row)
    if not rows:
        raise ValueError("verified-subset raw results are empty")
    return tuple(rows)


def _result_outcomes(
    *,
    summary: Mapping[str, Any],
    results_path: Path,
    projection: test_policy.CoordinateProjection,
    coordinate: VerifiedSubsetCoordinate,
) -> list[dict[str, object]]:
    summary_statuses = _summary_statuses(summary)
    expected = {test.path: test for test in projection.applicable}
    test_rows: dict[str, Mapping[str, Any]] = {}
    backend_rows: dict[str, Mapping[str, Any]] = {}
    for row in _load_result_rows(results_path):
        path = row.get("file")
        record_type = row.get("record_type")
        if not isinstance(path, str) or record_type not in {"test", "backend"}:
            raise ValueError("verified-subset raw result schema is invalid")
        normalized = test_policy.normalize_repo_relative(path, repo_root=ROOT)
        if normalized not in expected:
            raise ValueError(
                f"verified-subset raw result is outside the projection: {normalized}"
            )
        destination = test_rows if record_type == "test" else backend_rows
        if normalized in destination:
            raise ValueError(
                f"verified-subset raw result is duplicated: {record_type} {normalized}"
            )
        destination[normalized] = row

    expected_paths = sorted(expected)
    observed_closures = {
        "backend": set(backend_rows),
        "summary": set(summary_statuses),
        "test": set(test_rows),
    }
    expected_closure = set(expected_paths)
    closure_drift = []
    for stream, observed in observed_closures.items():
        missing = sorted(expected_closure - observed)
        extra = sorted(observed - expected_closure)
        if missing or extra:
            closure_drift.append(
                f"{stream}_missing={missing[:10]!r}, {stream}_extra={extra[:10]!r}"
            )
    if closure_drift:
        raise ValueError(
            "verified-subset harness did not execute the exact projected closure: "
            + "; ".join(closure_drift)
        )

    outcomes: list[dict[str, object]] = []
    for path in expected_paths:
        test = expected[path]
        row = test_rows[path]
        backend_row = backend_rows.get(path)
        raw_status = row.get("raw_status")
        resolved_status = row.get("resolved_status")
        if raw_status not in RESULT_STATUSES or resolved_status not in RESULT_STATUSES:
            raise ValueError(f"verified-subset result statuses are invalid: {path}")
        if summary_statuses[path] != resolved_status:
            raise ValueError(f"verified-subset summary/raw status disagreement: {path}")
        if row.get("expect_molt_fail") is not test.expect_molt_fail:
            raise ValueError(
                f"verified-subset expected-failure classification drifted: {path}"
            )
        if row.get("expected_failure_reason") != test.expected_failure_reason:
            raise ValueError(f"verified-subset expected-failure reason drifted: {path}")
        if row.get("compiler_target_python") != coordinate.python:
            raise ValueError(f"verified-subset compiler target Python drifted: {path}")
        backend_status: object = None
        backend_returncode: object = None
        backend_stdout_sha256: object = None
        backend_stderr_sha256: object = None
        if backend_row is not None:
            if backend_row.get("backend") != coordinate.backend:
                raise ValueError(f"verified-subset backend result drifted: {path}")
            backend_status = backend_row.get("raw_status")
            backend_returncode = backend_row.get("returncode")
            backend_stdout_sha256 = backend_row.get("stdout_sha256")
            backend_stderr_sha256 = backend_row.get("stderr_sha256")
        outcomes.append(
            {
                "backend": coordinate.backend,
                "backend_returncode": backend_returncode,
                "backend_status": backend_status,
                "backend_stderr_sha256": backend_stderr_sha256,
                "backend_stdout_sha256": backend_stdout_sha256,
                "comparison_law": row.get("comparison_law"),
                "compiler_target_python": coordinate.python,
                "cpython_returncode": row.get("cpython_returncode"),
                "cpython_stderr_sha256": row.get("cpython_stderr_sha256"),
                "cpython_stdout_sha256": row.get("cpython_stdout_sha256"),
                "expect_molt_fail": test.expect_molt_fail,
                "expected_failure_reason": test.expected_failure_reason,
                "path": path,
                "raw_status": raw_status,
                "reason_tag": row.get("reason_tag"),
                "resolved_status": resolved_status,
            }
        )
    return outcomes


def outcome_counts(results: Sequence[Mapping[str, object]]) -> dict[str, int]:
    return {
        "backend_failed": sum(item["backend_status"] == "fail" for item in results),
        "backend_missing": sum(item["backend_status"] is None for item in results),
        "backend_passed": sum(item["backend_status"] == "pass" for item in results),
        "errors": sum(item["raw_status"] == "error" for item in results),
        "executed": len(results),
        "expected_failures": sum(bool(item["expect_molt_fail"]) for item in results),
        "oom": sum(item["raw_status"] == "oom" for item in results),
        "raw_failed": sum(item["raw_status"] == "fail" for item in results),
        "raw_passed": sum(item["raw_status"] == "pass" for item in results),
        "resolved_failed": sum(item["resolved_status"] == "fail" for item in results),
        "resolved_passed": sum(item["resolved_status"] == "pass" for item in results),
        "skipped": sum(item["raw_status"] == "skip" for item in results),
        "uncalibrated": sum(item["raw_status"] == "uncalibrated" for item in results),
    }


def outcomes_pass(results: Sequence[Mapping[str, object]]) -> bool:
    def valid_sha256(value: object) -> bool:
        return (
            isinstance(value, str)
            and len(value) == 64
            and all(character in "0123456789abcdef" for character in value)
        )

    return bool(results) and all(
        item["raw_status"] == "pass"
        and item["resolved_status"] == "pass"
        and item["backend_status"] == "pass"
        and item["expect_molt_fail"] is False
        and item["comparison_law"] == compat_comparison.COMPARISON_LAW_VERSION
        and item["reason_tag"] is None
        and isinstance(item["cpython_returncode"], int)
        and isinstance(item["backend_returncode"], int)
        and valid_sha256(item["cpython_stdout_sha256"])
        and valid_sha256(item["cpython_stderr_sha256"])
        and valid_sha256(item["backend_stdout_sha256"])
        and valid_sha256(item["backend_stderr_sha256"])
        for item in results
    )


def _receipt_facts(
    *,
    coordinate: VerifiedSubsetCoordinate,
    policy: VerifiedSubsetPolicy,
    projection: test_policy.CoordinateProjection,
    results: Sequence[Mapping[str, object]],
    execution: Mapping[str, object],
) -> dict[str, object]:
    actual_paths = [str(item["path"]) for item in results]
    expected_paths = [test.path for test in projection.applicable]
    if actual_paths != expected_paths:
        raise ValueError("verified-subset outcomes differ from projected test paths")
    return {
        "authority_inputs": [
            path.relative_to(ROOT).as_posix()
            for path in verified_subset_authority_files(policy)
        ],
        "coordinate": coordinate.as_record(),
        "counts": outcome_counts(results),
        "execution": dict(execution),
        "fallback_policy": policy.fallback_policy,
        "outcomes": list(results),
        "projection": projection.closure_record(),
        "suites": [suite.as_record() for suite in policy.suites],
    }


def _file_sha256(path: Path) -> str:
    digest = hashlib.sha256()
    with path.open("rb") as stream:
        for chunk in iter(lambda: stream.read(1024 * 1024), b""):
            digest.update(chunk)
    return digest.hexdigest()


def _reference_python_identity(
    coordinate: VerifiedSubsetCoordinate,
) -> dict[str, object]:
    interpreter = python_interpreter.probe_python_command(
        (sys.executable,),
        env=os.environ,
        cwd=ROOT,
    )
    probe_code = (
        "import json,struct,sys,sysconfig;"
        "v=sys.version_info;"
        "print(json.dumps({"
        "'abi_flags':getattr(sys,'abiflags',''),"
        "'cache_tag':sys.implementation.cache_tag,"
        "'gil_disabled':bool(sysconfig.get_config_var('Py_GIL_DISABLED') or 0),"
        "'hexversion':sys.hexversion,"
        "'pointer_bits':struct.calcsize('P')*8,"
        "'version_info':[v.major,v.minor,v.micro,v.releaselevel,v.serial]"
        "},sort_keys=True))"
    )
    completed = harness_memory_guard.guarded_completed_process(
        [*interpreter.command, "-I", "-c", probe_code],
        prefix="MOLT_VERIFIED_SUBSET_PYTHON",
        cwd=ROOT,
        capture_output=True,
        text=True,
    )
    if completed.returncode != 0:
        raise ValueError("reference CPython identity probe failed")
    try:
        details = loads_exact(completed.stdout.strip().splitlines()[-1])
    except (IndexError, json.JSONDecodeError, ExactJsonError) as exc:
        raise ValueError("reference CPython identity probe was invalid") from exc
    if not isinstance(details, Mapping):
        raise ValueError("reference CPython identity probe was not an object")
    executable = Path(interpreter.executable).resolve(strict=True)
    version_info = details.get("version_info")
    if (
        not isinstance(version_info, list)
        or len(version_info) != 5
        or f"{version_info[0]}.{version_info[1]}" != coordinate.python
        or interpreter.version != coordinate.reference_python
        or interpreter.implementation != "CPython"
        or details.get("gil_disabled") is not False
    ):
        raise ValueError("reference interpreter differs from the coordinate")
    return {
        "abi_flags": details.get("abi_flags"),
        "cache_tag": details.get("cache_tag"),
        "command_executable": Path(interpreter.command[0]).name,
        "executable_name": executable.name,
        "executable_sha256": _file_sha256(executable),
        "gil_disabled": False,
        "hexversion": details.get("hexversion"),
        "implementation": interpreter.implementation,
        "pointer_bits": details.get("pointer_bits"),
        "version": interpreter.version,
        "version_info": version_info,
    }


def _rust_identity(coordinate: VerifiedSubsetCoordinate) -> dict[str, str]:
    rustc_name = shutil.which("rustc")
    if rustc_name is None:
        raise ValueError("rustc is unavailable for verified-subset execution")
    rustc = Path(rustc_name).resolve(strict=True)
    completed = harness_memory_guard.guarded_completed_process(
        [str(rustc), "-vV"],
        prefix="MOLT_VERIFIED_SUBSET_RUST",
        cwd=ROOT,
        capture_output=True,
        text=True,
    )
    if completed.returncode != 0:
        raise ValueError("rustc identity probe failed")
    fields = {
        key.strip(): value.strip()
        for line in completed.stdout.splitlines()
        if ":" in line
        for key, value in (line.split(":", 1),)
    }
    host = fields.get("host")
    if host != coordinate.rust_target:
        raise ValueError(
            f"rustc host {host!r} differs from coordinate {coordinate.rust_target!r}"
        )
    return {
        "binary_name": rustc.name,
        "binary_sha256": _file_sha256(rustc),
        "commit_date": fields.get("commit-date", ""),
        "commit_hash": fields.get("commit-hash", ""),
        "host": host,
        "llvm_version": fields.get("LLVM version", ""),
        "release": fields.get("release", ""),
    }


def _github_execution_identity(
    coordinate: VerifiedSubsetCoordinate, *, source_sha: str
) -> dict[str, object]:
    required = {
        "GITHUB_ACTIONS": "true",
        "GITHUB_SHA": source_sha,
        "MOLT_VERIFIED_SUBSET_RUNNER": coordinate.runner,
    }
    for key, expected in required.items():
        if os.environ.get(key) != expected:
            raise ValueError(f"verified-subset CI identity {key} is not {expected!r}")
    for key in (
        "GITHUB_RUN_ID",
        "GITHUB_RUN_ATTEMPT",
        "GITHUB_JOB",
        "GITHUB_WORKFLOW_REF",
    ):
        if not os.environ.get(key, "").strip():
            raise ValueError(f"verified-subset CI identity is missing {key}")
    expected_runner_os = {
        "linux": "Linux",
        "macos": "macOS",
        "windows": "Windows",
    }[coordinate.platform]
    expected_runner_arch = {
        "x86_64": "X64",
        "aarch64": "ARM64",
        "arm64": "ARM64",
    }[coordinate.arch]
    if os.environ.get("RUNNER_OS") != expected_runner_os:
        raise ValueError("GitHub runner OS differs from the coordinate")
    if os.environ.get("RUNNER_ARCH") != expected_runner_arch:
        raise ValueError("GitHub runner architecture differs from the coordinate")
    return {
        "job": os.environ["GITHUB_JOB"],
        "provider": "github-actions",
        "run_attempt": os.environ["GITHUB_RUN_ATTEMPT"],
        "run_id": os.environ["GITHUB_RUN_ID"],
        "runner_arch": os.environ["RUNNER_ARCH"],
        "runner_label": os.environ["MOLT_VERIFIED_SUBSET_RUNNER"],
        "runner_os": os.environ["RUNNER_OS"],
        "source_sha": os.environ["GITHUB_SHA"],
        "workflow_ref": os.environ["GITHUB_WORKFLOW_REF"],
    }


def _backend_identity(coordinate: VerifiedSubsetCoordinate) -> dict[str, object]:
    if coordinate.backend == "native":
        return {"backend": "native", "runner": "process"}
    node_name = shutil.which("node")
    if node_name is None:
        raise ValueError("node is unavailable for verified-subset WASM execution")
    node = Path(node_name).resolve(strict=True)
    completed = harness_memory_guard.guarded_completed_process(
        [str(node), "--version"],
        prefix="MOLT_VERIFIED_SUBSET_NODE",
        cwd=ROOT,
        capture_output=True,
        text=True,
    )
    version = completed.stdout.strip()
    if completed.returncode != 0 or not version:
        raise ValueError("node identity probe failed")
    return {
        "backend": "wasm",
        "binary_name": node.name,
        "binary_sha256": _file_sha256(node),
        "runner": "node-wasi",
        "version": version,
    }


def _execution_identity(
    coordinate: VerifiedSubsetCoordinate, *, source_sha: str
) -> dict[str, object]:
    require_current_host(coordinate)
    return {
        "backend": _backend_identity(coordinate),
        "ci": _github_execution_identity(coordinate, source_sha=source_sha),
        "host": {
            "arch": test_policy.current_architecture(),
            "platform": test_policy.current_platform_name(),
            "pointer_bits": struct.calcsize("P") * 8,
        },
        "python": _reference_python_identity(coordinate),
        "rust": _rust_identity(coordinate),
    }


def run_differential_suites(
    coordinate: VerifiedSubsetCoordinate,
    *,
    projection: test_policy.CoordinateProjection,
    results_path: Path,
    schedule_path: Path,
    summary_path: Path,
) -> subprocess.CompletedProcess[str]:
    require_current_host(coordinate)
    schedule_path.write_text(
        "".join(f"{test.path}\n" for test in projection.applicable),
        encoding="utf-8",
        newline="\n",
    )
    env = dict(os.environ)
    env["MOLT_DIFF_RESULTS_JSONL"] = str(results_path)
    env["MOLT_DIFF_PYTHON"] = sys.executable
    env["MOLT_VERIFIED_SUBSET_COORDINATE"] = coordinate.id
    env["MOLT_DIFF_TRUSTED"] = "0"
    env["MOLT_TRUSTED"] = "0"
    env.pop("MOLT_DIFF_CAPABILITIES", None)
    env.pop("MOLT_CAPABILITIES", None)
    cmd = [
        sys.executable,
        str(ROOT / "tests" / "molt_diff.py"),
        "--python-version",
        coordinate.reference_python,
        "--molt-target-python",
        coordinate.python,
        "--target",
        coordinate.backend,
        "--jobs",
        "1",
        "--json-output",
        str(summary_path),
        "--files-from",
        str(schedule_path),
    ]
    return harness_memory_guard.guarded_completed_process(
        cmd,
        prefix="MOLT_VERIFIED_SUBSET",
        cwd=ROOT,
        env=env,
        capture_output=False,
        text=True,
    )


def _receipt_files(receipt_root: Path) -> tuple[Path, ...]:
    absolute_root = receipt_root.absolute()
    root = receipt_root.resolve(strict=True)
    if not root.is_dir() or is_link_like(receipt_root) or absolute_root != root:
        raise ValueError("verified-subset receipt root must be a real directory")
    expected_count = len(verified_subset_coordinates())
    files: list[Path] = []
    stack = [root]
    while stack:
        directory = stack.pop()
        with os.scandir(directory) as entries:
            for entry in entries:
                metadata = entry.stat(follow_symlinks=False)
                path = Path(entry.path)
                if is_link_like(path):
                    raise ValueError(
                        f"verified-subset receipt tree contains a link: {entry.path}"
                    )
                if entry.is_dir(follow_symlinks=False):
                    stack.append(path)
                    continue
                if not entry.is_file(follow_symlinks=False) or path.suffix != ".json":
                    raise ValueError(
                        f"verified-subset receipt tree contains an unexpected entry: {path}"
                    )
                if metadata.st_size > _MAX_RECEIPT_BYTES:
                    raise ValueError(
                        f"verified-subset receipt exceeds size bound: {path}"
                    )
                files.append(path)
                if len(files) > expected_count:
                    raise ValueError(
                        "verified-subset receipt tree contains extra files"
                    )
    files.sort(key=lambda path: path.relative_to(root).as_posix())
    return tuple(files)


def verify_receipt_closure(*, receipt_root: Path, source_sha: str) -> None:
    expected_coordinates = {
        coordinate.id for coordinate in verified_subset_coordinates()
    }
    files = _receipt_files(receipt_root)
    if len(files) != len(expected_coordinates):
        raise ValueError(
            "verified-subset receipt count is not exact: "
            f"expected={len(expected_coordinates)}, got={len(files)}"
        )
    seen: set[str] = set()
    canonical_tool: object | None = None
    canonical_inputs: object | None = None
    for index, path in enumerate(files):
        payload = _load_summary(path)
        producer = payload.get("producer")
        tool = producer.get("tool") if isinstance(producer, Mapping) else None
        inputs = payload.get("inputs")
        if index == 0:
            canonical_tool = tool
            canonical_inputs = inputs
        elif tool != canonical_tool or inputs != canonical_inputs:
            raise ValueError(
                "verified-subset receipt common input records differ from "
                f"the canonical receipt: {path}"
            )
        problems = release_receipt.validate_receipt(
            payload,
            expected_kind=release_receipt.KIND_VERIFIED_SUBSET,
            expected_source_sha=source_sha,
            repo_root=ROOT,
            verify_inputs=index == 0,
        )
        if problems:
            raise ValueError(
                f"invalid verified-subset receipt {path}: {'; '.join(problems)}"
            )
        if payload.get("status") != release_receipt.STATUS_PASS:
            raise ValueError(f"verified-subset receipt did not pass: {path}")
        facts = payload.get("facts")
        coordinate = facts.get("coordinate") if isinstance(facts, Mapping) else None
        coordinate_id = (
            coordinate.get("id") if isinstance(coordinate, Mapping) else None
        )
        if (
            not isinstance(coordinate_id, str)
            or coordinate_id not in expected_coordinates
        ):
            raise ValueError(
                f"verified-subset receipt has an unknown coordinate: {path}"
            )
        if coordinate_id in seen:
            raise ValueError(
                f"verified-subset receipt coordinate is duplicated: {coordinate_id}"
            )
        seen.add(coordinate_id)
    if seen != expected_coordinates:
        missing = sorted(expected_coordinates - seen)
        raise ValueError(
            f"verified-subset receipt matrix is incomplete: missing={missing!r}"
        )


def _run_coordinate(
    *,
    coordinate: VerifiedSubsetCoordinate,
    policy: VerifiedSubsetPolicy,
    raw_argv: Sequence[str],
    receipt_path: Path | None,
    source_sha: str | None,
) -> int:
    destination = release_receipt.prepare_receipt_destination(
        repo_root=ROOT,
        receipt_path=receipt_path,
        source_sha=source_sha,
    )
    temp_root = ROOT / "tmp" / "verified-subset"
    temp_root.mkdir(parents=True, exist_ok=True)
    stage = Path(tempfile.mkdtemp(prefix=f"{coordinate.id}-", dir=temp_root))
    summary_path = stage / "summary.json"
    results_path = stage / "results.jsonl"
    schedule_path = stage / "schedule.txt"
    try:
        projection = verified_subset_projection(policy, coordinate)
        completed = run_differential_suites(
            coordinate,
            projection=projection,
            results_path=results_path,
            schedule_path=schedule_path,
            summary_path=summary_path,
        )
        summary = _load_summary(summary_path)
        results = _result_outcomes(
            summary=summary,
            results_path=results_path,
            projection=projection,
            coordinate=coordinate,
        )
        passed = completed.returncode == 0 and outcomes_pass(results)
        if destination is not None:
            input_paths = list(verified_subset_authority_files(policy))
            execution = _execution_identity(
                coordinate, source_sha=destination.source_sha
            )
            receipt = release_receipt.build_receipt(
                kind=release_receipt.KIND_VERIFIED_SUBSET,
                source_sha=destination.source_sha,
                status=(
                    release_receipt.STATUS_PASS
                    if passed
                    else release_receipt.STATUS_FAIL
                ),
                argv=raw_argv,
                tool_path=Path(__file__),
                facts=_receipt_facts(
                    coordinate=coordinate,
                    policy=policy,
                    projection=projection,
                    results=results,
                    execution=execution,
                ),
                input_paths=input_paths,
                repo_root=ROOT,
            )
            release_receipt.write_receipt(receipt, destination)
            print(f"verified_subset_receipt={destination.output_path}")
        return 0 if passed else 1
    finally:
        shutil.rmtree(stage, ignore_errors=True)


def main(argv: Sequence[str] | None = None) -> int:
    raw_argv = list(sys.argv[1:] if argv is None else argv)
    parser = argparse.ArgumentParser(description=__doc__)
    subparsers = parser.add_subparsers(dest="command", required=True)
    subparsers.add_parser("check", help="validate policy, test, and matrix closure")
    subparsers.add_parser("matrix", help="emit the exact release proof matrix")
    verify = subparsers.add_parser(
        "verify-receipts", help="verify one exact source-bound receipt matrix"
    )
    verify.add_argument("--source-sha", required=True)
    verify.add_argument("--receipt-root", required=True, type=Path)
    run = subparsers.add_parser("run", help="execute one exact matrix coordinate")
    run.add_argument("--coordinate", required=True)
    release_receipt.add_receipt_arguments(run)
    args = parser.parse_args(raw_argv)
    try:
        policy = load_manifest()
        if args.command == "check":
            projections = validate_manifest(policy)
            applicable_counts = [
                len(projection.applicable) for projection in projections
            ]
            expected_failure_paths = {
                test.path
                for projection in projections
                for test in projection.expected_failures
            }
            print(
                "verified subset policy: OK "
                f"coordinates={len(verified_subset_coordinates(policy))} "
                f"source_tests={len(verified_subset_test_files(policy))} "
                f"applicable={min(applicable_counts)}..{max(applicable_counts)} "
                f"expected_failure_debts={len(expected_failure_paths)}"
            )
            return 0
        if args.command == "matrix":
            print(
                json.dumps(
                    matrix_payload(policy), separators=(",", ":"), sort_keys=True
                )
            )
            return 0
        validate_manifest(policy)
        if args.command == "verify-receipts":
            verify_receipt_closure(
                receipt_root=args.receipt_root,
                source_sha=args.source_sha,
            )
            print(
                "verified subset receipts: OK "
                f"source_sha={args.source_sha} "
                f"coordinates={len(verified_subset_coordinates(policy))}"
            )
            return 0
        coordinate = verified_subset_coordinate_by_id(args.coordinate)
        return _run_coordinate(
            coordinate=coordinate,
            policy=policy,
            raw_argv=raw_argv,
            receipt_path=args.receipt,
            source_sha=args.source_sha,
        )
    except (OSError, ValueError) as exc:
        print(f"verified subset: ERROR: {exc}", file=sys.stderr)
        return 2


if __name__ == "__main__":
    raise SystemExit(main())
