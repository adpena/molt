from __future__ import annotations

import copy
import datetime as dt
import json
import subprocess
from pathlib import Path
from typing import Any

import pytest

from molt import verified_subset as verified_authority
from tools import release_criterion_receipt as receipt
from tools import verified_subset
from tools.compat import comparison, test_policy


SOURCE_SHA = "a" * 40
GENERATED_AT = "2026-08-14T12:00:00Z"
VALIDATION_NOW = dt.datetime(2026, 8, 14, 12, 1, tzinfo=dt.timezone.utc)


def _write(path: Path, data: str = "input\n") -> Path:
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(data, encoding="utf-8")
    return path


def _metrics(names: frozenset[str]) -> dict[str, float]:
    return {name: 0.0 for name in sorted(names)}


def _case(root: Path, kind: str) -> tuple[dict[str, object], list[Path], Path]:
    tool = _write(root / receipt.KIND_TO_TOOL[kind], "# producer\n")
    if kind == receipt.KIND_CANONICALIZATION_CONTRACT:
        metrics = _metrics(receipt.CANONICALIZATION_METRICS)
        baseline = _write(
            root / "tools" / "canonicalization_contract_baseline.json",
            json.dumps(metrics),
        )
        return (
            {
                "baseline_metrics": metrics,
                "baseline_path": "tools/canonicalization_contract_baseline.json",
                "improved_metrics": [],
                "metrics": metrics,
                "open_violations": 0,
                "regressed_metrics": [],
            },
            [baseline],
            tool,
        )
    if kind == receipt.KIND_STRUCTURAL_AUDIT:
        metrics = _metrics(receipt.STRUCTURAL_AUDIT_METRICS)
        baseline = _write(
            root / "tools" / "structural_audit_baseline.json",
            json.dumps(metrics),
        )
        return (
            {
                "baseline_metrics": metrics,
                "baseline_path": "tools/structural_audit_baseline.json",
                "findings_count": 0,
                "improved_metrics": [],
                "metrics": metrics,
                "regressed_metrics": [],
            },
            [baseline],
            tool,
        )
    registry_name = (
        "degrade_to_slow_registry.toml"
        if kind == receipt.KIND_DEGRADE_TO_SLOW_GATE
        else "fail_closed_registry.toml"
    )
    registry = _write(root / "tools" / registry_name, "[baseline]\n")
    if kind == receipt.KIND_DEGRADE_TO_SLOW_GATE:
        return (
            {
                "discovered_site_count": 2,
                "errors": [],
                "metabug_fix_pending_baseline": 0,
                "metabug_fix_pending_count": 0,
                "registry_path": f"tools/{registry_name}",
                "registry_row_count": 2,
                "warnings": [],
            },
            [registry],
            tool,
        )
    counts = {name: 0 for name in sorted(receipt.FAIL_CLOSED_CLASSES)}
    return (
        {
            "baseline_counts": counts,
            "class_counts": counts,
            "registered_site_count": 0,
            "registry_path": f"tools/{registry_name}",
            "violations": [],
        },
        [registry],
        tool,
    )


def _valid_receipt(root: Path, kind: str) -> receipt.Receipt:
    facts, inputs, tool = _case(root, kind)
    return receipt.build_receipt(
        kind=kind,
        source_sha=SOURCE_SHA,
        status=receipt.STATUS_PASS,
        argv=["--check", "--source-sha", SOURCE_SHA],
        tool_path=tool,
        facts=facts,
        input_paths=inputs,
        repo_root=root,
        generated_at=GENERATED_AT,
    )


def _verified_projection(
    coordinate: verified_authority.VerifiedSubsetCoordinate,
    *,
    expected_failure: bool = False,
) -> test_policy.CoordinateProjection:
    test = test_policy.ProjectedTest(
        path="tests/differential/basic/arith.py",
        source_sha256="2" * 64,
        applicable=True,
        exclusion_reason=None,
        verification_scope=test_policy.CPYTHON_EQUIVALENCE_SCOPE,
        expect_molt_fail=expected_failure,
        expected_failure_reason="compiler_gap" if expected_failure else None,
    )
    return test_policy.CoordinateProjection(
        python=coordinate.python,
        platform=coordinate.platform,
        arch=coordinate.arch,
        backend=coordinate.backend,
        tests=(test,),
    )


