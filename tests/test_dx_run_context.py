from __future__ import annotations

import json
import os
from pathlib import Path

import molt.dx as dx
from molt import custody_layout
import pytest
from molt.dx import (
    CANONICAL_RUN_ENV_KEYS,
    DX_ENV_KEYS,
    DxProject,
    RunContext,
    bind_repo_src_pythonpath,
    development_artifacts_requested,
    development_artifact_env,
    render_env,
)
from molt.path_custody import (
    host_path_is_within,
    pure_path_is_within,
)
from tools import hosted_ci_env, run_context_env


# Unit cases create independent synthetic project roots. The process-wide
# hosted checkout contract belongs to GITHUB_WORKSPACE, not those fixtures.
pytestmark = pytest.mark.usefixtures("developer_host_context")


def _clear_run_context_env(monkeypatch: pytest.MonkeyPatch) -> None:
    for key in set(CANONICAL_RUN_ENV_KEYS) | set(DX_ENV_KEYS):
        monkeypatch.delenv(key, raising=False)


def _without_compiler_wasm_toolchain(monkeypatch: pytest.MonkeyPatch) -> None:
    # `--dx` also projects the compiler checkout's own WASI SDK, whose custody
    # needs the hosted contract the autouse fixture removes.
    # These cases check run-context keys, and the WASM projection has its own.
    monkeypatch.setattr(
        run_context_env, "apply_provisioned_wasm_toolchain", lambda _root, _env: ()
    )


def _github_actions_custody_env(
    repo_root: Path, runner_temp: Path, *, sha: str = "a" * 40
) -> dict[str, str]:
    workflow = repo_root / ".github" / "workflows" / "ci.yml"
    workflow.parent.mkdir(parents=True, exist_ok=True)
    workflow.write_text("name: test\n", encoding="utf-8")
    runner_tool_cache = runner_temp.parent / "runner-tool-cache"
    runner_tool_cache.mkdir(parents=True, exist_ok=True)
    env = hosted_ci_env.hosted_job_env(
        repo_root,
        runner_temp,
        sha=sha,
        event_name="push",
        ref="refs/heads/main",
        job="platform-portability",
        run_id="12345",
        run_attempt="2",
    )
    env["RUNNER_TOOL_CACHE"] = str(runner_tool_cache.resolve())
    env["PATH"] = os.environ.get("PATH", "")
    return env


def test_run_context_installs_repo_local_defaults(tmp_path: Path) -> None:
    env = RunContext(tmp_path, session_prefix="test").canonical_env(
        {"PATH": "/usr/bin"},
        create_dirs=False,
    )

    assert env["MOLT_EXT_ROOT"] == str(tmp_path.resolve())
    assert env["MOLT_SESSION_ID"].startswith("test-")
    assert env["MOLT_SESSION_ID_GENERATED"] == "1"
    # No explicit MOLT_SESSION_ID -> STABLE persistent target dir (survives across
    # sessions for warm incremental rebuilds), not a per-session cold dir.
    assert env["CARGO_TARGET_DIR"] == str(tmp_path.resolve() / "target")
    assert env["MOLT_DIFF_CARGO_TARGET_DIR"] == env["CARGO_TARGET_DIR"]
    # Incremental is ON by default now (fast warm rebuilds against the persistent
    # target dir); it is forced to "0" only where sccache is actually wired, which
    # canonical_env does not do.
    assert env["CARGO_INCREMENTAL"] == "1"
    assert env["MOLT_CACHE"] == str(tmp_path.resolve() / ".molt_cache")
    # Scratch never lands in the source tree, even for a repo-local root.
    scratch = custody_layout.out_of_tree_scratch_root(tmp_path)
    assert env["MOLT_DIFF_ROOT"] == str(scratch / "diff")
    assert env["MOLT_DIFF_TMPDIR"] == str(scratch)
    assert env["UV_CACHE_DIR"] == str(tmp_path.resolve() / ".uv-cache")
    assert env["UV_PROJECT_ENVIRONMENT"].startswith(
        str(tmp_path.resolve() / "uv-project-envs")
    )
    assert env["PIP_CACHE_DIR"] == str(tmp_path.resolve() / ".pip-cache")
    assert env["PYTHONPYCACHEPREFIX"] == str(scratch / "pycache")
    assert env["TMPDIR"] == str(scratch)
    assert env["TMP"] == env["TMPDIR"]
    assert env["TEMP"] == env["TMPDIR"]


def test_run_context_preserves_explicit_root_and_session(tmp_path: Path) -> None:
    explicit_root = tmp_path / "external"
    explicit_target = tmp_path / "target-custom"
    env = RunContext(tmp_path, session_prefix="test").canonical_env(
        {
            "MOLT_EXT_ROOT": str(explicit_root),
            "CARGO_TARGET_DIR": str(explicit_target),
            "CARGO_INCREMENTAL": "1",
            "MOLT_SESSION_ID": "caller-session",
        },
        create_dirs=False,
    )

    assert env["MOLT_EXT_ROOT"] == str(explicit_root.resolve())
    assert env["CARGO_TARGET_DIR"] == str(explicit_target)
    assert env["MOLT_DIFF_CARGO_TARGET_DIR"] == str(explicit_target)
    assert env["CARGO_INCREMENTAL"] == "1"
    assert env["MOLT_SESSION_ID"] == "caller-session"
    assert "MOLT_SESSION_ID_GENERATED" not in env


def test_target_dir_stable_by_default_session_scoped_only_when_pinned(
    tmp_path: Path,
) -> None:
    # The cold-every-session killer: without a caller-pinned MOLT_SESSION_ID the
    # Cargo target dir is STABLE (persistent incremental cache reused across
    # sessions/processes); a caller that pins MOLT_SESSION_ID (perf/bench/test-shard
    # isolation) still gets an isolated per-session dir. Regressing this to a
    # per-PID default reintroduces a full cold compile on every invocation.
    ctx = RunContext(tmp_path, session_prefix="test")
    stable = ctx.canonical_env({"PATH": "/usr/bin"}, create_dirs=False)
    assert stable["CARGO_TARGET_DIR"] == str(tmp_path.resolve() / "target")
    assert stable["MOLT_SESSION_ID_GENERATED"] == "1"

    reentered = ctx.canonical_env(
        {
            "PATH": "/usr/bin",
            "MOLT_SESSION_ID": stable["MOLT_SESSION_ID"],
            "MOLT_SESSION_ID_GENERATED": "1",
        },
        create_dirs=False,
    )
    assert reentered["CARGO_TARGET_DIR"] == str(tmp_path.resolve() / "target")

    pinned = ctx.canonical_env(
        {"PATH": "/usr/bin", "MOLT_SESSION_ID": "shard-7"}, create_dirs=False
    )
    assert pinned["CARGO_TARGET_DIR"] == str(
        tmp_path.resolve() / "target" / "sessions" / "shard-7"
    )
    assert "MOLT_SESSION_ID_GENERATED" not in pinned


def test_pinned_sessions_sharing_a_long_prefix_get_distinct_targets(
    tmp_path: Path,
) -> None:
    # Agent lanes are named `agent-<task>-<pid>`; a long task name pushes the
    # PID past character 32, where the old component cut every ID.
    first = "agent-unit-agent-440e87baa421421f9c0d6f2e-67518"
    second = "agent-unit-agent-440e87baa421421f9c0d6f2e-67519"
    assert first[:32] == second[:32]
    ctx = RunContext(tmp_path, session_prefix="test")

    targets = {
        ctx.canonical_env(
            {"PATH": "/usr/bin", "MOLT_SESSION_ID": session}, create_dirs=False
        )["CARGO_TARGET_DIR"]
        for session in (first, second)
    }

    assert len(targets) == 2
    for target in targets:
        path = Path(target)
        assert path.parent == tmp_path.resolve() / "target" / "sessions"
        assert len(path.name) <= 32
        assert set(path.name) <= set(
            "abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789-_"
        )


