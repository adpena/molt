from __future__ import annotations

import json
import subprocess
from dataclasses import replace
from pathlib import Path

import pytest

from molt import verified_subset as authority
from tools import verified_subset
from tools.compat import comparison, test_policy


SOURCE_SHA = "1" * 40
SHA256 = "2" * 64


def _one_test_projection(
    coordinate: authority.VerifiedSubsetCoordinate,
    *,
    expected_failure: bool = False,
) -> test_policy.CoordinateProjection:
    test = test_policy.ProjectedTest(
        path="tests/differential/basic/arith.py",
        source_sha256=SHA256,
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


def _outcome(
    coordinate: authority.VerifiedSubsetCoordinate,
    *,
    expected_failure: bool = False,
) -> dict[str, object]:
    return {
        "backend": coordinate.backend,
        "backend_returncode": 1 if expected_failure else 0,
        "backend_status": "fail" if expected_failure else "pass",
        "backend_stderr_sha256": SHA256,
        "backend_stdout_sha256": SHA256,
        "comparison_law": comparison.COMPARISON_LAW_VERSION,
        "compiler_target_python": coordinate.python,
        "cpython_returncode": 0,
        "cpython_stderr_sha256": SHA256,
        "cpython_stdout_sha256": SHA256,
        "expect_molt_fail": expected_failure,
        "expected_failure_reason": "compiler_gap" if expected_failure else None,
        "path": "tests/differential/basic/arith.py",
        "raw_status": "fail" if expected_failure else "pass",
        "reason_tag": "xfail" if expected_failure else None,
        "resolved_status": "pass",
    }


def _raw_result_fixture(
    outcome: dict[str, object],
    *,
    compiler_target_python: str,
) -> tuple[dict[str, object], tuple[dict[str, object], dict[str, object]]]:
    summary = {
        "item_results": [
            {"path": outcome["path"], "status": outcome["resolved_status"]}
        ]
    }
    test_row = {
        "record_type": "test",
        "file": outcome["path"],
        "raw_status": outcome["raw_status"],
        "resolved_status": outcome["resolved_status"],
        "reason_tag": outcome["reason_tag"],
        "expect_molt_fail": outcome["expect_molt_fail"],
        "expected_failure_reason": outcome["expected_failure_reason"],
        "cpython_returncode": outcome["cpython_returncode"],
        "cpython_stdout_sha256": outcome["cpython_stdout_sha256"],
        "cpython_stderr_sha256": outcome["cpython_stderr_sha256"],
        "comparison_law": outcome["comparison_law"],
        "compiler_target_python": compiler_target_python,
    }
    backend_row = {
        "record_type": "backend",
        "file": outcome["path"],
        "backend": outcome["backend"],
        "raw_status": outcome["backend_status"],
        "expect_molt_fail": False,
        "returncode": outcome["backend_returncode"],
        "stdout_sha256": outcome["backend_stdout_sha256"],
        "stderr_sha256": outcome["backend_stderr_sha256"],
    }
    return summary, (test_row, backend_row)


def _execution(coordinate: authority.VerifiedSubsetCoordinate) -> dict[str, object]:
    backend: dict[str, object] = {"backend": coordinate.backend, "runner": "process"}
    if coordinate.backend == "wasm":
        backend = {
            "backend": "wasm",
            "binary_name": "node",
            "binary_sha256": SHA256,
            "runner": "node-wasi",
            "version": "v24.16.0",
        }
    runner_arch = "ARM64" if coordinate.arch in {"arm64", "aarch64"} else "X64"
    runner_os = {"linux": "Linux", "macos": "macOS", "windows": "Windows"}[
        coordinate.platform
    ]
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
            "runner_arch": runner_arch,
            "runner_label": coordinate.runner,
            "runner_os": runner_os,
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
            "executable_sha256": SHA256,
            "gil_disabled": False,
            "hexversion": 0,
            "implementation": "CPython",
            "pointer_bits": 64,
            "version": coordinate.reference_python,
            "version_info": version_info,
        },
        "rust": {
            "binary_name": "rustc",
            "binary_sha256": SHA256,
            "commit_date": "2026-01-01",
            "commit_hash": SHA256,
            "host": coordinate.rust_target,
            "llvm_version": "21.1.0",
            "release": "1.96.1",
        },
    }


