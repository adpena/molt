"""Hermetic coverage joins; these fixtures are not real performance evidence."""

import copy
from pathlib import Path

import pytest

from tools import perf_schema
from tools.release_matrix_acceptance import (
    ReleaseMatrix,
    SHARD_SCHEMA,
    required_matrix,
    shard_projection,
    performance_matrix_problems,
    full_release_problems,
)

ROOT = Path(__file__).resolve().parents[2]


def fixture_matrix():
    cells = tuple(
        {
            "id": f"perf:fixture:{n}",
            "python": "3.14",
            "reference_python": "3.14.3",
            "platform": "windows",
            "arch": "arm64",
            "rust_target": "aarch64-pc-windows-msvc",
            "backend": "wasm",
            "guest_profile": "release",
            "runtime_profile": "wasm-release",
            "compiler_profile": "release",
            "benchmark": f"fixture_{n}.py",
        }
        for n in range(6)
    )
    return ReleaseMatrix("a" * 40, "b" * 64, ("semantic:fixture",), cells, ())


def winning_cell():
    return dict(
        build_ok=True,
        molt_ok=True,
        cpython_ok=True,
        stable=True,
        measured_quiescent=True,
        verdict="GREEN",
        classification="GREEN_STABLE",
        repeat_stability="STABLE_ABOVE",
        repeat_passes=5,
        repeat_ci_lo=1.01,
        repeat_ci_hi=1.2,
        build_observation={
            "compiled_with_verified": False,
            "selected_profiles": {
                "guest_profile": "release",
                "runtime_profile": "wasm-release",
                "compiler_profile": "release",
                "target": "wasm",
            },
        },
        output_parity=perf_schema.output_parity_evidence(
            reference_observations=[("cpython", "result", "", 0)],
            molt_observations=[("molt", "result", "", 0)],
        ),
    )


def fixture_receipts(matrix, count=3):
    toolchains = {cell["id"]: "c" * 64 for cell in matrix.performance_cells}
    shards = []
    for index in range(count):
        shards.append(
            dict(
                schema=SHARD_SCHEMA,
                matrix_sha256=matrix.identity,
                source_sha=matrix.source_sha,
                authority_sha256=matrix.authority_sha256,
                index=index,
                count=count,
                cells=[
                    dict(
                        coordinate=cell,
                        toolchain_sha256=toolchains[cell["id"]],
                        observed_profiles={
                            k: cell[k]
                            for k in (
                                "guest_profile",
                                "runtime_profile",
                                "compiler_profile",
                            )
                        },
                        measurement=winning_cell(),
                    )
                    for cell in shard_projection(matrix, index=index, count=count)
                ],
            )
        )
    return shards, toolchains


def test_declared_matrix_preserves_semantic_floor_and_all_primary_workloads():
    from tools import bench_suites
    from molt.verified_subset import verified_subset_coordinates

    matrix = required_matrix(source_sha="a" * 40, root=ROOT)
    assert set(matrix.semantic_ids) == {c.id for c in verified_subset_coordinates()}
    assert len(matrix.semantic_ids) == 72
    cells = matrix.performance_cells
    assert {c["python"] for c in cells} == {"3.12", "3.13", "3.14"}
    assert len({(c["platform"], c["arch"]) for c in cells}) == 6
    assert {c["backend"] for c in cells} == {"native", "llvm", "wasm"}
    assert {c["guest_profile"] for c in cells} == {"dev", "release"}
    assert {c["benchmark"] for c in cells} == set(
        (
            *bench_suites.BENCHMARKS,
            *bench_suites.WS_BENCHMARKS,
            *bench_suites.DYNAMIC_BUILTIN_SLICES,
        )
    )
    assert len(cells) == 3 * 6 * 11 * len({c["benchmark"] for c in cells})
    assert matrix.blockers  # No unit-fixture result can claim release readiness.
    assert any("semantic backend llvm" in p for p in matrix.blockers)


def test_parallel_projection_is_disjoint_complete_and_order_independent():
    matrix = fixture_matrix()
    shards, tools = fixture_receipts(matrix)
    ids = [
        cell["id"]
        for n in range(3)
        for cell in shard_projection(matrix, index=n, count=3)
    ]
    assert len(ids) == len(set(ids)) == len(matrix.performance_cells)
    assert (
        performance_matrix_problems(
            matrix, list(reversed(shards)), toolchain_identities=tools
        )
        == []
    )