@pytest.mark.parametrize(
    ("session_id", "component"),
    [
        ("shard-7", "shard-7"),
        ("a" * 32, "a" * 32),
        # Rewritten IDs keep 15 sanitized characters, then 16 hex digits of
        # sha256(session ID): printf %s 'alpha/session:beta' | shasum -a 256.
        ("alpha/session:beta", "alpha_session_b-575cb2aec94ffa27"),
        # A safe ID that already has the digest shape cannot pass through, or
        # it could name another session's directory.
        ("dev-0123456789abcdef", "dev-0123456789a-f728f81103144400"),
    ],
)
def test_session_artifact_component_keeps_short_safe_ids_and_digests_the_rest(
    session_id: str, component: str
) -> None:
    assert dx.session_artifact_component(session_id) == component


def test_explicit_development_session_overrides_outer_generated_provenance(
    tmp_path: Path,
) -> None:
    env = development_artifact_env(
        tmp_path,
        {
            "PATH": "/usr/bin",
            "MOLT_SESSION_ID": "guard-outer",
            "MOLT_SESSION_ID_GENERATED": "1",
        },
        session_id="proof-shard",
        create_dirs=False,
    )

    assert env["MOLT_SESSION_ID"] == "proof-shard"
    assert "MOLT_SESSION_ID_GENERATED" not in env
    assert env["CARGO_TARGET_DIR"] == str(
        tmp_path.resolve() / "target" / "sessions" / "proof-shard"
    )


def test_repassed_outer_generated_session_keeps_shared_target_provenance(
    tmp_path: Path,
) -> None:
    env = development_artifact_env(
        tmp_path,
        {
            "PATH": "/usr/bin",
            "MOLT_SESSION_ID": "guard-outer",
            "MOLT_SESSION_ID_GENERATED": "1",
        },
        session_id="guard-outer",
        create_dirs=False,
    )

    assert env["MOLT_SESSION_ID"] == "guard-outer"
    assert env["MOLT_SESSION_ID_GENERATED"] == "1"
    assert env["CARGO_TARGET_DIR"] == str(tmp_path.resolve() / "target")


def test_live_dx_docs_do_not_reintroduce_session_scoped_default() -> None:
    repo = Path(__file__).resolve().parents[1]
    docs = [
        repo / "AGENTS.md",
        repo / "docs" / "agent" / "AGENTS.full.md",
        repo / "docs" / "agent" / "CLAUDE.full.md",
        repo / "docs" / "agent" / "PROOF_QUEUE.md",
        repo / "docs" / "ops" / "INTEGRATION.md",
        repo / "docs" / "design" / "foundation" / "56_dx_buildspeed_tooling.md",
        repo
        / "docs"
        / "design"
        / "foundation"
        / "73_efficient_builds_toolchain_provisioning_binary_cdn.md",
        repo / "docs" / "OPERATIONS.md",
    ]
    forbidden = [
        "MOLT_SESSION_ID` **must be set BEFORE",
        "Default Cargo output is session-scoped",
        "Cargo output remains session-scoped",
        "ALWAYS set `MOLT_SESSION_ID` before ANY build command",
        'CARGO_TARGET_DIR="${MOLT_EXT_ROOT:?}/target/sessions/$MOLT_SESSION_ID"',
        "Agents **MUST** use `export MOLT_SESSION_ID",
    ]

    offenders: list[str] = []
    for path in docs:
        text = path.read_text(encoding="utf-8")
        for needle in forbidden:
            if needle in text:
                offenders.append(f"{path.relative_to(repo)}: {needle}")

    assert offenders == []


def test_development_artifact_env_session_id_overrides_ambient_session(
    tmp_path: Path,
) -> None:
    env = development_artifact_env(
        tmp_path,
        {
            "MOLT_SESSION_ID": "pytest-ambient",
        },
        session_prefix="test",
        session_id="stable-proof",
        create_dirs=False,
    )

    assert env["MOLT_SESSION_ID"] == "stable-proof"
    assert env["CARGO_TARGET_DIR"] == str(
        Path(env["MOLT_EXT_ROOT"]) / "target" / "sessions" / "stable-proof"
    )


def test_run_context_prefers_healthy_external_artifact_root(tmp_path: Path) -> None:
    repo_root = tmp_path / "repo"
    external_root = tmp_path / "external-ssd" / "Molt"
    repo_root.mkdir()
    env = RunContext(
        repo_root,
        session_prefix="test",
        prefer_external_artifacts=True,
    ).canonical_env(
        {
            "MOLT_EXTERNAL_ARTIFACT_ROOTS": str(external_root),
            "MOLT_EXTERNAL_MIN_FREE_GB": "0",
            "TMPDIR": "/var/folders/example/T/",
        },
        create_dirs=True,
    )

    resolved_external = external_root.resolve()
    assert env["MOLT_EXT_ROOT"] == str(resolved_external)
    assert env["CARGO_TARGET_DIR"] == str(resolved_external / "target")
    assert env["MOLT_DIFF_TMPDIR"] == str(resolved_external / "tmp")
    assert resolved_external.is_dir()


def test_run_context_prefers_windows_external_drive_artifact_root_by_default(
    monkeypatch,
    tmp_path: Path,
) -> None:
    repo_root = tmp_path / "repo"
    external_root = tmp_path / "external-drive" / "Molt"
    repo_root.mkdir()

    env = RunContext(
        repo_root,
        session_prefix="test",
        prefer_external_artifacts=True,
    ).canonical_env(
        {
            "MOLT_EXTERNAL_ARTIFACT_ROOTS": str(external_root),
            "MOLT_EXTERNAL_MIN_FREE_GB": "0",
        },
        create_dirs=True,
    )

    resolved_external = external_root.resolve()
    assert env["MOLT_EXT_ROOT"] == str(resolved_external)
    assert env["CARGO_TARGET_DIR"] == str(resolved_external / "target")
    assert env["MOLT_DIFF_TMPDIR"] == str(resolved_external / "tmp")
    assert env["TMPDIR"] == str(resolved_external / "tmp")
    assert resolved_external.is_dir()


def test_run_context_skips_unhealthy_windows_external_candidate(
    monkeypatch,
    tmp_path: Path,
) -> None:
    repo_root = tmp_path / "repo"
    unhealthy = tmp_path / "unhealthy" / "Molt"
    healthy = tmp_path / "healthy" / "Molt"
    repo_root.mkdir()

    def fake_accepts_child_dirs(path: Path, *, create_dirs: bool) -> bool:
        del create_dirs
        return path != unhealthy

    monkeypatch.setattr(
        dx, "_artifact_root_accepts_child_dirs", fake_accepts_child_dirs
    )

    env = RunContext(
        repo_root,
        session_prefix="test",
        prefer_external_artifacts=True,
    ).canonical_env(
        {
            "MOLT_EXTERNAL_ARTIFACT_ROOTS": os.pathsep.join(
                (str(unhealthy), str(healthy))
            ),
            "MOLT_EXTERNAL_MIN_FREE_GB": "0",
        },
        create_dirs=True,
    )

    resolved_external = healthy.resolve()
    assert env["MOLT_EXT_ROOT"] == str(resolved_external)
    assert env["TMPDIR"] == str(resolved_external / "tmp")


@pytest.mark.parametrize("suffix", ["", "build"])
def test_run_context_external_requirement_refuses_checkout_outputs(tmp_path, suffix):
    repo = tmp_path / "repo"
    repo.mkdir()
    with pytest.raises(
        dx.DxConfigError, match="MOLT_EXT_ROOT must be outside the checkout"
    ):
        RunContext(repo).canonical_env(
            {
                "MOLT_EXT_ROOT": str(repo / suffix),
                "MOLT_REQUIRE_EXTERNAL_ARTIFACTS": "1",
            },
            create_dirs=False,
        )


