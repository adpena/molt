from __future__ import annotations

from molt._wasm_abi_generated import WASM_EXECUTION_MANIFEST_VERSION

import os
import sys
from pathlib import Path

import pytest

from molt.process_environment import NameRule, ProcessEnvironment

sys.path.insert(0, str(Path(__file__).resolve().parents[2] / "tools"))

from tools import perf_scoreboard as scoreboard
from tools import perf_scoreboard_measure as measure
from tools.perf_scoreboard_build_profiles import profile_selection
from tools.perf_scoreboard_model import (
    NATIVE_CRANELIFT,
    NATIVE_LLVM,
    BACKENDS_BY_NAME,
    WASM,
)
from tests.release_lane_fixtures import EXPECTED_LANES


@pytest.fixture
def source_only_perf_checkout(monkeypatch, tmp_path):
    from molt import dx
    from tests.test_dx_run_context import (
        _clear_run_context_env,
        _github_actions_custody_env,
    )

    source = tmp_path / "runner-work" / "molt" / "molt"
    source.mkdir(parents=True)
    runner_temp = tmp_path / "runner-temp"
    sha = "b" * 40
    facts = _github_actions_custody_env(source, runner_temp, sha=sha)
    _clear_run_context_env(monkeypatch)
    monkeypatch.delenv("MOLT_BENCH_TMP_ROOT", raising=False)
    for key, value in facts.items():
        monkeypatch.setenv(key, value)
    monkeypatch.setattr(dx, "git_checkout_head", lambda _root: sha)
    monkeypatch.setattr(measure, "REPO_ROOT", source)
    monkeypatch.setattr(measure.bench, "REPO_ROOT", source)
    # Preserve the production default's relationship to its source checkout.
    # Moving this to artifacts in the fixture would hide constructor writes.
    monkeypatch.setattr(measure.bench, "BENCH_RESULTS_DIR", source / "bench/results")
    return source, runner_temp / "molt-custody"


@pytest.mark.parametrize("spec", [NATIVE_CRANELIFT, NATIVE_LLVM, WASM])
@pytest.mark.parametrize("explicit_artifacts", [False, True])
def test_perfscore_uses_admitted_artifacts_for_explicit_lane(
    monkeypatch, source_only_perf_checkout, spec, explicit_artifacts
):
    from molt import dx

    source, custody = source_only_perf_checkout
    artifact_root = custody / "selected-artifacts" if explicit_artifacts else custody
    if explicit_artifacts:
        monkeypatch.setenv("MOLT_EXT_ROOT", str(artifact_root))
    # The outer guard's generated session and shared targets are not this lane.
    monkeypatch.setenv("MOLT_SESSION_ID", "guard-123")
    monkeypatch.setenv("MOLT_SESSION_ID_GENERATED", "1")
    old_target = custody / "outer-target"
    old_diff_target = custody / "outer-diff-target"
    monkeypatch.setenv("CARGO_TARGET_DIR", str(old_target))
    monkeypatch.setenv("MOLT_DIFF_CARGO_TARGET_DIR", str(old_diff_target))

    env = measure._perfscore_build_env(spec, "release-output")

    expected_target = (
        artifact_root
        / "target/checkouts"
        / dx.checkout_component(source)
        / "sessions/perfscore"
    )
    assert env["MOLT_EXT_ROOT"] == str(artifact_root)
    assert env["CARGO_TARGET_DIR"] == str(expected_target)
    assert env["MOLT_DIFF_CARGO_TARGET_DIR"] == str(expected_target)
    assert expected_target.is_dir()
    assert not old_target.exists() and not old_diff_target.exists()
    assert env["MOLT_SESSION_ID"] == "perfscore"
    assert "MOLT_SESSION_ID_GENERATED" not in env
    assert env["MOLT_RELEASE_CARGO_PROFILE"] == "release-output"
    assert env["MOLT_WASM_CARGO_PROFILE"] == "release-output"
    assert env["MOLT_RELEASE_BACKEND_CARGO_PROFILE"] == "release"
    for key in dx.CANONICAL_ROOT_ENV_KEYS:
        if value := env.get(key):
            assert not Path(value).is_relative_to(source), key
    assert not (source / "target").exists()
    assert not (source / "bench/results").exists()
    # Constructing the lane must not replace the parent process's environment.
    assert os.environ["MOLT_SESSION_ID"] == "guard-123"
    assert os.environ["MOLT_SESSION_ID_GENERATED"] == "1"
    assert os.environ["CARGO_TARGET_DIR"] == str(old_target)


def test_perfscore_preserves_real_source_only_custody_refusal(
    monkeypatch, source_only_perf_checkout
):
    from molt.dx import DxConfigError

    source, _custody = source_only_perf_checkout
    monkeypatch.setenv("MOLT_EXT_ROOT", str(source / "forbidden-artifacts"))
    with pytest.raises(DxConfigError, match="checkout cannot own MOLT_EXT_ROOT"):
        measure._perfscore_build_env(NATIVE_CRANELIFT, "release-fast")
    assert not (source / "forbidden-artifacts").exists()


@pytest.mark.parametrize("generated", [False, True])
def test_cpython_perfscore_session_is_explicit_without_build_reselection(
    monkeypatch, tmp_path, generated
):
    target = str(tmp_path / "outer-target")
    monkeypatch.setenv("MOLT_SESSION_ID", "guard-123")
    monkeypatch.setenv("CARGO_TARGET_DIR", target)
    if generated:
        monkeypatch.setenv("MOLT_SESSION_ID_GENERATED", "1")
    else:
        monkeypatch.delenv("MOLT_SESSION_ID_GENERATED", raising=False)

    env = measure._cpython_run_env()

    assert env["MOLT_SESSION_ID"] == "perfscore"
    assert "MOLT_SESSION_ID_GENERATED" not in env
    assert env["CARGO_TARGET_DIR"] == target
    assert not Path(target).exists()
    assert os.environ["MOLT_SESSION_ID"] == "guard-123"


