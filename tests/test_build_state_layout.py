import os
from pathlib import Path
import shutil

import pytest

from molt.build_state_layout import build_state_root, project_build_state_root
from molt.memory_guard_paths import (
    harness_guard_artifact_dir,
    memory_guard_state_root,
    pytest_guard_summary_dir,
)
from molt.dx import (
    development_artifact_env,
    project_cargo_target_dir,
    proof_scratch_root,
)
from molt.cli.runtime_paths import _build_state_root_cached
from tools.build_control_path import build_control_output
from tools.harness_memory_guard import canonical_harness_env
from tests.process_guard_common import install_module_view


@pytest.mark.parametrize("pinned_session", [False, True])
@pytest.mark.parametrize("profile", ["absolute", "relative", "default"])
@pytest.mark.parametrize("target", ["absolute", "relative", "default"])
@pytest.mark.parametrize("explicit_state", [False, True])
def test_ci_build_control_output_uses_admitted_consumer_root(
    tmp_path: Path,
    target: str,
    explicit_state: bool,
    profile: str,
    pinned_session: bool,
):
    repo = Path(__file__).resolve().parents[1]
    env = {"MOLT_EXT_ROOT": str(tmp_path / "canonical")}
    if pinned_session:
        env["MOLT_SESSION_ID"] = "retained-ci-diagnostics"
    if target != "default":
        env["CARGO_TARGET_DIR"] = (
            str(tmp_path / "payload")
            if target == "absolute"
            else "target/ci-control-probe"
        )
    if explicit_state:
        env["MOLT_BUILD_STATE_DIR"] = str(tmp_path / "canonical" / "operator-state")
    if profile != "default":
        env["MOLT_GUARD_PROFILE_LOG"] = (
            str(tmp_path / "selected-profile.jsonl")
            if profile == "absolute"
            else "diagnostics/selected-profile.jsonl"
        )
    admitted = canonical_harness_env(env, repo_root=repo)
    expected = project_build_state_root(repo, admitted)
    expected_profile = (
        tmp_path / "selected-profile.jsonl"
        if profile == "absolute"
        else repo / "diagnostics/selected-profile.jsonl"
        if profile == "relative"
        else harness_guard_artifact_dir(repo, admitted) / "commands.jsonl"
    )
    producer_env = development_artifact_env(
        repo, admitted, session_prefix="test-loop-join-dev", create_dirs=False
    )
    producer_root = _build_state_root_cached(
        str(repo),
        producer_env.get("MOLT_BUILD_STATE_DIR"),
        os.fspath(project_cargo_target_dir(repo, producer_env)),
        producer_env.get("MOLT_EXT_ROOT"),
    )
    assert producer_root == expected
    outputs = dict(
        line.split("=", 1)
        for line in build_control_output(env, repo_root=repo).splitlines()
    )
    assert outputs == {
        "root": str(expected),
        "profile_log": str(expected_profile),
        "guard_state_root": str(memory_guard_state_root(repo, admitted)),
        "pytest_guard_root": str(pytest_guard_summary_dir(repo, admitted)),
        "proof_scratch_root": str(tmp_path / "canonical" / "tmp"),
    }
    assert not expected_profile.exists(), "profile projection must not create evidence"
    assert expected.is_relative_to(tmp_path / "canonical")
    assert not expected.exists(), "path projection must not create control directories"


@pytest.mark.parametrize("queued", [False, True])
def test_ci_evidence_and_tool_outputs_share_the_admitted_proof_scratch_root(
    tmp_path: Path, queued: bool
) -> None:
    repo = tmp_path / "repo"
    repo.mkdir()
    env = {"MOLT_EXT_ROOT": str(tmp_path / "canonical")}
    expected = tmp_path / "canonical" / "tmp"
    if queued:
        expected = tmp_path / "queue" / "issued-scratch"
        env["MOLT_PROOF_SCRATCH_ROOT"] = str(expected)
    admitted = canonical_harness_env(env, repo_root=repo)
    producer_env = development_artifact_env(repo, admitted, create_dirs=False)
    outputs = dict(
        line.split("=", 1)
        for line in build_control_output(env, repo_root=repo).splitlines()
    )

    assert outputs["proof_scratch_root"] == str(expected)
    assert proof_scratch_root(repo, producer_env) == expected
    assert not expected.is_relative_to(repo)
    assert not expected.exists(), "path projection must not create evidence roots"