def test_run_context_prefers_external_without_rejecting_explicit_user_output_root(
    monkeypatch,
    tmp_path: Path,
) -> None:
    repo_root = tmp_path / "repo"
    user_output_root = repo_root / "build" / "wasm" / "case"
    repo_root.mkdir()

    env = RunContext(
        repo_root,
        session_prefix="test",
        prefer_external_artifacts=True,
    ).canonical_env(
        {
            "MOLT_EXT_ROOT": str(user_output_root),
            "MOLT_EXTERNAL_MIN_FREE_GB": "0",
        },
        create_dirs=False,
    )

    resolved_output_root = user_output_root.resolve()
    assert env["MOLT_EXT_ROOT"] == str(resolved_output_root)
    assert env["CARGO_TARGET_DIR"] == str(resolved_output_root / "target")


def test_run_context_require_external_artifacts_forces_candidate(
    monkeypatch,
    tmp_path: Path,
) -> None:
    repo_root = tmp_path / "repo"
    external_root = tmp_path / "external-drive" / "Molt"
    repo_root.mkdir()

    env = RunContext(repo_root, session_prefix="test").canonical_env(
        {
            "MOLT_REQUIRE_EXTERNAL_ARTIFACTS": "1",
            "MOLT_EXTERNAL_ARTIFACT_ROOTS": str(external_root),
            "MOLT_EXTERNAL_MIN_FREE_GB": "0",
        },
        create_dirs=True,
    )

    assert env["MOLT_EXT_ROOT"] == str(external_root.resolve())
    assert env["CARGO_TARGET_DIR"] == str(external_root.resolve() / "target")


def test_development_artifacts_requested_is_explicit_dev_control_plane() -> None:
    assert not development_artifacts_requested({})
    assert not development_artifacts_requested({"MOLT_REQUIRE_EXTERNAL_ARTIFACTS": ""})
    assert development_artifacts_requested({"MOLT_REQUIRE_EXTERNAL_ARTIFACTS": "1"})
    assert development_artifacts_requested({"MOLT_PREFER_EXTERNAL_ARTIFACTS": "true"})
    assert development_artifacts_requested({"MOLT_PREFER_EXTERNAL_ARTIFACTS": "yes"})


@pytest.mark.parametrize(
    "key", ["CARGO_TARGET_DIR", "MOLT_CACHE", "UV_CACHE_DIR", "TMPDIR"]
)
def test_run_context_external_requirement_checks_explicit_output_consumers(
    tmp_path, key
):
    repo = tmp_path / "repo"
    repo.mkdir()
    with pytest.raises(dx.DxConfigError, match=f"{key} must be outside the checkout"):
        RunContext(repo).canonical_env(
            {
                "MOLT_EXT_ROOT": str(tmp_path / "outside"),
                "MOLT_REQUIRE_EXTERNAL_ARTIFACTS": "1",
                key: str(repo / "output"),
            },
            create_dirs=False,
        )


def test_run_context_preserves_nonambient_tmpdir_with_external_root(
    tmp_path: Path,
) -> None:
    repo_root = tmp_path / "repo"
    external_root = tmp_path / "external-ssd" / "Molt"
    explicit_tmp = tmp_path / "explicit-tmp"
    repo_root.mkdir()
    env = RunContext(
        repo_root,
        session_prefix="test",
        prefer_external_artifacts=True,
    ).canonical_env(
        {
            "MOLT_EXTERNAL_ARTIFACT_ROOTS": str(external_root),
            "MOLT_EXTERNAL_MIN_FREE_GB": "0",
            "TMPDIR": str(explicit_tmp),
        },
        create_dirs=False,
    )

    assert env["MOLT_EXT_ROOT"] == str(external_root.resolve())
    assert env["TMPDIR"] == str(explicit_tmp)


def test_run_context_can_force_repo_defaults_except_explicit_keys(
    tmp_path: Path,
) -> None:
    ambient_root = tmp_path / "ambient"
    explicit_cache = tmp_path / "cache"
    forced_keys = tuple(key for key in CANONICAL_RUN_ENV_KEYS if key != "MOLT_CACHE")
    env = RunContext(tmp_path, session_prefix="forced").canonical_env(
        {
            "MOLT_EXT_ROOT": str(ambient_root),
            "MOLT_CACHE": str(explicit_cache),
            "MOLT_SESSION_ID": "ambient-session",
        },
        create_dirs=False,
        force_default_keys=forced_keys,
    )

    assert env["MOLT_EXT_ROOT"] == str(tmp_path.resolve())
    assert env["MOLT_SESSION_ID"].startswith("forced-")
    assert env["CARGO_TARGET_DIR"] == str(
        tmp_path.resolve() / "target" / "sessions" / env["MOLT_SESSION_ID"]
    )
    assert env["MOLT_CACHE"] == str(explicit_cache)


def test_run_context_shell_exports_are_eval_safe(tmp_path: Path) -> None:
    env = RunContext(tmp_path, session_prefix="quote").canonical_env(
        {
            "MOLT_SESSION_ID": 'session-"$`\\',
        },
        create_dirs=False,
    )

    shell = run_context_env.emit_shell_exports(env, ("MOLT_SESSION_ID",))

    assert shell == 'export MOLT_SESSION_ID="session-\\"\\$\\`\\\\"'


def test_run_context_env_dx_uses_stable_uv_project_environment(
    monkeypatch: pytest.MonkeyPatch,
    tmp_path: Path,
    capsys: pytest.CaptureFixture[str],
) -> None:
    _clear_run_context_env(monkeypatch)
    _without_compiler_wasm_toolchain(monkeypatch)
    ambient_pythonpath = tmp_path / "ambient-pythonpath"
    monkeypatch.setenv("PYTHONPATH", str(ambient_pythonpath))

    assert (
        run_context_env.main(
            [
                "--root",
                str(tmp_path),
                "--dx",
                "--format",
                "json",
            ]
        )
        == 0
    )

    payload = json.loads(capsys.readouterr().out)
    env = payload["env"]
    assert env["MOLT_SESSION_ID"].startswith("run-")
    assert env["MOLT_SESSION_ID_GENERATED"] == "1"
    assert env["UV_PROJECT_ENVIRONMENT"] == str(
        dx.stable_uv_project_env_dir(
            tmp_path, purpose="dx", python="3.12", source_root=tmp_path
        )
    )
    assert env["PYTHONPATH"].split(os.pathsep) == [
        str(tmp_path.resolve() / "src"),
        str(ambient_pythonpath),
    ]


def test_run_context_env_session_id_scopes_cargo_not_uv_project_environment(
    monkeypatch: pytest.MonkeyPatch,
    tmp_path: Path,
    capsys: pytest.CaptureFixture[str],
) -> None:
    _clear_run_context_env(monkeypatch)
    _without_compiler_wasm_toolchain(monkeypatch)

    assert (
        run_context_env.main(
            [
                "--root",
                str(tmp_path),
                "--session-id",
                "witness-warm",
                "--dx",
                "--format",
                "json",
            ]
        )
        == 0
    )

    payload = json.loads(capsys.readouterr().out)
    env = payload["env"]
    assert env["MOLT_SESSION_ID"] == "witness-warm"
    assert "MOLT_SESSION_ID_GENERATED" not in env
    assert env["CARGO_TARGET_DIR"] == str(
        tmp_path.resolve() / "target" / "sessions" / "witness-warm"
    )
    assert env["UV_PROJECT_ENVIRONMENT"] == str(
        dx.stable_uv_project_env_dir(
            tmp_path, purpose="dx", python="3.12", source_root=tmp_path
        )
    )


def test_run_context_env_preserves_explicit_uv_project_environment(
    monkeypatch: pytest.MonkeyPatch,
    tmp_path: Path,
    capsys: pytest.CaptureFixture[str],
) -> None:
    _without_compiler_wasm_toolchain(monkeypatch)
    explicit = tmp_path / "custom-venv"
    monkeypatch.setenv("UV_PROJECT_ENVIRONMENT", str(explicit))

    assert (
        run_context_env.main(
            [
                "--root",
                str(tmp_path),
                "--dx",
                "--format",
                "json",
            ]
        )
        == 0
    )

    payload = json.loads(capsys.readouterr().out)
    env = payload["env"]
    assert env["UV_PROJECT_ENVIRONMENT"] == str(explicit.resolve())


