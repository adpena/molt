"""Teeth for tools/phase_exit_manifest.py: the fixed §5 phase predicate.

Every evidence row is projected from real producer envelopes: Pact acceptance
receipts written by tools/pact_witness_acceptance.py, an E2 board emitted
through tools/perf_scoreboard.py's verdict law and writer with profile facts
from the CLI publication observer over synthetic artifact bytes, and E3/E4
receipts built by tools/release_criterion_receipt.py. tools/release_exit_gate.py
assembles them, and its storage/source/statistical verifier checks every byte
against a hermetic source tree holding exact copies of bound files. Actual
compiler/runtime admission is unavailable, so this fixture isolates that
precondition only inside release-scoreboard validation. The phase projection
always retains the real toolchain validator and rejects unadmitted E2 facts.
These synthetic bundles are test inputs, never release acceptance evidence.
Only inputs this checkout cannot supply are fixtured: the Pact registry (the
live one has no native pact-witness coordinate yet), the verified-subset test
projection (one differential test per real coordinate), measured perf and CI
facts, and the Git custody preflight of bundle assembly. The hermetic tree has
no Git, so the bundle reads its findings ledger from the tree instead of the
blob at the source commit; tests/tools/test_release_exit_gate.py keeps the real
blob read.
"""

from __future__ import annotations

import base64
import hashlib
import json
import re
import shlex
import shutil
import sys
from collections.abc import Callable, Mapping, Sequence
from pathlib import Path
from types import SimpleNamespace
from typing import Any

import pytest

from tests.process_guard_common import run_isolated_python_probe

from molt.exact_json import (
    canonical_json_bytes,
    canonical_json_sha256,
    loads_exact,
    write_exact,
)
from molt.verified_subset import load_verified_subset_policy
from tests.tools.verified_subset_fixtures import synthetic_validation
from tests.tools.receipt_engine_fixtures import install_observed_runtime
from tests.tools.perf_scoreboard_fixtures import write_synthetic_build_observation
from tests.wasm_execution_manifest import write_wasm_execution_manifest
from tools import legacy_inventory as li
from tools import pact_witness_acceptance as acceptance
from tools import phase_exit_manifest as pem
from tools.compat import comparison, test_policy

REPO_ROOT = Path(__file__).resolve().parents[2]
for _import_root in (REPO_ROOT / "tools", REPO_ROOT / "src"):
    if str(_import_root) not in sys.path:
        sys.path.insert(0, str(_import_root))

import perf_scoreboard as ps  # noqa: E402

reg = pem.reg
rcr = pem.rcr
pa = pem.pa
vs = pem.vs

SOURCE_SHA = "a" * 40
SIGNABLE = "S0"
LEDGER = "docs/agent/V1_HANDOFF_FINDINGS.md"
CLOSED_LEDGER = """# Findings

## Open: release blockers

| ID | Finding | Evidence |
|----|---------|----------|

## Fixed after the handoff

| ID | Defect | Integrated fix and verification boundary |
|----|--------|------------------------------------------|
| HF-F1 (was HF-1) | A closed defect. | Fixed at 1234567; its regression passes. |
"""
OPEN_LEDGER = CLOSED_LEDGER.replace(
    "|----|---------|----------|\n",
    "|----|---------|----------|\n| HF-7 | An open defect. | CI run 7. |\n",
)
E2_REQUIREMENT = "H0.perf.cpython_floor_scoreboard"
CPYTHON_BASELINE = "3.12.13"
TRIPLES = {"native": "x86_64-pc-windows-msvc", "wasm": "wasm32-wasip1"}
PACKAGES = {  # package -> (version, canonical identity, seal)
    "numpy": ("2.5.1", "1" * 64, "3" * 64),
    "scipy": ("1.18.0", "2" * 64, "4" * 64),
}
E4_INPUTS = {
    rcr.KIND_CANONICALIZATION_CONTRACT: "tools/canonicalization_contract_baseline.json",
    rcr.KIND_DEGRADE_TO_SLOW_GATE: "tools/degrade_to_slow_registry.toml",
    rcr.KIND_FAIL_CLOSED_GATE: "tools/fail_closed_registry.toml",
    rcr.KIND_STRUCTURAL_AUDIT: "tools/structural_audit_baseline.json",
}
# Row fields each producer schema does not record today. H0 stays false on
# exactly these nulls until the producers carry the fields.
UNRECORDED = {
    "e1": frozenset({"toolchain_digest"}),
    "e2": frozenset({"command", "toolchain_digest"}),
    "e3": frozenset(),
    "e4": frozenset(),
}
MISSING_FIELD = re.compile(
    r"schema: evidence\[\d+\] \((?P<requirement>[^)]+)\) "
    r"field (?P<field>\w+) is missing"
)
SHA256 = re.compile(r"[0-9a-f]{64}")
# Only verified-subset receipts record every §5 row field today, so the signing
# machinery is proven over a phase those real receipts can satisfy.
SIGNABLE_PHASE = f"""
[[phase]]
id = "{SIGNABLE}"
title = "Signable verified-subset phase"
matrix_authority = "tools/verified_subset.py matrix"

[[phase.requirement]]
id = "{SIGNABLE}.verified_subset"
authority = "tools/verified_subset.py"
evidence_role = "e3_*"
matrix_cells = ["verified-subset:*"]

[[phase.obligation]]
id = "E3-VERIFIED-SUBSET"
description = "Every exact verified-subset coordinate has a passing receipt"
closed_by = "{SIGNABLE}.verified_subset"
"""


def _registry(_root: object = None) -> dict[tuple[str, str, str], dict[str, Any]]:
    """The shared E1 coordinates; the checked-out registry has no native row."""
    return {
        ("3.12", "cpython-abi", triple): {
            "target": target,
            "variant": {
                "cpython": "3.12",
                "abi_tier": "cpython-abi",
                "target_triple": triple,
            },
            "packages": {
                package: {
                    "version": version,
                    "module_set": "pact-witness",
                    "identity_sha256": identity,
                }
                for package, (version, identity, _seal) in PACKAGES.items()
            },
        }
        for target, triple in TRIPLES.items()
    }


def _projection(coordinate: Any) -> test_policy.CoordinateProjection:
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


def _validation(*, repo_root: Path = vs.ROOT) -> Any:
    """The real policy and coordinates, each projecting one differential test."""
    return synthetic_validation(repo_root, _projection)


def _checked_out(root: Path, source_sha: str) -> None:
    """Bundle assembly's Git custody preflight; the hermetic tree has no Git."""
    assert source_sha == SOURCE_SHA
    assert (root / rcr.KIND_TO_TOOL[rcr.KIND_VERIFIED_SUBSET]).is_file()


def _source_in_tree(root: Path, source_sha: str, path: Any) -> tuple[str, str]:
    """Source facts at the bundle source; the hermetic tree has no Git.

    The ledger comes from the tree. H0 is the v1 contract phase, so the
    project version is a v1 fixture rather than the checkout's version.
    """
    assert source_sha == SOURCE_SHA
    if path == reg.PYPROJECT_PATH:
        data = b'[project]\nname = "molt"\nversion = "1.0.0"\n'
    else:
        assert path.as_posix() == LEDGER
        data = (root / LEDGER).read_bytes()
    blob = hashlib.sha1(b"blob %d\0" % len(data) + data).hexdigest()
    return blob, data.decode("utf-8")


