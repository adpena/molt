from __future__ import annotations

import datetime as dt
import hashlib
import json
from pathlib import Path, PurePosixPath
import shutil
import subprocess
import sys
from collections.abc import Mapping
from typing import Any

import pytest
from tools import release_exit_gate, verified_subset
from tools.compat import comparison, test_policy


REPO_ROOT = Path(__file__).resolve().parents[2]
SOURCE_SHA = "a" * 40
NOW = dt.datetime(2026, 8, 14, 1, 0, tzinfo=dt.timezone.utc)


def _load_gate(monkeypatch: pytest.MonkeyPatch, *, stub_source: bool = True):
    module = release_exit_gate
    monkeypatch.setattr(module.pa.perf_schema, "validate_board", lambda _doc: [])
    if stub_source:
        monkeypatch.setattr(module, "_assert_clean_landed_source", lambda *_args: None)
    monkeypatch.setattr(
        module,
        "_shared_scientific_registry_coordinates",
        lambda _root: _expected_registry(),
    )
    monkeypatch.setattr(
        verified_subset,
        "verified_subset_projection",
        lambda _policy, coordinate, **_kwargs: _verified_projection(coordinate),
    )
    monkeypatch.setattr(
        module.rcr,
        "stable_file_sha256",
        lambda path, **_kwargs: hashlib.sha256(
            str(Path(path).resolve()).encode("utf-8")
        ).hexdigest(),
    )
    return module


def _expected_registry() -> dict[tuple[str, str, str], dict[str, Any]]:
    result: dict[tuple[str, str, str], dict[str, Any]] = {}
    for target, target_triple in (
        ("wasm", "wasm32-wasip1"),
        ("native", "x86_64-pc-windows-msvc"),
    ):
        coordinate = ("3.12", "cpython-abi", target_triple)
        result[coordinate] = {
            "target": target,
            "variant": {
                "cpython": coordinate[0],
                "abi_tier": coordinate[1],
                "target_triple": coordinate[2],
            },
            "packages": {
                "numpy": {
                    "version": "2.5.1",
                    "module_set": "pact-witness",
                    "identity_sha256": "1" * 64,
                },
                "scipy": {
                    "version": "1.18.0",
                    "module_set": "pact-witness",
                    "identity_sha256": "2" * 64,
                },
            },
        }
    return result


def _hashed_without_role(gate, path: Path, receipt_path: Path) -> dict[str, Any]:
    return {
        key: value
        for key, value in gate.pwr.artifact_receipt(
            "ignored",
            path,
            receipt_path=receipt_path,
        ).items()
        if key != "role"
    }


