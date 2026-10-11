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
from molt.verified_subset import load_verified_subset_policy
from tests.process_guard_common import run_guarded_test_process
from tests.tools.verified_subset_fixtures import synthetic_validation
from tests.tools.receipt_engine_fixtures import install_observed_runtime
from tests.tools.perf_scoreboard_fixtures import write_synthetic_build_observation
from tools import perf_scoreboard, release_exit_gate, verified_subset
from tools.compat import comparison, test_policy


REPO_ROOT = Path(__file__).resolve().parents[2]
SOURCE_SHA = "a" * 40
NOW = dt.datetime(2026, 8, 14, 1, 0, tzinfo=dt.timezone.utc)
_REAL_TOOLCHAIN_PROBLEMS = release_exit_gate.pa.scoreboard_observed_toolchain_problems
_REAL_BLOB_AT_SOURCE = release_exit_gate._blob_at_source
LEDGER = "docs/agent/V1_HANDOFF_FINDINGS.md"
PYPROJECT_V1 = '[project]\nname = "molt"\nversion = "1.0.0"\n'
PYPROJECT_V0 = '[project]\nname = "molt"\nversion = "0.9.0"\n'
_LEDGER_HEAD = """# Findings

## Open: release blockers

| ID | Finding | Evidence |
|----|---------|----------|
"""
_LEDGER_FIXED = """
## Fixed after the handoff

| ID | Defect | Integrated fix and verification boundary |
|----|--------|------------------------------------------|
| HF-F1 (was HF-1) | A closed defect. | Fixed at 1234567; its regression passes. |
"""
# Each ledger states its open rows literally; the tests compare the gate with
# these literals, never with a second parse.
CLOSED_LEDGER = _LEDGER_HEAD + _LEDGER_FIXED
OPEN_LEDGER = (
    _LEDGER_HEAD
    + "| HF-7 | An open defect. | CI run 7. |\n"
    + "| V1-3 | An open requirement. | Its acceptance obligation. |\n"
    + _LEDGER_FIXED
)
RESURRECTED_LEDGER = (
    _LEDGER_HEAD + "| HF-1 | Brought back by a merge. | CI run 1. |\n" + _LEDGER_FIXED
)
UNEVIDENCED_LEDGER = CLOSED_LEDGER + "| HF-F2 | A defect with no evidence. |  |\n"
ONE_OPEN_LEDGER = (
    _LEDGER_HEAD + "| HF-7 | An open defect. | CI run 7. |\n" + _LEDGER_FIXED
)


def _fixture_source(_root: Path, _sha: str, path: PurePosixPath) -> tuple[str, str]:
    """A closed ledger and a v1 project version for the synthetic source."""
    if path == release_exit_gate.finding_status.LEDGER_PATH:
        return "c" * 40, CLOSED_LEDGER
    assert path == release_exit_gate.PYPROJECT_PATH
    return "d" * 40, PYPROJECT_V1


@pytest.mark.parametrize("target", [[], {}, None, True])
def test_registry_target_wrong_types_return_diagnostics(target: object) -> None:
    _coordinates, problems = release_exit_gate._validate_registry_snapshot(
        [
            {
                "target": target,
                "variant": {
                    "cpython": "3.12",
                    "abi_tier": "cpython-abi",
                    "target_triple": "x86_64-unknown-linux-gnu",
                },
                "packages": {},
            }
        ]
    )
    assert any("target must be native or wasm" in problem for problem in problems)


def _load_gate(
    monkeypatch: pytest.MonkeyPatch,
    *,
    stub_source: bool = True,
    stub_toolchain_admission: bool = True,
):
    install_observed_runtime(monkeypatch)
    module = release_exit_gate
    # Synthetic bundle tests isolate storage/source/registry contracts, not real
    # compiler/runtime admission. Dedicated regressions below retain the actual
    # fail-closed toolchain authority; this stub is never emitted as evidence.
    if stub_toolchain_admission:
        monkeypatch.setattr(
            module.pa, "scoreboard_observed_toolchain_problems", lambda _doc: []
        )
    monkeypatch.setattr(module.pa.perf_schema, "validate_board", lambda _doc: [])
    if stub_source:
        monkeypatch.setattr(module, "_assert_clean_landed_source", lambda *_args: None)
        # The synthetic source SHA names no Git commit. Its ledger holds only
        # fixed rows and it claims v1.0.0; the real-Git controls below keep
        # the actual blob read.
        monkeypatch.setattr(module, "_blob_at_source", _fixture_source)
    monkeypatch.setattr(
        module,
        "_shared_scientific_registry_coordinates",
        lambda _root: _expected_registry(),
    )
    monkeypatch.setattr(
        verified_subset,
        "validate_manifest",
        lambda **kwargs: synthetic_validation(
            kwargs["repo_root"], _verified_projection
        ),
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
        "generated_at": "2026-08-14T12:00:00Z",
        "producer": {
            "argv": ["tools/pact_witness_acceptance.py", "--target", target],
            "execution_tools": None,
        },
    }
    receipt_path.write_text(json.dumps(payload), encoding="utf-8")
    return receipt_path