_REAL_RELEASE_SCOREBOARD_PROBLEMS = pa.release_scoreboard_problems


def _fixture_release_scoreboard_problems(payload, **kwargs):
    """Isolate only release admission while retaining real phase rejection.

    Bundle storage and source tests require an admitted input precondition.
    No used-byte receipt producer exists yet. This narrowly scoped assumption
    ends before phase projection or direct toolchain-negative assertions run;
    compiler/runtime observations stay unknown and no used-byte admission is
    fabricated. Selected-profile observations remain subject to real validation.
    """
    with pytest.MonkeyPatch.context() as patch:
        patch.setattr(pa, "scoreboard_observed_toolchain_problems", lambda _doc: [])
        return _REAL_RELEASE_SCOREBOARD_PROBLEMS(payload, **kwargs)


def _admit_fixture_inputs(patch: pytest.MonkeyPatch) -> None:
    patch.setattr(
        pa, "release_scoreboard_problems", _fixture_release_scoreboard_problems
    )
    install_observed_runtime(patch)
    patch.setattr(reg, "_shared_scientific_registry_coordinates", _registry)
    patch.setattr(vs, "validate_manifest", _validation)
    patch.setattr(reg, "_assert_clean_landed_source", _checked_out)
    patch.setattr(reg, "_blob_at_source", _source_in_tree)
    patch.setattr(ps, "_git_rev", lambda: SOURCE_SHA)


def _source_root(base: Path) -> Path:
    """A hermetic source tree: byte-exact copies of every file receipts bind."""
    root = base / "source"
    authority_files = vs.verified_subset_authority_files(load_verified_subset_policy())
    bound = {
        *(path.relative_to(vs.ROOT).as_posix() for path in authority_files),
        *rcr.KIND_TO_TOOL.values(),
        *E4_INPUTS.values(),
    }
    for relative in sorted(bound):
        target = root / relative
        target.parent.mkdir(parents=True, exist_ok=True)
        shutil.copyfile(vs.ROOT / relative, target)
    (root / "config" / "phase_exit_requirements.toml").write_text(
        pem.REQUIREMENTS_PATH.read_text(encoding="utf-8") + SIGNABLE_PHASE,
        encoding="utf-8",
        newline="\n",
    )
    (root / "config" / "legacy_inventory.toml").write_text(
        f'schema = "{li.SCHEMA}"\n', encoding="utf-8", newline="\n"
    )
    (root / LEDGER).parent.mkdir(parents=True, exist_ok=True)
    (root / LEDGER).write_text(CLOSED_LEDGER, encoding="utf-8", newline="\n")
    return root


def _verified_execution(
    coordinate: Any,
    *,
    run_id: str = "42",
    run_attempt: str = "1",
    python_sha256: str = "5" * 64,
    rustc_sha256: str = "6" * 64,
    node_sha256: str = "7" * 64,
) -> dict[str, object]:
    """The typed execution identity tools/verified_subset.py records in CI."""
    backend: dict[str, object] = {"backend": coordinate.backend, "runner": "process"}
    if coordinate.backend == "wasm":
        backend = {
            "backend": "wasm",
            "binary_name": "node",
            "binary_sha256": node_sha256,
            "runner": "node-wasi",
            "version": "v24.16.0",
        }
    runner_os = {"linux": "Linux", "macos": "macOS", "windows": "Windows"}
    return {
        "backend": backend,
        "profiles": vs.execution_profiles(
            coordinate.build_profile, backend=coordinate.backend
        ),
        "ci": {
            "job": "verified-subset",
            "provider": "github-actions",
            "run_attempt": run_attempt,
            "run_id": run_id,
            "runner_arch": (
                "ARM64" if coordinate.arch in {"aarch64", "arm64"} else "X64"
            ),
            "runner_label": coordinate.runner,
            "runner_os": runner_os[coordinate.platform],
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
            "cache_tag": "cpython-" + coordinate.python.replace(".", ""),
            "command_executable": "python",
            "executable_name": "python",
            "executable_sha256": python_sha256,
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
            "binary_sha256": rustc_sha256,
            "commit_date": "2026-01-01",
            "commit_hash": "8" * 40,
            "host": coordinate.rust_target,
            "llvm_version": "21.1.0",
            "release": "1.96.1",
        },
    }


def _verified_subset_receipt(
    root: Path,
    validation: Any,
    coordinate: Any,
    destination: Path,
    **identity: str,
) -> dict[str, Any]:
    """Build one E3 receipt through the release-criterion producer."""
    empty, four = hashlib.sha256(b"").hexdigest(), hashlib.sha256(b"4\n").hexdigest()
    outcome = {
        "backend": coordinate.backend,
        "backend_returncode": 0,
        "backend_status": "pass",
        "backend_stderr_sha256": empty,
        "backend_stdout_sha256": four,
        "comparison_law": comparison.COMPARISON_LAW_VERSION,
        "compiler_target_python": coordinate.python,
        "cpython_returncode": 0,
        "cpython_stderr_sha256": empty,
        "cpython_stdout_sha256": four,
        "expect_molt_fail": False,
        "expected_failure_reason": None,
        "path": "tests/differential/basic/arith.py",
        "raw_status": "pass",
        "reason_tag": None,
        "resolved_status": "pass",
    }
    passed = vs.outcomes_pass([outcome])
    return rcr.build_receipt(
        kind=rcr.KIND_VERIFIED_SUBSET,
        source_sha=SOURCE_SHA,
        status=rcr.STATUS_PASS if passed else rcr.STATUS_FAIL,
        argv=[
            "run",
            "--coordinate",
            coordinate.id,
            "--receipt",
            str(destination),
            "--source-sha",
            SOURCE_SHA,
        ],
        tool_path=root / rcr.KIND_TO_TOOL[rcr.KIND_VERIFIED_SUBSET],
        facts=vs._receipt_facts(
            coordinate=coordinate,
            policy=validation.policy,
            projection=validation.projection(coordinate),
            results=[outcome],
            execution=_verified_execution(coordinate, **identity),
        ),
        input_paths=vs.verified_subset_authority_files(
            validation.policy, repo_root=root
        ),
        repo_root=root,
        verified_subset_validation=validation,
    )