@pytest.mark.parametrize(
    "field,value",
    [
        ("source_sha", "d" * 40),
        ("authority_sha256", "d" * 64),
        ("matrix_sha256", "d" * 64),
        ("index", True),
        ("count", 0),
    ],
)
def test_wrong_source_authority_or_shard_coordinate_fails(field, value):
    matrix = fixture_matrix()
    shards, tools = fixture_receipts(matrix)
    shards[0][field] = value
    assert performance_matrix_problems(matrix, shards, toolchain_identities=tools)


@pytest.mark.parametrize(
    "defect",
    [
        "missing",
        "duplicate",
        "profile",
        "toolchain",
        "unknown",
        "malformed",
        "xfail",
        "noisy",
        "slower",
        "nan",
        "parity",
    ],
)
def test_each_missing_or_invalid_individual_measurement_blocks(defect):
    matrix = fixture_matrix()
    shards, tools = fixture_receipts(matrix, count=1)
    row = shards[0]["cells"][0]
    if defect == "missing":
        shards[0]["cells"].pop()
    elif defect == "duplicate":
        shards[0]["cells"].append(copy.deepcopy(row))
    elif defect == "profile":
        row["observed_profiles"]["runtime_profile"] = "release-fast"
    elif defect == "toolchain":
        row["toolchain_sha256"] = "d" * 64
    elif defect == "unknown":
        row["coordinate"] = {**row["coordinate"], "python": "3.12"}
    elif defect == "malformed":
        row["coordinate"] = {"id": []}
    elif defect == "xfail":
        row["measurement"]["verdict"] = "XFAIL"
    elif defect == "noisy":
        row["measurement"]["repeat_stability"] = "STRADDLES"
    elif defect == "slower":
        row["measurement"]["repeat_ci_lo"] = 1.0
    elif defect == "nan":
        row["measurement"]["repeat_ci_lo"] = float("nan")
    elif defect == "parity":
        row["measurement"]["output_parity"]["ok"] = False
    assert performance_matrix_problems(matrix, shards, toolchain_identities=tools)


def test_unmeasured_and_empty_custody_cannot_count_as_pass():
    matrix = fixture_matrix()
    shards, _ = fixture_receipts(matrix)
    assert performance_matrix_problems(matrix, [], toolchain_identities={})
    assert performance_matrix_problems(matrix, shards, toolchain_identities={})


def test_full_join_requires_semantics_and_retains_other_acceptance_blockers(tmp_path):
    matrix = fixture_matrix()
    shards, tools = fixture_receipts(matrix)
    assert full_release_problems(
        matrix,
        shards,
        toolchain_identities=tools,
        semantic_bundle_manifest=tmp_path / "missing.json",
    )
    blocked = ReleaseMatrix(
        matrix.source_sha,
        matrix.authority_sha256,
        matrix.semantic_ids,
        matrix.performance_cells,
        ("resource budget missing",),
    )
    shards, tools = fixture_receipts(blocked)
    assert "resource budget missing" in full_release_problems(
        blocked,
        shards,
        toolchain_identities=tools,
        semantic_bundle_manifest=tmp_path / "missing.json",
    )


def test_mixed_or_duplicate_partitions_cannot_be_joined_as_one_run():
    matrix = fixture_matrix()
    shards, tools = fixture_receipts(matrix)
    assert performance_matrix_problems(
        matrix, [*shards, copy.deepcopy(shards[0])], toolchain_identities=tools
    )
    shards[-1]["count"] = 2
    assert performance_matrix_problems(matrix, shards, toolchain_identities=tools)


def test_profile_claims_and_experimental_exclusions_are_visible_not_runtime_wins():
    matrix = required_matrix(source_sha="a" * 40, root=ROOT)
    assert {cell["compiler_profile"] for cell in matrix.performance_cells} == {
        "release"
    }
    assert "release-output" in {
        cell["runtime_profile"] for cell in matrix.performance_cells
    }
    assert "release-size" in {
        cell["runtime_profile"] for cell in matrix.performance_cells
    }
    assert "wasm-release" in {
        cell["runtime_profile"] for cell in matrix.performance_cells
    }
    assert any(
        "metric advertised_ecosystem_workloads" in blocker
        for blocker in matrix.blockers
    )