def _verified_outcome(
    coordinate: verified_authority.VerifiedSubsetCoordinate,
    *,
    expected_failure: bool = False,
    raw_status: str = "pass",
) -> dict[str, object]:
    resolved_status, reason_tag = test_policy.resolve_expected_failure_status(
        expect_molt_fail=expected_failure,
        raw_status=raw_status,
        cpython_returncode=0,
    )
    return {
        "backend": coordinate.backend,
        "backend_returncode": 0 if raw_status == "pass" else 1,
        "backend_status": raw_status,
        "backend_stderr_sha256": "2" * 64,
        "backend_stdout_sha256": "2" * 64,
        "comparison_law": comparison.COMPARISON_LAW_VERSION,
        "compiler_target_python": coordinate.python,
        "cpython_returncode": 0,
        "cpython_stderr_sha256": "2" * 64,
        "cpython_stdout_sha256": "2" * 64,
        "expect_molt_fail": expected_failure,
        "expected_failure_reason": "compiler_gap" if expected_failure else None,
        "path": "tests/differential/basic/arith.py",
        "raw_status": raw_status,
        "reason_tag": reason_tag,
        "resolved_status": resolved_status,
    }


def _verified_execution(
    coordinate: verified_authority.VerifiedSubsetCoordinate,
) -> dict[str, object]:
    backend: dict[str, object] = {
        "backend": coordinate.backend,
        "runner": "process",
    }
    if coordinate.backend == "wasm":
        backend = {
            "backend": "wasm",
            "binary_name": "node",
            "binary_sha256": "2" * 64,
            "runner": "node-wasi",
            "version": "v24.16.0",
        }
    return {
        "backend": backend,
        "ci": {
            "job": "coordinate",
            "provider": "github-actions",
            "run_attempt": "1",
            "run_id": "42",
            "runner_arch": (
                "ARM64" if coordinate.arch in {"aarch64", "arm64"} else "X64"
            ),
            "runner_label": coordinate.runner,
            "runner_os": {
                "linux": "Linux",
                "macos": "macOS",
                "windows": "Windows",
            }[coordinate.platform],
            "source_sha": SOURCE_SHA,
            "workflow_ref": "molt/verified-subset.yml@refs/heads/main",
        },
        "host": {
            "arch": coordinate.arch,
            "platform": coordinate.platform,
            "pointer_bits": 64,
        },
        "python": {
            "abi_flags": "",
            "cache_tag": f"cpython-{coordinate.python.replace('.', '')}",
            "command_executable": "python",
            "executable_name": "python",
            "executable_sha256": "2" * 64,
            "gil_disabled": False,
            "hexversion": 0,
            "implementation": "CPython",
            "pointer_bits": 64,
            "version": coordinate.reference_python,
            "version_info": [
                *(int(part) for part in coordinate.reference_python.split(".")),
                "final",
                0,
            ],
        },
        "rust": {
            "binary_name": "rustc",
            "binary_sha256": "2" * 64,
            "commit_date": "2026-01-01",
            "commit_hash": "2" * 64,
            "host": coordinate.rust_target,
            "llvm_version": "21.1.0",
            "release": "1.96.1",
        },
    }