def _structural_receipt(
    root: Path, kind: str, destination: Path, *, errors: Sequence[str] = ()
) -> Path:
    """Write one E4 receipt through the release-criterion producer."""
    assert not errors or kind == rcr.KIND_DEGRADE_TO_SLOW_GATE
    relative = E4_INPUTS[kind]
    if kind == rcr.KIND_DEGRADE_TO_SLOW_GATE:
        facts: dict[str, Any] = {
            "discovered_site_count": 0,
            "errors": list(errors),
            "metabug_fix_pending_baseline": 0,
            "metabug_fix_pending_count": 0,
            "registry_path": relative,
            "registry_row_count": 0,
            "warnings": [],
        }
    elif kind == rcr.KIND_FAIL_CLOSED_GATE:
        zero = {name: 0 for name in sorted(rcr.FAIL_CLOSED_CLASSES)}
        facts = {
            "baseline_counts": zero,
            "class_counts": dict(zero),
            "registered_site_count": 0,
            "registry_path": relative,
            "violations": [],
        }
    else:
        baseline = loads_exact((root / relative).read_text(encoding="utf-8"))
        count = (
            "open_violations"
            if kind == rcr.KIND_CANONICALIZATION_CONTRACT
            else "findings_count"
        )
        facts = {
            "baseline_metrics": baseline,
            "baseline_path": relative,
            "improved_metrics": [],
            "metrics": baseline,
            "regressed_metrics": [],
            count: 0,
        }
    receipt = rcr.build_receipt(
        kind=kind,
        source_sha=SOURCE_SHA,
        status=rcr.STATUS_FAIL if errors else rcr.STATUS_PASS,
        argv=["--receipt", str(destination), "--source-sha", SOURCE_SHA],
        tool_path=root / rcr.KIND_TO_TOOL[kind],
        facts=facts,
        input_paths=[root / relative],
        repo_root=root,
    )
    destination.parent.mkdir(parents=True, exist_ok=True)
    write_exact(destination, receipt, exclusive=True)
    return destination


def _package_seal(package: str) -> SimpleNamespace:
    """The seal record tools/pact_witness_acceptance.py reads per package."""
    version, identity, seal = PACKAGES[package]
    return SimpleNamespace(
        validation=SimpleNamespace(
            recorded=SimpleNamespace(package_version=version, name="pact-witness")
        ),
        seal=SimpleNamespace(seal_sha256=seal),
        canonical_identity=SimpleNamespace(canonical_sha256=identity),
    )


def _pact_receipt(base: Path, target: str) -> Path:
    """Write one acceptance receipt through the Pact acceptance producer."""
    attempt = base / target
    build = attempt / "build"
    run = attempt / "run"
    build.mkdir(parents=True)
    run.mkdir()
    program = build / "program"
    program.write_bytes(f"{target} witness image".encode())
    manifest = None
    if target == "wasm":
        runtime = build / "runtime.wasm"
        runtime.write_bytes(b"witness runtime image")
        manifest = write_wasm_execution_manifest(build, app=program, runtime=runtime)
    outputs = {}
    for name in ("candidate_outputs.npz", "reference_oracle.npz", "gates.json"):
        outputs[name] = run / name
        outputs[name].write_bytes(f"{target} {name}".encode())
    return acceptance._write_acceptance_receipt(
        descriptor=acceptance._ExecutionDescriptor(
            target, program, (str(program),), manifest
        ),
        source_sha=SOURCE_SHA,
        seals=SimpleNamespace(
            variant=SimpleNamespace(
                cpython="3.12", abi_tier="cpython-abi", target_triple=TRIPLES[target]
            ),
            receipt=_package_seal,
        ),
        candidate=outputs["candidate_outputs.npz"],
        reference=outputs["reference_oracle.npz"],
        gates=outputs["gates.json"],
        attempt_dir=attempt,
        producer_argv=["tools/pact_witness_acceptance.py", "--target", target],
    )


def _perf_provenance() -> dict[str, object]:
    """The provenance perf_scoreboard.gather_provenance records on a quiet tip."""
    return {
        "origin_sha": SOURCE_SHA,
        "local_head_sha": SOURCE_SHA,
        "merge_base_sha": SOURCE_SHA,
        "dirty_tree": False,
        "diverges_from_origin": False,
        "benchmark_tool_sha": "b" * 64,
        "benchmark_tool_identity_schema": "molt-perf-tool-family-v1",
        "benchmark_tool_last_commit": SOURCE_SHA,
        "benchmark_tool_modified": False,
        "backend_binary_identity": {
            f"{backend}/{pa.CANONICAL_PERF_PROFILE}": f"target/{backend}/backend|1|1"
            for backend in sorted(pa.CANONICAL_PERF_BACKENDS)
        },
        "stdlib_cache_key": "c" * 16,
        "authoritative": True,
        "authoritative_reason": "tree == origin/main, clean, tool unmodified",
        "require_quiescent": True,
        "quiescent": True,
        "quiescence": {
            "quiet": True,
            "quiescence_wait_timeout_s": float(pa.CANONICAL_PERF_QUIESCENCE_WAIT),
            "reasons": [],
        },
    }


def _board(
    path: Path,
    *,
    cold_loss: tuple[str, str] | None = None,
    extra: Mapping[str, object] | None = None,
) -> Path:
    """Emit a canonical E2 board through the scoreboard verdict law and writer.

    `cold_loss` names one (benchmark, backend) cell whose cold run loses to
    CPython by a startup tax inside its budget: WARN_COLD_FLOOR, which the
    statistical release predicate rejects along with the phase projection.
    """
    parity = pa.perf_schema.output_parity_evidence(
        reference_observations=[
            ("cpython:cold", "ok\n", "", 0),
            ("cpython:warm:p0:s0", "ok\n", "", 0),
        ],
        molt_observations=[
            ("molt:cold", "ok\n", "", 0),
            ("molt:warm:p0:s0", "ok\n", "", 0),
        ],
    )
    cells = []
    for benchmark in pa.CANONICAL_PERF_BENCHMARKS:
        for backend in sorted(pa.CANONICAL_PERF_BACKENDS):
            cell = ps.Cell(
                benchmark=benchmark,
                target="native",
                backend=backend,
                profile=pa.CANONICAL_PERF_PROFILE,
            )
            cell.build_observation = write_synthetic_build_observation(
                path.parent
                / "build-observations"
                / backend
                / f"{Path(benchmark).stem}.fixture",
                backend=cell.backend,
                profile=cell.profile,
            )
            cell.build_ok = cell.molt_ok = cell.cpython_ok = cell.stable = True
            cell.binary_size_kib, cell.compile_time_s = 512.0, 0.4
            cell.molt_peak_rss_mib, cell.cpython_peak_rss_mib = 18.0, 15.0
            cell.warm_molt_s, cell.warm_cpython_s = 0.010, 0.020
            cell.cold_molt_s, cell.cold_cpython_s = (
                (0.060, 0.040) if (benchmark, backend) == cold_loss else (0.011, 0.025)
            )
            cell.output_parity = dict(parity)
            cell.log_artifact = f"bench/scoreboard/logs/{Path(benchmark).stem}.log"
            cell.repeat_passes = int(pa.CANONICAL_PERF_REPEAT)
            cell.repeat_warm_speedups = [2.0] * cell.repeat_passes
            cell.repeat_ci_lo, cell.repeat_ci_hi = 1.9, 2.1
            cell.repeat_stability = "STABLE_ABOVE"
            cell.finalize(budget_ms=100.0, authoritative=True)
            cells.append(cell)
    ps.apply_classification(cells, quiescent=True)
    oracle = ps.CpythonOracle(
        cmd=(sys.executable,),
        executable=sys.executable,
        version=CPYTHON_BASELINE,
        implementation="CPython",
        sys_platform=sys.platform,
        machine=ps.platform.machine(),
        arch=ps._host_arch(),
        pointer_bits=ps._host_pointer_bits(),
    )
    doc = ps.build_scoreboard_doc(
        cells,
        benchmarks_run=list(pa.CANONICAL_PERF_BENCHMARKS),
        benchmarks_deferred=[],
        cpython_version=CPYTHON_BASELINE,
        samples=int(pa.CANONICAL_PERF_SAMPLES),
        warmup=int(pa.CANONICAL_PERF_WARMUP),
        provenance=_perf_provenance(),
        cpython_identity={**oracle.host_metadata(), "molt_target_python": "3.12"},
    )
    doc.update(extra or {})
    ps._write_scoreboard_doc(path, doc, context="phase-exit fixture board")
    return path