def test_same_target_shares_canonical_control_across_receipt_roots(tmp_path: Path):
    repo = tmp_path / "repo"
    artifact = tmp_path / "canonical"
    target = tmp_path / "output" / "cargo-target"
    first = {
        "MOLT_EXT_ROOT": str(artifact),
        "CARGO_TARGET_DIR": str(target),
        "MOLT_DIFF_ROOT": str(artifact / "receipt-one"),
    }
    second = {**first, "MOLT_DIFF_ROOT": str(artifact / "receipt-two")}
    expected = build_state_root(
        project_root=repo, cargo_target=target, environment=first
    )
    assert expected == build_state_root(
        project_root=repo, cargo_target=target, environment=second
    )
    assert expected == project_build_state_root(repo, first)
    assert expected == _build_state_root_cached(
        str(repo), None, str(target), str(artifact)
    )
    assert expected != build_state_root(
        project_root=repo,
        cargo_target=target.parent / "other",
        environment=first,
    )
    assert expected.is_relative_to(artifact / "tmp" / "build-control")


def test_explicit_build_state_override_is_preserved(tmp_path: Path):
    repo = tmp_path / "repo"
    target = tmp_path / "target"
    env = {
        "MOLT_EXT_ROOT": str(tmp_path / "canonical"),
        "CARGO_TARGET_DIR": str(target),
        "MOLT_BUILD_STATE_DIR": "operator-state",
    }
    expected = repo / "operator-state"
    assert (
        build_state_root(project_root=repo, cargo_target=target, environment=env)
        == expected
    )
    assert project_build_state_root(repo, env) == expected