def test_run_context_dx_env_installs_cross_platform_tool_defaults(
    tmp_path: Path,
) -> None:
    env = RunContext(tmp_path, session_prefix="dx").dx_env(
        {"MOLT_BACKEND_DAEMON_SOCKET_ROOT": str(tmp_path / "sockets")},
        create_dirs=False,
    )

    assert env["MOLT_BACKEND_DAEMON_SOCKET_DIR"].startswith(
        str((tmp_path / "sockets").resolve())
    )
    assert env["SCCACHE_DIR"] == str(tmp_path.resolve() / ".sccache")
    assert env["SCCACHE_CACHE_SIZE"] == "10G"
    # sccache is off-by-default on Windows (0 hits + mid-compile crashes there);
    # auto-on elsewhere. The default is platform-derived, so assert accordingly.
    assert env["MOLT_USE_SCCACHE"] == ("0" if os.name == "nt" else "1")
    assert env["MOLT_DIFF_ALLOW_RUSTC_WRAPPER"] == "1"
    assert env["MOLT_CACHE_MAX_GB"] == "30"
    assert env["MOLT_CACHE_MAX_AGE_DAYS"] == "30"


def test_dx_env_sets_uv_copy_link_mode_for_windows_exfat_root(
    monkeypatch,
    tmp_path: Path,
) -> None:
    repo_root = tmp_path / "repo"
    external_root = tmp_path / "external" / "Molt"
    repo_root.mkdir()
    monkeypatch.setattr(dx, "_artifact_root_is_windows_exfat", lambda _path: True)

    env = RunContext(
        repo_root,
        session_prefix="test",
        prefer_external_artifacts=True,
    ).dx_env(
        {
            "MOLT_EXTERNAL_ARTIFACT_ROOTS": str(external_root),
            "MOLT_EXTERNAL_MIN_FREE_GB": "0",
        },
        create_dirs=True,
    )

    assert env["MOLT_EXT_ROOT"] == str(external_root.resolve())
    assert env["UV_LINK_MODE"] == "copy"


def test_dx_env_preserves_explicit_uv_link_mode_on_exfat_root(
    monkeypatch,
    tmp_path: Path,
) -> None:
    repo_root = tmp_path / "repo"
    external_root = tmp_path / "external" / "Molt"
    repo_root.mkdir()
    monkeypatch.setattr(dx, "_artifact_root_is_windows_exfat", lambda _path: True)

    env = RunContext(
        repo_root,
        session_prefix="test",
        prefer_external_artifacts=True,
    ).dx_env(
        {
            "MOLT_EXTERNAL_ARTIFACT_ROOTS": str(external_root),
            "MOLT_EXTERNAL_MIN_FREE_GB": "0",
            "UV_LINK_MODE": "hardlink",
        },
        create_dirs=True,
    )

    assert env["UV_LINK_MODE"] == "hardlink"


def test_dx_env_renders_shell_neutral_and_powershell(tmp_path: Path) -> None:
    env = RunContext(tmp_path, session_prefix="quote").dx_env(
        {
            "MOLT_SESSION_ID": "session-'value'",
        },
        create_dirs=False,
    )

    dotenv = render_env(env, ("MOLT_SESSION_ID",), "dotenv")
    powershell = render_env(env, ("MOLT_SESSION_ID",), "powershell")

    assert dotenv == "MOLT_SESSION_ID=session-'value'"
    assert powershell == "$env:MOLT_SESSION_ID = 'session-''value'''"


def test_dx_project_preserves_explicit_root_with_external_defaults(
    tmp_path: Path,
) -> None:
    project_root = tmp_path / "repo"
    project_root.mkdir()
    (project_root / "pyproject.toml").write_text(
        """
[tool.molt.dx]
prefer_external_artifacts = true

[tool.molt.dx.env]
MOLT_EXT_ROOT = "{artifact_root}"
MOLT_CACHE = "{artifact_root}/.molt_cache"
UV_CACHE_DIR = "{artifact_root}/.uv-cache"
PYTHONPATH = "{root}/src"
""".lstrip(),
        encoding="utf-8",
    )
    explicit_root = tmp_path / "operator-root"

    env = DxProject(project_root).canonical_env(
        {
            "PATH": "/usr/bin",
            "MOLT_EXT_ROOT": str(explicit_root),
        },
        create_dirs=False,
    )

    resolved_root = explicit_root.resolve()
    assert env["MOLT_EXT_ROOT"] == str(resolved_root)
    assert env["CARGO_TARGET_DIR"] == str(resolved_root / "target")
    assert env["MOLT_CACHE"] == str(resolved_root / ".molt_cache")
    assert env["PYTHONPATH"] == str(project_root / "src")


@pytest.mark.parametrize("key", ["TMPDIR", "MOLT_DIFF_ROOT", "PYTHONPYCACHEPREFIX"])
def test_dx_project_rejects_templated_scratch_roots(tmp_path: Path, key: str) -> None:
    project_root = tmp_path / "repo"
    project_root.mkdir()
    (project_root / "pyproject.toml").write_text(
        f'[tool.molt.dx.env]\n{key} = "{{artifact_root}}/tmp"\n', encoding="utf-8"
    )
    with pytest.raises(dx.DxConfigError, match=f"must not set {key}: scratch roots"):
        DxProject(project_root).canonical_env({"PATH": "/usr/bin"}, create_dirs=False)


def test_dx_project_scratch_stays_out_of_an_in_checkout_artifact_root() -> None:
    root = DxProject.from_current_repo().root
    env = DxProject(root).canonical_env(
        {"PATH": "/usr/bin", "MOLT_EXT_ROOT": str(root)}, create_dirs=False
    )
    scratch = custody_layout.scratch_root(root, root)
    assert not scratch.is_relative_to(root)
    # Every scratch key agrees with the one layout authority.
    assert {key: env[key] for key in ("TMPDIR", "MOLT_DIFF_TMPDIR")} == {
        "TMPDIR": str(scratch),
        "MOLT_DIFF_TMPDIR": str(scratch),
    }
    assert env["MOLT_DIFF_ROOT"] == str(scratch / "diff")
    assert env["PYTHONPYCACHEPREFIX"] == str(scratch / "pycache")


def test_dx_project_dx_env_uses_same_key_authority(tmp_path: Path) -> None:
    project_root = tmp_path / "repo"
    project_root.mkdir()
    (project_root / "pyproject.toml").write_text(
        "[tool.molt.dx]\nprefer_external_artifacts = false\n",
        encoding="utf-8",
    )

    env = DxProject(project_root).dx_env({"PATH": "/usr/bin"}, create_dirs=False)

    assert tuple(key for key in DX_ENV_KEYS if key in env)
    assert env["MOLT_EXT_ROOT"] == str(project_root.resolve())
    assert env["SCCACHE_DIR"] == str(project_root.resolve() / ".sccache")
    assert env["PYTHONPATH"] == str(project_root.resolve() / "src")


def test_bind_repo_src_pythonpath_deletes_ambient_import_authority(
    tmp_path: Path,
) -> None:
    repo_root = tmp_path / "repo"
    ambient = tmp_path / "unrelated-src"
    env = {"PYTHONPATH": os.pathsep.join((str(ambient), "relative-src"))}

    bind_repo_src_pythonpath(repo_root, env)

    assert env["PYTHONPATH"] == str(repo_root.resolve() / "src")


def test_default_artifact_root_is_the_custody_root(
    monkeypatch,
    tmp_path: Path,
) -> None:
    # The checkout-family root is the only automatic root on every OS. Capacity-
    # selected volumes may be explicit outputs, but never custody by label.
    primary = tmp_path / "primary"
    repo_root = primary / "molt-src"
    repo_root.mkdir(parents=True)
    monkeypatch.setattr(
        dx, "_host_scratch_roots", lambda: ((tmp_path / "ambient").resolve(),)
    )
    monkeypatch.setattr(
        dx,
        "canonical_molt_root",
        lambda _root, *, require_exists=True: primary.resolve(),
    )

    roots = dx._default_external_artifact_roots(repo_root)

    assert roots == (primary,)