def _release_bundle(
    inputs: Mapping[str, Any],
    board: Path,
    output_root: Path,
    *,
    e4: Sequence[Path] | None = None,
) -> tuple[Path, Any]:
    """Assemble through the release-exit producer and its self-verification."""
    return reg.assemble_release_bundle(
        source_sha=SOURCE_SHA,
        e1_receipts=inputs["e1"],
        e2_scoreboard=board,
        e3_receipts=inputs["e3"],
        e4_receipts=inputs["e4"] if e4 is None else e4,
        repo_root=inputs["root"],
        output_root=output_root,
    )


@pytest.fixture(scope="module")
def producer_inputs(tmp_path_factory: pytest.TempPathFactory) -> dict[str, Any]:
    """Producer envelopes in a synthetic admitted bundle; E2 phase remains FAIL."""
    base = tmp_path_factory.mktemp("phase-exit")
    with pytest.MonkeyPatch.context() as patch:
        _admit_fixture_inputs(patch)
        root = _source_root(base)
        receipts = base / "release inputs"
        validation = vs.validate_manifest(repo_root=root)
        e3 = []
        for coordinate in validation.coordinates:
            destination = receipts / "e3" / f"{coordinate.id}.json"
            destination.parent.mkdir(parents=True, exist_ok=True)
            receipt = _verified_subset_receipt(
                root, validation, coordinate, destination
            )
            write_exact(destination, receipt, exclusive=True)
            e3.append(destination)
        inputs = {
            "root": root,
            "e1": [_pact_receipt(receipts / "e1", target) for target in TRIPLES],
            "e3": e3,
            "e4": [
                _structural_receipt(root, kind, receipts / "e4" / f"{kind}.json")
                for kind in sorted(E4_INPUTS)
            ],
            "board": _board(receipts / "e2" / "scoreboard.json"),
        }
        bundle, report = _release_bundle(inputs, inputs["board"], base / "dist")
        assert report.passed, report.problems
    return {**inputs, "bundle": bundle}


@pytest.fixture
def workspace(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
    producer_inputs: dict[str, Any],
) -> dict[str, Any]:
    """Per-test copies of the source tree and the verified release bundle."""
    _admit_fixture_inputs(monkeypatch)
    root = tmp_path / "source"
    shutil.copytree(producer_inputs["root"], root)
    bundle = producer_inputs["bundle"]
    copied = tmp_path / "dist" / bundle.parent.name
    shutil.copytree(bundle.parent, copied)
    phase_dir = tmp_path / "phase"
    phase_dir.mkdir()
    return {
        **producer_inputs,
        "root": root,
        "bundle": copied / bundle.name,
        "phase_dir": phase_dir,
    }


def _load(path: Path) -> dict[str, Any]:
    return loads_exact(path.read_text(encoding="utf-8"))


def _requirements(ws: Mapping[str, Any], phase_id: str) -> tuple[pem.Requirement, ...]:
    phases = pem.load_phases(ws["root"] / "config" / "phase_exit_requirements.toml")
    return pem.expand_requirements(phases[phase_id], pem.generated_matrix())


def _evidence(ws: Mapping[str, Any], role: str) -> Path:
    """The bundle artifact the release-exit manifest binds to one evidence role."""
    bundle = ws["bundle"]
    record = next(item for item in _load(bundle)["evidence"] if item["role"] == role)
    return bundle.parent / record["path"]


def _attest(manifest_path: Path) -> Path:
    """Write a Sigstore-shaped bundle whose in-toto subject binds the manifest bytes."""
    subject = pem._manifest_bytes_sha256(_load(manifest_path))
    statement = {
        "_type": "https://in-toto.io/Statement/v1",
        "subject": [{"name": manifest_path.name, "digest": {"sha256": subject}}],
        "predicateType": "https://slsa.dev/provenance/v1",
        "predicate": {},
    }
    bundle = {
        "mediaType": "application/vnd.dev.sigstore.bundle.v0.3+json",
        "verificationMaterial": {},
        "dsseEnvelope": {
            "payload": base64.b64encode(json.dumps(statement).encode()).decode(),
            "payloadType": "application/vnd.in-toto+json",
            "signatures": [{"sig": "dGVzdA=="}],
        },
    }
    path = manifest_path.with_name(f"{manifest_path.stem}.sigstore.json")
    path.write_text(json.dumps(bundle), encoding="utf-8")
    return path


def _assemble(
    ws: Mapping[str, Any], *, phase: str = SIGNABLE, attest: bool = True
) -> tuple[Path, pem.PhaseReport]:
    def assemble(attestation: Path | None) -> tuple[Path, pem.PhaseReport]:
        return pem.assemble_phase_manifest(
            phase_id=phase,
            commit=SOURCE_SHA,
            bundle_manifest=ws["bundle"],
            output=ws["phase_dir"] / f"{phase}.json",
            attestation=attestation,
            root=ws["root"],
        )

    output, report = assemble(None)
    return assemble(_attest(output)) if attest else (output, report)


def _verify(
    ws: Mapping[str, Any], *, phase: str = SIGNABLE, commit: str = SOURCE_SHA
) -> pem.PhaseReport:
    return pem.verify_phase_manifest(
        ws["phase_dir"] / f"{phase}.json",
        release_commit=commit,
        bundle_manifest=ws["bundle"],
        root=ws["root"],
    )


def _rewrite(path: Path, mutate: Callable[[dict[str, Any]], object]) -> None:
    manifest = _load(path)
    mutate(manifest)
    write_exact(path, manifest)


def _resign(path: Path, mutate: Callable[[dict[str, Any]], object]) -> None:
    """Rewrite a manifest and attach a fresh attestation over the new bytes."""
    _rewrite(path, mutate)
    manifest = _load(path)
    manifest["signed_attestation"] = pem._attestation_record(_attest(path), path)
    write_exact(path, manifest)


def _prepare(
    ws: Mapping[str, Any], *, phase: str = SIGNABLE, commit: str = SOURCE_SHA
) -> Path:
    return pem.prepare_phase_signing_subject(
        phase_id=phase,
        commit=commit,
        bundle_manifest=ws["bundle"],
        output=ws["phase_dir"] / f"{phase}.subject.json",
        root=ws["root"],
    )


def _seal(
    ws: Mapping[str, Any], subject: Path, attestation: Path
) -> tuple[Path, pem.PhaseReport]:
    return pem.seal_phase_manifest(
        subject=subject,
        attestation=attestation,
        bundle_manifest=ws["bundle"],
        output=ws["phase_dir"] / f"{SIGNABLE}.json",
        root=ws["root"],
    )