def _scoreboard_cell(
    gate, benchmark: str, backend: str, *, observation_root: Path
) -> dict[str, object]:
    return {
        "benchmark": benchmark,
        "target": "native",
        "backend": backend,
        "profile": gate.pa.CANONICAL_PERF_PROFILE,
        "build_observation": write_synthetic_build_observation(
            observation_root / backend / f"{Path(benchmark).stem}.fixture",
            backend=backend,
            profile=gate.pa.CANONICAL_PERF_PROFILE,
        ),
        "build_ok": True,
        "run_blocked": False,
        "molt_ok": True,
        "cpython_ok": True,
        "warm_speedup": 2.0,
        "verdict": gate.pa.perf_schema.VERDICT_GREEN,
        "classification": gate.pa.perf_schema.CLASS_GREEN,
        "stable": True,
        "repeat_stability": "STABLE_ABOVE",
        "repeat_ci_lo": 1.5,
        "repeat_ci_hi": 2.5,
        "output_parity": gate.pa.perf_schema.output_parity_evidence(
            reference_observations=[("cpython:cold", "result\n", "", 0)],
            molt_observations=[("molt:cold", "result\n", "", 0)],
        ),
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
        "host": {
            "cpython_oracle": {"version": "3.12.13"},
            "molt_target_python": "3.12",
        },
        "provenance": {
            "benchmark_tool_identity_schema": "molt-perf-tool-family-v1",
            "benchmark_tool_sha": perf_scoreboard._benchmark_tool_identity()[
                "ondisk_blob_sha"
            ],
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
                            observation_root=path.parent / "build-observations",
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
        "profiles": verified_subset.execution_profiles(
            coordinate.build_profile, backend=coordinate.backend
        ),
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

    policy = load_verified_subset_policy()
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
                "audit_engine": None,
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
    inputs = _inputs(tmp_path, gate)
    captures = []
    validations = []
    capture = verified_subset.validate_manifest
    validate = gate.rcr.validate_receipt

    def capture_once(**kwargs):
        result = capture(**kwargs)
        captures.append(result)
        return result

    def validate_in_batch(*args, **kwargs):
        if kwargs["expected_kind"] == gate.rcr.KIND_VERIFIED_SUBSET:
            validations.append(kwargs.get("verified_subset_validation"))
        return validate(*args, **kwargs)

    monkeypatch.setattr(verified_subset, "validate_manifest", capture_once)
    monkeypatch.setattr(gate.rcr, "validate_receipt", validate_in_batch)

    manifest_path, report = gate.assemble_release_bundle(**inputs)
    assert len(captures) == 1
    assert validations and all(item is captures[0] for item in validations)

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
        "findings",
    }
    assert payload["status"] == gate.STATUS_PASS
    assert payload["findings"] == {
        "ledger": LEDGER,
        "ledger_blob": "c" * 40,
        "open": [],
        "version": "1.0.0",
        "v1_contract": True,
    }
    assert report.open_findings == ()
    assert report.version == "1.0.0"
    assert [item["role"] for item in payload["evidence"]] == sorted(
        gate._expected_evidence_roles(verified_subset.verified_subset_coordinates())
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


@pytest.mark.parametrize("failure", ["capture", "mutation"])
def test_release_verification_fails_closed_on_inventory_errors_without_rescanning(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
    failure: str,
) -> None:
    gate = _load_gate(monkeypatch)
    manifest, _report = _assemble(tmp_path, gate)
    capture = verified_subset.validate_manifest
    calls = []

    def failing_capture(**kwargs):
        calls.append(True)
        if failure == "capture":
            raise ValueError("injected source capture failure")
        return capture(**kwargs)

    def changed(_self):
        raise ValueError("injected source mutation")

    monkeypatch.setattr(verified_subset, "validate_manifest", failing_capture)
    if failure == "mutation":
        monkeypatch.setattr(
            verified_subset.VerifiedSubsetValidation, "verify_unchanged", changed
        )
    report = gate.verify_release_bundle(manifest, repo_root=REPO_ROOT, now=NOW)
    assert not report.passed
    assert len(calls) == 1
    assert any("injected source" in problem for problem in report.problems)


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
    created = run_guarded_test_process(
        ["cmd.exe", "/d", "/c", "mklink", "/J", str(junction), str(outside)],
        check=False,
        capture_output=True,
        text=True,
    )
    if created.returncode != 0:
        pytest.skip(f"junction creation is unavailable: {str(created.stderr).strip()}")
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


@pytest.mark.parametrize("forged_verified", (False, True))
def test_assembly_rejects_unadmitted_e2_toolchains(
    tmp_path, monkeypatch, forged_verified
):
    gate = _load_gate(monkeypatch, stub_toolchain_admission=False)
    inputs = _inputs(tmp_path, gate)
    payload = _read(inputs["e2_scoreboard"])
    if forged_verified:
        for cell in gate.pa.perf_schema.flatten_cells(payload):
            cell["build_observation"]["compiled_with_verified"] = True
    assert gate.pa.canonical_scoreboard_shape_problems(payload) == []
    _write(inputs["e2_scoreboard"], payload)
    with pytest.raises(ValueError, match="invalid E2 scoreboard") as exc_info:
        gate.assemble_release_bundle(**inputs)
    assert "used-byte admission receipt is unavailable" in str(exc_info.value)


def test_verification_rejects_unadmitted_e2_toolchains(tmp_path, monkeypatch):
    gate = _load_gate(monkeypatch)
    manifest, _report = _assemble(tmp_path, gate)
    monkeypatch.setattr(
        gate.pa, "scoreboard_observed_toolchain_problems", _REAL_TOOLCHAIN_PROBLEMS
    )
    report = gate.verify_release_bundle(manifest, repo_root=REPO_ROOT, now=NOW)
    assert report.passed is False
    assert any(
        "E2:" in problem and "used-byte admission receipt is unavailable" in problem
        for problem in report.problems
    )


def _git(repo: Path, *args: str) -> str:
    return run_guarded_test_process(
        ["git", *args],
        cwd=repo,
        check=True,
        capture_output=True,
        text=True,
        encoding="utf-8",
    ).stdout.strip()


@pytest.fixture()
def ledger_repo(tmp_path: Path) -> Path:
    root = tmp_path / "ledger-repo"
    root.mkdir()
    _git(root, "init", "-q", "-b", "main")
    _git(root, "config", "user.email", "dev@example.com")
    _git(root, "config", "user.name", "Dev")
    _git(root, "config", "commit.gpgsign", "false")
    return root


def _commit_ledger(repo: Path, text: str, pyproject: str | None = PYPROJECT_V1) -> str:
    files = {LEDGER: text, "pyproject.toml": pyproject}
    for relative, content in files.items():
        if content is None:
            continue
        path = repo / relative
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_text(content, encoding="utf-8", newline="\n")
        _git(repo, "add", "--", relative)
    _git(repo, "commit", "-q", "-m", "Record the source facts")
    return _git(repo, "rev-parse", "HEAD")


def _ledger_from(monkeypatch: pytest.MonkeyPatch, gate, repo: Path, sha: str) -> None:
    """Read the real ledger and pyproject blobs of *sha* for the synthetic source.

    The synthetic receipts bind source "a" * 40, which names no commit. Only
    the commit lookup moves to the temporary repository; the Git blob reads,
    the projection and the status derivation stay real.
    """
    monkeypatch.setattr(
        gate,
        "_blob_at_source",
        lambda _root, _sha, path: _REAL_BLOB_AT_SOURCE(repo, sha, path),
    )


def test_source_findings_read_the_ledger_blob_at_the_source_revision(
    ledger_repo: Path,
) -> None:
    gate = release_exit_gate
    opened = _commit_ledger(ledger_repo, OPEN_LEDGER)
    closed = _commit_ledger(ledger_repo, CLOSED_LEDGER)
    # A dirty checkout must not change the facts of a committed revision.
    (ledger_repo / LEDGER).write_text(OPEN_LEDGER, encoding="utf-8", newline="\n")

    at_open = gate.source_findings(ledger_repo, opened)
    at_closed = gate.source_findings(ledger_repo, closed)

    assert at_open.status.open_keys == ("HF-7", "V1-3")
    assert at_open.status.problems == ()
    assert at_open.version == "1.0.0"
    assert at_open.holding_findings == ("HF-7", "V1-3")
    assert at_open.ledger_blob == _git(ledger_repo, "rev-parse", f"{opened}:{LEDGER}")
    assert at_closed.status.open_keys == ()
    assert at_closed.ledger_blob == _git(ledger_repo, "rev-parse", f"{closed}:{LEDGER}")
    with pytest.raises(ValueError, match="invalid object name"):
        gate.source_findings(ledger_repo, "b" * 40)
    tree = _git(ledger_repo, "rev-parse", f"{closed}^{{tree}}")
    with pytest.raises(ValueError, match="rev-parse"):
        gate.source_findings(ledger_repo, tree)


def test_open_finding_at_the_source_refuses_pass_and_names_every_id(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
    ledger_repo: Path,
    capsys: pytest.CaptureFixture[str],
) -> None:
    gate = _load_gate(monkeypatch)
    opened = _commit_ledger(ledger_repo, OPEN_LEDGER)
    _ledger_from(monkeypatch, gate, ledger_repo, opened)

    manifest_path, report = _assemble(tmp_path, gate)

    # Every typed receipt passes; only the open findings hold the release.
    assert report.problems == ()
    assert report.passed is False
    assert report.status == gate.STATUS_FAIL
    assert report.open_findings == ("HF-7", "V1-3")
    payload = _read(manifest_path)
    assert payload["status"] == gate.STATUS_FAIL
    assert payload["findings"] == {
        "ledger": LEDGER,
        "ledger_blob": _git(ledger_repo, "rev-parse", f"{opened}:{LEDGER}"),
        "open": ["HF-7", "V1-3"],
        "version": "1.0.0",
        "v1_contract": True,
    }
    gate._print_report(report)
    printed = capsys.readouterr().out
    assert "version 1.0.0 claims the v1 public stable contract" in printed
    assert "open findings (2, hold this release): HF-7, V1-3" in printed

    payload["status"] = gate.STATUS_PASS
    _write(manifest_path, payload)
    forged_status = gate.verify_release_bundle(
        manifest_path, repo_root=REPO_ROOT, now=NOW
    )
    assert forged_status.passed is False
    assert any("status is not derived" in problem for problem in forged_status.problems)

    payload["findings"]["open"] = []
    _write(manifest_path, payload)
    forged_join = gate.verify_release_bundle(
        manifest_path, repo_root=REPO_ROOT, now=NOW
    )
    assert forged_join.passed is False
    assert (
        "manifest findings.open omits findings open at the source: HF-7, V1-3"
        in forged_join.problems
    )


def test_zero_open_ledger_passes_until_the_source_ledger_disagrees(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
    ledger_repo: Path,
) -> None:
    gate = _load_gate(monkeypatch)
    closed = _commit_ledger(ledger_repo, CLOSED_LEDGER)
    opened = _commit_ledger(ledger_repo, OPEN_LEDGER)
    _ledger_from(monkeypatch, gate, ledger_repo, closed)

    manifest_path, report = _assemble(tmp_path, gate)

    assert report.passed is True
    assert report.open_findings == ()
    assert _read(manifest_path)["findings"]["open"] == []

    # The verifier rereads the source ledger; it never trusts the recorded join.
    _ledger_from(monkeypatch, gate, ledger_repo, opened)
    reread = gate.verify_release_bundle(manifest_path, repo_root=REPO_ROOT, now=NOW)
    assert reread.passed is False
    assert reread.open_findings == ("HF-7", "V1-3")
    assert (
        "manifest findings.open omits findings open at the source: HF-7, V1-3"
        in reread.problems
    )
    assert any("status is not derived" in problem for problem in reread.problems)
    assert any(
        "ledger_blob is not the ledger" in problem for problem in reread.problems
    )


@pytest.mark.parametrize(
    ("ledger", "defect"),
    [
        (RESURRECTED_LEDGER, "HF-1 is open, but a fixed row says it was HF-1"),
        (UNEVIDENCED_LEDGER, "fixed row HF-F2 (line 13) records no fix"),
    ],
    ids=["resurrected", "unevidenced"],
)
def test_invalid_source_ledger_is_refused(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
    ledger_repo: Path,
    ledger: str,
    defect: str,
) -> None:
    gate = _load_gate(monkeypatch)
    manifest_path, report = _assemble(tmp_path / "closed", gate)
    assert report.passed is True
    invalid = _commit_ledger(ledger_repo, ledger)
    _ledger_from(monkeypatch, gate, ledger_repo, invalid)

    with pytest.raises(
        ValueError, match="findings ledger at the release source"
    ) as exc:
        _assemble(tmp_path / "invalid", gate)
    assert defect in str(exc.value)

    reread = gate.verify_release_bundle(manifest_path, repo_root=REPO_ROOT, now=NOW)
    assert reread.passed is False
    assert any(defect in problem for problem in reread.problems)


@pytest.mark.parametrize(
    ("pyproject", "version", "status"),
    [(PYPROJECT_V1, "1.0.0", "FAIL"), (PYPROJECT_V0, "0.9.0", "PASS")],
    ids=["v1-claims-contract", "v0-pre-release"],
)
def test_open_finding_holds_only_a_version_that_claims_the_v1_contract(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
    ledger_repo: Path,
    capsys: pytest.CaptureFixture[str],
    pyproject: str,
    version: str,
    status: str,
) -> None:
    gate = _load_gate(monkeypatch)
    source = _commit_ledger(ledger_repo, ONE_OPEN_LEDGER, pyproject)
    _ledger_from(monkeypatch, gate, ledger_repo, source)

    manifest_path, report = _assemble(tmp_path, gate)

    assert report.problems == ()
    assert report.status == status
    assert report.passed is (status == "PASS")
    # Both versions record and report the open row.
    assert report.open_findings == ("HF-7",)
    assert report.version == version
    findings = _read(manifest_path)["findings"]
    assert (findings["open"], findings["version"]) == (["HF-7"], version)
    assert findings["v1_contract"] is (version == "1.0.0")
    gate._print_report(report)
    printed = capsys.readouterr().out
    if version == "1.0.0":
        assert "open findings (1, hold this release): HF-7" in printed
    else:
        assert "version 0.9.0 does not claim the v1 public stable contract" in printed
        assert "open findings (1, do not hold this release): HF-7" in printed

    # A recorded version cannot move the release across the contract line.
    payload = _read(manifest_path)
    payload["findings"]["version"] = "0.9.0" if version == "1.0.0" else "1.0.0"
    payload["findings"]["v1_contract"] = version != "1.0.0"
    _write(manifest_path, payload)
    forged = gate.verify_release_bundle(manifest_path, repo_root=REPO_ROOT, now=NOW)
    assert forged.passed is False
    assert any(
        "findings.version is not the project version at the source" in problem
        for problem in forged.problems
    )


@pytest.mark.parametrize(
    ("pyproject", "defect"),
    [
        (None, "path 'pyproject.toml' does not exist"),
        ('[project]\nname = "molt"\n', "declares no project version"),
        ('[project]\nname = "molt"\nversion = "1.0"\n', "invalid release version"),
    ],
    ids=["missing", "no-version", "malformed"],
)
def test_unreadable_source_version_fails_closed(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
    ledger_repo: Path,
    pyproject: str | None,
    defect: str,
) -> None:
    gate = _load_gate(monkeypatch)
    manifest_path, report = _assemble(tmp_path / "readable", gate)
    assert report.passed is True
    source = _commit_ledger(ledger_repo, CLOSED_LEDGER, pyproject)
    _ledger_from(monkeypatch, gate, ledger_repo, source)

    with pytest.raises(ValueError, match="pyproject.toml|release version") as exc:
        _assemble(tmp_path / "unreadable", gate)
    assert defect in str(exc.value)

    reread = gate.verify_release_bundle(manifest_path, repo_root=REPO_ROOT, now=NOW)
    assert reread.passed is False
    assert reread.version is None
    assert any(
        "cannot read the release source facts" in problem and defect in problem
        for problem in reread.problems
    )
    assert any("status is not derived" in problem for problem in reread.problems)