def test_clone_outside_a_checkout_family_keeps_scratch_out_of_the_source_tree(
    monkeypatch,
    tmp_path: Path,
) -> None:
    # A plain clone is its own custody root, so artifacts stay repo-local (the
    # Cargo norm). Scratch, temp, and bytecode must still never land in the
    # source tree, where runtime fixtures reject them.
    repo_root = tmp_path / "clone"
    repo_root.mkdir()
    monkeypatch.setattr(
        dx, "_host_scratch_roots", lambda: ((tmp_path / "ambient").resolve(),)
    )
    monkeypatch.setattr(
        dx,
        "canonical_molt_root",
        lambda root, *, require_exists=True: Path(root).resolve(),
    )

    env = RunContext(
        repo_root, session_prefix="test", prefer_external_artifacts=True
    ).canonical_env({"MOLT_EXTERNAL_MIN_FREE_GB": "0"}, create_dirs=False)

    source = repo_root.resolve()
    assert Path(env["MOLT_EXT_ROOT"]) == source
    for key in (
        "TMPDIR",
        "TMP",
        "TEMP",
        "MOLT_DIFF_TMPDIR",
        "MOLT_DIFF_ROOT",
        "PYTHONPYCACHEPREFIX",
    ):
        scratch = Path(env[key])
        assert source not in (scratch, *scratch.parents), (key, scratch)
    assert Path(env["TMPDIR"]) == custody_layout.out_of_tree_scratch_root(repo_root)


@pytest.fixture
def family_layout(tmp_path: Path, monkeypatch: pytest.MonkeyPatch) -> tuple[Path, Path]:
    """A `<family>/worktrees/lane` checkout family; returns (family, lane).

    The host temp root is pinned away from ``tmp_path``: a session inside a
    run context has ``TMPDIR`` at the family's run scratch, which holds the
    pytest temp root, and would make every fixture project "explicit scratch".
    """
    monkeypatch.setattr(
        dx, "_host_scratch_roots", lambda: ((tmp_path / "ambient").resolve(),)
    )
    family = tmp_path / "Molt"
    lane = family / "worktrees" / "lane"
    lane.mkdir(parents=True)
    return family.resolve(), lane.resolve()


def test_artifact_root_is_what_canonical_env_exports(
    tmp_path: Path, family_layout: tuple[Path, Path]
) -> None:
    family, lane = family_layout
    explicit = tmp_path / "external"

    # Unset: the family root, never the worktree.
    assert dx.artifact_root(lane, {}) == family
    # Set: the operator's root; a relative value anchors at the checkout.
    assert dx.artifact_root(lane, {"MOLT_EXT_ROOT": str(explicit)}) == (
        explicit.resolve()
    )
    assert dx.artifact_root(lane, {"MOLT_EXT_ROOT": "out"}) == lane / "out"
    for env in ({}, {"MOLT_EXT_ROOT": str(explicit)}):
        exported = RunContext(lane).canonical_env(env, create_dirs=False)
        assert Path(exported["MOLT_EXT_ROOT"]) == dx.artifact_root(lane, env)


def test_root_env_enters_only_the_family_molt_roots(
    tmp_path: Path, family_layout: tuple[Path, Path]
) -> None:
    """A test session builds where a developer run does (HF-114)."""
    family, lane = family_layout
    caller = {
        "TMPDIR": str(tmp_path / "caller-tmp"),
        "UV_PROJECT_ENVIRONMENT": str(tmp_path / "caller-venv"),
        "MOLT_SESSION_ID": "pytest-41",
        "MOLT_SESSION_ID_GENERATED": "1",
    }

    env = RunContext(lane, session_prefix="pytest").root_env(caller)

    # Only the Molt roots enter; tool caches, scratch and the session stay.
    assert set(env) == set(caller) | set(dx.MOLT_ROOT_ENV_KEYS)
    assert {key: env[key] for key in caller} == caller
    assert env["MOLT_EXT_ROOT"] == str(family)
    # A generated session id never scopes the target: the family's stable one.
    assert env["CARGO_TARGET_DIR"] == str(family / "target")
    assert env["MOLT_DIFF_CARGO_TARGET_DIR"] == str(family / "target")
    for key in dx.MOLT_ROOT_ENV_KEYS:
        path = Path(env[key])
        assert lane not in (path, *path.parents), (key, path)
    assert not (family / "target").exists()


def test_root_env_keeps_explicit_roots_and_pinned_sessions(
    tmp_path: Path, family_layout: tuple[Path, Path]
) -> None:
    family, lane = family_layout
    explicit = tmp_path / "explicit-target"

    kept = RunContext(lane).root_env({"CARGO_TARGET_DIR": str(explicit)})
    pinned = RunContext(lane).root_env({"MOLT_SESSION_ID": "shard-a"})

    assert kept["CARGO_TARGET_DIR"] == str(explicit.resolve())
    assert kept["MOLT_DIFF_CARGO_TARGET_DIR"] == str(explicit.resolve())
    assert pinned["CARGO_TARGET_DIR"] == str(family / "target" / "sessions" / "shard-a")
    assert pinned["MOLT_SESSION_ID"] == "shard-a"