def _write_e1_receipt(
    root: Path,
    gate,
    *,
    target: str,
    source_sha: str = SOURCE_SHA,
) -> Path:
    expected = next(
        item for item in _expected_registry().values() if item["target"] == target
    )
    receipt_root = root / target
    receipt_root.mkdir(parents=True)
    receipt_path = receipt_root / "acceptance-receipt.json"
    candidate = receipt_root / "candidate_outputs.npz"
    reference = receipt_root / "reference_oracle.npz"
    candidate.write_bytes(f"{target}-candidate".encode())
    reference.write_bytes(f"{target}-reference".encode())
    artifact_paths = {
        "candidate_outputs": candidate,
        "reference_oracle": reference,
    }
    if target == "native":
        target_artifact = receipt_root / "artifacts" / "native" / "app.exe"
        target_artifact.parent.mkdir(parents=True)
        target_artifact.write_bytes(b"native-app")
    else:
        wasm_root = receipt_root / "artifacts" / "wasm"
        wasm_root.mkdir(parents=True)
        target_artifact = wasm_root / "app.wasm"
        runtime = wasm_root / "runtime.wasm"
        target_artifact.write_bytes(b"wasm-app")
        runtime.write_bytes(b"wasm-runtime")
        manifest = wasm_root / "manifest.json"
        manifest.write_text(
            json.dumps(
                {
                    "version": 2,
                    "mode": "split-runtime",
                    "modules": {
                        "app": {
                            "path": "app.wasm",
                            "sha256": gate.stable_file_sha256(
                                target_artifact,
                                label="test target artifact",
                            ),
                            "size": target_artifact.stat().st_size,
                        },
                        "runtime": {
                            "path": "runtime.wasm",
                            "sha256": gate.stable_file_sha256(
                                runtime,
                                label="test runtime artifact",
                            ),
                            "size": runtime.stat().st_size,
                        },
                    },
                    "entry": {"module": "app", "function": "molt_main"},
                }
            ),
            encoding="utf-8",
        )
        artifact_paths["execution_manifest"] = manifest
    artifact_paths["target_artifact"] = target_artifact
    parity_gate = receipt_root / "artifacts" / "parity" / "gates.json"
    parity_gate.parent.mkdir(parents=True)
    parity_gate.write_text("{}\n", encoding="utf-8")
    payload = {
        "schema_version": gate.pwr.SCHEMA_VERSION,
        "kind": gate.pwr.KIND,
        "status": gate.pwr.STATUS_PASS,
        "target": target,
        "variant": expected["variant"],
        "packages": {
            package: {**item, "seal_sha256": str(index) * 64}
            for index, (package, item) in enumerate(
                expected["packages"].items(),
                start=3,
            )
        },
        "git": {"source_sha": source_sha},
        "artifacts": [
            gate.pwr.artifact_receipt(role, path, receipt_path=receipt_path)
            for role, path in sorted(artifact_paths.items())
        ],
        "parity_gate": _hashed_without_role(gate, parity_gate, receipt_path),
        "iteration_mode": False,
    }
    receipt_path.write_text(json.dumps(payload), encoding="utf-8")
    return receipt_path


def _scoreboard_cell(gate, benchmark: str, backend: str) -> dict[str, object]:
    return {
        "benchmark": benchmark,
        "target": "native",
        "backend": backend,
        "profile": gate.pa.CANONICAL_PERF_PROFILE,
        "build_ok": True,
        "run_blocked": False,
        "molt_ok": True,
        "cpython_ok": True,
        "warm_speedup": 2.0,
        "verdict": gate.pa.perf_schema.VERDICT_GREEN,
        "classification": gate.pa.perf_schema.CLASS_GREEN,
        "repeat_passes": int(gate.pa.CANONICAL_PERF_REPEAT),
        "measured_quiescent": True,
    }


def _write_scoreboard(
    path: Path,
    gate,
    *,
    source_sha: str = SOURCE_SHA,
    generated_at: str = "2026-08-14T00:00:00+00:00",
    quiescent: bool = True,
) -> Path:
    suite = gate.pa.CANONICAL_PERF_BENCHMARKS
    backends = sorted(gate.pa.CANONICAL_PERF_BACKENDS)
    payload = {
        "schema_version": gate.pa.perf_schema.SCHEMA_VERSION,
        "kind": gate.E2_SCOREBOARD_KIND,
        "generated_at": generated_at,
        "git_rev": source_sha,
        "provenance": {
            "origin_sha": source_sha,
            "local_head_sha": source_sha,
            "merge_base_sha": source_sha,
            "dirty_tree": False,
            "authoritative": True,
            "require_quiescent": True,
            "quiescent": quiescent,
            "quiescence": {
                "quiet": quiescent,
                "quiescence_wait_timeout_s": float(
                    gate.pa.CANONICAL_PERF_QUIESCENCE_WAIT
                ),
            },
            "backend_binary_identity": {
                f"{backend}/{gate.pa.CANONICAL_PERF_PROFILE}": f"{backend}-identity"
                for backend in backends
            },
        },
        "methodology": {
            "samples_per_phase": int(gate.pa.CANONICAL_PERF_SAMPLES),
            "warmup_runs": int(gate.pa.CANONICAL_PERF_WARMUP),
        },
        "summary": {"classify_active": True, "gate_fails": False},
        "benchmarks_run": list(suite),
        "scoreboard": {
            benchmark: {
                "native": {
                    backend: {
                        gate.pa.CANONICAL_PERF_PROFILE: _scoreboard_cell(
                            gate,
                            benchmark,
                            backend,
                        )
                    }
                    for backend in backends
                }
            }
            for benchmark in suite
        },
    }
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(json.dumps(payload), encoding="utf-8")
    return path