def _verified_receipt(
    monkeypatch: pytest.MonkeyPatch,
    *,
    expected_failure: bool = False,
    raw_status: str = "pass",
) -> tuple[receipt.Receipt, verified_authority.VerifiedSubsetCoordinate]:
    policy = verified_authority.load_verified_subset_policy()
    coordinate = verified_authority.verified_subset_coordinates(policy)[0]
    projection = _verified_projection(
        coordinate,
        expected_failure=expected_failure,
    )
    tool = verified_subset.ROOT / receipt.KIND_TO_TOOL[receipt.KIND_VERIFIED_SUBSET]
    monkeypatch.setattr(
        verified_subset,
        "verified_subset_projection",
        lambda _policy, _coordinate, **_kwargs: projection,
    )
    monkeypatch.setattr(
        verified_subset,
        "verified_subset_authority_files",
        lambda _policy: (tool,),
    )
    outcome = _verified_outcome(
        coordinate,
        expected_failure=expected_failure,
        raw_status=raw_status,
    )
    payload = receipt.build_receipt(
        kind=receipt.KIND_VERIFIED_SUBSET,
        source_sha=SOURCE_SHA,
        status=(
            receipt.STATUS_PASS
            if verified_subset.outcomes_pass([outcome])
            else receipt.STATUS_FAIL
        ),
        argv=[
            "run",
            "--coordinate",
            coordinate.id,
            "--receipt",
            "receipt.json",
            "--source-sha",
            SOURCE_SHA,
        ],
        tool_path=tool,
        facts=verified_subset._receipt_facts(
            coordinate=coordinate,
            policy=policy,
            projection=projection,
            results=[outcome],
            execution=_verified_execution(coordinate),
        ),
        input_paths=[tool],
        repo_root=verified_subset.ROOT,
        generated_at=GENERATED_AT,
    )
    return payload, coordinate


def _validate_verified(payload: object) -> tuple[str, ...]:
    return receipt.validate_receipt(
        payload,
        expected_kind=receipt.KIND_VERIFIED_SUBSET,
        expected_source_sha=SOURCE_SHA,
        repo_root=verified_subset.ROOT,
        now=VALIDATION_NOW,
    )


@pytest.mark.parametrize("kind", sorted(receipt.KINDS - {receipt.KIND_VERIFIED_SUBSET}))
def test_all_release_criterion_receipts_validate_exact_schema(
    tmp_path: Path, kind: str
) -> None:
    payload = _valid_receipt(tmp_path, kind)

    assert (
        receipt.validate_receipt(
            payload,
            expected_kind=kind,
            expected_source_sha=SOURCE_SHA,
            repo_root=tmp_path,
            now=VALIDATION_NOW,
        )
        == ()
    )
    assert set(payload) == {
        "schema_version",
        "kind",
        "source_sha",
        "generated_at",
        "status",
        "producer",
        "facts",
        "inputs",
    }


def test_receipt_rejects_unknown_fields_source_drift_and_mutated_inputs(
    tmp_path: Path,
) -> None:
    payload = _valid_receipt(tmp_path, receipt.KIND_DEGRADE_TO_SLOW_GATE)
    payload["unknown"] = True  # type: ignore[typeddict-unknown-key]
    registry = tmp_path / payload["facts"]["registry_path"]  # type: ignore[index]
    registry.write_text("mutated\n", encoding="utf-8")

    problems = receipt.validate_receipt(
        payload,
        expected_kind=receipt.KIND_DEGRADE_TO_SLOW_GATE,
        expected_source_sha="b" * 40,
        repo_root=tmp_path,
        now=VALIDATION_NOW,
    )

    assert any("unknown=['unknown']" in problem for problem in problems)
    assert any("source_sha differs" in problem for problem in problems)
    assert any("checksum mismatch" in problem for problem in problems)


def test_receipt_rejects_nonportable_paths_and_future_timestamp(
    tmp_path: Path,
) -> None:
    payload = _valid_receipt(tmp_path, receipt.KIND_DEGRADE_TO_SLOW_GATE)
    payload["inputs"][0]["path"] = "tools\\degrade_to_slow_registry.toml"
    payload["generated_at"] = "2099-01-01T00:00:00Z"

    problems = receipt.validate_receipt(
        payload,
        expected_kind=receipt.KIND_DEGRADE_TO_SLOW_GATE,
        expected_source_sha=SOURCE_SHA,
        repo_root=tmp_path,
        now=VALIDATION_NOW,
    )

    assert any("relative POSIX path" in problem for problem in problems)
    assert "receipt generated_at is in the future" in problems


def test_criterion_inputs_reject_portable_casefold_collisions(
    monkeypatch: pytest.MonkeyPatch,
    tmp_path: Path,
) -> None:
    def record(path: Path, *, repo_root: Path):
        del repo_root
        return {
            "path": "Evidence.json" if path.name == "first" else "evidence.json",
            "sha256": "0" * 64,
            "size": 1,
        }

    monkeypatch.setattr(receipt, "input_record", record)
    with pytest.raises(ValueError, match="portable filesystem identity"):
        receipt.sorted_input_records(
            [tmp_path / "first", tmp_path / "second"], repo_root=tmp_path
        )


