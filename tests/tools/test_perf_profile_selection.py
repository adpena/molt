from __future__ import annotations

import os
import sys
from pathlib import Path

import pytest

sys.path.insert(0, str(Path(__file__).resolve().parents[2] / "tools"))

import perf_scoreboard as scoreboard
import perf_scoreboard_measure as measure
from perf_scoreboard_build_profiles import profile_selection
from perf_scoreboard_model import NATIVE_CRANELIFT, NATIVE_LLVM, WASM


@pytest.mark.parametrize("spec", [NATIVE_CRANELIFT, NATIVE_LLVM, WASM])
@pytest.mark.parametrize("profile", ["release-fast", "release-output", "dev-fast"])
@pytest.mark.parametrize("ambient", [False, True])
def test_profile_selection_is_explicit(monkeypatch, spec, profile, ambient):
    keys = profile_selection(spec, profile).environment()
    for key in keys:
        if ambient:
            monkeypatch.setenv(key, "dev-fast")
        else:
            monkeypatch.delenv(key, raising=False)
    env = measure._perfscore_build_env(spec, profile)
    expected_guest = profile
    assert env["MOLT_RELEASE_CARGO_PROFILE"] == expected_guest
    assert env["MOLT_DEV_CARGO_PROFILE"] == expected_guest
    assert env["MOLT_WASM_CARGO_PROFILE"] == expected_guest
    assert env["MOLT_BACKEND_PROFILE"] == "release"
    assert env["MOLT_RELEASE_BACKEND_CARGO_PROFILE"] == "release"
    assert env["MOLT_RUNTIME_BUILD_PROFILE"] == ""
    assert env["MOLT_RUNTIME_WASM_INCREMENTAL"] == "0"


@pytest.mark.parametrize("profile", ["release-fast", "release-output", "dev-fast"])
def test_backend_resolver_uses_controlled_host_profile(monkeypatch, tmp_path, profile):
    controlled = tmp_path / "controlled"
    ambient = tmp_path / "ambient"
    name = "molt-backend.exe" if os.name == "nt" else "molt-backend"
    expected = controlled / "release" / name
    expected.parent.mkdir(parents=True)
    expected.write_bytes(b"controlled compiler")
    stale = ambient / "release-fast" / name
    stale.parent.mkdir(parents=True)
    stale.write_bytes(b"unrelated compiler")
    monkeypatch.setenv("CARGO_TARGET_DIR", str(ambient))
    monkeypatch.setattr(
        scoreboard,
        "_perfscore_build_env",
        lambda spec, profile: {"CARGO_TARGET_DIR": str(controlled)},
    )
    assert (
        scoreboard._resolve_backend_binary_path(NATIVE_CRANELIFT, profile) == expected
    )
    expected.unlink()
    assert scoreboard._resolve_backend_binary_path(NATIVE_CRANELIFT, profile) is None


def test_unknown_profile_does_not_alias_release():
    with pytest.raises(ValueError, match="unknown performance profile"):
        profile_selection(NATIVE_CRANELIFT, "release-typo")


def test_batch_request_carries_distinct_profile_inputs(tmp_path):
    import bench

    environments = [
        measure._perfscore_build_env(NATIVE_CRANELIFT, p)
        for p in ("release-fast", "release-output")
    ]
    requests = [
        bench._molt_build_params(
            script=str(tmp_path / "program.py"),
            extra_args=[],
            env=env,
            build_profile="release",
            out_dir=tmp_path,
        )
        for env in environments
    ]
    assert requests[0]["env_overrides"]["MOLT_RELEASE_CARGO_PROFILE"] == "release-fast"
    assert (
        requests[1]["env_overrides"]["MOLT_RELEASE_CARGO_PROFILE"] == "release-output"
    )
    assert requests[0] != requests[1]


@pytest.mark.parametrize("spec", [NATIVE_CRANELIFT, NATIVE_LLVM, WASM])
@pytest.mark.parametrize("profile", ["release-fast", "release-output", "dev-fast"])
def test_profile_binding_requires_actual_selected_coordinates(spec, profile):
    from perf_scoreboard_build_profiles import profile_binding_problems

    selection = profile_selection(spec, profile)
    facts = {
        "guest_profile": selection.cli_build_profile,
        "compiler_profile": selection.host_cargo_profile,
        "runtime_profile": selection.guest_cargo_profile,
        "target": spec.build_target,
    }

    def check(observation):
        return profile_binding_problems(
            observation, build_target=spec.build_target, profile=profile
        )

    assert check({"selected_profiles": facts, "compiled_with_verified": False}) == []
    assert check(None)
    assert check({"compiler": {"path": f"target/{profile}/molt-backend"}})
    for key in facts:
        corrupt = dict(facts, **{key: "ambient-dev-fast"})
        assert check({"selected_profiles": corrupt})