def _verified_projection(coordinate) -> test_policy.CoordinateProjection:
    test = test_policy.ProjectedTest(
        path="tests/differential/basic/arith.py",
        source_sha256="9" * 64,
        applicable=True,
        exclusion_reason=None,
        verification_scope=test_policy.CPYTHON_EQUIVALENCE_SCOPE,
        expect_molt_fail=False,
        expected_failure_reason=None,
    )
    return test_policy.CoordinateProjection(
        python=coordinate.python,
        platform=coordinate.platform,
        arch=coordinate.arch,
        backend=coordinate.backend,
        tests=(test,),
    )


def _verified_outcome(coordinate) -> dict[str, object]:
    return {
        "backend": coordinate.backend,
        "backend_returncode": 0,
        "backend_status": "pass",
        "backend_stderr_sha256": "8" * 64,
        "backend_stdout_sha256": "8" * 64,
        "comparison_law": comparison.COMPARISON_LAW_VERSION,
        "compiler_target_python": coordinate.python,
        "cpython_returncode": 0,
        "cpython_stderr_sha256": "8" * 64,
        "cpython_stdout_sha256": "8" * 64,
        "expect_molt_fail": False,
        "expected_failure_reason": None,
        "path": "tests/differential/basic/arith.py",
        "raw_status": "pass",
        "reason_tag": None,
        "resolved_status": "pass",
    }


def _verified_execution(coordinate, source_sha: str) -> dict[str, object]:
    backend: dict[str, object] = {"backend": coordinate.backend, "runner": "process"}
    if coordinate.backend == "wasm":
        backend = {
            "backend": "wasm",
            "binary_name": "node",
            "binary_sha256": "8" * 64,
            "runner": "node-wasi",
            "version": "v24.16.0",
        }
    version_info = [
        *(int(part) for part in coordinate.reference_python.split(".")),
        "final",
        0,
    ]
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
            "source_sha": source_sha,
            "workflow_ref": "molt/verified-subset.yml@refs/heads/main",
        },
        "host": {
            "arch": coordinate.arch,
            "platform": coordinate.platform,
            "pointer_bits": 64,
        },
        "python": {
            "abi_flags": "",
            "cache_tag": "cpython",
            "command_executable": "python",
            "executable_name": "python",
            "executable_sha256": "8" * 64,
            "gil_disabled": False,
            "hexversion": 0,
            "implementation": "CPython",
            "pointer_bits": 64,
            "version": coordinate.reference_python,
            "version_info": version_info,
        },
        "rust": {
            "binary_name": "rustc",
            "binary_sha256": "8" * 64,
            "commit_date": "2026-01-01",
            "commit_hash": "8" * 64,
            "host": coordinate.rust_target,
            "llvm_version": "21.1.0",
            "release": "1.96.1",
        },
    }