@pytest.mark.parametrize(
    ("field", "malformed"),
    (
        ("kind", []),
        ("status", {}),
        ("producer", {"argv": [], "tool": []}),
        ("inputs", [{"path": [], "sha256": 3, "size": False}]),
        ("facts", []),
    ),
)
def test_receipt_rejects_malformed_types_without_raising(
    tmp_path: Path, field: str, malformed: object
) -> None:
    payload = copy.deepcopy(_valid_receipt(tmp_path, receipt.KIND_DEGRADE_TO_SLOW_GATE))
    payload[field] = malformed  # type: ignore[literal-required]

    problems = receipt.validate_receipt(
        payload,
        expected_kind=receipt.KIND_DEGRADE_TO_SLOW_GATE,
        expected_source_sha=SOURCE_SHA,
        repo_root=tmp_path,
        now=VALIDATION_NOW,
    )

    assert problems


def test_metric_receipt_rejects_non_numeric_metrics_without_raising(
    tmp_path: Path,
) -> None:
    payload = _valid_receipt(tmp_path, receipt.KIND_CANONICALIZATION_CONTRACT)
    metric = next(iter(receipt.CANONICALIZATION_METRICS))
    payload["facts"]["metrics"][metric] = "zero"  # type: ignore[index]

    problems = receipt.validate_receipt(
        payload,
        expected_kind=receipt.KIND_CANONICALIZATION_CONTRACT,
        expected_source_sha=SOURCE_SHA,
        repo_root=tmp_path,
        now=VALIDATION_NOW,
    )

    assert any("finite non-negative numbers" in problem for problem in problems)