@pytest.mark.parametrize(
    "spec,profile",
    [
        (BACKENDS_BY_NAME[backend], runtime)
        for backend, _target, _guest, runtime, _compiler in EXPECTED_LANES
    ],
)
@pytest.mark.parametrize("ambient", [False, True])
def test_profile_selection_is_explicit(monkeypatch, spec, profile, ambient):
    monkeypatch.setenv("MOLT_BACKEND", "llvm" if ambient else "cranelift")
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
    assert env["MOLT_BACKEND"] == ("llvm" if ambient else "cranelift")
    assert profile_selection(spec, profile).codegen_backend == (
        "llvm" if spec.backend == "llvm" else "cranelift"
    )
    assert env["MOLT_RELEASE_BACKEND_CARGO_PROFILE"] == "release"
    assert env["MOLT_RUNTIME_BUILD_PROFILE"] == ""
    assert env["MOLT_RUNTIME_WASM_INCREMENTAL"] == "0"


def test_perfscore_lets_the_canonical_env_place_its_session_target(
    monkeypatch, tmp_path
):
    # Under hosted custody the checkout may own no Cargo target dir, so the
    # session's target must come from the canonical env authority, never from
    # the repository path or an ambient value.
    monkeypatch.setenv("CARGO_TARGET_DIR", str(tmp_path / "ambient"))
    received: list[dict[str, str]] = []

    def canonical(base):
        received.append(dict(base))
        return ProcessEnvironment(base)

    monkeypatch.setattr(measure.bench, "_canonical_bench_env", canonical)
    measure._perfscore_build_env(NATIVE_CRANELIFT, "release-fast")

    assert received[0]["MOLT_SESSION_ID"] == "perfscore"
    assert "CARGO_TARGET_DIR" not in received[0]


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
        lambda spec, profile: ProcessEnvironment({"CARGO_TARGET_DIR": str(controlled)}),
    )
    assert (
        scoreboard._resolve_backend_binary_path(NATIVE_CRANELIFT, profile) == expected
    )
    expected.unlink()
    assert scoreboard._resolve_backend_binary_path(NATIVE_CRANELIFT, profile) is None


def test_unknown_profile_does_not_alias_release():
    with pytest.raises(ValueError, match="unsupported release lane"):
        profile_selection(NATIVE_CRANELIFT, "release-typo")


def test_batch_request_carries_distinct_profile_inputs(tmp_path):
    from tools import bench

    requests = []

    class Client:
        def request(self, op, *, params, env_overrides, timeout):
            assert op == "build" and timeout == 10.0
            requests.append({"params": params, "env_overrides": dict(env_overrides)})
            return {"ok": True, "returncode": 0}

    server = bench._BenchBatchBuildServer.__new__(bench._BenchBatchBuildServer)
    server._client = Client()
    environments = [
        measure._perfscore_build_env(NATIVE_CRANELIFT, p)
        for p in ("release-fast", "release-output")
    ]
    for env in environments:
        params = bench._molt_build_params(
            script=str(tmp_path / "program.py"),
            extra_args=[],
            env=env,
            build_profile="release",
            out_dir=tmp_path,
        )
        server.request_build(params, env_overrides=env, timeout_s=10.0)
    assert requests[0]["env_overrides"]["MOLT_RELEASE_CARGO_PROFILE"] == "release-fast"
    assert (
        requests[1]["env_overrides"]["MOLT_RELEASE_CARGO_PROFILE"] == "release-output"
    )
    assert requests[0] != requests[1]