def _write_typed_receipts(
    root: Path,
    gate,
    *,
    source_sha: str = SOURCE_SHA,
    failing_kind: str | None = None,
) -> tuple[list[Path], list[Path]]:
    root.mkdir(parents=True)
    generated_at = "2026-08-14T00:00:00Z"

    def write(
        kind: str, *, status: str, facts: dict[str, Any], inputs: list[Path]
    ) -> Path:
        receipt = gate.rcr.build_receipt(
            kind=kind,
            source_sha=source_sha,
            status=status,
            argv=["--check"],
            tool_path=REPO_ROOT / gate.rcr.KIND_TO_TOOL[kind],
            facts=facts,
            input_paths=inputs,
            repo_root=REPO_ROOT,
            generated_at=generated_at,
        )
        path = root / f"{kind}.json"
        path.write_text(json.dumps(receipt), encoding="utf-8")
        return path

    policy = verified_subset.load_manifest()
    verified_inputs = list(verified_subset.verified_subset_authority_files(policy))
    verified_tool = gate.rcr.input_record(
        REPO_ROOT / gate.rcr.KIND_TO_TOOL[gate.rcr.KIND_VERIFIED_SUBSET],
        repo_root=REPO_ROOT,
    )
    verified_input_records = gate.rcr.sorted_input_records(
        verified_inputs, repo_root=REPO_ROOT
    )
    verified_receipts: list[Path] = []
    for coordinate in verified_subset.verified_subset_coordinates(policy):
        projection = _verified_projection(coordinate)
        outcome = _verified_outcome(coordinate)
        path = root / f"verified_subset.{coordinate.id}.json"
        receipt = {
            "schema_version": gate.rcr.SCHEMA_VERSION,
            "kind": gate.rcr.KIND_VERIFIED_SUBSET,
            "source_sha": source_sha,
            "generated_at": generated_at,
            "status": gate.rcr.STATUS_PASS,
            "producer": {
                "argv": [
                    verified_tool["path"],
                    "run",
                    "--coordinate",
                    coordinate.id,
                    "--receipt",
                    str(path),
                    "--source-sha",
                    source_sha,
                ],
                "tool": verified_tool,
            },
            "facts": verified_subset._receipt_facts(
                coordinate=coordinate,
                policy=policy,
                projection=projection,
                results=[outcome],
                execution=_verified_execution(coordinate, source_sha),
            ),
            "inputs": verified_input_records,
        }
        path.write_text(json.dumps(receipt), encoding="utf-8")
        verified_receipts.append(path)

    canonical_baseline = REPO_ROOT / "tools" / "canonicalization_contract_baseline.json"
    canonical_metrics = json.loads(canonical_baseline.read_text(encoding="utf-8"))
    canonical = write(
        gate.rcr.KIND_CANONICALIZATION_CONTRACT,
        status=gate.rcr.STATUS_PASS,
        facts={
            "baseline_metrics": canonical_metrics,
            "baseline_path": canonical_baseline.relative_to(REPO_ROOT).as_posix(),
            "improved_metrics": [],
            "metrics": canonical_metrics,
            "open_violations": 0,
            "regressed_metrics": [],
        },
        inputs=[canonical_baseline],
    )

    structural_baseline = REPO_ROOT / "tools" / "structural_audit_baseline.json"
    structural_metrics = json.loads(structural_baseline.read_text(encoding="utf-8"))
    structural = write(
        gate.rcr.KIND_STRUCTURAL_AUDIT,
        status=gate.rcr.STATUS_PASS,
        facts={
            "baseline_metrics": structural_metrics,
            "baseline_path": structural_baseline.relative_to(REPO_ROOT).as_posix(),
            "findings_count": 0,
            "improved_metrics": [],
            "metrics": structural_metrics,
            "regressed_metrics": [],
        },
        inputs=[structural_baseline],
    )

    degrade_registry = REPO_ROOT / "tools" / "degrade_to_slow_registry.toml"
    degrade_fails = failing_kind == gate.rcr.KIND_DEGRADE_TO_SLOW_GATE
    degrade = write(
        gate.rcr.KIND_DEGRADE_TO_SLOW_GATE,
        status=gate.rcr.STATUS_FAIL if degrade_fails else gate.rcr.STATUS_PASS,
        facts={
            "discovered_site_count": 0,
            "errors": ["forced failure"] if degrade_fails else [],
            "metabug_fix_pending_baseline": 0,
            "metabug_fix_pending_count": 0,
            "registry_path": degrade_registry.relative_to(REPO_ROOT).as_posix(),
            "registry_row_count": 0,
            "warnings": [],
        },
        inputs=[degrade_registry],
    )

    fail_closed_registry = REPO_ROOT / "tools" / "fail_closed_registry.toml"
    zero_classes = {name: 0 for name in gate.rcr.FAIL_CLOSED_CLASSES}
    fail_closed = write(
        gate.rcr.KIND_FAIL_CLOSED_GATE,
        status=gate.rcr.STATUS_PASS,
        facts={
            "baseline_counts": zero_classes,
            "class_counts": zero_classes,
            "registered_site_count": 0,
            "registry_path": fail_closed_registry.relative_to(REPO_ROOT).as_posix(),
            "violations": [],
        },
        inputs=[fail_closed_registry],
    )
    return verified_receipts, [canonical, degrade, fail_closed, structural]