def test_publication_observes_selected_profiles_without_claiming_loaded_compiler(
    tmp_path,
):
    from molt.cli.build_results import _observed_build_toolchain

    output = tmp_path / "guest.exe"
    output.write_bytes(b"guest bytes")
    selection = {
        "guest_profile": "release",
        "compiler_profile": "release",
        "runtime_profile": "release-output",
        "target": "native",
    }
    receipt = _observed_build_toolchain(
        backend_bin=None, runtime_lib=None, output=output, selected_profiles=selection
    )
    assert receipt["selected_profiles"] == selection
    assert receipt["compiled_with_verified"] is False
    assert receipt["artifact"]["identity"]
    selection["runtime_profile"] = "changed caller"
    assert receipt["selected_profiles"]["runtime_profile"] == "release-output"


def test_batch_restart_preserves_owned_profile_and_isolates_next_cell(monkeypatch):
    import bench

    clients = []

    class Client:
        def __init__(self, *args, **kwargs):
            self.env = dict(kwargs["env"])
            self.closed = False
            clients.append(self)

        def close(self, **kwargs):
            self.closed = True

    monkeypatch.setattr(bench, "BatchCompileServerClient", Client)
    fast = bench._BenchBatchBuildServer(
        measure._perfscore_build_env(NATIVE_CRANELIFT, "release-fast")
    )
    fast.restart()
    output = bench._BenchBatchBuildServer(
        measure._perfscore_build_env(NATIVE_CRANELIFT, "release-output")
    )
    assert clients[0].closed
    assert clients[0].env == clients[1].env
    assert clients[1].env["MOLT_RELEASE_CARGO_PROFILE"] == "release-fast"
    assert clients[2].env["MOLT_RELEASE_CARGO_PROFILE"] == "release-output"
    fast.close()
    output.close()


@pytest.mark.parametrize(
    "backend,profile",
    [
        ("native", "dev-fast"),
        ("native", "release-fast"),
        ("native", "release-output"),
        ("native", "release-size"),
        ("llvm", "dev-fast"),
        ("llvm", "release-fast"),
        ("llvm", "release-output"),
        ("llvm", "release-size"),
        ("wasm", "dev-fast"),
        ("wasm", "release-output"),
        ("wasm", "wasm-release"),
    ],
)
def test_every_declared_acceptance_profile_remains_distinct(backend, profile):
    from perf_scoreboard_model import BACKENDS_BY_NAME

    spec = BACKENDS_BY_NAME[backend]
    selection = profile_selection(spec, profile)
    assert selection.guest_cargo_profile == profile
    assert selection.host_cargo_profile == "release"
    env = measure._perfscore_build_env(spec, profile)
    assert env["MOLT_WASM_CARGO_PROFILE"] == profile


def test_wasm_only_profile_is_not_a_native_alias():
    with pytest.raises(ValueError, match="WASM artifact coordinate"):
        profile_selection(NATIVE_CRANELIFT, "wasm-release")


@pytest.mark.parametrize("minor", [12, 13, 14])
def test_profile_change_invalidates_real_build_cache_variant(minor):
    from molt.cli.backend_cache_setup import _build_cache_variant
    from molt.target_python import TargetPythonVersion

    common = dict(
        profile="release",
        backend_cargo="release",
        emit="bin",
        stdlib_split=True,
        codegen_env="fixed-codegen",
        linked=False,
        target_python=TargetPythonVersion(3, minor, 0),
    )
    variants = {
        _build_cache_variant(runtime_cargo=profile, **common)
        for profile in (
            "release-fast",
            "release-output",
            "release-size",
            "wasm-release",
        )
    }
    assert len(variants) == 4


def test_cold_provenance_uses_measured_compiler_and_rejects_lane_change(tmp_path):
    from molt.toolchain_identity import executable_content_identity
    from molt.cli.backend_execution import _backend_binary_identity
    from perf_scoreboard_build_profiles import record_measured_backend_identity

    compiler = tmp_path / "molt-backend.exe"
    compiler.write_bytes(b"actual selected compiler bytes")
    identity = executable_content_identity(compiler, label="test selected compiler")
    observation = {"compiler": {"identity": identity}}
    provenance = {"backend_binary_identity": {"native/release-output": None}}
    record_measured_backend_identity(
        provenance, observation=observation, backend="native", profile="release-output"
    )
    assert provenance["backend_binary_identity"][
        "native/release-output"
    ] == _backend_binary_identity(compiler)
    assert (
        provenance["backend_binary_identity_before_build"]["native/release-output"]
        is None
    )
    record_measured_backend_identity(
        provenance, observation=observation, backend="native", profile="release-output"
    )
    compiler.write_bytes(b"replacement compiler")
    changed = {
        "compiler": {
            "identity": executable_content_identity(
                compiler, label="replacement compiler"
            )
        }
    }
    with pytest.raises(ValueError, match="changed within measured lane"):
        record_measured_backend_identity(
            provenance, observation=changed, backend="native", profile="release-output"
        )
    assert provenance["backend_binary_identity"][
        "native/release-output"
    ] != _backend_binary_identity(compiler)