@pytest.mark.usefixtures("developer_host_context")
@pytest.mark.parametrize("session", ["none", "pinned", "generated"])
@pytest.mark.parametrize(
    "request_env",
    [
        "none",
        "artifact-root-only",
        "MOLT_PREFER_EXTERNAL_ARTIFACTS",
        "MOLT_REQUIRE_EXTERNAL_ARTIFACTS",
    ],
)
def test_every_consumer_reads_one_project_cargo_target(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
    request_env: str,
    session: str,
) -> None:
    """HF-133: the CLI default target has one rule and every reader agrees.

    A project builds in ``<project>/target`` unless a development artifact
    request moves it to the artifact root; ``MOLT_EXT_ROOT`` alone does not.
    Only a pinned session scopes it.
    """
    from molt.backend_daemon_custody import backend_daemon_root_from_env
    from molt.cli import (
        backend_execution,
        cargo_execution,
        lockfiles,
        mlir_backend,
        runtime_paths,
    )
    from molt.cli.cargo_execution import _cargo_build_env
    from molt.cli.wasm_host import (
        molt_wasm_host_exe_name,
        resolve_molt_wasm_host_binary,
    )
    import tools.compile_governor as compile_governor

    project = tmp_path / "project"
    project.mkdir()
    external = tmp_path / "external"
    monkeypatch.setattr(cargo_execution, "compiler_source_root", lambda: project)
    base = project
    if request_env != "none":
        monkeypatch.setenv("MOLT_EXT_ROOT", str(external))
    if request_env.startswith("MOLT_"):
        monkeypatch.setenv(request_env, "1")
        base = external.resolve()
    if session != "none":
        monkeypatch.setenv("MOLT_SESSION_ID", "lane-a")
    if session == "generated":
        monkeypatch.setenv("MOLT_SESSION_ID_GENERATED", "1")
    expected = base / "target"
    if base != project:
        from molt.dx import checkout_component

        expected = expected / "checkouts" / checkout_component(project)
    if session == "pinned":
        expected = expected / "sessions" / "lane-a"
    # The Cargo build environment creates the run context's roots; keep the
    # toolchain root, which a plain clone would place in itself, in scratch.
    monkeypatch.setenv("MOLT_TARGET_ROOT", str(tmp_path / "toolchain-root"))
    monkeypatch.setattr(backend_execution, "installed_compiler", lambda _root: None)
    install_module_view(
        monkeypatch, "shutil", shutil, mlir_backend, which=lambda _: None
    )
    monkeypatch.delenv("MOLT_WASM_HOST_BIN", raising=False)

    assert project_cargo_target_dir(project, os.environ) == expected
    assert runtime_paths._cargo_target_root(project) == expected
    control = build_state_root(
        project_root=project, cargo_target=expected, environment=os.environ
    )
    assert runtime_paths._build_state_root(project) == control
    assert project_build_state_root(project, os.environ) == control
    assert (
        backend_daemon_root_from_env(os.environ, project_root=project)
        == control / "backend_daemon"
    )
    assert lockfiles._lock_check_cache_path(project, "uv") == (
        expected / "lock_checks" / "uv.json"
    )
    assert backend_execution._backend_bin_path(project, "dev-fast").parent == (
        expected / "dev-fast"
    )
    host = expected / "dev-fast" / molt_wasm_host_exe_name()
    host.parent.mkdir(parents=True)
    host.write_bytes(b"host")
    assert resolve_molt_wasm_host_binary(project, cargo_profile="dev-fast") == str(host)
    mlir = expected / "release" / mlir_backend._mlir_backend_executable_name()
    mlir.parent.mkdir(parents=True)
    mlir.write_bytes(b"mlir")
    assert mlir_backend._find_mlir_backend_binary(project) == mlir
    if request_env.startswith("MOLT_"):
        # The CLI's Cargo builds use the same target: no per-process session.
        assert _cargo_build_env()["CARGO_TARGET_DIR"] == str(expected)
    compiler_root = Path(compile_governor.__file__).resolve().parents[1]
    assert compile_governor._guard_root(os.environ) == (
        project_build_state_root(compiler_root, os.environ) / "compile_guard"
    )


def test_daemon_session_sweep_stays_in_its_source_checkout(tmp_path, monkeypatch):
    from molt.cli import backend_execution
    from molt.dx import CANONICAL_RUN_ENV_KEYS

    for key in CANONICAL_RUN_ENV_KEYS:
        monkeypatch.delenv(key, raising=False)
    monkeypatch.delenv("MOLT_BUILD_STATE_DIR", raising=False)
    artifact = tmp_path / "artifacts"
    first, second = tmp_path / "first", tmp_path / "second"
    for root in (first, second):
        root.mkdir()
    monkeypatch.setenv("MOLT_EXT_ROOT", str(artifact))
    monkeypatch.setenv("MOLT_PREFER_EXTERNAL_ARTIFACTS", "1")
    monkeypatch.setenv("MOLT_SESSION_ID", "current")
    caller = dict(os.environ)

    def dead_identity(source, target):
        target.mkdir(parents=True)
        state = (
            build_state_root(
                project_root=source, cargo_target=target, environment=caller
            )
            / "backend_daemon"
        )
        state.mkdir(parents=True)
        record = state / "dead.identity.json"
        # Invalid content is independently stale; no process needs to be probed.
        record.write_text("{}", encoding="utf-8")
        return record

    current = dead_identity(first, project_cargo_target_dir(first, caller))
    sibling = dead_identity(
        first, project_cargo_target_dir(first, {**caller, "MOLT_SESSION_ID": "other"})
    )
    foreign = dead_identity(second, project_cargo_target_dir(second, caller))
    obsolete = dead_identity(first, artifact / "target" / "sessions" / "old")
    assert backend_execution._sweep_orphaned_backend_daemon_locks(first) == 2
    assert not current.exists() and not sibling.exists()
    assert foreign.exists() and obsolete.exists()