def test_live_h0_expands_over_the_generated_matrix_to_the_bundle_roles() -> None:
    phase = pem.load_phases()["H0"]
    matrix = pem.generated_matrix()
    expanded = pem.expand_requirements(phase, matrix)
    assert {row.evidence_role for row in expanded} == reg._expected_evidence_roles(
        vs.verified_subset_coordinates()
    )
    verified = {
        row.evidence_role: row.matrix_cells
        for row in expanded
        if row.evidence_role.startswith(reg.VERIFIED_SUBSET_EVIDENCE_PREFIX)
    }
    assert len(matrix["include"]) >= 36
    assert verified == {
        reg.verified_subset_evidence_role(record["id"]): (
            f"{pem.VERIFIED_SUBSET_CELL_PREFIX}{record['id']}",
        )
        for record in matrix["include"]
    }
    assert SHA256.fullmatch(canonical_json_sha256(matrix))


def test_h0_rejects_unrecorded_fields_and_unadmitted_perf_toolchain(
    workspace: dict[str, Any],
) -> None:
    output, report = _assemble(workspace, phase="H0")
    missing: dict[str, set[str]] = {}
    other_problems: list[str] = []
    for problem in report.problems:
        match = MISSING_FIELD.fullmatch(problem)
        if match is None:
            other_problems.append(problem)
        else:
            missing.setdefault(match["requirement"], set()).add(match["field"])
    assert other_problems == [
        f"evidence: {E2_REQUIREMENT} status is 'FAIL', not PASS",
        "obligations: open obligations remain: E2-CPYTHON-FLOOR",
    ]
    requirements = _requirements(workspace, "H0")
    assert missing == {
        requirement.id: set(UNRECORDED[requirement.evidence_role[:2]])
        for requirement in requirements
        if UNRECORDED[requirement.evidence_role[:2]]
    }
    assert not report.green
    manifest = _load(output)
    assert manifest["open_obligations"] == ["E2-CPYTHON-FLOOR"]
    rows = {row["requirement_id"]: row for row in manifest["evidence"]}
    assert set(rows) == {requirement.id for requirement in requirements}
    for requirement in requirements:
        row = rows[requirement.id]
        artifact = _evidence(workspace, requirement.evidence_role)
        payload = _load(artifact)
        unrecorded = UNRECORDED[requirement.evidence_role[:2]]
        assert row["status"] == (
            pem.STATUS_FAIL
            if requirement.evidence_role == "e2_scoreboard"
            else pem.STATUS_PASS
        )
        assert row["matrix_cells"] == list(requirement.matrix_cells)
        assert (
            row["artifact_sha256"] == hashlib.sha256(artifact.read_bytes()).hexdigest()
        )
        if "observed_at" not in unrecorded:
            assert row["observed_at"] == payload["generated_at"]
        if "command" not in unrecorded:
            assert shlex.split(row["command"]) == payload["producer"]["argv"]


def test_h0_rejects_cold_loss_at_canonical_bundle_admission(
    workspace: dict[str, Any], tmp_path: Path
) -> None:
    cold_loss = (pa.CANONICAL_PERF_BENCHMARKS[0], "llvm")
    board = _board(tmp_path / "warn" / "scoreboard.json", cold_loss=cold_loss)
    summary = _load(board)["summary"]
    assert summary["gate_fails"] is True
    assert len(summary["verdict_breakdown"]["WARN_COLD_FLOOR"]) == 1
    with pytest.raises(ValueError, match="invalid E2 scoreboard"):
        _release_bundle(workspace, board, tmp_path / "warn-dist")


def test_rows_never_read_board_keys_the_perf_authority_does_not_validate(
    workspace: dict[str, Any], tmp_path: Path
) -> None:
    benchmark = pa.CANONICAL_PERF_BENCHMARKS[0]
    board = _board(
        tmp_path / "open" / "scoreboard.json",
        extra={
            "cells": [
                {"benchmark": benchmark, "verdict": pa.perf_schema.VERDICT_BUILD_FAILED}
            ],
            "command": pa.CANONICAL_GATE,
            "observed_at": "2026-01-01T00:00:00Z",
            "toolchain": {"rustc": "1.96.1"},
            "toolchain_digest": "0" * 64,
        },
    )
    bundle, bundle_report = _release_bundle(workspace, board, tmp_path / "open-dist")
    assert bundle_report.passed, bundle_report.problems
    rows = {
        row["requirement_id"]: row
        for row in pem.project_evidence(bundle, _requirements(workspace, "H0"))
    }
    assert rows[E2_REQUIREMENT] == {
        "requirement_id": E2_REQUIREMENT,
        "authority": "tools/perf_authority.py",
        "command": None,
        "artifact_sha256": hashlib.sha256(board.read_bytes()).hexdigest(),
        "matrix_cells": [
            "perf:native:llvm:release-fast",
            "perf:native:native:release-fast",
        ],
        "status": pem.STATUS_FAIL,
        "toolchain_digest": None,
        "observed_at": _load(board)["generated_at"],
    }


def test_verified_subset_toolchain_digest_binds_tools_not_run_custody(
    workspace: dict[str, Any],
) -> None:
    root = workspace["root"]
    validation = vs.validate_manifest(repo_root=root)
    coordinate = next(cell for cell in validation.coordinates if cell.backend == "wasm")
    role = reg.verified_subset_evidence_role(coordinate.id)

    def toolchain(**identity: str) -> str | None:
        receipt = _verified_subset_receipt(
            root, validation, coordinate, root.parent / "receipt.json", **identity
        )
        return pem._receipt_facts(role, receipt).toolchain_digest

    baseline = toolchain()
    assert baseline is not None and SHA256.fullmatch(baseline)
    assert toolchain(run_id="43", run_attempt="2") == baseline
    for identity in ("python_sha256", "rustc_sha256", "node_sha256"):
        assert toolchain(**{identity: "9" * 64}) != baseline, identity


def test_rows_read_only_the_schema_their_role_names(
    producer_inputs: dict[str, Any],
) -> None:
    structural = next(
        path for path in producer_inputs["e4"] if path.stem == rcr.KIND_STRUCTURAL_AUDIT
    )
    envelopes = {
        "e1_native": _load(producer_inputs["e1"][0]),
        "e2_scoreboard": _load(producer_inputs["board"]),
        "e4_structural_audit": _load(structural),
    }
    for role, payload in envelopes.items():
        assert pem._receipt_facts(role, payload).status == (
            pem.STATUS_FAIL if role == "e2_scoreboard" else pem.STATUS_PASS
        )
        for other in envelopes.keys() - {role}:
            assert pem._receipt_facts(other, payload) == pem._UNRECOGNIZED, other