def _inputs(
    tmp_path: Path,
    gate,
    *,
    failing_kind: str | None = None,
    source_sha: str = SOURCE_SHA,
) -> dict[str, Any]:
    inputs = tmp_path / "inputs"
    e1_root = inputs / "e1"
    e1 = [
        _write_e1_receipt(e1_root, gate, target="native", source_sha=source_sha),
        _write_e1_receipt(e1_root, gate, target="wasm", source_sha=source_sha),
    ]
    scoreboard = _write_scoreboard(
        inputs / "e2" / "scoreboard.json", gate, source_sha=source_sha
    )
    e3_receipts, e4 = _write_typed_receipts(
        inputs / "typed",
        gate,
        source_sha=source_sha,
        failing_kind=failing_kind,
    )
    return {
        "source_sha": source_sha,
        "e1_receipts": e1,
        "e2_scoreboard": scoreboard,
        "e3_receipts": e3_receipts,
        "e4_receipts": e4,
        "repo_root": REPO_ROOT,
        "output_root": tmp_path / "dist" / "release-exit",
        "now": NOW,
    }


def _assemble(tmp_path: Path, gate, *, failing_kind: str | None = None):
    return gate.assemble_release_bundle(
        **_inputs(tmp_path, gate, failing_kind=failing_kind)
    )


def _read(path: Path) -> dict[str, Any]:
    return json.loads(path.read_text(encoding="utf-8"))


def _write(path: Path, payload: Mapping[str, Any]) -> None:
    path.write_text(json.dumps(payload, sort_keys=True), encoding="utf-8")