@pytest.mark.parametrize(
    "spec,profile",
    [
        (BACKENDS_BY_NAME[backend], runtime)
        for backend, _target, _guest, runtime, _compiler in EXPECTED_LANES
    ],
)
def test_profile_binding_requires_actual_selected_coordinates(spec, profile):
    from tools.perf_scoreboard_build_profiles import profile_binding_problems

    selection = profile_selection(spec, profile)
    facts = {
        "backend": spec.backend,
        "guest_profile": selection.guest_profile,
        "compiler_profile": selection.compiler_profile,
        "runtime_profile": selection.runtime_profile,
        "target": spec.build_target,
    }

    def check(observation):
        return profile_binding_problems(
            observation, backend=spec.backend, profile=profile
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
        "backend": "native",
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
    from tools import bench

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
    fast.start()
    fast.restart()
    output = bench._BenchBatchBuildServer(
        measure._perfscore_build_env(NATIVE_CRANELIFT, "release-output")
    )
    output.start()
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
    from tools.perf_scoreboard_model import BACKENDS_BY_NAME

    spec = BACKENDS_BY_NAME[backend]
    selection = profile_selection(spec, profile)
    assert selection.runtime_profile == profile
    assert selection.compiler_profile == "release"
    env = measure._perfscore_build_env(spec, profile)
    assert env["MOLT_WASM_CARGO_PROFILE"] == profile


def test_wasm_only_profile_is_not_a_native_alias():
    with pytest.raises(ValueError, match="unsupported release lane"):
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
    from molt.backend_binary_identity import identify_backend_binary
    from tools.perf_scoreboard_build_profiles import record_measured_build_identity

    compiler = tmp_path / "molt-backend.exe"
    compiler.write_bytes(b"actual selected compiler bytes")
    identity = executable_content_identity(compiler, label="test selected compiler")
    observation = {"compiler": {"identity": identity}, "runtime": None}
    provenance = {"backend_binary_identity": {"native/release-output": None}}
    record_measured_build_identity(
        provenance, observation=observation, backend="native", profile="release-output"
    )
    assert provenance["backend_binary_identity"][
        "native/release-output"
    ] == identify_backend_binary(compiler)
    assert (
        provenance["backend_binary_identity_before_build"]["native/release-output"]
        is None
    )
    record_measured_build_identity(
        provenance, observation=observation, backend="native", profile="release-output"
    )
    compiler.write_bytes(b"replacement compiler")
    changed = {
        "compiler": {
            "identity": executable_content_identity(
                compiler, label="replacement compiler"
            )
        },
        "runtime": None,
    }
    with pytest.raises(ValueError, match="changed within measured lane"):
        record_measured_build_identity(
            provenance, observation=changed, backend="native", profile="release-output"
        )
    assert provenance["backend_binary_identity"][
        "native/release-output"
    ] != identify_backend_binary(compiler)


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
    from tools.perf_scoreboard_build_profiles import record_measured_build_identity

    with pytest.raises(ValueError, match="canonical selected compiler identity"):
        record_measured_build_identity(
            {}, observation=observation, backend="native", profile="release-fast"
        )


def test_measured_provenance_binds_the_linked_runtime_library(tmp_path):
    from molt.toolchain_identity import (
        executable_content_identity,
        stable_file_content_identity,
    )
    from tools.perf_scoreboard_build_profiles import record_measured_build_identity

    compiler = tmp_path / "molt-backend"
    compiler.write_bytes(b"compiler bytes")
    runtime = tmp_path / "libmolt_runtime.a"
    runtime.write_bytes(b"runtime archive bytes")

    def observation() -> dict[str, object]:
        return {
            "compiler": {
                "identity": executable_content_identity(compiler, label="compiler")
            },
            "runtime": {
                "path": str(runtime),
                "identity": stable_file_content_identity(runtime, label="runtime"),
            },
        }

    provenance: dict = {}
    record_measured_build_identity(
        provenance, observation=observation(), backend="native", profile="release-fast"
    )
    first = provenance["runtime_identity"]["native/release-fast"]
    assert isinstance(first, str) and len(first) == 64
    record_measured_build_identity(
        provenance, observation=observation(), backend="native", profile="release-fast"
    )
    assert provenance["runtime_identity"]["native/release-fast"] == first
    runtime.write_bytes(b"rebuilt runtime archive")
    with pytest.raises(
        ValueError, match="runtime library changed within measured lane"
    ):
        record_measured_build_identity(
            provenance,
            observation=observation(),
            backend="native",
            profile="release-fast",
        )
    # A lane that links no runtime artifact records that fact, not a guess.
    record_measured_build_identity(
        provenance,
        observation={**observation(), "runtime": None},
        backend="wasm",
        profile="release-fast",
    )
    assert provenance["runtime_identity"]["wasm/release-fast"] is None


@pytest.mark.parametrize(
    ("runtime_fact", "message"),
    [
        ("absent", "lacks its runtime fact"),
        ({"error": "runtime archive vanished"}, "runtime archive vanished"),
        (
            {"identity": {"filename": "lib.a", "size": 0, "sha256": "a" * 64}},
            "canonical runtime identity",
        ),
    ],
)
def test_failed_runtime_observation_cannot_become_provenance(
    tmp_path, runtime_fact, message
):
    from molt.toolchain_identity import executable_content_identity
    from tools.perf_scoreboard_build_profiles import record_measured_build_identity

    compiler = tmp_path / "molt-backend"
    compiler.write_bytes(b"compiler bytes")
    observation: dict[str, object] = {
        "compiler": {"identity": executable_content_identity(compiler, label="c")}
    }
    if runtime_fact != "absent":
        observation["runtime"] = runtime_fact
    with pytest.raises(ValueError, match=message):
        record_measured_build_identity(
            {}, observation=observation, backend="native", profile="release-fast"
        )


def test_profile_regressions_and_producer_closure_are_mandatory_proofs():
    import tomllib

    root = Path(__file__).resolve().parents[2]
    plan = tomllib.loads((root / "tools/proof_plan.toml").read_text(encoding="utf-8"))
    commands = {command["id"]: command for command in plan["command"]}
    docs = commands["repository.docs-tests"]
    assert docs["tiers"] == ["pre-push", "pr", "main"]
    assert {
        "tests/tools/test_perf_profile_selection.py",
        "tests/tools/test_perf_scoreboard.py",
        "tests/tools/test_perf_schema.py",
        "tests/tools/test_check_perf_gate_wiring.py",
        "tests/tools/test_perf_authority.py",
    } <= set(docs["pytest"])
    assert (
        "tests/tools/test_release_matrix_acceptance.py"
        in commands["repository.release-supply-chain"]["pytest"]
    )
    from tools import perf_methodology

    assert set(perf_methodology.load().paths) | {
        "config/perf_methodology.toml",
        "tools/perf_methodology.py",
    } <= set(plan["authority_inputs"])
    assert "tests/tools/test_perf_methodology.py" in docs["pytest"]
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
@pytest.mark.parametrize("rule", [NameRule.EXACT, NameRule.WINDOWS])
@pytest.mark.parametrize(
    "profile", ["release-fast", "release-output", "release-size", "dev-fast"]
)
def test_profiling_rebuild_preserves_minor_and_profile_over_conflicting_defaults(
    monkeypatch, tmp_path, minor, rule, profile
):
    from tools import bench
    from tools import perf_scoreboard_profile as profiling
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

    monkeypatch.setattr(profiling, "_profiling_tmp_root", lambda env: tmp_path)
    monkeypatch.setattr(bench, "prepare_molt_binary", prepare)
    monkeypatch.setenv("MOLT_RELEASE_CARGO_PROFILE", "ambient-wrong-profile")
    environment = ProcessEnvironment(
        measure._perfscore_build_env(NATIVE_CRANELIFT, profile), rule=rule
    ).updated({"MOLT_KEEP_SYMBOLS": "0"})
    before = dict(environment)
    monkeypatch.setattr(profiling, "_perfscore_build_env", lambda *_: environment)
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
    assert isinstance(captured["env"], ProcessEnvironment)
    assert captured["env"].rule is rule
    assert captured["env"]["MOLT_KEEP_SYMBOLS"] == "1"
    assert dict(environment) == before
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
    from tools import perf_scoreboard_profile as profiling

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
    from tools import perf_scoreboard_profile as profiling

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


@pytest.mark.parametrize(
    "backend,profile",
    [("unknown", "release-fast"), ("wasm", "release-fast"), ("llvm", "unknown")],
)
def test_profile_binding_reports_unsupported_observed_lane(backend, profile):
    from tools.perf_scoreboard_build_profiles import profile_binding_problems

    assert profile_binding_problems(
        {"selected_profiles": {}}, backend=backend, profile=profile
    ) == [f"unsupported release lane: backend={backend}, runtime_profile={profile}"]


@pytest.mark.parametrize(
    "spec,expected", [(NATIVE_CRANELIFT, "cranelift"), (NATIVE_LLVM, "llvm")]
)
def test_lane_backend_is_a_build_flag_for_batch_and_cli_builds(
    monkeypatch, tmp_path, spec, expected
):
    """The lane's codegen backend travels as ``--backend``, never ``MOLT_BACKEND``.

    Since HF-60 neither ``molt build`` nor the batch build server reads the
    backend from the environment, so a lane that set only the variable built
    with Cranelift.
    """
    from tools import bench
    from tools.perf_scoreboard_build_profiles import profile_selection

    monkeypatch.delenv("MOLT_BACKEND", raising=False)
    env = measure._perfscore_build_env(spec, "release-fast")
    params = bench._molt_build_params(
        script=str(tmp_path / "program.py"),
        extra_args=[
            "--backend",
            profile_selection(spec, "release-fast").codegen_backend,
        ],
        env=env,
        build_profile="release",
        out_dir=tmp_path,
    )

    assert "MOLT_BACKEND" not in env
    assert params["codegen_backend"] == expected
    assert "backend" not in params


@pytest.mark.parametrize(
    "outcome",
    [
        "success",
        "build-failure",
        "invalid-json",
        "missing-artifact",
        "io-error",
        "cancel",
    ],
)
def test_wasm_build_uses_admitted_scratch_and_closes_failed_owners(
    monkeypatch, source_only_perf_checkout, outcome
):
    import json
    import subprocess

    source, custody = source_only_perf_checkout
    env = measure._perfscore_build_env(WASM, "wasm-release")
    outputs = []
    original = (
        KeyboardInterrupt("independent cancellation")
        if outcome == "cancel"
        else OSError("independent compiler I/O")
    )

    def compiler(command, **kwargs):
        directory = Path(command[command.index("--out-dir") + 1])
        outputs.append(directory)
        assert directory.parent == Path(env["MOLT_BENCH_TMP_ROOT"])
        assert directory.is_relative_to(custody)
        assert not directory.is_relative_to(source)
        assert directory.is_dir()
        assert command[command.index("--python-version") + 1] == "3.13"
        assert command[command.index("--target") + 1] == "wasm"
        if outcome in {"io-error", "cancel"}:
            raise original
        assert "--linked" in command and "--require-linked" in command
        assert "--trusted" not in command
        wasm = directory / "linked.wasm"
        manifest = directory / "manifest.json"
        if outcome == "success":
            wasm.write_bytes(b"independent artifact")
            manifest.write_text(
                json.dumps(
                    {
                        "version": WASM_EXECUTION_MANIFEST_VERSION,
                        "mode": "linked",
                        "modules": {"linked": {"path": wasm.name}},
                    }
                ),
                encoding="utf-8",
            )
        stdout = (
            "not json"
            if outcome == "invalid-json"
            else json.dumps(
                {
                    "data": {
                        "artifacts": {
                            "linked_wasm": str(wasm),
                            "manifest": str(manifest),
                        }
                    }
                }
            )
        )
        return subprocess.CompletedProcess(
            command, 1 if outcome == "build-failure" else 0, stdout, "fixture failure"
        )

    monkeypatch.setattr(
        measure.bench.harness_memory_guard, "guarded_completed_process", compiler
    )
    monkeypatch.setattr(measure.bench, "_prune_backend_daemons", lambda env: None)
    if outcome in {"cancel", "io-error"}:
        with pytest.raises(type(original)) as caught:
            measure.bench.prepare_molt_binary(
                str(source / "ordinary.py"),
                env=env,
                build_profile="release",
                build_target="wasm",
                extra_args=["--python-version", "3.13"],
            )
        assert caught.value is original
    else:
        result = measure.bench.prepare_molt_binary(
            str(source / "ordinary.py"),
            env=env,
            build_profile="release",
            build_target="wasm",
            extra_args=["--python-version", "3.13"],
        )
        if outcome == "success":
            assert isinstance(result, measure.bench.MoltBinary)
            assert Path(result.temp_dir.name) == outputs[0]
            assert result.path.is_file()
            measure._release_binary(result, None)
        else:
            assert isinstance(result, measure.bench.MoltFailure)
    assert len(outputs) == 1 and not outputs[0].exists()
    assert not (source / "tmp").exists()


def test_wasm_build_refuses_source_scratch_before_guard(
    monkeypatch, source_only_perf_checkout
):
    from molt.dx import DxConfigError

    source, _custody = source_only_perf_checkout
    env = measure._perfscore_build_env(WASM, "wasm-release").updated(
        {"MOLT_BENCH_TMP_ROOT": str(source / "forbidden-wasm")}
    )
    monkeypatch.setattr(
        measure.bench.harness_memory_guard,
        "guarded_completed_process",
        lambda *a, **kw: pytest.fail("forbidden scratch reached compiler"),
    )
    with pytest.raises(DxConfigError, match="MOLT_BENCH_TMP_ROOT"):
        measure.bench.prepare_molt_binary(
            str(source / "ordinary.py"),
            env=env,
            build_profile="release",
            build_target="wasm",
            extra_args=["--python-version", "3.13"],
        )
    assert not (source / "forbidden-wasm").exists()


@pytest.mark.parametrize(
    "outcome",
    [
        "success",
        "build-failure",
        "timeout",
        "missing-binary",
        "io-error",
        "cancel",
        "sample-failure",
        "sample-exception",
    ],
)
def test_scoreboard_codon_owns_scratch_for_one_measurement(
    monkeypatch, source_only_perf_checkout, tmp_path, outcome
):
    from types import SimpleNamespace

    source, custody = source_only_perf_checkout
    selected = []
    original = (
        KeyboardInterrupt("independent build cancellation")
        if outcome == "cancel"
        else RuntimeError("independent sampling error")
    )

    def build(command, **kwargs):
        output = Path(command[command.index("-o") + 1])
        selected.append(output.parent)
        assert output.is_relative_to(custody)
        assert output.parent.is_dir()
        if outcome == "io-error":
            raise OSError("independent build I/O")
        if outcome == "cancel":
            raise original
        if outcome != "missing-binary":
            output.write_bytes(b"independent Codon artifact")
        return SimpleNamespace(
            returncode=1 if outcome == "build-failure" else 0,
            timed_out=outcome == "timeout",
            stdout="",
            stderr="fixture failed",
        )

    def sample(binary, *args, **kwargs):
        assert Path(binary).parent == selected[-1]
        assert Path(binary).is_file()
        if outcome == "sample-exception":
            raise original
        return None if outcome == "sample-failure" else 0.5

    monkeypatch.setattr(
        scoreboard.harness_memory_guard, "guarded_completed_process", build
    )
    monkeypatch.setattr(scoreboard, "_measure_codon_warm", sample)
    runner = scoreboard.CodonRunner(str(tmp_path / "bin/codon"))
    cell = scoreboard.Cell(
        benchmark="tests/benchmarks/bench_fib.py",
        target="native",
        backend="native",
        profile="release-fast",
    )

    def measure_once():
        runner.measure_into(
            cell,
            script_path=scoreboard.REPO_ROOT / "tests/benchmarks/bench_fib.py",
            run_args=[],
            samples=1,
            warmup=0,
            rss_mb=64,
            timeout_s=1.0,
            log_lines=[],
        )

    if outcome in {"cancel", "sample-exception"}:
        with pytest.raises(type(original)) as caught:
            measure_once()
        assert caught.value is original
    else:
        measure_once()
    assert len(selected) == 1 and not selected[0].exists()
    assert runner._tmp_dir is None
    runner.close()
    assert not (source / "tmp").exists()


@pytest.mark.parametrize("caller", ["measure", "profile"])
@pytest.mark.parametrize("original_kind", ["classified", "unexpected", "cleanup-only"])
def test_actual_build_cleanup_failure_escapes_measurement_and_profile_callers(
    monkeypatch, source_only_perf_checkout, tmp_path, caller, original_kind
):
    import subprocess
    from tools import perf_scoreboard_profile as profiling
    from molt import temporary_artifacts

    source, _custody = source_only_perf_checkout
    script = source / "ordinary.py"
    script.write_text(
        'def main():\n    print(1)\n\nif __name__ == "__main__":\n    main()\n',
        encoding="utf-8",
    )
    original = (
        RuntimeError("independent classified build error")
        if original_kind == "classified"
        else OSError("independent compiler I/O")
    )
    outputs = []

    def compiler(command, **kwargs):
        outputs.append(Path(command[command.index("--out-dir") + 1]))
        assert outputs[0].is_dir()
        if original_kind != "cleanup-only":
            raise original
        return subprocess.CompletedProcess(command, 1, "", "ordinary failed build")

    monkeypatch.setattr(measure.bench, "_prune_backend_daemons", lambda env: None)
    monkeypatch.setattr(
        measure.bench.harness_memory_guard, "guarded_completed_process", compiler
    )
    monkeypatch.setattr(profiling, "_profiling_tmp_root", lambda env: tmp_path)
    with monkeypatch.context() as scoped:
        scoped.setattr(
            temporary_artifacts,
            "delete_path",
            lambda path: (False, "independent deletion refusal"),
        )
        with pytest.raises(
            OSError if original_kind == "cleanup-only" else type(original)
        ) as caught:
            if caller == "profile":
                profiling.build_profiling_binary(
                    script,
                    spec=NATIVE_CRANELIFT,
                    profile="release-fast",
                    target_python_version="3.12",
                    inner_loops=2,
                    log_lines=[],
                )
            else:
                measure.measure_cell(
                    script_path=script,
                    spec=NATIVE_CRANELIFT,
                    profile="release-fast",
                    samples=1,
                    warmup=0,
                    rss_mb=64,
                    timeout_s=1.0,
                    batch_server=None,
                    cpython_cmd=("fixture-cpython",),
                    target_python_version="3.12",
                    log_dir=source / "logs",
                )
        if original_kind != "cleanup-only":
            assert caught.value is original
        owners = caught.value.benchmark_temporary_directories
        assert len(owners) == (2 if caller == "profile" else 1)
        assert all(
            type(owner) is temporary_artifacts.OwnedTemporaryDirectory
            for owner in owners
        )
        assert Path(owners[0].name) == outputs[0]
        assert outputs[0].is_dir()
        assert not (source / "logs").exists()
    for owner in owners:
        owner.cleanup()
    assert not outputs[0].exists()


def test_codon_handled_build_io_survives_cleanup_failure(
    monkeypatch, source_only_perf_checkout, tmp_path
):
    from molt import temporary_artifacts

    original = OSError("independent Codon build I/O")
    runner = scoreboard.CodonRunner(str(tmp_path / "bin/codon"))
    output = []

    def compiler(command, **kwargs):
        output.append(Path(command[command.index("-o") + 1]))
        raise original

    monkeypatch.setattr(
        scoreboard.harness_memory_guard, "guarded_completed_process", compiler
    )
    cell = scoreboard.Cell(
        benchmark="tests/benchmarks/bench_fib.py",
        target="native",
        backend="native",
        profile="release-fast",
    )
    with monkeypatch.context() as scoped:
        scoped.setattr(
            temporary_artifacts,
            "delete_path",
            lambda path: (False, "independent deletion refusal"),
        )
        with pytest.raises(OSError) as caught:
            runner.measure_into(
                cell,
                script_path=scoreboard.REPO_ROOT / "tests/benchmarks/bench_fib.py",
                run_args=[],
                samples=1,
                warmup=0,
                rss_mb=64,
                timeout_s=1.0,
                log_lines=[],
            )
        assert caught.value is original
        assert original.benchmark_temporary_directories == (runner._tmp_dir,)
        assert type(runner._tmp_dir) is temporary_artifacts.OwnedTemporaryDirectory
        assert Path(runner._tmp_dir.name) == output[0].parent
        assert any(
            "independent deletion refusal" in note for note in original.__notes__
        )
    runner.close()
    assert runner._tmp_dir is None
    assert not output[0].parent.exists()


def test_nested_codon_and_native_cleanup_retain_both_exact_owners(
    monkeypatch, source_only_perf_checkout, tmp_path
):
    import json
    import subprocess
    from molt import temporary_artifacts

    source, custody = source_only_perf_checkout
    original = OSError("independent nested Codon build I/O")
    codon = scoreboard.CodonRunner(str(tmp_path / "bin/codon"))
    outputs = {}

    def compiler(command, **kwargs):
        if "--out-dir" in command:
            directory = Path(command[command.index("--out-dir") + 1])
            outputs["native"] = directory
            artifact = directory / "ordinary"
            artifact.write_bytes(b"independent native artifact")
            return subprocess.CompletedProcess(
                command,
                0,
                json.dumps(
                    {
                        "data": {
                            "output": str(artifact),
                            "observed_toolchain": {
                                "selected_profiles": {
                                    "backend": "native",
                                    "guest_profile": "release",
                                    "compiler_profile": "release",
                                    "runtime_profile": "release-fast",
                                    "target": "native",
                                }
                            },
                        },
                    }
                ),
                "",
            )
        outputs["codon"] = Path(command[command.index("-o") + 1]).parent
        raise original

    monkeypatch.setattr(measure.bench, "_prune_backend_daemons", lambda env: None)
    monkeypatch.setattr(
        measure.bench.harness_memory_guard, "guarded_completed_process", compiler
    )
    monkeypatch.setattr(
        measure,
        "_run_measured_command",
        lambda *a, **kw: measure.RunOutcome(
            True, 0.1, 1.0, "ok", 0, stdout="same\n", stderr=""
        ),
    )
    with monkeypatch.context() as scoped:
        scoped.setattr(
            temporary_artifacts,
            "delete_path",
            lambda path: (False, "independent nested deletion refusal"),
        )
        with pytest.raises(OSError) as caught:
            measure.measure_cell(
                script_path=scoreboard.REPO_ROOT / "tests/benchmarks/bench_fib.py",
                spec=NATIVE_CRANELIFT,
                profile="release-fast",
                samples=1,
                warmup=0,
                rss_mb=64,
                timeout_s=1.0,
                batch_server=None,
                cpython_cmd=("fixture-cpython",),
                target_python_version="3.12",
                log_dir=source / "logs",
                codon_runner=codon,
            )
        assert caught.value is original
        owners = original.benchmark_temporary_directories
        assert type(owners) is tuple and len(owners) == 2
        assert owners[0] is codon._tmp_dir
        assert owners[0] is not owners[1]
        assert [Path(owner.name) for owner in owners] == [
            outputs["codon"],
            outputs["native"],
        ]
        assert all(
            type(owner) is temporary_artifacts.OwnedTemporaryDirectory
            for owner in owners
        )
        assert all(
            Path(owner.name).is_dir() and Path(owner.name).is_relative_to(custody)
            for owner in owners
        )
        assert (
            len(
                [
                    note
                    for note in original.__notes__
                    if "nested deletion refusal" in note
                ]
            )
            == 2
        )
        # A repeated failed retirement preserves the same exact pair; no owner
        # may be duplicated or replace the other allocation in diagnostics.
        with pytest.raises(OSError) as repeated:
            measure.bench._cleanup_bench_temp(owners[0], original)
        assert repeated.value is original
        assert original.benchmark_temporary_directories is owners
    codon.close()
    owners[1].cleanup()
    assert all(not Path(owner.name).exists() for owner in owners)
    assert not (source / "logs").exists()


@pytest.mark.parametrize("cleanup_failure", [False, True])
def test_profiling_build_log_failure_closes_or_retains_untransferred_binary(
    monkeypatch, source_only_perf_checkout, tmp_path, cleanup_failure
):
    from tools import perf_scoreboard_profile as profiling
    from molt import temporary_artifacts

    owner = temporary_artifacts.OwnedTemporaryDirectory(dir=tmp_path)
    artifact = Path(owner.name) / "program"
    artifact.write_bytes(b"independent profiling artifact")
    binary = measure.bench.MoltBinary(artifact, owner, 0.1, 1.0)
    original = OSError("independent profiling build log failure")

    class FailingLog(list):
        def append(self, line):
            assert "profiling binary built" in line
            raise original

    monkeypatch.setattr(profiling, "_profiling_tmp_root", lambda env: tmp_path)
    monkeypatch.setattr(measure.bench, "prepare_molt_binary", lambda *a, **kw: binary)
    with monkeypatch.context() as scoped:
        if cleanup_failure:
            scoped.setattr(
                temporary_artifacts,
                "delete_path",
                lambda path: (False, "independent profiling deletion refusal"),
            )
        with pytest.raises(OSError) as caught:
            profiling.build_profiling_binary(
                scoreboard.REPO_ROOT / "tests/benchmarks/bench_fib.py",
                spec=NATIVE_CRANELIFT,
                profile="release-fast",
                target_python_version="3.12",
                inner_loops=2,
                log_lines=FailingLog(),
            )
        assert caught.value is original
        if cleanup_failure:
            assert original.benchmark_temporary_directories[-1] is owner
            assert len(original.benchmark_temporary_directories) == 2
            assert artifact.is_file()
    if cleanup_failure:
        for retained in original.benchmark_temporary_directories:
            retained.cleanup()
    assert not Path(owner.name).exists()


@pytest.mark.parametrize(
    ("capture_outcome", "cleanup_outcome", "nested_owner"),
    [
        ("success", "removed", False),
        ("io", "removed", False),
        ("cancel", "removed", False),
        ("io", "replaced", False),
        ("cancel", "replaced", False),
        ("success", "replaced", False),
        ("io", "refused", False),
        ("io", "refused", True),
    ],
)
def test_hot_controller_owns_successful_binary_through_capture_and_cleanup(
    monkeypatch,
    source_only_perf_checkout,
    tmp_path,
    capture_outcome,
    cleanup_outcome,
    nested_owner,
):
    from tools import perf_scoreboard_profile as profiling
    from molt import temporary_artifacts

    source, custody = source_only_perf_checkout
    owner = temporary_artifacts.OwnedTemporaryDirectory(dir=tmp_path)
    allocation = Path(owner.name)
    artifact = allocation / "program"
    artifact.write_bytes(b"independent successful profiling build")
    binary = measure.bench.MoltBinary(artifact, owner, 0.1, 1.0)
    original = (
        KeyboardInterrupt("independent capture cancellation")
        if capture_outcome == "cancel"
        else OSError("independent capture I/O failure")
    )
    unrelated = RuntimeError("unrelated active caller exception")
    saved_allocation = tmp_path / "saved-original-allocation"
    prior_owner = (
        temporary_artifacts.OwnedTemporaryDirectory(dir=tmp_path)
        if nested_owner
        else None
    )
    captures = []
    profile_result = {"refused": True, "refused_reason": "ordinary finite capture"}

    def build(script, **kwargs):
        assert artifact.read_bytes() == b"independent successful profiling build"
        return binary, {"available": True}

    def capture(path, **kwargs):
        captures.append(path)
        assert path == artifact
        assert Path(kwargs["env"]["MOLT_EXT_ROOT"]) == custody
        if cleanup_outcome == "replaced":
            allocation.rename(saved_allocation)
            allocation.mkdir()
            (allocation / "foreign-sentinel").write_bytes(b"must survive")
        if capture_outcome != "success":
            raise original
        return profile_result

    monkeypatch.setattr(profiling, "build_profiling_binary", build)
    monkeypatch.setattr(profiling, "capture_hot_only_profile", capture)
    script = scoreboard.REPO_ROOT / "tests/benchmarks/bench_fib.py"

    def run():
        return profiling.run_hot_only_profiles(
            scripts=[script],
            spec=NATIVE_CRANELIFT,
            profile="release-fast",
            target_python_version="3.12",
            inner_loops=2,
            rss_mb=64,
        )

    try:
        with monkeypatch.context() as scoped:
            if cleanup_outcome == "refused":
                scoped.setattr(
                    temporary_artifacts,
                    "delete_path",
                    lambda path: (False, "independent deletion refusal"),
                )
            if prior_owner is not None:
                with pytest.raises(OSError) as prior_failure:
                    measure.bench._cleanup_bench_temp(prior_owner, original)
                assert prior_failure.value is original
                assert original.benchmark_temporary_directories == (prior_owner,)
            # A successful controller must not mistake an exception in its
            # caller's active except block for its own operation failure.
            try:
                raise unrelated
            except RuntimeError:
                if capture_outcome == "success" and cleanup_outcome == "removed":
                    cells = run()
                    assert cells[0]["profile_result"] is profile_result
                else:
                    expected_error = (
                        ValueError if capture_outcome == "success" else type(original)
                    )
                    with pytest.raises(expected_error) as caught:
                        run()
                    assert caught.value is not unrelated
                    if capture_outcome != "success":
                        assert caught.value is original
                    if cleanup_outcome != "removed":
                        expected_owners = (
                            (prior_owner, owner)
                            if prior_owner is not None
                            else (owner,)
                        )
                        retained = caught.value.benchmark_temporary_directories
                        assert type(retained) is tuple
                        assert len(retained) == len(expected_owners)
                        assert all(a is b for a, b in zip(retained, expected_owners))
                    else:
                        assert not hasattr(
                            caught.value, "benchmark_temporary_directories"
                        )
            assert captures == [artifact]
            assert not hasattr(unrelated, "benchmark_temporary_directories")
            if cleanup_outcome == "removed":
                assert not allocation.exists()
            elif cleanup_outcome == "replaced":
                assert (allocation / "foreign-sentinel").read_bytes() == b"must survive"
                assert (saved_allocation / "program").is_file()
            else:
                assert artifact.is_file()
            assert not (source / "bench/results").exists()
    finally:
        if saved_allocation.exists():
            (allocation / "foreign-sentinel").unlink()
            allocation.rmdir()
            saved_allocation.rename(allocation)
        owner.cleanup()
        if prior_owner is not None:
            prior_owner.cleanup()


@pytest.mark.parametrize("profile", ["dev-fast", "release-output", "wasm-release"])
@pytest.mark.parametrize("guest_mismatch", [False, True])
@pytest.mark.parametrize("engine_mutation", [None, "build", "sample"])
def test_wasm_controller_measures_manifest_deployment_and_exact_guest_output(
    monkeypatch, source_only_perf_checkout, profile, guest_mismatch, engine_mutation
):
    """Actual builder/measurement/lifetime, independent compiler and sampler seams."""
    import json
    import subprocess

    source, _custody = source_only_perf_checkout
    builds = []
    resolutions = []
    calls = []
    guest_bytes = b"\0asm\1\0\0\0" + b"linked-runtime-and-app" * 100
    facts = next(
        row for row in EXPECTED_LANES if row[0] == "wasm" and row[3] == profile
    )
    selected = dict(
        zip(
            (
                "backend",
                "target",
                "guest_profile",
                "runtime_profile",
                "compiler_profile",
            ),
            facts,
        )
    )
    script = source / "bench_probe.py"
    script.write_text('print("guest output")\n', encoding="utf-8")
    for key in ("NODE_OPTIONS", "NODE_NO_WARNINGS", "PYTHONHOME"):
        monkeypatch.setenv(key, "independent-ambient-policy")
    monkeypatch.setattr(measure.bench, "_prune_backend_daemons", lambda env: None)

    def compiler(command, **kwargs):
        root = Path(command[command.index("--out-dir") + 1])
        builds.append(root)
        if engine_mutation == "build":
            node_path.write_bytes(b"replacement Node engine image")
        assert command[command.index("--target") + 1] == "wasm"
        assert "--linked" in command and "--require-linked" in command
        assert command[command.index("--python-version") + 1] == "3.13"
        assert kwargs["env"]["MOLT_WASM_CARGO_PROFILE"] == profile
        linked = root / "selected-linked.wasm"
        linked.write_bytes(guest_bytes)
        # A raw intermediate and a decoy are never execution/size authorities.
        intermediate = root / "output.wasm"
        intermediate.write_bytes(b"raw")
        (root / "decoy.wasm").write_bytes(b"wrong" * 2000)
        manifest = root / "manifest.json"
        manifest.write_text(
            json.dumps(
                {
                    "version": WASM_EXECUTION_MANIFEST_VERSION,
                    "mode": "linked",
                    "modules": {"linked": {"path": linked.name}},
                }
            ),
            encoding="utf-8",
        )
        return subprocess.CompletedProcess(
            command,
            0,
            json.dumps(
                {
                    "data": {
                        "output": str(intermediate),
                        "artifacts": {
                            "linked_wasm": str(linked),
                            "manifest": str(manifest),
                        },
                        "observed_toolchain": {"selected_profiles": selected},
                    }
                }
            ),
            "",
        )

    from molt import node_runtime

    node_path = source / "selected-node"
    node_path.write_bytes(b"independent Node engine image")
    monkeypatch.setattr(node_runtime, "resolve_executable", lambda *a, **kw: node_path)
    monkeypatch.setattr(node_runtime, "pinned_executable", lambda *a, **kw: None)

    def node_probe(command, **kwargs):
        resolutions.append(kwargs)
        assert kwargs["env"].get("NODE_OPTIONS") is None
        assert kwargs["env"].get("NODE_NO_WARNINGS") is None
        assert kwargs["env"].get("PYTHONHOME") is None
        assert kwargs["env"].get("PYTHONPATH") is None
        assert command == [str(node_path), "-p", "process.versions.node"]
        return subprocess.CompletedProcess(command, 0, "24.1.0\n", "")

    monkeypatch.setattr(node_runtime.process_guard, "run_completed_command", node_probe)

    def sample(command, **kwargs):
        calls.append((command, kwargs))
        if engine_mutation == "sample":
            node_path.write_bytes(b"replacement Node engine image")
        is_molt = command[0] == str(node_path)
        stderr = "guest stderr\n"
        if is_molt:
            capture = kwargs["guest_stderr_path"]
            assert capture == builds[0] / "guest-stderr.bin"
            assert capture.is_file()
            assert command == [
                str(node_path),
                str(source / "wasm/run_wasm.js"),
                "--guest-stderr",
                str(capture),
                "--",
                str(builds[0] / "manifest.json"),
            ]
            assert (builds[0] / "selected-linked.wasm").read_bytes() == guest_bytes
            if guest_mismatch and "warmup" in kwargs["label"]:
                stderr = "different warmup guest stderr\n"
        else:
            assert kwargs.get("guest_stderr_path") is None
        return measure.RunOutcome(
            True,
            0.1 if is_molt else 0.2,
            1.0,
            "ok",
            0,
            stdout="guest output\n",
            stderr=stderr,
            host_stderr="host warning retained\n" if is_molt else "",
        )

    monkeypatch.setattr(
        measure.bench.harness_memory_guard, "guarded_completed_process", compiler
    )
    monkeypatch.setattr(measure.bench, "resolve_benchmark_run_args", lambda path: [])
    monkeypatch.setattr(measure, "_run_measured_command", sample)
    cell = measure.measure_cell(
        script_path=script,
        spec=WASM,
        profile=profile,
        samples=2,
        warmup=1,
        repeat=2,
        rss_mb=64,
        timeout_s=1.0,
        batch_server=None,
        cpython_cmd=("fixture-python",),
        target_python_version="3.13",
        log_dir=source / "logs",
    )
    assert cell.execution_host["executable"] == str(node_path)
    assert cell.execution_host["version"] == "24.1.0"
    assert len(builds) == 1 and len(resolutions) == 1
    if engine_mutation is not None:
        assert not cell.molt_ok
        assert cell.molt_failure_status == "run_host_changed"
        assert "changed" in cell.note
        assert len(calls) == (0 if engine_mutation == "build" else 1)
        assert not builds[0].exists()
        return
    assert len(calls) == 14  # each runtime: cold + two (warmup + two timed)
    assert cell.build_ok and cell.molt_ok and cell.cpython_ok
    assert cell.run_blocked is False
    assert cell.output_parity["ok"] is not guest_mismatch
    assert cell.binary_size_kib == round(len(guest_bytes) / 1024, 1)
    assert cell.repeat_passes == 2
    log = (source / cell.log_artifact).read_text(encoding="utf-8")
    assert log.count("host warning retained") == 7
    assert not builds[0].exists()


def test_wasm_hot_only_refuses_before_source_read_or_build(monkeypatch):
    from tools import perf_scoreboard_profile as profiling

    monkeypatch.setattr(
        measure.bench,
        "prepare_molt_binary",
        lambda *a, **kw: pytest.fail("WASM reached native profiler build"),
    )
    binary, metadata = profiling.build_profiling_binary(
        Path("never-read.py"),
        spec=WASM,
        profile="wasm-release",
        target_python_version="3.12",
        inner_loops=2,
        log_lines=[],
    )
    assert binary is None and metadata["refused"]
    assert "native sampler" in metadata["reason"]


@pytest.mark.parametrize("sampler_kind", ["cycle", "hot"])
@pytest.mark.parametrize(
    "outcome",
    ["success", "sampler-failure", "workload-failure", "read-error", "cancel"],
)
@pytest.mark.parametrize("cleanup_failure", [False, True])
def test_sampler_output_owns_exact_directory_through_every_exit(
    monkeypatch, tmp_path, sampler_kind, outcome, cleanup_failure
):
    from types import SimpleNamespace
    from tools import perf_scoreboard_profile as profiling
    from molt import temporary_artifacts

    module = scoreboard if sampler_kind == "cycle" else profiling
    allocations = []
    children = []
    original = (
        KeyboardInterrupt("independent cancellation")
        if outcome == "cancel"
        else OSError("independent sampler failure")
    )
    allocate = measure.bench._new_bench_temp

    def owned(root, env):
        owner = allocate(root, env)
        allocations.append(owner)
        return owner

    class Child:
        returncode = 0
        closed = False

        def wait(self, **kwargs):
            self.closed = True
            return 0

    def start(command, **kwargs):
        sampler = command[0] == "/fixture/sample"
        if (sampler and outcome == "sampler-failure") or (
            not sampler and outcome == "workload-failure"
        ):
            raise original
        if sampler:
            output = Path(command[command.index("-f") + 1])
            assert output.parent == Path(allocations[0].name)
            output.write_text("independent sampler output", encoding="utf-8")
        child = Child()
        children.append(child)
        return child

    def read(path, **kwargs):
        assert Path(path).is_file()
        assert all(child.closed for child in children)
        if outcome in {"read-error", "cancel"}:
            raise original
        return [{"symbol": "molt_work", "lib": "program", "self_samples": 100}]

    monkeypatch.setattr(measure.bench, "_new_bench_temp", owned)
    monkeypatch.setattr(module, "_resolve_sampler", lambda: "/fixture/sample")
    monkeypatch.setattr(module, "_profiling_popen", start)
    monkeypatch.setattr(
        module, "_terminate", lambda child: setattr(child, "closed", True)
    )
    monkeypatch.setattr(module, "read_self_time", read)
    monkeypatch.setattr(
        profiling,
        "_time_one_run",
        lambda *a, **kw: SimpleNamespace(ok=True, elapsed_s=4.0),
    )
    env = {"MOLT_BENCH_TMP_ROOT": str(tmp_path)}

    def run():
        if sampler_kind == "cycle":
            return scoreboard.capture_cycle_profile(
                ["program"], env=env, rss_mb=64, timeout_s=1.0, sample_seconds=1
            )
        return profiling.capture_hot_only_profile(
            tmp_path / "program",
            run_args=[],
            env=env,
            rss_mb=64,
            inner_loops=2,
            warmup_s=0.0,
            window_s=1.0,
        )

    with monkeypatch.context() as scoped:
        if cleanup_failure:
            scoped.setattr(
                temporary_artifacts,
                "delete_path",
                lambda path: (False, "independent cleanup refusal"),
            )
        if outcome in {"read-error", "cancel"} or cleanup_failure:
            with pytest.raises(
                type(original) if outcome in {"read-error", "cancel"} else OSError
            ) as caught:
                run()
            if outcome in {"read-error", "cancel"}:
                assert caught.value is original
            if cleanup_failure:
                assert caught.value.benchmark_temporary_directories == tuple(
                    allocations
                )
        else:
            result = run()
            assert result["available"] is (outcome == "success")
    assert len(allocations) == 1
    assert all(child.closed for child in children)
    if cleanup_failure:
        allocations[0].cleanup()
    assert not Path(allocations[0].name).exists()