def test_signable_phase_over_real_receipts_binds_every_clause(
    workspace: dict[str, Any],
) -> None:
    output, report = _assemble(workspace)
    assert report.green, report.problems
    manifest = _load(output)
    assert set(manifest) == pem._MANIFEST_KEYS
    assert manifest["matrix_digest"] == canonical_json_sha256(pem.generated_matrix())
    assert manifest["open_obligations"] == []
    assert manifest["legacy_count"] == 0
    requirements = sorted(_requirements(workspace, SIGNABLE), key=lambda r: r.id)
    assert [row["requirement_id"] for row in manifest["evidence"]] == [
        requirement.id for requirement in requirements
    ]
    for requirement, row in zip(requirements, manifest["evidence"], strict=True):
        artifact = _evidence(workspace, requirement.evidence_role)
        receipt = _load(artifact)
        assert set(row) == pem._EVIDENCE_KEYS
        assert (row["authority"], row["status"]) == (
            "tools/verified_subset.py",
            pem.STATUS_PASS,
        )
        assert row["matrix_cells"] == [
            f"verified-subset:{receipt['facts']['coordinate']['id']}"
        ]
        assert row["matrix_cells"] == list(requirement.matrix_cells)
        assert (
            row["artifact_sha256"] == hashlib.sha256(artifact.read_bytes()).hexdigest()
        )
        assert shlex.split(row["command"]) == receipt["producer"]["argv"]
        assert row["observed_at"] == receipt["generated_at"]
        assert SHA256.fullmatch(row["toolchain_digest"])
    assert _verify(workspace).green


def test_unsigned_manifest_is_false_only_on_its_signature(
    workspace: dict[str, Any],
) -> None:
    _, report = _assemble(workspace, attest=False)
    assert report.problems == ("signature: manifest carries no signed attestation",)


def test_attestation_must_bind_these_manifest_bytes(workspace: dict[str, Any]) -> None:
    output, _ = _assemble(workspace)
    _rewrite(output, lambda m: m.__setitem__("legacy_count", 0))
    assert _verify(workspace).green
    # Any byte change under the signature slot breaks the binding.
    _rewrite(output, lambda m: m["evidence"][0].__setitem__("observed_at", "later"))
    report = _verify(workspace)
    assert any("does not bind these manifest bytes" in p for p in report.problems)


def test_commit_must_match_release_commit(workspace: dict[str, Any]) -> None:
    _assemble(workspace)
    report = _verify(workspace, commit="c" * 40)
    assert not report.green
    assert any(p.startswith("commit:") for p in report.problems)


def test_failing_release_bundle_keeps_a_phase_false_even_when_its_rows_pass(
    workspace: dict[str, Any], tmp_path: Path
) -> None:
    failing = rcr.KIND_DEGRADE_TO_SLOW_GATE
    e4 = [
        _structural_receipt(
            workspace["root"],
            kind,
            tmp_path / "failing e4" / f"{kind}.json",
            errors=("unregistered slow path",) if kind == failing else (),
        )
        for kind in sorted(E4_INPUTS)
    ]
    bundle, bundle_report = _release_bundle(
        workspace, workspace["board"], tmp_path / "failing-dist", e4=e4
    )
    assert (bundle_report.status, bundle_report.problems) == (reg.STATUS_FAIL, ())
    workspace["bundle"] = bundle
    output, report = _assemble(workspace)
    assert {row["status"] for row in _load(output)["evidence"]} == {pem.STATUS_PASS}
    assert report.problems == (
        "hashes: release-exit bundle does not verify: status is 'FAIL'",
    )


def test_missing_and_duplicate_rows_are_false(workspace: dict[str, Any]) -> None:
    output, _ = _assemble(workspace)
    first = _load(output)["evidence"][0]["requirement_id"]
    _rewrite(output, lambda m: m["evidence"].append(dict(m["evidence"][0])))
    report = _verify(workspace)
    assert (
        f"evidence: {first} needs exactly one evidence row, found 2" in report.problems
    )
    _rewrite(
        output,
        lambda m: m.__setitem__(
            "evidence", [r for r in m["evidence"] if r["requirement_id"] != first]
        ),
    )
    report = _verify(workspace)
    assert (
        f"evidence: {first} needs exactly one evidence row, found 0" in report.problems
    )


def test_matrix_digest_drift_is_false(workspace: dict[str, Any]) -> None:
    output, _ = _assemble(workspace)
    _resign(output, lambda m: m.__setitem__("matrix_digest", "0" * 64))
    assert _verify(workspace).problems == (
        "matrix: manifest matrix_digest does not match the generated "
        "verified-subset matrix",
    )


def test_registered_legacy_lane_is_false_and_stale_counts_are_named(
    workspace: dict[str, Any],
) -> None:
    _assemble(workspace)
    root = workspace["root"]
    (root / "legacy_lane.py").write_text("LEGACY = True\n", encoding="utf-8")
    (root / "config" / "legacy_inventory.toml").write_text(
        f'schema = "{li.SCHEMA}"\n\n'
        "[[item]]\n"
        'id = "fixture-legacy-lane"\n'
        'path = "legacy_lane.py"\n'
        'superseded_by = "tools/phase_exit_manifest.py"\n'
        'removal_release = "1.0.0"\n'
        'reason = "a registered legacy lane that still exists"\n',
        encoding="utf-8",
        newline="\n",
    )
    assert _verify(workspace).problems == (
        "legacy: manifest legacy_count 0 is stale; inventory reports 1",
        "legacy: legacy_count is 1, not 0",
    )


def test_bundle_evidence_byte_drift_is_false(workspace: dict[str, Any]) -> None:
    _assemble(workspace)
    requirement = _requirements(workspace, SIGNABLE)[0]
    artifact = _evidence(workspace, requirement.evidence_role)
    artifact.write_text(json.dumps(_load(artifact), indent=4), encoding="utf-8")
    problems = _verify(workspace).problems
    assert any(
        problem.startswith("hashes: release-exit bundle does not verify:")
        and "checksum mismatch" in problem
        for problem in problems
    )
    assert (
        f"hashes: {requirement.id} artifact_sha256 does not match the bundle "
        "evidence bytes" in problems
    )
    assert "evidence: rows do not match the current bundle projection" in problems


def test_signing_subject_bytes_have_one_canonical_identity(
    workspace: dict[str, Any],
) -> None:
    output, report = _assemble(workspace)
    assert report.green
    manifest = _load(output)
    unsigned = {**manifest, "signed_attestation": None}
    expected = canonical_json_bytes(unsigned)
    assert pem.signing_subject_bytes(manifest) == expected
    assert pem.signing_subject_bytes(unsigned) == expected
    assert pem._manifest_bytes_sha256(manifest) == hashlib.sha256(expected).hexdigest()
    assert manifest["signed_attestation"] is not None


def test_prepare_publishes_exact_subject_but_never_waives_signature(
    workspace: dict[str, Any],
) -> None:
    subject = _prepare(workspace)
    payload = _load(subject)
    assert subject.read_bytes() == pem.signing_subject_bytes(payload)
    assert not subject.read_bytes().endswith(b"\n")
    assert payload["signed_attestation"] is None
    report = pem.verify_phase_manifest(
        subject,
        release_commit=SOURCE_SHA,
        bundle_manifest=workspace["bundle"],
        root=workspace["root"],
    )
    assert not report.green
    assert report.problems == ("signature: manifest carries no signed attestation",)


def test_prepare_never_signs_h0_while_producers_omit_row_fields(
    workspace: dict[str, Any],
) -> None:
    with pytest.raises(
        ValueError,
        match=r"not ready: .*\(H0\.pact\.kernel_a\.native\) field toolchain_digest is missing",
    ):
        _prepare(workspace, phase="H0")
    assert not (workspace["phase_dir"] / "H0.subject.json").exists()