def _source_repo(tmp_path):
    import subprocess

    root = tmp_path / "source"
    (root / "src").mkdir(parents=True)
    (root / "src/main.py").write_bytes(b"import dependency\n")
    (root / "src/dependency.py").write_bytes(b"VALUE = 1\n")

    def git(*args):
        return subprocess.run(
            ["git", "-C", str(root), *args], check=True, capture_output=True
        ).stdout

    git("init")
    git("config", "core.autocrlf", "false")
    git("add", "src/main.py", "src/dependency.py")
    git(
        "-c",
        "user.name=Matrix fixture",
        "-c",
        "user.email=matrix@fixture.invalid",
        "commit",
        "-m",
        "source fixture",
    )
    return root, git("rev-parse", "HEAD").decode().strip(), git


def test_source_admission_binds_all_tracked_bytes_without_import_reanalysis(tmp_path):
    from tools.release_matrix_acceptance import source_admission_problems

    root, sha, git = _source_repo(tmp_path)
    assert source_admission_problems(root=root, source_sha=sha) == []
    # A sibling that was never listed in the acceptance module still invalidates
    # source admission, even when an index hint would hide ordinary git diff.
    git("update-index", "--assume-unchanged", "src/dependency.py")
    (root / "src/dependency.py").write_bytes(b"VALUE = 2\n")
    assert source_admission_problems(root=root, source_sha=sha)


@pytest.mark.parametrize(
    "tracked,ignored,top_level",
    [
        (False, False, False),
        (True, False, False),
        (False, True, False),
        (False, False, True),
    ],
)
def test_future_dependency_not_in_declared_tree_fails_closed(
    tmp_path, tracked, ignored, top_level
):
    from tools.release_matrix_acceptance import source_admission_problems

    root, sha, git = _source_repo(tmp_path)
    path = "future.py" if top_level else "src/future.py"
    (root / path).write_bytes(b"VALUE = 99\n")
    if tracked:
        git("add", path)
    if ignored:
        (root / ".git/info/exclude").write_text("src/future.py\n")
    assert source_admission_problems(root=root, source_sha=sha)


def test_worktree_newline_transformation_is_not_exact_byte_evidence(tmp_path):
    from tools.release_matrix_acceptance import source_admission_problems

    root, sha, _ = _source_repo(tmp_path)
    (root / "src/main.py").write_bytes(b"import dependency\r\n")
    assert source_admission_problems(root=root, source_sha=sha)


def test_canonical_benchmark_registry_cannot_silently_drop_a_workload(monkeypatch):
    from tools import bench_suites

    monkeypatch.setattr(bench_suites, "BENCHMARKS", bench_suites.BENCHMARKS[1:])
    with pytest.raises(ValueError, match="benchmark ownership"):
        required_matrix(source_sha="a" * 40, root=ROOT)


@pytest.mark.parametrize(
    "field", ["guest_profile", "runtime_profile", "compiler_profile", "target"]
)
def test_matrix_rejects_labels_disconnected_from_selected_build(field):
    matrix = fixture_matrix()
    shards, toolchains = fixture_receipts(matrix)
    row = next(row for shard in shards for row in shard["cells"])
    row["measurement"]["build_observation"]["selected_profiles"][field] = (
        "wrong-profile"
    )
    # Top-level coordinate labels still exactly match the expected authority.
    assert row["observed_profiles"]["runtime_profile"] == "wasm-release"
    problems = performance_matrix_problems(
        matrix, shards, toolchain_identities=toolchains
    )
    assert any("measured build profile observation" in p for p in problems)


def test_matrix_rejects_historical_missing_build_profile_binding():
    matrix = fixture_matrix()
    shards, toolchains = fixture_receipts(matrix)
    row = next(row for shard in shards for row in shard["cells"])
    row["measurement"].pop("build_observation")
    assert any(
        "measured build profile observation" in p
        for p in performance_matrix_problems(
            matrix, shards, toolchain_identities=toolchains
        )
    )