def test_verified_subset_pass_requires_full_outcomes_not_green_counts(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    payload, _ = _verified_receipt(monkeypatch)
    payload["facts"]["outcomes"] = []

    problems = _validate_verified(payload)

    assert "facts.outcomes must not be empty" in problems


@pytest.mark.parametrize(
    ("raw_status", "reason_tag"),
    (("fail", "xfail"), ("pass", "xpass")),
)
def test_verified_subset_expected_failure_outcomes_cannot_certify_pass(
    monkeypatch: pytest.MonkeyPatch,
    raw_status: str,
    reason_tag: str,
) -> None:
    payload, _ = _verified_receipt(
        monkeypatch,
        expected_failure=True,
        raw_status=raw_status,
    )
    assert payload["status"] == receipt.STATUS_FAIL
    assert payload["facts"]["outcomes"][0]["reason_tag"] == reason_tag
    payload["status"] = receipt.STATUS_PASS

    problems = _validate_verified(payload)

    assert "receipt status must be FAIL for verified-subset facts" in problems


@pytest.mark.parametrize(
    ("backend_row_count", "problem"),
    ((0, "backend_missing="), (2, "duplicated: backend")),
)
def test_verified_subset_raw_results_require_one_backend_row_per_test(
    tmp_path: Path,
    backend_row_count: int,
    problem: str,
) -> None:
    coordinate = verified_authority.verified_subset_coordinates()[0]
    projection = _verified_projection(coordinate)
    outcome = _verified_outcome(coordinate)
    summary = {
        "item_results": [
            {"path": outcome["path"], "status": outcome["resolved_status"]}
        ]
    }
    test_row = {
        "file": outcome["path"],
        "record_type": "test",
        "raw_status": outcome["raw_status"],
        "resolved_status": outcome["resolved_status"],
        "reason_tag": outcome["reason_tag"],
        "expect_molt_fail": outcome["expect_molt_fail"],
        "expected_failure_reason": outcome["expected_failure_reason"],
        "cpython_returncode": outcome["cpython_returncode"],
        "cpython_stdout_sha256": outcome["cpython_stdout_sha256"],
        "cpython_stderr_sha256": outcome["cpython_stderr_sha256"],
        "comparison_law": outcome["comparison_law"],
        "compiler_target_python": outcome["compiler_target_python"],
    }
    backend_row = {
        "file": outcome["path"],
        "record_type": "backend",
        "backend": outcome["backend"],
        "raw_status": outcome["backend_status"],
        "returncode": outcome["backend_returncode"],
        "stdout_sha256": outcome["backend_stdout_sha256"],
        "stderr_sha256": outcome["backend_stderr_sha256"],
    }
    results_path = tmp_path / "results.jsonl"
    rows = [test_row, *([backend_row] * backend_row_count)]
    results_path.write_text(
        "".join(f"{json.dumps(row)}\n" for row in rows),
        encoding="utf-8",
    )

    with pytest.raises(ValueError, match=problem):
        verified_subset._result_outcomes(
            summary=summary,
            results_path=results_path,
            projection=projection,
            coordinate=coordinate,
        )


@pytest.mark.parametrize(
    ("field_path", "mutated", "problem"),
    (
        (("source_sha",), "b" * 40, "source_sha differs"),
        (
            ("facts", "execution", "python", "version"),
            "3.99.0",
            "facts.execution.python differs from coordinate",
        ),
        (
            ("facts", "execution", "ci", "runner_label"),
            "tampered-runner",
            "facts.execution.ci runner differs from coordinate",
        ),
        (
            ("facts", "execution", "rust", "host"),
            "tampered-rust-target",
            "facts.execution.rust differs from coordinate",
        ),
    ),
)
def test_verified_subset_receipt_rejects_identity_tampering(
    monkeypatch: pytest.MonkeyPatch,
    field_path: tuple[str, ...],
    mutated: str,
    problem: str,
) -> None:
    payload, _ = _verified_receipt(monkeypatch)
    target: Any = payload
    for field in field_path[:-1]:
        target = target[field]
    target[field_path[-1]] = mutated

    problems = _validate_verified(payload)

    assert any(problem in violation for violation in problems)


@pytest.mark.parametrize("mutation", ("missing", "tampered"))
def test_verified_subset_receipt_binds_compiler_target_python(
    monkeypatch: pytest.MonkeyPatch,
    mutation: str,
) -> None:
    payload, _ = _verified_receipt(monkeypatch)
    outcome = payload["facts"]["outcomes"][0]
    if mutation == "missing":
        del outcome["compiler_target_python"]
        expected_problem = "facts.outcomes[0] schema is invalid"
    else:
        outcome["compiler_target_python"] = "3.99"
        expected_problem = "facts.outcomes[0].compiler_target_python differs"

    problems = _validate_verified(payload)

    assert expected_problem in problems


def _git(root: Path, *args: str) -> str:
    completed = subprocess.run(
        ["git", "-C", str(root), *args],
        check=True,
        capture_output=True,
        text=True,
        encoding="utf-8",
    )
    return completed.stdout.strip()


def test_receipt_destination_requires_absent_output_and_clean_exact_head(
    tmp_path: Path,
) -> None:
    repo = tmp_path / "source"
    repo.mkdir()
    _git(repo, "init")
    _git(repo, "config", "user.email", "receipt-test@example.invalid")
    _git(repo, "config", "user.name", "Receipt Test")
    tracked = _write(repo / "tracked.txt")
    _git(repo, "add", "tracked.txt")
    _git(repo, "commit", "-m", "source")
    head = _git(repo, "rev-parse", "HEAD")
    output = tmp_path / "receipt.json"

    destination = receipt.assert_clean_source(
        repo_root=repo,
        source_sha=head,
        output_path=output,
    )
    assert destination.source_sha == head

    tracked.write_text("dirty\n", encoding="utf-8")
    with pytest.raises(ValueError, match="clean source checkout"):
        receipt.assert_clean_source(
            repo_root=repo,
            source_sha=head,
            output_path=output,
        )
    _git(repo, "checkout", "--", "tracked.txt")

    with pytest.raises(ValueError, match="source mismatch"):
        receipt.assert_clean_source(
            repo_root=repo,
            source_sha="b" * len(head),
            output_path=output,
        )
    output.write_text("occupied\n", encoding="utf-8")
    with pytest.raises(ValueError, match="already exists"):
        receipt.assert_clean_source(
            repo_root=repo,
            source_sha=head,
            output_path=output,
        )


def _capture_producer(monkeypatch, module, tmp_path: Path) -> dict[str, object]:
    captured: dict[str, object] = {}
    destination = receipt.ReceiptDestination(
        repo_root=Path(module.__file__).resolve().parents[1],
        output_path=tmp_path / "criterion-receipt.json",
        source_sha=SOURCE_SHA,
    )
    monkeypatch.setattr(
        module.release_receipt,
        "prepare_receipt_destination",
        lambda **_kwargs: destination,
    )

    def _build(**kwargs):
        captured.update(kwargs)
        return {}

    monkeypatch.setattr(module.release_receipt, "build_receipt", _build)
    monkeypatch.setattr(module.release_receipt, "write_receipt", lambda *_args: None)
    return captured


@pytest.mark.parametrize(
    ("module_name", "kind", "baseline_name"),
    (
        (
            "canonicalization_contract",
            receipt.KIND_CANONICALIZATION_CONTRACT,
            "canonicalization_contract_baseline.json",
        ),
        (
            "structural_audit",
            receipt.KIND_STRUCTURAL_AUDIT,
            "structural_audit_baseline.json",
        ),
    ),
)
def test_metric_gate_emits_baseline_bound_facts(
    tmp_path: Path,
    monkeypatch,
    module_name: str,
    kind: str,
    baseline_name: str,
) -> None:
    module = __import__(f"tools.{module_name}", fromlist=[module_name])
    captured = _capture_producer(monkeypatch, module, tmp_path)
    root = Path(module.__file__).resolve().parents[1]
    baseline = json.loads((root / "tools" / baseline_name).read_text(encoding="utf-8"))
    monkeypatch.setattr(module, "run_all", lambda *_args, **_kwargs: [])
    monkeypatch.setattr(module, "ratchet_metrics", lambda _findings: baseline)

    result = module.main(
        [
            "--root",
            str(root),
            "--receipt",
            str(tmp_path / "criterion-receipt.json"),
            "--source-sha",
            SOURCE_SHA,
        ]
    )

    assert result == 0
    assert captured["kind"] == kind
    assert captured["facts"]["baseline_metrics"] == baseline  # type: ignore[index]
    assert captured["input_paths"] == [root / "tools" / baseline_name]


def test_degrade_to_slow_gate_emits_registry_bound_facts(
    tmp_path: Path, monkeypatch
) -> None:
    from tools import degrade_to_slow_gate

    captured = _capture_producer(monkeypatch, degrade_to_slow_gate, tmp_path)
    report = degrade_to_slow_gate.GateReport(
        discovered_site_count=3,
        registry_row_count=3,
    )
    monkeypatch.setattr(degrade_to_slow_gate, "run_gate", lambda: report)

    result = degrade_to_slow_gate.main(
        [
            "--receipt",
            str(tmp_path / "criterion-receipt.json"),
            "--source-sha",
            SOURCE_SHA,
        ]
    )

    assert result == 0
    assert captured["kind"] == receipt.KIND_DEGRADE_TO_SLOW_GATE
    assert captured["facts"]["discovered_site_count"] == 3  # type: ignore[index]
    assert captured["input_paths"] == [degrade_to_slow_gate.REGISTRY_PATH]


def test_fail_closed_gate_emits_registry_counts_without_text_sniffing(
    tmp_path: Path, monkeypatch
) -> None:
    from tools import fail_closed_gate

    captured = _capture_producer(monkeypatch, fail_closed_gate, tmp_path)
    monkeypatch.setattr(fail_closed_gate, "run_gate", lambda *_args: [])

    result = fail_closed_gate.main(
        [
            "--receipt",
            str(tmp_path / "criterion-receipt.json"),
            "--source-sha",
            SOURCE_SHA,
        ]
    )

    assert result == 0
    assert captured["kind"] == receipt.KIND_FAIL_CLOSED_GATE
    assert captured["facts"]["registered_site_count"] == sum(  # type: ignore[index]
        captured["facts"]["class_counts"].values()  # type: ignore[index,union-attr]
    )
    assert captured["input_paths"] == [fail_closed_gate.REGISTRY_PATH]