def test_prepare_requires_the_bundle_source_commit(workspace: dict[str, Any]) -> None:
    with pytest.raises(ValueError, match="source_sha is not the release commit"):
        _prepare(workspace, commit="c" * 40)
    assert not (workspace["phase_dir"] / f"{SIGNABLE}.subject.json").exists()


def test_seal_uses_existing_subject_without_reprojection(
    workspace: dict[str, Any], monkeypatch: pytest.MonkeyPatch
) -> None:
    subject = _prepare(workspace)
    raw = subject.read_bytes()
    attestation = _attest(subject)

    def forbidden_projection(**_kwargs):
        pytest.fail(
            "seal attempted to replace the signed subject with a new projection"
        )

    monkeypatch.setattr(pem, "_project_phase_manifest", forbidden_projection)
    output, report = _seal(workspace, subject, attestation)
    assert report.green, report.problems
    assert pem.signing_subject_bytes(_load(output)) == raw
    assert subject.read_bytes() == raw


def test_seal_refuses_evidence_changed_after_prepare(
    workspace: dict[str, Any],
) -> None:
    subject = _prepare(workspace)
    attestation = _attest(subject)
    requirement = _requirements(workspace, SIGNABLE)[0]
    artifact = _evidence(workspace, requirement.evidence_role)
    artifact.write_text(json.dumps(_load(artifact), indent=4), encoding="utf-8")
    with pytest.raises(
        ValueError, match="bundle evidence bytes|current bundle projection"
    ):
        _seal(workspace, subject, attestation)
    assert not (workspace["phase_dir"] / f"{SIGNABLE}.json").exists()
    assert not list(workspace["phase_dir"].glob(".molt-phase-seal-*.tmp"))


@pytest.mark.parametrize(
    "field,forge",
    [
        ("command", lambda value: value + " --forged"),
        ("observed_at", lambda _value: "2026-09-24T00:00:00Z"),
        ("toolchain_digest", lambda _value: "0" * 64),
        ("matrix_cells", lambda value: [*value, "verified-subset:invented"]),
    ],
)
def test_even_resigned_rows_must_match_all_current_receipt_fields(
    workspace: dict[str, Any], field: str, forge: Callable[[Any], Any]
) -> None:
    subject = _prepare(workspace)
    payload = _load(subject)
    row = payload["evidence"][0]
    row[field] = forge(row[field])
    subject.write_bytes(pem.signing_subject_bytes(payload))
    attestation = _attest(subject)
    with pytest.raises(ValueError, match="current bundle projection"):
        _seal(workspace, subject, attestation)
    assert not (workspace["phase_dir"] / f"{SIGNABLE}.json").exists()


def test_seal_refuses_noncanonical_unsigned_bytes(workspace: dict[str, Any]) -> None:
    subject = _prepare(workspace)
    attestation = _attest(subject)
    subject.write_bytes(subject.read_bytes() + b"\n")
    with pytest.raises(ValueError, match="canonical unsigned bytes"):
        _seal(workspace, subject, attestation)
    assert not (workspace["phase_dir"] / f"{SIGNABLE}.json").exists()


def test_seal_refuses_an_already_attached_subject(workspace: dict[str, Any]) -> None:
    output, report = _assemble(workspace)
    assert report.green
    subject = output.with_name("already-signed.json")
    subject.write_bytes(canonical_json_bytes(_load(output)))
    with pytest.raises(ValueError, match="canonical unsigned bytes"):
        _seal(workspace, subject, output.with_name(f"{output.stem}.sigstore.json"))


def test_seal_requires_adjacent_attestation(workspace: dict[str, Any]) -> None:
    subject = _prepare(workspace)
    attestation = _attest(subject)
    foreign = workspace["phase_dir"].parent / attestation.name
    attestation.rename(foreign)
    with pytest.raises(ValueError, match="adjacent"):
        _seal(workspace, subject, foreign)
    assert not (workspace["phase_dir"] / f"{SIGNABLE}.json").exists()


def test_verify_rejects_non_adjacent_signature_path(workspace: dict[str, Any]) -> None:
    output, _ = _assemble(workspace)
    _rewrite(
        output,
        lambda manifest: manifest["signed_attestation"].__setitem__(
            "path", f"../{output.stem}.sigstore.json"
        ),
    )
    report = _verify(workspace)
    assert not report.green
    assert any(
        "signature: attestation is unreadable" in problem for problem in report.problems
    )


def test_seal_refuses_signature_of_another_subject(workspace: dict[str, Any]) -> None:
    subject = _prepare(workspace)
    other = subject.with_name("other.json")
    other.write_bytes(pem.signing_subject_bytes({**_load(subject), "commit": "c" * 40}))
    attestation = _attest(other)
    with pytest.raises(ValueError, match="does not bind these manifest bytes"):
        _seal(workspace, subject, attestation)
    assert not (workspace["phase_dir"] / f"{SIGNABLE}.json").exists()


def test_seal_does_not_replace_existing_manifest(workspace: dict[str, Any]) -> None:
    subject = _prepare(workspace)
    attestation = _attest(subject)
    output = workspace["phase_dir"] / f"{SIGNABLE}.json"
    output.write_bytes(b"foreign output")
    with pytest.raises(FileExistsError):
        _seal(workspace, subject, attestation)
    assert output.read_bytes() == b"foreign output"


def test_prepare_and_seal_cli_route_through_the_same_authority(
    workspace: dict[str, Any],
    monkeypatch: pytest.MonkeyPatch,
    capsys: pytest.CaptureFixture[str],
) -> None:
    prepare = pem.prepare_phase_signing_subject
    seal = pem.seal_phase_manifest
    monkeypatch.setattr(
        pem,
        "prepare_phase_signing_subject",
        lambda **kwargs: prepare(**kwargs, root=workspace["root"]),
    )
    monkeypatch.setattr(
        pem,
        "seal_phase_manifest",
        lambda **kwargs: seal(**kwargs, root=workspace["root"]),
    )
    subject = workspace["phase_dir"] / f"{SIGNABLE}.subject.json"
    assert (
        pem.main(
            [
                "prepare",
                "--phase",
                SIGNABLE,
                "--commit",
                SOURCE_SHA,
                "--release-exit-manifest",
                str(workspace["bundle"]),
                "--output",
                str(subject),
            ]
        )
        == 0
    )
    assert "PREPARED (unsigned, not phase green)" in capsys.readouterr().out
    attestation = _attest(subject)
    assert (
        pem.main(
            [
                "seal",
                "--subject",
                str(subject),
                "--attestation",
                str(attestation),
                "--release-exit-manifest",
                str(workspace["bundle"]),
                "--output",
                str(workspace["phase_dir"] / f"{SIGNABLE}.json"),
            ]
        )
        == 0
    )
    assert f"{SIGNABLE} GREEN" in capsys.readouterr().out


def test_e4_projection_never_admits_an_unobserved_runtime(
    producer_inputs: dict[str, Any],
) -> None:
    structural = next(
        path for path in producer_inputs["e4"] if path.stem == rcr.KIND_STRUCTURAL_AUDIT
    )
    payload = _load(structural)
    payload["producer"]["audit_engine"]["runtime_closure"] = None
    facts = pem._receipt_facts("e4_structural_audit", payload)
    assert facts.status == pem.STATUS_FAIL
    assert facts.toolchain_digest is None