@pytest.mark.parametrize(
    "observation",
    [
        None,
        {},
        {"compiler": {}},
        {"compiler": {"identity": {"sha256": "a" * 64, "size": True}}},
    ],
)
def test_missing_selected_compiler_cannot_become_provenance(observation):
    from perf_scoreboard_build_profiles import record_measured_backend_identity

    with pytest.raises(ValueError, match="canonical selected compiler identity"):
        record_measured_backend_identity(
            {}, observation=observation, backend="native", profile="release-fast"
        )


def test_profile_regressions_and_producer_closure_are_mandatory_proofs():
    import tomllib

    root = Path(__file__).resolve().parents[2]
    plan = tomllib.loads((root / "tools/proof_plan.toml").read_text())
    commands = {command["id"]: command for command in plan["command"]}
    docs = commands["repository.docs-tests"]
    assert docs["tiers"] == ["pre-push", "pr", "main"]
    assert {
        "tests/tools/test_perf_profile_selection.py",
        "tests/tools/test_perf_scoreboard.py",
        "tests/tools/test_perf_schema.py",
        "tests/tools/test_check_perf_gate_wiring.py",
        "tests/tools/test_perf_authority.py",
    } <= set(docs["argv"])
    assert (
        "tests/tools/test_release_matrix_acceptance.py"
        in commands["repository.release-supply-chain"]["argv"]
    )
    assert set(scoreboard.PERF_TOOL_IDENTITY_PATHS) <= set(plan["authority_inputs"])
    assert {
        "tests/tools/test_perf_profile_selection.py",
        "tools/perf_scoreboard_build_profiles.py",
        "tools/release_matrix_acceptance.py",
        "tests/tools/test_release_matrix_acceptance.py",
        "src/molt/cli/backend_cache_setup.py",
        "src/molt/cli/cargo_profiles.py",
        "src/molt/cli/runtime_wasm_build_policy.py",
    } <= set(plan["authority_inputs"])


@pytest.mark.parametrize("minor", ["3.12", "3.13", "3.14"])
@pytest.mark.parametrize(
    "profile", ["release-fast", "release-output", "release-size", "dev-fast"]
)
def test_profiling_rebuild_preserves_minor_and_profile_over_conflicting_defaults(
    monkeypatch, tmp_path, minor, profile
):
    import bench
    import perf_scoreboard_profile as profiling
    from molt.target_python import _resolve_target_python_version

    captured = {}

    def prepare(script, **kwargs):
        captured.update(kwargs)
        return bench.classify_molt_process_failure(
            phase="build",
            returncode=1,
            stderr="source-only request capture",
            elapsed_s=0.0,
            default_status="build_failed",
        )

    monkeypatch.setattr(profiling, "_profiling_tmp_root", lambda: tmp_path)
    monkeypatch.setattr(bench, "prepare_molt_binary", prepare)
    monkeypatch.setenv("MOLT_RELEASE_CARGO_PROFILE", "ambient-wrong-profile")
    script = Path(__file__).resolve().parents[2] / "tests/benchmarks/bench_fib.py"
    _, metadata = profiling.build_profiling_binary(
        script,
        spec=NATIVE_CRANELIFT,
        profile=profile,
        target_python_version=minor,
        inner_loops=2,
        log_lines=[],
    )
    assert metadata["target_python"] == minor
    assert captured["env"]["MOLT_RELEASE_CARGO_PROFILE"] == profile
    params = bench._molt_build_params(
        script=str(script),
        out_dir=tmp_path,
        build_profile=captured["build_profile"],
        env=captured["env"],
        extra_args=captured["extra_args"],
    )
    assert params["python_version"] == minor
    conflicting = "3.14" if minor == "3.12" else "3.12"
    selected = _resolve_target_python_version(
        explicit=params["python_version"],
        build_config={"python_version": conflicting},
        project_root=tmp_path,
    )
    assert selected.short == minor


@pytest.mark.parametrize("minor", ["3.12", "3.13", "3.14"])
def test_hot_profile_controller_forwards_the_selected_minor(monkeypatch, minor):
    import perf_scoreboard_profile as profiling

    received = []

    def build(script, **kwargs):
        received.append(kwargs["target_python_version"])
        return None, {"reason": "source-only refusal"}

    monkeypatch.setattr(profiling, "build_profiling_binary", build)
    script = Path(__file__).resolve().parents[2] / "tests/benchmarks/bench_fib.py"
    cells = profiling.run_hot_only_profiles(
        scripts=[script],
        spec=NATIVE_CRANELIFT,
        profile="release-output",
        target_python_version=minor,
        inner_loops=2,
        rss_mb=64,
    )
    assert received == [minor]
    assert cells[0]["target_python"] == minor


@pytest.mark.parametrize("minor", ["", "3.11", "3.15", "4.12"])
def test_profiling_refuses_an_unbound_or_unsupported_minor(minor):
    import perf_scoreboard_profile as profiling

    binary, metadata = profiling.build_profiling_binary(
        Path("never-read.py"),
        spec=NATIVE_CRANELIFT,
        profile="release-output",
        target_python_version=minor,
        inner_loops=2,
        log_lines=[],
    )
    assert binary is None
    assert metadata["refused"] is True
    assert "explicit supported target Python" in metadata["reason"]