def test_root_env_gives_a_plain_clone_its_own_unscoped_target(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    monkeypatch.setattr(
        dx, "_host_scratch_roots", lambda: ((tmp_path / "ambient").resolve(),)
    )
    clone = tmp_path / "src" / "clone"
    clone.mkdir(parents=True)
    generated = {"MOLT_SESSION_ID": "pytest-41", "MOLT_SESSION_ID_GENERATED": "1"}

    env = RunContext(clone.resolve()).root_env(generated)

    # A plain clone is its own artifact root and builds in its own target,
    # as a developer run does, but never under a generated session.
    assert env["MOLT_EXT_ROOT"] == str(clone.resolve())
    assert env["CARGO_TARGET_DIR"] == str(clone.resolve() / "target")
    # Differential scratch still leaves the checkout.
    for key in ("MOLT_DIFF_ROOT", "MOLT_DIFF_TMPDIR"):
        path = Path(env[key])
        assert clone.resolve() not in (path, *path.parents), (key, path)


def test_configured_artifact_root_is_none_when_unset_or_blank(tmp_path: Path) -> None:
    assert dx.configured_artifact_root({}, relative_to=tmp_path) is None
    assert dx.configured_artifact_root(
        {"MOLT_EXT_ROOT": " "}, relative_to=tmp_path
    ) is (None)
    assert (
        dx.configured_artifact_root({"MOLT_EXT_ROOT": "rel"}, relative_to=tmp_path)
        == (tmp_path / "rel").resolve()
    )


def test_artifact_root_refuses_the_checkout_when_external_is_required(
    tmp_path: Path,
) -> None:
    clone = tmp_path / "clone"
    clone.mkdir()
    with pytest.raises(dx.DxConfigError, match="outside the checkout"):
        dx.artifact_root(
            clone,
            {"MOLT_EXT_ROOT": str(clone), "MOLT_REQUIRE_EXTERNAL_ARTIFACTS": "1"},
        )


def test_scratch_dir_and_tmpdir_share_one_root(
    family_layout: tuple[Path, Path],
) -> None:
    family, lane = family_layout

    assert dx.scratch_root(lane, {}) == family / "tmp"
    assert dx.scratch_dir(lane, "bench", {}) == family / "tmp" / "bench"
    assert dx.scratch_dir(lane, "runtime_safety/miri", {}) == (
        family / "tmp" / "runtime_safety" / "miri"
    )
    exported = RunContext(lane).canonical_env({}, create_dirs=False)
    assert Path(exported["TMPDIR"]) == dx.scratch_root(lane, {})


@pytest.mark.parametrize("purpose", ["", "/abs", "a/../b", "a//b", ".", "a\\b"])
def test_scratch_purpose_must_be_a_relative_name(tmp_path: Path, purpose: str) -> None:
    with pytest.raises(ValueError, match="relative name"):
        dx.scratch_dir(tmp_path, purpose, {})


def test_scratch_never_lands_in_a_plain_clone(tmp_path: Path) -> None:
    clone = tmp_path / "src" / "clone"
    clone.mkdir(parents=True)
    for env in ({}, {"MOLT_EXT_ROOT": str(clone)}):
        for path in (
            dx.scratch_root(clone, env),
            dx.scratch_dir(clone, "bench", env),
            dx.control_state_dir(clone, "memory_guard", env),
            dx.proof_scratch_root(clone, env),
        ):
            assert clone.resolve() not in (path, *path.parents), (env, path)


def test_memory_storage_moves_scratch_but_not_control_state(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch, family_layout: tuple[Path, Path]
) -> None:
    family, lane = family_layout
    ram = tmp_path / "ram"
    ram.mkdir()
    env = {"MOLT_SCRATCH_STORAGE": str(ram)}

    scratch = dx.scratch_root(lane, env)
    assert scratch.parent == ram.resolve()
    assert dx.scratch_dir(lane, "gs", env) == scratch / "gs"
    # Locks and guard markers stay where every process looks for them.
    assert dx.control_state_dir(lane, "memory_guard", env) == (
        family / "tmp" / "memory_guard"
    )
    exported = RunContext(lane).canonical_env(env, create_dirs=False)
    assert Path(exported["TMPDIR"]) == scratch
    assert Path(exported["MOLT_DIFF_ROOT"]) == scratch / "diff"

    shm = tmp_path / "shm"
    shm.mkdir()
    monkeypatch.setattr(dx, "SHARED_MEMORY_ROOT", shm)
    assert dx.scratch_root(lane, {"MOLT_SCRATCH_STORAGE": "memory"}).parent == (
        shm.resolve()
    )


@pytest.mark.parametrize("raw", ["", "disk"])
def test_disk_storage_is_the_default(raw: str) -> None:
    storage = dx.scratch_storage({"MOLT_SCRATCH_STORAGE": raw})
    assert storage.mode == "disk"
    assert storage.memory_root is None


def test_memory_storage_never_creates_a_ram_disk(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    missing = tmp_path / "not-mounted"
    with pytest.raises(dx.DxConfigError, match="never creates or mounts"):
        dx.scratch_storage({"MOLT_SCRATCH_STORAGE": str(missing)})
    assert not missing.exists()
    monkeypatch.setattr(dx, "SHARED_MEMORY_ROOT", missing)
    with pytest.raises(dx.DxConfigError, match="does not have"):
        dx.scratch_storage({"MOLT_SCRATCH_STORAGE": "memory"})
    with pytest.raises(dx.DxConfigError, match="absolute path"):
        dx.scratch_storage({"MOLT_SCRATCH_STORAGE": "ram"})


def test_memory_storage_inside_the_checkout_is_refused(
    family_layout: tuple[Path, Path],
) -> None:
    _family_root, lane = family_layout
    inside = lane / "ram"
    inside.mkdir()
    with pytest.raises(dx.DxConfigError, match="outside the checkout"):
        dx.scratch_root(lane, {"MOLT_SCRATCH_STORAGE": str(inside)})


def test_proof_scratch_root_prefers_the_queue_issued_root(
    tmp_path: Path, family_layout: tuple[Path, Path]
) -> None:
    family, lane = family_layout
    issued = tmp_path / "queue" / "scratch"

    assert dx.proof_scratch_root(lane, {}) == family / "tmp"
    assert dx.proof_scratch_root(lane, {"MOLT_PROOF_SCRATCH_ROOT": str(issued)}) == (
        issued.resolve()
    )


def test_run_context_keeps_explicit_d_scratch_out_of_toolchain_custody(
    monkeypatch,
    tmp_path: Path,
) -> None:
    repo_root = tmp_path / "repo"
    repo_root.mkdir()
    if os.name != "nt":
        pytest.skip("concrete drive-path resolution requires a Windows host")
    env = RunContext(
        repo_root,
        session_prefix="test",
        prefer_external_artifacts=True,
    ).canonical_env({"MOLT_EXT_ROOT": r"D:\scratch"}, create_dirs=False)

    assert env["MOLT_EXT_ROOT"] == str(Path(r"D:\scratch").resolve())
    assert env["MOLT_TARGET_ROOT"] == str(dx.checkout_custody(repo_root).toolchain_root)


def test_run_context_projects_selected_artifact_root(
    monkeypatch,
    tmp_path: Path,
) -> None:
    primary = tmp_path / "Molt"
    repo_root = primary / "molt-src"
    repo_root.mkdir(parents=True)
    monkeypatch.setattr(
        dx, "_host_scratch_roots", lambda: ((tmp_path / "ambient").resolve(),)
    )
    monkeypatch.setattr(
        dx,
        "canonical_molt_root",
        lambda _root, *, require_exists=True: primary.resolve(),
    )

    env = RunContext(
        repo_root,
        session_prefix="test",
        prefer_external_artifacts=True,
    ).dx_env(
        {
            "PATH": "/usr/bin",
            "MOLT_EXTERNAL_ARTIFACT_ROOTS": str(primary),
            "MOLT_EXTERNAL_MIN_FREE_GB": "0",
        },
        create_dirs=False,
    )
    payload = dx.dx_env_payload(env, DX_ENV_KEYS)["env"]
    assert payload["MOLT_EXT_ROOT"] == env["MOLT_EXT_ROOT"]

    assert env["MOLT_EXT_ROOT"] == str(primary.resolve())


def test_run_context_fallback_preserves_checkout_family_artifact_custody(
    monkeypatch: pytest.MonkeyPatch,
    tmp_path: Path,
) -> None:
    custody_root = tmp_path / "Molt"
    worktree = custody_root / "worktrees" / "lane"
    is_dir = Path.is_dir
    monkeypatch.setattr(
        Path, "is_dir", lambda path: path == custody_root or is_dir(path)
    )
    monkeypatch.setattr(
        dx, "_host_scratch_roots", lambda: ((tmp_path / "ambient").resolve(),)
    )
    monkeypatch.setattr(
        dx, "require_external_artifact_root", lambda *args, **kwargs: None
    )

    env = RunContext(
        worktree,
        session_prefix="proof-rust",
        prefer_external_artifacts=True,
    ).dx_env({}, create_dirs=False)

    assert env["MOLT_EXT_ROOT"] == str(custody_root.resolve())
    assert env["CARGO_TARGET_DIR"] == str(custody_root.resolve() / "target")


def test_toolchain_root_is_child_of_canonical_custody_root(tmp_path: Path) -> None:
    custody = tmp_path / "custody"
    worktree = custody / "worktrees" / "lane"
    worktree.mkdir(parents=True)
    resolved = dx.checkout_custody(worktree)
    assert (
        resolved.toolchain_root
        == resolved.custody_root / dx.DEFAULT_TARGET_ROOT_DIRNAME
    )


@pytest.mark.parametrize("name", ["OneDrive", "ordinary"])
def test_canonical_custody_uses_checkout_family_not_directory_brand(tmp_path, name):
    family = tmp_path / name
    checkout = family / "molt-src"
    checkout.mkdir(parents=True)
    assert dx.canonical_molt_root(checkout) == family.resolve()


@pytest.mark.parametrize(
    ("runner_source", "runner_temp", "runner_custody"),
    [
        (
            r"D:\a\molt\molt",
            r"D:\a\_temp",
            r"D:\a\_temp\molt-proof-queue-windows",
        ),
        (
            "/home/runner/work/molt/molt",
            "/home/runner/work/_temp",
            "/home/runner/work/_temp/molt-proof-queue-linux",
        ),
        (
            "/Users/runner/work/molt/molt",
            "/Users/runner/work/_temp",
            "/Users/runner/work/_temp/molt-proof-queue-macos",
        ),
    ],
    ids=("windows", "linux", "macos"),
)
def test_path_roles_distinguish_hosted_runner_matrix_from_durable_authority(
    runner_source: str,
    runner_temp: str,
    runner_custody: str,
) -> None:
    assert pure_path_is_within(runner_custody, runner_temp)
    assert host_path_is_within(runner_custody, runner_temp)


def test_verified_github_checkout_separates_source_from_execution_custody(
    monkeypatch: pytest.MonkeyPatch, tmp_path: Path
) -> None:
    repo_root = tmp_path / "runner-work" / "molt" / "molt"
    repo_root.mkdir(parents=True)
    runner_temp = tmp_path / "runner-temp"
    sha = "b" * 40
    env = _github_actions_custody_env(repo_root, runner_temp, sha=sha)
    monkeypatch.setattr(dx, "git_checkout_head", lambda _root: sha)

    custody = dx.checkout_custody(repo_root, env)
    resolved = RunContext(
        repo_root, session_prefix="queue", prefer_external_artifacts=True
    ).canonical_env(env, create_dirs=True)

    assert custody.kind == "github-actions-ephemeral"
    assert custody.source_root == repo_root.resolve()
    assert custody.custody_root == (runner_temp / "molt-custody").resolve()
    assert Path(resolved["MOLT_EXT_ROOT"]) == custody.custody_root
    assert Path(resolved["MOLT_TARGET_ROOT"]) == custody.toolchain_root
    for key in dx.CANONICAL_ROOT_ENV_KEYS:
        value = resolved.get(key)
        if value:
            assert not dx._path_is_within(Path(value), repo_root), key


def test_github_actions_flag_alone_cannot_self_attest_custody(tmp_path: Path) -> None:
    custody = dx.checkout_custody(
        tmp_path,
        {"GITHUB_ACTIONS": "true", "CI": "true"},
    )

    assert custody.kind in {"durable", "explicit-scratch"}
    assert custody.kind != "github-actions-ephemeral"
    assert custody.custody_root == tmp_path.resolve()


def test_explicit_scratch_preserves_explicit_external_artifact_authority(
    monkeypatch: pytest.MonkeyPatch, tmp_path: Path
) -> None:
    repo_root = tmp_path / "repo"
    external_root = tmp_path / "external" / "Molt"
    repo_root.mkdir()
    external_root.mkdir(parents=True)
    monkeypatch.setattr(dx.tempfile, "gettempdir", lambda: str(tmp_path))

    custody = dx.checkout_custody(repo_root)
    env = RunContext(
        repo_root,
        session_prefix="scratch",
        prefer_external_artifacts=True,
    ).canonical_env(
        {
            "MOLT_EXTERNAL_ARTIFACT_ROOTS": str(external_root),
            "MOLT_EXTERNAL_MIN_FREE_GB": "0",
        },
        create_dirs=True,
    )

    assert custody.kind == "explicit-scratch"
    assert not custody.source_only
    assert env["MOLT_EXT_ROOT"] == str(external_root.resolve())


def test_workflow_issued_scratch_root_does_not_depend_on_tempfile_cache(
    monkeypatch: pytest.MonkeyPatch, tmp_path: Path
) -> None:
    workspace = tmp_path / "workspace"
    workspace.mkdir()
    runner_temp = tmp_path / "runner-temp"
    env = _github_actions_custody_env(workspace, runner_temp)
    issued_root = Path(env[dx.GITHUB_ACTIONS_EPHEMERAL_ROOT_ENV])
    repo_root = issued_root / "tmp" / "pytest" / "repo"
    repo_root.mkdir(parents=True)
    monkeypatch.setattr(dx.tempfile, "gettempdir", lambda: str(tmp_path / "ambient"))
    monkeypatch.setenv("GITHUB_ACTIONS", "true")
    monkeypatch.setenv("CI", "true")
    monkeypatch.setenv("RUNNER_TEMP", str(runner_temp))

    custody = dx.checkout_custody(repo_root, {})

    assert custody.kind == "explicit-scratch"
    assert custody.custody_root == repo_root.resolve()


def test_child_environment_cannot_fabricate_scratch_custody(monkeypatch, tmp_path):
    repo = tmp_path / "untrusted" / "repo"
    repo.mkdir(parents=True)
    monkeypatch.setattr(
        dx, "_host_scratch_roots", lambda: (tmp_path / "real-host-temp",)
    )
    custody = dx.checkout_custody(
        repo,
        {
            "GITHUB_ACTIONS": "true",
            "CI": "true",
            "RUNNER_TEMP": str(repo.parent),
        },
    )
    assert custody.kind == "durable"
    assert custody.custody_root == repo.resolve()


@pytest.mark.parametrize(
    ("key", "value", "message"),
    [
        ("GITHUB_WORKSPACE", "wrong-workspace", "GITHUB_WORKSPACE"),
        ("GITHUB_SHA", "c" * 40, "checkout HEAD mismatch"),
        ("GITHUB_REPOSITORY", "attacker/fork", "checked-in workflow ref"),
        ("RUNNER_OS", "wrong-os", "RUNNER_OS"),
    ],
)
def test_github_checkout_custody_rejects_mismatched_reserved_facts(
    monkeypatch: pytest.MonkeyPatch,
    tmp_path: Path,
    key: str,
    value: str,
    message: str,
) -> None:
    repo_root = tmp_path / "repo"
    repo_root.mkdir()
    runner_temp = tmp_path / "runner-temp"
    sha = "d" * 40
    env = _github_actions_custody_env(repo_root, runner_temp, sha=sha)
    env[key] = value
    monkeypatch.setattr(dx, "git_checkout_head", lambda _root: sha)

    with pytest.raises(dx.DxConfigError, match=message):
        dx.checkout_custody(repo_root, env)


def test_github_checkout_custody_rejects_root_outside_runner_temp(
    monkeypatch: pytest.MonkeyPatch, tmp_path: Path
) -> None:
    repo_root = tmp_path / "repo"
    repo_root.mkdir()
    runner_temp = tmp_path / "runner-temp"
    sha = "e" * 40
    env = _github_actions_custody_env(repo_root, runner_temp, sha=sha)
    env[dx.GITHUB_ACTIONS_EPHEMERAL_ROOT_ENV] = str(tmp_path / "outside")
    monkeypatch.setattr(dx, "git_checkout_head", lambda _root: sha)

    with pytest.raises(dx.DxConfigError, match="child of RUNNER_TEMP"):
        dx.checkout_custody(repo_root, env)


@pytest.mark.parametrize("key", ["MOLT_TARGET_ROOT", "MOLT_EXT_ROOT"])
@pytest.mark.parametrize("symlinked", [False, True])
def test_ephemeral_checkout_rejects_canonical_root_inside_source_tree(
    monkeypatch: pytest.MonkeyPatch, tmp_path: Path, key: str, symlinked: bool
) -> None:
    repo_root = tmp_path / "repo"
    repo_root.mkdir()
    runner_temp = tmp_path / "runner-temp"
    sha = "f" * 40
    env = _github_actions_custody_env(repo_root, runner_temp, sha=sha)
    selected = repo_root
    if symlinked:
        selected = tmp_path / "repo-alias"
        try:
            selected.symlink_to(repo_root, target_is_directory=True)
        except OSError as exc:
            pytest.skip(f"directory symlinks are unavailable: {exc}")
    env[key] = str(selected / "artifacts")
    monkeypatch.setattr(dx, "git_checkout_head", lambda _root: sha)

    with pytest.raises(dx.DxConfigError, match=f"cannot own {key}"):
        RunContext(repo_root).canonical_env(env, create_dirs=False)


@pytest.mark.skipif(os.name != "nt", reason="drive-letter semantics are Windows-only")
def test_verified_github_checkout_on_d_is_source_only(
    monkeypatch: pytest.MonkeyPatch, tmp_path: Path
) -> None:
    fixture_repo = tmp_path / "fixture-repo"
    fixture_repo.mkdir()
    sha = "1" * 40
    env = _github_actions_custody_env(fixture_repo, tmp_path / "fixture-temp", sha=sha)
    source_root = Path(r"D:\a\molt\molt")
    runner_temp = Path(r"D:\a\_temp")
    env["GITHUB_WORKSPACE"] = str(source_root)
    env["RUNNER_TEMP"] = str(runner_temp)
    env[dx.GITHUB_ACTIONS_EPHEMERAL_ROOT_ENV] = str(
        runner_temp / "molt-proof-queue-12345-2-windows-2022"
    )
    monkeypatch.setattr(dx, "git_checkout_head", lambda _root: sha)

    custody = dx._github_actions_checkout_custody(
        source_root, env, require_exists=False
    )

    assert custody is not None
    assert custody.kind == "github-actions-ephemeral"
    assert custody.source_root == source_root.resolve()
    assert custody.custody_root != custody.source_root


@pytest.mark.skipif(os.name != "nt", reason="drive-letter semantics are Windows-only")
def test_verified_windows_ci_keeps_d_toolchain_cache_ephemeral(
    monkeypatch: pytest.MonkeyPatch, tmp_path: Path
) -> None:
    repo_root = tmp_path / "repo"
    repo_root.mkdir()
    sha = "2" * 40
    env = _github_actions_custody_env(repo_root, tmp_path / "runner-temp", sha=sha)
    env["RUNNER_TOOL_CACHE"] = r"D:\hostedtoolcache\windows"
    monkeypatch.setattr(dx, "git_checkout_head", lambda _root: sha)

    custody = dx.checkout_custody(repo_root, env, require_exists=False)

    assert custody.ephemeral
    assert (
        custody.toolchain_root == custody.custody_root / dx.DEFAULT_TARGET_ROOT_DIRNAME
    )


def test_canonical_env_preserves_explicit_toolchain_root_and_adds_ruff_cache(
    monkeypatch,
    tmp_path: Path,
) -> None:
    repo_root = tmp_path / "repo"
    external_root = tmp_path / "external" / "Molt"
    custody_root = tmp_path / "custody"
    repo_root.mkdir()
    custody_root.mkdir()
    monkeypatch.setattr(
        dx,
        "checkout_custody",
        lambda _root, _env=None, require_exists=True: dx.CheckoutCustody(
            source_root=repo_root.resolve(),
            custody_root=custody_root.resolve(),
            toolchain_root=custody_root.resolve() / dx.DEFAULT_TARGET_ROOT_DIRNAME,
            kind="durable",
        ),
    )
    monkeypatch.setattr(
        dx,
        "_default_external_artifact_roots",
        lambda _root, _env=None: (external_root,),
    )

    explicit_toolchain = tmp_path / "explicit-toolchain"
    env = RunContext(
        repo_root, session_prefix="test", prefer_external_artifacts=True
    ).canonical_env(
        {"MOLT_EXTERNAL_MIN_FREE_GB": "0", "MOLT_TARGET_ROOT": str(explicit_toolchain)},
        create_dirs=True,
    )

    resolved_output = external_root.resolve()
    assert env["MOLT_TARGET_ROOT"] == str(explicit_toolchain.resolve())
    assert env["RUFF_CACHE_DIR"] == str(resolved_output / ".ruff-cache")


def test_uv_project_env_is_stable_across_sessions(tmp_path: Path) -> None:
    """The uv project env authority must be STABLE across sessions.

    The DX churn fix: repeated `uv run --active` proofs (each a fresh
    MOLT_SESSION_ID) reuse ONE uv project environment instead of minting a fresh
    `.venv` per session. The env is keyed on (source, purpose, python), never the
    session.
    """
    ctx = RunContext(tmp_path, session_prefix="proof")
    base = {"MOLT_EXT_ROOT": str(tmp_path)}
    env_a = ctx.uv_project_env_dir({**base, "MOLT_SESSION_ID": "sess-aaa-111"})
    env_b = ctx.uv_project_env_dir({**base, "MOLT_SESSION_ID": "sess-bbb-222"})
    assert env_a == env_b
    assert env_a == dx.stable_uv_project_env_dir(
        tmp_path, purpose="dx", python="3.12", source_root=tmp_path
    )
    assert "sess-aaa" not in str(env_a) and "sess-bbb" not in str(env_b)


def test_uv_project_env_isolated_by_editable_source_root(tmp_path: Path) -> None:
    artifact_root = tmp_path / "artifacts"
    source_a = tmp_path / "worktree-a"
    source_b = tmp_path / "worktree-b"
    source_a.mkdir()
    source_b.mkdir()

    env_a = RunContext(source_a).uv_project_env_dir(
        {"MOLT_EXT_ROOT": str(artifact_root)}
    )
    env_b = RunContext(source_b).uv_project_env_dir(
        {"MOLT_EXT_ROOT": str(artifact_root)}
    )

    assert env_a != env_b
    assert env_a.parent == env_b.parent
    assert "src-worktree-a-" in env_a.name
    assert "src-worktree-b-" in env_b.name


def test_uv_project_env_explicit_override_is_honored(tmp_path: Path) -> None:
    ctx = RunContext(tmp_path, session_prefix="proof")
    explicit = tmp_path / "explicit-venv"
    env = ctx.uv_project_env_dir(
        {"MOLT_EXT_ROOT": str(tmp_path), "UV_PROJECT_ENVIRONMENT": str(explicit)}
    )
    assert env == explicit.resolve()


def test_uv_project_env_custom_purpose_and_python(tmp_path: Path) -> None:
    ctx = RunContext(tmp_path, session_prefix="proof")
    env = ctx.uv_project_env_dir(
        {
            "MOLT_EXT_ROOT": str(tmp_path),
            "MOLT_UV_PROJECT_PURPOSE": "witness",
            "MOLT_UV_PROJECT_PYTHON": "3.13",
            "MOLT_SESSION_ID": "sess-ignored",
        }
    )
    assert env == dx.stable_uv_project_env_dir(
        tmp_path, purpose="witness", python="3.13", source_root=tmp_path
    )


@pytest.mark.parametrize("name", ["OneDrive", "OneDrive - Example Org"])
def test_run_context_preserves_explicit_named_roots(tmp_path, name):
    repo = tmp_path / name / "repo"
    repo.mkdir(parents=True)
    output = tmp_path / name / "output"
    toolchain = tmp_path / name / "tools"
    env = RunContext(repo).canonical_env(
        {
            "MOLT_EXT_ROOT": str(output),
            "MOLT_TARGET_ROOT": str(toolchain),
            "MOLT_REQUIRE_EXTERNAL_ARTIFACTS": "1",
        },
        create_dirs=False,
    )
    assert env["MOLT_EXT_ROOT"] == str(output.resolve())
    assert env["MOLT_TARGET_ROOT"] == str(toolchain.resolve())


def test_render_env_spells_hyphenated_names_per_shell() -> None:
    env = {
        "CC_wasm32-wasip1": "/sdk/clang",
        "CC_wasm32_wasip1": "/sdk/clang",
        "MOLT_EXT_ROOT": "/root",
    }
    keys = tuple(env)

    posix = dx.render_env(env, keys, "posix")
    assert "CC_wasm32-wasip1" not in posix  # not a POSIX shell name
    assert "export CC_wasm32_wasip1=" in posix
    assert "export MOLT_EXT_ROOT=" in posix

    powershell = dx.render_env(env, keys, "powershell")
    assert "${env:CC_wasm32-wasip1} = " in powershell
    assert "$env:MOLT_EXT_ROOT = " in powershell

    with pytest.raises(dx.DxConfigError, match="no underscore spelling"):
        dx.render_env({"ORPHAN-NAME": "x"}, ("ORPHAN-NAME",), "posix")