def test_green_perf_statistics_cannot_admit_unknown_used_toolchain(
    producer_inputs: dict[str, Any],
) -> None:
    payload = _load(producer_inputs["board"])
    cells = pa.perf_schema.flatten_cells(payload)
    assert cells and all(not pa.release_cell_problems(cell) for cell in cells)
    assert pa.canonical_scoreboard_shape_problems(payload) == []
    for cell in cells:
        observation = cell["build_observation"]
        assert observation["kind"] == "molt-build-observation-v1"
        assert observation["selected_profiles"] == {
            "backend": cell["backend"],
            "guest_profile": "release",
            "compiler_profile": "release",
            "runtime_profile": "release-fast",
            "target": "native",
        }
        assert observation["compiled_with_verified"] is False
        assert observation["compiler"] is None
        assert observation["runtime"] is None
        artifact = Path(observation["artifact"]["path"])
        assert (
            observation["artifact"]["identity"]["sha256"]
            == hashlib.sha256(artifact.read_bytes()).hexdigest()
        )
    assert any(
        "used-byte admission receipt is unavailable" in problem
        for problem in pa.scoreboard_observed_toolchain_problems(payload)
    )
    facts = pem._receipt_facts("e2_scoreboard", payload)
    assert facts.status == pem.STATUS_FAIL
    assert facts.toolchain_digest is None


def test_fixture_release_admission_is_scoped_away_from_phase_toolchain_checks(
    workspace: dict[str, Any],
    tmp_path: Path,
) -> None:
    payload = _load(workspace["board"])
    validator = pa.scoreboard_observed_toolchain_problems
    before = validator(payload)
    assert before
    assert any(
        "used-byte admission receipt is unavailable" in problem for problem in before
    )
    _bundle, synthetic_report = _release_bundle(
        workspace, workspace["board"], tmp_path / "isolated-structural-fixture"
    )
    assert synthetic_report.passed
    assert pa.scoreboard_observed_toolchain_problems is validator
    assert validator(payload) == before
    assert pem._receipt_facts("e2_scoreboard", payload).status == pem.STATUS_FAIL
    assert any(
        "used-byte admission receipt is unavailable" in problem
        for problem in _REAL_RELEASE_SCOREBOARD_PROBLEMS(
            payload, expected_source_sha=SOURCE_SHA
        )
    )


@pytest.mark.parametrize("backend", ["native", "llvm"])
@pytest.mark.parametrize(
    "field", [None, "guest_profile", "compiler_profile", "runtime_profile", "target"]
)
def test_fixture_release_admission_still_rejects_unbound_profiles(
    workspace: dict[str, Any], tmp_path: Path, backend: str, field: str | None
) -> None:
    payload = _load(workspace["board"])
    assert pa.canonical_scoreboard_shape_problems(payload) == []
    cell = next(
        cell
        for cell in pa.perf_schema.flatten_cells(payload)
        if cell["backend"] == backend
    )
    observation = cell["build_observation"]
    if field is None:
        observation.pop("selected_profiles")
        diagnostic = "missing selected-profile observation"
    else:
        observation["selected_profiles"][field] = "unselected-coordinate"
        diagnostic = f"selected {field}:"
    board = tmp_path / "unbound-scoreboard.json"
    write_exact(board, payload, exclusive=True)
    with pytest.raises(ValueError, match="invalid E2 scoreboard") as rejected:
        _release_bundle(workspace, board, tmp_path / "unbound-dist")
    assert diagnostic in str(rejected.value)


def test_phase_input_allocation_does_not_reserve_policy_headroom(tmp_path):
    path = tmp_path / "small-phase-input.json"
    raw = b'{"phase":"C0"}'
    path.write_bytes(raw)
    measurements = run_isolated_python_probe(
        """
        import gc
        import json
        from pathlib import Path
        import sys
        import tracemalloc
        from tools import phase_exit_manifest as pem

        path = Path(sys.argv[1])
        measurements = []
        if tracemalloc.is_tracing():
            raise RuntimeError("probe requires exclusive allocation tracing")
        for allowance in (1024 * 1024, 8 * 1024 * 1024):
            pem._MAX_JSON_BYTES = allowance
            gc.collect()
            tracemalloc.start()
            try:
                raw = pem._read_bytes(path, label="phase fixture")
                _, peak = tracemalloc.get_traced_memory()
            finally:
                tracemalloc.stop()
            measurements.append({"raw_hex": raw.hex(), "peak": peak})
        print(json.dumps(measurements))
        """,
        args=[path],
    )
    assert [bytes.fromhex(row["raw_hex"]) for row in measurements] == [raw] * 2
    peaks = [row["peak"] for row in measurements]
    # A seven-MiB increase in permission must not allocate that unused space.
    assert peaks[1] - peaks[0] < 256 * 1024


def test_open_finding_at_the_release_commit_is_an_open_obligation(
    workspace: dict[str, Any], tmp_path: Path
) -> None:
    (workspace["root"] / LEDGER).write_text(OPEN_LEDGER, encoding="utf-8", newline="\n")
    bundle, bundle_report = _release_bundle(
        workspace, workspace["board"], tmp_path / "open-dist"
    )
    assert bundle_report.problems == ()
    assert bundle_report.status == reg.STATUS_FAIL
    assert bundle_report.open_findings == ("HF-7",)
    held = {**workspace, "bundle": bundle}

    output, report = _assemble(held)

    assert _load(output)["open_obligations"] == ["HF-7"]
    assert not report.green
    assert "obligations: open obligations remain: HF-7" in report.problems
    with pytest.raises(ValueError, match="open obligations remain: HF-7"):
        _prepare(held)

    # Dropping the finding from a re-signed manifest cannot hide it.
    _resign(output, lambda manifest: manifest.update(open_obligations=[]))
    hidden = _verify(held)
    assert "obligations: open finding HF-7 is not listed as open" in hidden.problems


def test_phase_rereads_the_ledger_instead_of_the_recorded_join(
    workspace: dict[str, Any],
) -> None:
    output, report = _assemble(workspace)
    assert report.green, report.problems
    (workspace["root"] / LEDGER).write_text(OPEN_LEDGER, encoding="utf-8", newline="\n")

    reread = _verify(workspace)

    assert not reread.green
    assert "obligations: open finding HF-7 is not listed as open" in reread.problems
    assert any(
        "findings.open omits findings open at the source: HF-7" in problem
        for problem in reread.problems
    )
    assert _load(output)["open_obligations"] == []


def test_phase_obligations_must_not_use_finding_ids(tmp_path: Path) -> None:
    requirements = tmp_path / "phase_exit_requirements.toml"
    requirements.write_text(
        pem.REQUIREMENTS_PATH.read_text(encoding="utf-8").replace(
            'id = "KA"\n', 'id = "HF-3"\n'
        ),
        encoding="utf-8",
        newline="\n",
    )

    with pytest.raises(ValueError, match="obligation HF-3 uses a finding ID"):
        pem.load_phases(requirements)