def _write_receipts(root: Path, monkeypatch: pytest.MonkeyPatch) -> tuple[Path, ...]:
    policy = authority.load_verified_subset_policy()
    inputs = list(verified_subset.verified_subset_authority_files(policy))
    monkeypatch.setattr(
        verified_subset,
        "verified_subset_projection",
        lambda _policy, coordinate, **_kwargs: _one_test_projection(coordinate),
    )
    paths: list[Path] = []
    for coordinate in authority.verified_subset_coordinates(policy):
        projection = _one_test_projection(coordinate)
        results = [_outcome(coordinate)]
        path = root / f"{coordinate.id}.json"
        payload = verified_subset.release_receipt.build_receipt(
            kind=verified_subset.release_receipt.KIND_VERIFIED_SUBSET,
            source_sha=SOURCE_SHA,
            status=verified_subset.release_receipt.STATUS_PASS,
            argv=[
                "run",
                "--coordinate",
                coordinate.id,
                "--receipt",
                str(path),
                "--source-sha",
                SOURCE_SHA,
            ],
            tool_path=verified_subset.ROOT / "tools" / "verified_subset.py",
            facts=verified_subset._receipt_facts(
                coordinate=coordinate,
                policy=policy,
                projection=projection,
                results=results,
                execution=_execution(coordinate),
            ),
            input_paths=inputs,
            repo_root=verified_subset.ROOT,
            generated_at="2026-09-03T00:00:00Z",
        )
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_text(json.dumps(payload), encoding="utf-8")
        paths.append(path)
    return tuple(paths)


def test_policy_generates_exact_version_host_backend_closure() -> None:
    policy = authority.load_verified_subset_policy()
    coordinates = authority.verified_subset_coordinates(policy)

    assert policy.python_versions == ("3.12", "3.13", "3.14")
    assert policy.reference_cpython == ("3.12.13", "3.13.11", "3.14.3")
    assert policy.backends == ("native", "wasm")
    assert policy.abi == "cpython-language"
    assert policy.concurrency == "gil"
    assert len(coordinates) == 36
    assert {
        (coordinate.python, coordinate.platform, coordinate.arch, coordinate.backend)
        for coordinate in coordinates
    } == {
        (python, target["platform"], target["arch"], backend)
        for python in policy.python_versions
        for target in authority.RELEASE_TARGETS
        for backend in policy.backends
    }


def test_policy_file_does_not_redeclare_fixed_coordinate_authorities(
    tmp_path: Path,
) -> None:
    default = authority.load_verified_subset_policy()
    root = tmp_path / "repo"
    config = root / "config"
    suite = root / "suite"
    config.mkdir(parents=True)
    suite.mkdir()
    policy_path = config / "verified_subset.toml"
    reference_versions = ", ".join(
        json.dumps(version) for version in default.reference_cpython
    )
    document = (
        'schema = "molt.verified-subset.v1"\n'
        f"reference_cpython = [{reference_versions}]\n"
        'excluded_verification_scopes = ["capability_policy", '
        '"dynamic_execution_policy"]\n'
        "differential_suites = [\n"
        '  { cpython_equivalence_floor = 1, path = "suite", '
        "recursive = false },\n"
        "]\n"
    )
    policy_path.write_text(document, encoding="utf-8")

    loaded = authority.load_verified_subset_policy(policy_path)
    assert loaded.fallback_policy == "error"
    assert loaded.abi == "cpython-language"
    assert loaded.concurrency == "gil"
    assert loaded.backends == ("native", "wasm")

    policy_path.write_text(document + 'fallback_policy = "error"\n', encoding="utf-8")
    with pytest.raises(ValueError, match="schema or keys are not exact"):
        authority.load_verified_subset_policy(policy_path)


def test_policy_owns_one_deduplicated_basic_and_stdlib_source_closure() -> None:
    policy = authority.load_verified_subset_policy()
    files = verified_subset.verified_subset_test_files(policy)
    independently_expanded = tuple(
        sorted(
            path.resolve()
            for suite in policy.suites
            for path in (
                (verified_subset.ROOT / suite.path).rglob("*.py")
                if suite.recursive
                else (verified_subset.ROOT / suite.path).glob("*.py")
            )
        )
    )

    assert files == independently_expanded
    assert files == tuple(sorted(set(files)))
    sources = test_policy.load_test_sources(files, repo_root=verified_subset.ROOT)
    for suite in policy.suites:
        prefix = f"{suite.path}/"
        equivalence_count = sum(
            source.path.startswith(prefix)
            and source.metadata.verification_scope
            == test_policy.CPYTHON_EQUIVALENCE_SCOPE
            for source in sources
        )
        assert equivalence_count >= suite.cpython_equivalence_floor