def test_assemble_writes_one_portable_source_addressed_bundle(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    gate = _load_gate(monkeypatch)

    manifest_path, report = _assemble(tmp_path, gate)

    assert manifest_path == (
        tmp_path / "dist" / "release-exit" / SOURCE_SHA / "release-exit.json"
    )
    assert report.passed is True
    payload = _read(manifest_path)
    assert set(payload) == {
        "schema_version",
        "kind",
        "source_sha",
        "status",
        "registry",
        "evidence",
    }
    assert payload["status"] == gate.STATUS_PASS
    assert [item["role"] for item in payload["evidence"]] == sorted(
        gate._expected_evidence_roles()
    )
    for item in payload["evidence"]:
        relative = PurePosixPath(item["path"])
        assert not relative.is_absolute()
        assert ".." not in relative.parts
        path = manifest_path.parent.joinpath(*relative.parts)
        assert item["size"] == path.stat().st_size
        assert item["sha256"] == gate.stable_file_sha256(
            path,
            label="test release exit artifact",
        )


def test_release_exit_accepts_sha256_repository_object_id(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    gate = _load_gate(monkeypatch)
    source_sha = "a" * 64

    manifest_path, report = gate.assemble_release_bundle(
        **_inputs(tmp_path, gate, source_sha=source_sha)
    )

    assert report.passed is True
    assert report.source_sha == source_sha
    assert manifest_path.parent.name == source_sha

    copied_runtime = (
        manifest_path.parent
        / "evidence"
        / "e1"
        / "wasm"
        / "artifacts"
        / "wasm"
        / "runtime.wasm"
    )
    assert copied_runtime.read_bytes() == b"wasm-runtime"

    relocated = tmp_path / "relocated"
    shutil.copytree(manifest_path.parent, relocated)
    relocated_report = gate.verify_release_bundle(
        relocated / "release-exit.json",
        repo_root=REPO_ROOT,
        now=NOW,
    )
    assert relocated_report.passed is True


def test_verify_rejects_mutated_transitive_wasm_artifact(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    gate = _load_gate(monkeypatch)
    manifest_path, _ = _assemble(tmp_path, gate)
    runtime = (
        manifest_path.parent
        / "evidence"
        / "e1"
        / "wasm"
        / "artifacts"
        / "wasm"
        / "runtime.wasm"
    )
    runtime.write_bytes(b"mutated-runtime")

    report = gate.verify_release_bundle(
        manifest_path,
        repo_root=REPO_ROOT,
        now=NOW,
    )

    assert report.passed is False
    assert any("modules.runtime artifact" in problem for problem in report.problems)


def test_manifest_status_is_derived_from_typed_receipts(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    gate = _load_gate(monkeypatch)
    manifest_path, report = _assemble(
        tmp_path,
        gate,
        failing_kind=gate.rcr.KIND_DEGRADE_TO_SLOW_GATE,
    )

    assert report.problems == ()
    assert report.passed is False
    assert report.status == gate.STATUS_FAIL
    assert _read(manifest_path)["status"] == gate.STATUS_FAIL


def test_assembly_rejects_mixed_source_revisions(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    gate = _load_gate(monkeypatch)
    inputs = _inputs(tmp_path, gate)
    e3 = inputs["e3_receipts"][0]
    payload = _read(e3)
    payload["source_sha"] = "b" * 40
    _write(e3, payload)

    with pytest.raises(ValueError, match="differs from the expected release source"):
        gate.assemble_release_bundle(**inputs)


def test_assembly_rejects_tampered_verified_compiler_target(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    gate = _load_gate(monkeypatch)
    inputs = _inputs(tmp_path, gate)
    e3 = inputs["e3_receipts"][0]
    payload = _read(e3)
    payload["facts"]["outcomes"][0]["compiler_target_python"] = "3.99"
    _write(e3, payload)

    with pytest.raises(ValueError, match="compiler_target_python differs"):
        gate.assemble_release_bundle(**inputs)


def test_assembly_rejects_e1_registry_identity_drift(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    gate = _load_gate(monkeypatch)
    inputs = _inputs(tmp_path, gate)
    native_receipt = inputs["e1_receipts"][0]
    payload = _read(native_receipt)
    payload["packages"]["numpy"]["identity_sha256"] = "f" * 64
    _write(native_receipt, payload)

    with pytest.raises(ValueError, match="identity_sha256 differs"):
        gate.assemble_release_bundle(**inputs)


def test_verify_rejects_registry_snapshot_drift_from_checked_out_source(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    gate = _load_gate(monkeypatch)
    manifest_path, assembled = _assemble(tmp_path, gate)
    assert assembled.passed is True
    changed_registry = _expected_registry()
    native = next(
        item for item in changed_registry.values() if item["target"] == "native"
    )
    native["packages"]["numpy"]["identity_sha256"] = "f" * 64
    monkeypatch.setattr(
        gate,
        "_shared_scientific_registry_coordinates",
        lambda _root: changed_registry,
    )

    report = gate.verify_release_bundle(
        manifest_path,
        repo_root=REPO_ROOT,
        now=NOW,
    )

    assert report.passed is False
    assert (
        "release-exit registry differs from the checked-out canonical scientific "
        "registry" in report.problems
    )


@pytest.mark.parametrize("count_key", ("e1_receipts", "e4_receipts"))
def test_assembly_requires_exact_input_cardinality(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
    count_key: str,
) -> None:
    gate = _load_gate(monkeypatch)
    inputs = _inputs(tmp_path, gate)
    inputs[count_key] = inputs[count_key][:-1]

    with pytest.raises(ValueError, match="requires exactly"):
        gate.assemble_release_bundle(**inputs)


def test_verify_rejects_unknown_fields_duplicate_roles_and_escapes(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    gate = _load_gate(monkeypatch)
    manifest_path, _ = _assemble(tmp_path, gate)
    payload = _read(manifest_path)
    payload["unknown"] = True
    duplicate = dict(payload["evidence"][0])
    payload["evidence"].append(duplicate)
    payload["evidence"][1]["path"] = "../escape.json"
    _write(manifest_path, payload)

    report = gate.verify_release_bundle(
        manifest_path,
        repo_root=REPO_ROOT,
        now=NOW,
    )

    assert any("unknown=['unknown']" in problem for problem in report.problems)
    assert "manifest evidence roles must not duplicate" in report.problems
    assert any("portable relative POSIX" in problem for problem in report.problems)


def test_verify_rejects_unbound_bundle_files_and_nonderived_status(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    gate = _load_gate(monkeypatch)
    manifest_path, _ = _assemble(tmp_path, gate)
    payload = _read(manifest_path)
    payload["status"] = gate.STATUS_FAIL
    _write(manifest_path, payload)
    (manifest_path.parent / "unbound.txt").write_text("unbound", encoding="utf-8")

    report = gate.verify_release_bundle(
        manifest_path,
        repo_root=REPO_ROOT,
        now=NOW,
    )

    assert any("status is not derived" in problem for problem in report.problems)
    assert any("contains unbound files" in problem for problem in report.problems)


def test_e1_closure_rejects_casefold_collision_across_transitive_modules(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    gate = _load_gate(monkeypatch)
    receipt_path = _write_e1_receipt(
        tmp_path / "e1",
        gate,
        target="wasm",
    )
    payload = _read(receipt_path)
    runtime = receipt_path.parent / "artifacts" / "wasm" / "runtime.wasm"
    upper_runtime = runtime.with_name("RUNTIME.wasm")
    if not upper_runtime.exists():
        shutil.copyfile(runtime, upper_runtime)
    candidate = next(
        item for item in payload["artifacts"] if item["role"] == "candidate_outputs"
    )
    candidate.update(
        {
            "path": "artifacts/wasm/RUNTIME.wasm",
            "sha256": gate.stable_file_sha256(
                upper_runtime,
                label="test upper runtime artifact",
            ),
            "size": upper_runtime.stat().st_size,
        }
    )

    with pytest.raises(ValueError, match="portable filesystem identity"):
        gate._e1_closure(receipt_path, payload)


def test_verify_rejects_symlink_directory_escape(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    gate = _load_gate(monkeypatch)
    manifest_path, _ = _assemble(tmp_path, gate)
    outside = tmp_path / "outside"
    outside.mkdir()
    (outside / "external.json").write_text("{}\n", encoding="utf-8")
    link = manifest_path.parent / "external-evidence"
    try:
        link.symlink_to(outside, target_is_directory=True)
    except OSError as exc:
        pytest.skip(f"directory symlinks are unavailable: {exc}")

    report = gate.verify_release_bundle(
        manifest_path,
        repo_root=REPO_ROOT,
        now=NOW,
    )

    assert any(
        "symbolic links, junctions, or reparse points" in problem
        and "external-evidence" in problem
        for problem in report.problems
    )


@pytest.mark.skipif(sys.platform != "win32", reason="Windows junction contract")
def test_verify_rejects_windows_junction_reparse_point(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    gate = _load_gate(monkeypatch)
    manifest_path, _ = _assemble(tmp_path, gate)
    outside = tmp_path / "junction-target"
    outside.mkdir()
    (outside / "external.json").write_text("{}\n", encoding="utf-8")
    junction = manifest_path.parent / "junction-evidence"
    created = subprocess.run(
        ["cmd.exe", "/d", "/c", "mklink", "/J", str(junction), str(outside)],
        check=False,
        capture_output=True,
        text=True,
    )
    if created.returncode != 0:
        pytest.skip(f"junction creation is unavailable: {created.stderr.strip()}")
    try:
        report = gate.verify_release_bundle(
            manifest_path,
            repo_root=REPO_ROOT,
            now=NOW,
        )
    finally:
        junction.rmdir()

    assert any(
        "symbolic links, junctions, or reparse points" in problem
        and "junction-evidence" in problem
        for problem in report.problems
    )


def test_inventory_reports_bound_file_escape(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    gate = _load_gate(monkeypatch)
    bundle = tmp_path / "bundle"
    bundle.mkdir()
    outside = tmp_path / "outside.json"
    outside.write_text("{}\n", encoding="utf-8")

    problems = gate._bundle_inventory_problems(
        bundle,
        expected_files={outside},
    )

    assert any("bound file escapes the bundle" in problem for problem in problems)


def test_verify_rejects_duplicate_json_keys(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    gate = _load_gate(monkeypatch)
    manifest = tmp_path / "release-exit.json"
    manifest.write_text('{"schema_version":2,"schema_version":2}', encoding="utf-8")

    report = gate.verify_release_bundle(manifest, repo_root=REPO_ROOT, now=NOW)

    assert report.passed is False
    assert any("duplicate JSON key" in problem for problem in report.problems)


def test_assembly_rejects_nonquiescent_or_future_scoreboard(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    gate = _load_gate(monkeypatch)
    inputs = _inputs(tmp_path, gate)
    scoreboard = inputs["e2_scoreboard"]
    payload = _read(scoreboard)
    payload["generated_at"] = "2026-08-15T00:00:00+00:00"
    payload["provenance"]["quiescent"] = False
    _write(scoreboard, payload)

    with pytest.raises(ValueError) as exc_info:
        gate.assemble_release_bundle(**inputs)

    message = str(exc_info.value)
    assert "provenance.quiescent must be true" in message
    assert "unreasonably far in the future" in message


def test_source_preflight_rejects_dirty_and_unlanded_revisions(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    gate = _load_gate(monkeypatch, stub_source=False)

    def dirty_git(_root: Path, *args: str) -> subprocess.CompletedProcess[str]:
        if args[0] == "rev-parse":
            return subprocess.CompletedProcess(args, 0, SOURCE_SHA, "")
        if args[0] == "status":
            return subprocess.CompletedProcess(args, 0, " M source.py\n", "")
        raise AssertionError(args)

    monkeypatch.setattr(gate, "_run_git", dirty_git)
    with pytest.raises(ValueError, match="clean source checkout"):
        gate._assert_clean_landed_source(tmp_path, SOURCE_SHA)

    def unlanded_git(_root: Path, *args: str) -> subprocess.CompletedProcess[str]:
        if args[0] == "rev-parse":
            return subprocess.CompletedProcess(args, 0, SOURCE_SHA, "")
        if args[0] == "status":
            return subprocess.CompletedProcess(args, 0, "", "")
        if args[0] == "merge-base":
            return subprocess.CompletedProcess(args, 1, "", "")
        raise AssertionError(args)

    monkeypatch.setattr(gate, "_run_git", unlanded_git)
    with pytest.raises(ValueError, match="not landed on origin/main"):
        gate._assert_clean_landed_source(tmp_path, SOURCE_SHA)


def test_cli_exposes_only_assemble_and_verify(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    gate = _load_gate(monkeypatch)

    with pytest.raises(SystemExit) as exc_info:
        gate.main(["--allow-missing-evidence"])

    assert exc_info.value.code == 2