def test_physical_suite_collection_ignores_generated_lane_manifests(
    tmp_path: Path,
) -> None:
    root = tmp_path / "repo"
    suite = root / "suite"
    nested = suite / "nested"
    nested.mkdir(parents=True)
    first = suite / "first.py"
    second = nested / "second.py"
    first.write_text("print('first')\n", encoding="utf-8")
    second.write_text("print('second')\n", encoding="utf-8")
    (suite / "TESTS.txt").write_text("suite/first.py\n", encoding="utf-8")

    assert test_policy.collect_physical_test_files(
        (("suite", False),), repo_root=root
    ) == (first.resolve(),)
    assert test_policy.collect_physical_test_files(
        (("suite", True),), repo_root=root
    ) == (first.resolve(), second.resolve())


def test_physical_suite_collection_rejects_link_like_entries(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    root = tmp_path / "repo"
    suite = root / "suite"
    suite.mkdir(parents=True)
    linked = suite / "linked.py"
    linked.write_text("print('linked')\n", encoding="utf-8")
    real_is_link_like = test_policy.is_link_like
    monkeypatch.setattr(
        test_policy,
        "is_link_like",
        lambda path: Path(path) == linked or real_is_link_like(Path(path)),
    )

    with pytest.raises(ValueError, match="link or reparse point"):
        test_policy.collect_physical_test_files((("suite", False),), repo_root=root)


def test_physical_suite_collection_rejects_portable_identity_collisions(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    root = tmp_path / "repo"
    suite = root / "suite"
    suite.mkdir(parents=True)
    (suite / "first.py").write_text("print('first')\n", encoding="utf-8")
    (suite / "second.py").write_text("print('second')\n", encoding="utf-8")
    monkeypatch.setattr(test_policy, "portable_path_identity", lambda _path: "same")

    with pytest.raises(ValueError, match="collide on portable filesystems"):
        test_policy.collect_physical_test_files((("suite", False),), repo_root=root)


def test_suite_equivalence_floor_fails_closed_on_evidence_contraction() -> None:
    policy = authority.load_verified_subset_policy()
    suites = (
        replace(
            policy.suites[0],
            cpython_equivalence_floor=(policy.suites[0].cpython_equivalence_floor + 1),
        ),
        *policy.suites[1:],
    )

    with pytest.raises(ValueError, match="contracted below its CPython-equivalence"):
        verified_subset.validate_suite_equivalence_floors(
            replace(policy, suites=suites)
        )


def test_coordinate_projection_excludes_inapplicable_and_policy_scope_rows() -> None:
    policy = authority.load_verified_subset_policy()
    coordinate = next(
        item
        for item in authority.verified_subset_coordinates(policy)
        if item.id == "windows-x86_64-py312-cpython-language-gil-wasm"
    )
    projection = verified_subset.verified_subset_projection(policy, coordinate)

    assert len(projection.tests) == len(
        verified_subset.verified_subset_test_files(policy)
    )
    assert len(projection.applicable) + len(projection.excluded) == len(
        projection.tests
    )
    assert set(projection.expected_failures).issubset(projection.applicable)
    assert all(
        test.verification_scope == test_policy.CPYTHON_EQUIVALENCE_SCOPE
        for test in projection.applicable
    )
    assert any(
        test.exclusion_reason
        == "verified-subset scope exclusion: dynamic_execution_policy"
        for test in projection.excluded
    )


def test_excluded_scope_is_typed_and_distinct_from_conformance_debt(
    tmp_path: Path,
) -> None:
    root = tmp_path / "repo"
    source = root / "tests" / "differential" / "basic" / "capability.py"
    source.parent.mkdir(parents=True)
    source.write_text(
        "# MOLT_META: verified_subset_scope=capability_policy "
        "expect_fail=molt expect_fail_reason=requires_ffi\n",
        encoding="utf-8",
    )

    prepared = test_policy.load_test_sources((source,), repo_root=root)
    projection = test_policy.project_prepared_coordinate(
        prepared,
        python="3.12",
        platform="linux",
        arch="x86_64",
        backend="native",
        excluded_verification_scopes=frozenset({"capability_policy"}),
    )

    assert prepared[0].metadata.verification_scope == "capability_policy"
    assert projection.applicable == ()
    assert projection.expected_failures == ()
    assert projection.excluded[0].exclusion_reason == (
        "verified-subset scope exclusion: capability_policy"
    )


@pytest.mark.parametrize(
    "metadata, message",
    [
        (
            "verified_subset_scope=unknown expect_fail=molt",
            "must be one of",
        ),
        (
            "verified_subset_scope=capability_policy",
            "must remain an explicit expected divergence",
        ),
        (
            "verified_subset_scope=capability_policy,dynamic_execution_policy "
            "expect_fail=molt",
            "must select exactly one",
        ),
    ],
)
def test_verification_scope_metadata_fails_closed(
    tmp_path: Path, metadata: str, message: str
) -> None:
    root = tmp_path / "repo"
    source = root / "case.py"
    source.parent.mkdir(parents=True)
    source.write_text(f"# MOLT_META: {metadata}\n", encoding="utf-8")

    with pytest.raises(ValueError, match=message):
        test_policy.load_test_sources((source,), repo_root=root)


def test_projection_refuses_excluding_cpython_equivalence_scope() -> None:
    coordinate = authority.verified_subset_coordinates()[0]
    with pytest.raises(ValueError, match="cannot be excluded"):
        test_policy.project_prepared_coordinate(
            (),
            python=coordinate.python,
            platform=coordinate.platform,
            arch=coordinate.arch,
            backend=coordinate.backend,
            excluded_verification_scopes=frozenset(
                {test_policy.CPYTHON_EQUIVALENCE_SCOPE}
            ),
        )


def test_run_differential_suites_uses_projected_file_schedule(
    monkeypatch: pytest.MonkeyPatch,
    tmp_path: Path,
) -> None:
    policy = authority.load_verified_subset_policy()
    coordinate = next(
        item
        for item in authority.verified_subset_coordinates(policy)
        if item.python == "3.14" and item.backend == "wasm"
    )
    projection = _one_test_projection(coordinate)
    captured: dict[str, object] = {}
    monkeypatch.setenv("MOLT_DIFF_TRUSTED", "1")
    monkeypatch.setenv("MOLT_TRUSTED", "1")
    monkeypatch.setenv("MOLT_DIFF_CAPABILITIES", "all")
    monkeypatch.setenv("MOLT_CAPABILITIES", "all")
    monkeypatch.setattr(
        verified_subset, "require_current_host", lambda _coordinate: None
    )

    def fake_guarded_completed_process(cmd, **kwargs):
        captured["cmd"] = cmd
        captured["kwargs"] = kwargs
        return subprocess.CompletedProcess(cmd, 0, stdout=None, stderr=None)

    monkeypatch.setattr(
        verified_subset.harness_memory_guard,
        "guarded_completed_process",
        fake_guarded_completed_process,
    )
    summary = tmp_path / "summary.json"
    results = tmp_path / "results.jsonl"
    schedule = tmp_path / "schedule.txt"
    verified_subset.run_differential_suites(
        coordinate,
        projection=projection,
        results_path=results,
        schedule_path=schedule,
        summary_path=summary,
    )

    assert schedule.read_text(encoding="utf-8") == (
        "tests/differential/basic/arith.py\n"
    )
    cmd = captured["cmd"]
    assert cmd[-2:] == ["--files-from", str(schedule)]
    assert cmd[cmd.index("--python-version") + 1] == coordinate.reference_python
    assert cmd[cmd.index("--molt-target-python") + 1] == coordinate.python
    assert coordinate.reference_python != coordinate.python
    env = captured["kwargs"]["env"]
    assert env["MOLT_DIFF_RESULTS_JSONL"] == str(results)
    assert env["MOLT_DIFF_PYTHON"] == verified_subset.sys.executable
    assert env["MOLT_DIFF_TRUSTED"] == "0"
    assert env["MOLT_TRUSTED"] == "0"
    assert "MOLT_DIFF_CAPABILITIES" not in env
    assert "MOLT_CAPABILITIES" not in env


def test_applicable_expected_failure_can_never_pass_release_conformance() -> None:
    policy = authority.load_verified_subset_policy()
    coordinate = authority.verified_subset_coordinates(policy)[0]
    projection = _one_test_projection(coordinate, expected_failure=True)
    results = [_outcome(coordinate, expected_failure=True)]

    assert not verified_subset.outcomes_pass(results)
    assert projection.closure_record()["expected_failures"] == 1


def test_raw_result_rows_are_joined_with_summary_and_backend(
    tmp_path: Path,
) -> None:
    policy = authority.load_verified_subset_policy()
    coordinate = authority.verified_subset_coordinates(policy)[0]
    projection = _one_test_projection(coordinate)
    outcome = _outcome(coordinate)
    summary, rows = _raw_result_fixture(
        outcome,
        compiler_target_python=coordinate.python,
    )
    results_path = tmp_path / "results.jsonl"
    results_path.write_text(
        "".join(f"{json.dumps(row)}\n" for row in rows),
        encoding="utf-8",
    )

    assert verified_subset._result_outcomes(
        summary=summary,
        results_path=results_path,
        projection=projection,
        coordinate=coordinate,
    ) == [outcome]


def test_raw_result_rows_reject_compiler_target_coordinate_mismatch(
    tmp_path: Path,
) -> None:
    coordinate = authority.verified_subset_coordinates()[0]
    projection = _one_test_projection(coordinate)
    outcome = _outcome(coordinate)
    summary, rows = _raw_result_fixture(
        outcome,
        compiler_target_python="3.99",
    )
    results_path = tmp_path / "results.jsonl"
    results_path.write_text(
        "".join(f"{json.dumps(row)}\n" for row in rows),
        encoding="utf-8",
    )

    with pytest.raises(ValueError, match="compiler target Python drifted"):
        verified_subset._result_outcomes(
            summary=summary,
            results_path=results_path,
            projection=projection,
            coordinate=coordinate,
        )


def test_verify_receipts_requires_every_passing_coordinate_once(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    _write_receipts(tmp_path, monkeypatch)
    verified_subset.verify_receipt_closure(
        receipt_root=tmp_path,
        source_sha=SOURCE_SHA,
    )


def test_verify_receipts_byte_verifies_common_inputs_once(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    paths = _write_receipts(tmp_path, monkeypatch)
    original = verified_subset.release_receipt.validate_receipt
    verify_inputs_calls: list[bool] = []

    def recording_validate_receipt(*args, **kwargs):
        verify_inputs_calls.append(kwargs["verify_inputs"])
        return original(*args, **kwargs)

    monkeypatch.setattr(
        verified_subset.release_receipt,
        "validate_receipt",
        recording_validate_receipt,
    )

    verified_subset.verify_receipt_closure(
        receipt_root=tmp_path,
        source_sha=SOURCE_SHA,
    )

    assert len(verify_inputs_calls) == len(paths)
    assert verify_inputs_calls == [True, *([False] * (len(paths) - 1))]


@pytest.mark.parametrize(
    ("field", "replacement"),
    [("sha256", "3" * 64), ("size", 0), ("path", "tools/other.py")],
)
def test_verify_receipts_rejects_late_common_input_record_tamper(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
    field: str,
    replacement: object,
) -> None:
    paths = _write_receipts(tmp_path, monkeypatch)
    late_path = sorted(paths)[1]
    payload = json.loads(late_path.read_text(encoding="utf-8"))
    payload["inputs"][0][field] = replacement
    late_path.write_text(json.dumps(payload), encoding="utf-8")

    with pytest.raises(ValueError, match="common input records differ"):
        verified_subset.verify_receipt_closure(
            receipt_root=tmp_path,
            source_sha=SOURCE_SHA,
        )


def test_explicit_projection_sources_bypass_process_cache(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    policy = authority.load_verified_subset_policy()
    coordinate = authority.verified_subset_coordinates(policy)[0]
    calls: list[tuple[test_policy.TestPolicySource, ...]] = []

    def project(sources, **_kwargs):
        calls.append(tuple(sources))
        return _one_test_projection(coordinate)

    monkeypatch.setattr(test_policy, "project_prepared_coordinate", project)
    monkeypatch.setattr(verified_subset, "_verified_subset_test_sources", lambda _p: ())
    verified_subset._cached_verified_subset_projection.cache_clear()

    verified_subset.verified_subset_projection(policy, coordinate)
    verified_subset.verified_subset_projection(policy, coordinate)
    verified_subset.verified_subset_projection(policy, coordinate, sources=())
    verified_subset.verified_subset_projection(policy, coordinate, sources=())

    assert calls == [(), (), ()]
    verified_subset._cached_verified_subset_projection.cache_clear()


def test_verify_receipts_rejects_duplicate_coordinate(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    paths = _write_receipts(tmp_path, monkeypatch)
    paths[-1].write_bytes(paths[0].read_bytes())
    with pytest.raises(ValueError, match="duplicated"):
        verified_subset.verify_receipt_closure(
            receipt_root=tmp_path,
            source_sha=SOURCE_SHA,
        )


def test_verify_receipts_rejects_failed_coordinate(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    paths = _write_receipts(tmp_path, monkeypatch)
    payload = json.loads(paths[0].read_text(encoding="utf-8"))
    payload["status"] = verified_subset.release_receipt.STATUS_FAIL
    paths[0].write_text(json.dumps(payload), encoding="utf-8")
    with pytest.raises(ValueError, match="receipt status must be PASS"):
        verified_subset.verify_receipt_closure(
            receipt_root=tmp_path,
            source_sha=SOURCE_SHA,
        )
