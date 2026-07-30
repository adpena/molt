from __future__ import annotations

import json
import os
from pathlib import Path

import molt.dx as dx
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
    CustodyPathRole,
    forbidden_for_role,
    host_path_is_within,
    pure_path_is_within,
)
from tools import run_context_env


@pytest.fixture(autouse=True)
def _isolate_unit_paths_from_hosted_checkout_contract(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    # Unit cases create independent synthetic project roots.  The process-wide
    # hosted checkout contract belongs to GITHUB_WORKSPACE, not those fixtures.
    monkeypatch.delenv(dx.GITHUB_ACTIONS_EPHEMERAL_ROOT_ENV, raising=False)


def _clear_run_context_env(monkeypatch: pytest.MonkeyPatch) -> None:
    for key in set(CANONICAL_RUN_ENV_KEYS) | set(DX_ENV_KEYS):
        monkeypatch.delenv(key, raising=False)


def _github_actions_custody_env(
    repo_root: Path, runner_temp: Path, *, sha: str = "a" * 40
) -> dict[str, str]:
    repository = "adpena/molt"
    workflow = repo_root / ".github" / "workflows" / "ci.yml"
    workflow.parent.mkdir(parents=True, exist_ok=True)
    workflow.write_text("name: test\n", encoding="utf-8")
    runner_temp.mkdir(parents=True, exist_ok=True)
    runner_tool_cache = runner_temp.parent / "runner-tool-cache"
    runner_tool_cache.mkdir(parents=True, exist_ok=True)
    event_path = runner_temp / "event.json"
    event_path.write_text(
        json.dumps({"repository": {"full_name": repository}}),
        encoding="utf-8",
    )
    runner_arch = {
        "amd64": "X64",
        "x86_64": "X64",
        "aarch64": "ARM64",
        "arm64": "ARM64",
        "x86": "X86",
        "i386": "X86",
        "i686": "X86",
    }[dx.platform.machine().lower()]
    return {
        dx.GITHUB_ACTIONS_EPHEMERAL_ROOT_ENV: str(runner_temp / "molt-custody"),
        "GITHUB_ACTIONS": "true",
        "CI": "true",
        "GITHUB_REPOSITORY": repository,
        "GITHUB_SERVER_URL": "https://github.com",
        "GITHUB_API_URL": "https://api.github.com",
        "GITHUB_WORKSPACE": str(repo_root.resolve()),
        "GITHUB_WORKFLOW_REF": (
            f"{repository}/.github/workflows/ci.yml@refs/heads/main"
        ),
        "GITHUB_WORKFLOW_SHA": sha,
        "GITHUB_EVENT_PATH": str(event_path),
        "GITHUB_EVENT_NAME": "push",
        "GITHUB_REF": "refs/heads/main",
        "GITHUB_SHA": sha,
        "GITHUB_RUN_ID": "12345",
        "GITHUB_RUN_ATTEMPT": "2",
        "GITHUB_JOB": "platform-portability",
        "RUNNER_TEMP": str(runner_temp.resolve()),
        "RUNNER_TOOL_CACHE": str(runner_tool_cache.resolve()),
        "RUNNER_OS": "Windows"
        if os.name == "nt"
        else ("macOS" if dx.sys.platform == "darwin" else "Linux"),
        "RUNNER_ARCH": runner_arch,
        "PATH": os.environ.get("PATH", ""),
    }


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
    assert env["MOLT_DIFF_ROOT"] == str(tmp_path.resolve() / "tmp" / "diff")
    assert env["MOLT_DIFF_TMPDIR"] == str(tmp_path.resolve() / "tmp")
    assert env["UV_CACHE_DIR"] == str(tmp_path.resolve() / ".uv-cache")
    assert env["UV_PROJECT_ENVIRONMENT"].startswith(
        str(tmp_path.resolve() / "tmp" / "uv-project-envs")
    )
    assert env["PIP_CACHE_DIR"] == str(tmp_path.resolve() / ".pip-cache")
    assert env["PYTHONPYCACHEPREFIX"] == str(tmp_path.resolve() / "tmp" / "pycache")
    assert env["TMPDIR"] == str(tmp_path.resolve() / "tmp")
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
            "MOLT_ALLOW_C_DRIVE_ARTIFACTS": "1",
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
            "MOLT_ALLOW_C_DRIVE_ARTIFACTS": "1",
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
    monkeypatch.setattr(dx, "_is_windows_c_drive_path", lambda _path: False)

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
    monkeypatch.setattr(dx, "_is_windows_c_drive_path", lambda _path: False)

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


def test_run_context_rejects_windows_c_drive_artifact_root_by_default(
    monkeypatch,
    tmp_path: Path,
) -> None:
    repo_root = tmp_path / "repo"
    c_root = tmp_path / "c-artifacts"
    repo_root.mkdir()
    monkeypatch.setattr(dx, "_is_windows_c_drive_path", lambda _path: True)

    with pytest.raises(dx.DxConfigError, match="must not be placed on C"):
        RunContext(
            repo_root,
            session_prefix="test",
            prefer_external_artifacts=True,
        ).canonical_env(
            {
                "MOLT_EXT_ROOT": str(c_root),
                "MOLT_REQUIRE_EXTERNAL_ARTIFACTS": "1",
                "MOLT_EXTERNAL_MIN_FREE_GB": "0",
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
    monkeypatch.setattr(dx, "_is_windows_c_drive_path", lambda _path: True)

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
    monkeypatch.setattr(dx, "_is_windows_c_drive_path", lambda _path: False)

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
    assert development_artifacts_requested({"MOLT_USE_EXTERNAL_ARTIFACTS": "yes"})


def test_run_context_rejects_explicit_c_drive_canonical_root(
    monkeypatch,
    tmp_path: Path,
) -> None:
    repo_root = tmp_path / "repo"
    external_root = tmp_path / "external-drive" / "Molt"
    c_target = tmp_path / "c-drive-target"
    repo_root.mkdir()
    monkeypatch.setattr(
        dx,
        "_is_windows_c_drive_path",
        lambda path: path == c_target.resolve(),
    )

    with pytest.raises(dx.DxConfigError, match="CARGO_TARGET_DIR resolved"):
        RunContext(repo_root, session_prefix="test").canonical_env(
            {
                "MOLT_REQUIRE_EXTERNAL_ARTIFACTS": "1",
                "MOLT_EXTERNAL_ARTIFACT_ROOTS": str(external_root),
                "MOLT_EXTERNAL_MIN_FREE_GB": "0",
                "CARGO_TARGET_DIR": str(c_target),
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
            "MOLT_ALLOW_C_DRIVE_ARTIFACTS": "1",
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
    monkeypatch.setenv("MOLT_ALLOW_C_DRIVE_ARTIFACTS", "1")
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


def test_run_context_env_can_emit_session_scoped_uv_project_environment(
    monkeypatch: pytest.MonkeyPatch,
    tmp_path: Path,
    capsys: pytest.CaptureFixture[str],
) -> None:
    _clear_run_context_env(monkeypatch)
    monkeypatch.setenv("MOLT_ALLOW_C_DRIVE_ARTIFACTS", "1")

    assert (
        run_context_env.main(
            [
                "--root",
                str(tmp_path),
                "--dx",
                "--session-scoped-uv-project-env",
                "--format",
                "json",
            ]
        )
        == 0
    )

    payload = json.loads(capsys.readouterr().out)
    env = payload["env"]
    assert env["UV_PROJECT_ENVIRONMENT"] == str(
        tmp_path.resolve() / "tmp" / "uv-project-envs" / env["MOLT_SESSION_ID"]
    )


def test_run_context_env_session_id_overrides_missing_ambient_session(
    monkeypatch: pytest.MonkeyPatch,
    tmp_path: Path,
    capsys: pytest.CaptureFixture[str],
) -> None:
    _clear_run_context_env(monkeypatch)
    monkeypatch.setenv("MOLT_ALLOW_C_DRIVE_ARTIFACTS", "1")

    assert (
        run_context_env.main(
            [
                "--root",
                str(tmp_path),
                "--session-id",
                "witness-warm",
                "--dx",
                "--session-scoped-uv-project-env",
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
        tmp_path.resolve() / "tmp" / "uv-project-envs" / "witness-warm"
    )


def test_run_context_env_preserves_explicit_uv_project_environment(
    monkeypatch: pytest.MonkeyPatch,
    tmp_path: Path,
    capsys: pytest.CaptureFixture[str],
) -> None:
    explicit = tmp_path / "custom-venv"
    monkeypatch.setenv("UV_PROJECT_ENVIRONMENT", str(explicit))
    monkeypatch.setenv("MOLT_ALLOW_C_DRIVE_ARTIFACTS", "1")

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
    monkeypatch.setattr(dx, "_is_windows_c_drive_path", lambda _path: False)
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
    monkeypatch.setattr(dx, "_is_windows_c_drive_path", lambda _path: False)
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
MOLT_DIFF_ROOT = "{artifact_root}/tmp/diff"
MOLT_DIFF_TMPDIR = "{artifact_root}/tmp"
UV_CACHE_DIR = "{artifact_root}/.uv-cache"
TMPDIR = "{artifact_root}/tmp"
PYTHONPATH = "{root}/src"
""".lstrip(),
        encoding="utf-8",
    )
    explicit_root = tmp_path / "operator-root"

    env = DxProject(project_root).canonical_env(
        {
            "PATH": "/usr/bin",
            "MOLT_EXT_ROOT": str(explicit_root),
            "MOLT_ALLOW_C_DRIVE_ARTIFACTS": "1",
        },
        create_dirs=False,
    )

    resolved_root = explicit_root.resolve()
    assert env["MOLT_EXT_ROOT"] == str(resolved_root)
    assert env["CARGO_TARGET_DIR"] == str(resolved_root / "target")
    assert env["MOLT_CACHE"] == str(resolved_root / ".molt_cache")
    assert env["PYTHONPATH"] == str(project_root / "src")


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


def test_default_windows_artifact_roots_has_no_volume_fallback(
    monkeypatch,
    tmp_path: Path,
) -> None:
    # The checkout-family root is the only automatic Windows root. Capacity-
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

    roots = dx._default_windows_external_artifact_roots(repo_root)

    assert roots == (primary,)


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


def test_run_context_attests_selected_windows_c_artifact_root(
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
    monkeypatch.setattr(dx, "_is_windows_c_drive_path", lambda _path: True)

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

    assert env["MOLT_EXT_ROOT"] == str(primary.resolve())
    assert env["MOLT_ALLOW_C_DRIVE_ARTIFACTS"] == "1"
    assert payload["MOLT_ALLOW_C_DRIVE_ARTIFACTS"] == "1"


def test_toolchain_root_is_child_of_canonical_custody_root(tmp_path: Path) -> None:
    custody = tmp_path / "custody"
    worktree = custody / "worktrees" / "lane"
    worktree.mkdir(parents=True)
    resolved = dx.checkout_custody(worktree)
    assert (
        resolved.toolchain_root
        == resolved.custody_root / dx.DEFAULT_TARGET_ROOT_DIRNAME
    )


@pytest.mark.parametrize(
    "path",
    (
        r"D:\Molt\worktrees\lane",
        "D:/other/molt-src",
        r"\\?\D:\Molt\worktrees\lane",
        "//?/D:/Molt/worktrees/lane",
        r"\\.\D:\Molt\worktrees\lane",
        r"\??\D:\Molt\worktrees\lane",
    ),
    ids=(
        "normal",
        "slash",
        "win32-device",
        "win32-device-slash",
        "dos-device",
        "nt-object-manager",
    ),
)
def test_canonical_custody_fails_closed_on_entire_d_drive(path: str) -> None:
    with pytest.raises(dx.DxConfigError, match=r"forbidden D:"):
        dx.canonical_molt_root(path, require_exists=False)
    assert forbidden_for_role(path, CustodyPathRole.DURABLE_AUTHORITY)
    assert not forbidden_for_role(path, CustodyPathRole.HOSTED_SOURCE)
    assert not forbidden_for_role(path, CustodyPathRole.HOSTED_EXECUTION)


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
    assert forbidden_for_role(
        r"D:\Molt\worktrees\lane", CustodyPathRole.DURABLE_AUTHORITY
    )
    assert forbidden_for_role(r"D:\other\molt-src", CustodyPathRole.DURABLE_AUTHORITY)
    assert not forbidden_for_role(runner_source, CustodyPathRole.HOSTED_SOURCE)
    assert not forbidden_for_role(runner_custody, CustodyPathRole.HOSTED_EXECUTION)
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
    monkeypatch.setattr(dx, "_git_checkout_head", lambda _root: sha)

    custody = dx.checkout_custody(repo_root, env)
    resolved = RunContext(
        repo_root, session_prefix="queue", prefer_external_artifacts=True
    ).canonical_env(env, create_dirs=True)

    assert custody.kind == "github-actions-ephemeral"
    assert custody.source_root == repo_root.resolve()
    assert custody.custody_root == (runner_temp / "molt-custody").resolve()
    assert Path(resolved["MOLT_EXT_ROOT"]) == custody.custody_root
    assert Path(resolved["MOLT_TARGET_ROOT"]) == custody.toolchain_root
    assert not forbidden_for_role(
        custody.toolchain_root, CustodyPathRole.HOSTED_EXECUTION
    )
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


@pytest.mark.skipif(os.name != "nt", reason="requires concrete Windows drive roles")
def test_child_environment_cannot_fabricate_d_drive_scratch_custody(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    monkeypatch.delenv("GITHUB_ACTIONS", raising=False)
    monkeypatch.delenv("CI", raising=False)
    monkeypatch.delenv("RUNNER_TEMP", raising=False)

    with pytest.raises(dx.DxConfigError, match=r"forbidden D:"):
        dx.checkout_custody(
            Path(r"D:\untrusted\repo"),
            {
                "GITHUB_ACTIONS": "true",
                "CI": "true",
                "RUNNER_TEMP": r"D:\untrusted",
            },
            require_exists=False,
        )


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
    monkeypatch.setattr(dx, "_git_checkout_head", lambda _root: sha)

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
    monkeypatch.setattr(dx, "_git_checkout_head", lambda _root: sha)

    with pytest.raises(dx.DxConfigError, match="child of RUNNER_TEMP"):
        dx.checkout_custody(repo_root, env)


def test_ephemeral_checkout_rejects_canonical_root_inside_source_tree(
    monkeypatch: pytest.MonkeyPatch, tmp_path: Path
) -> None:
    repo_root = tmp_path / "repo"
    repo_root.mkdir()
    runner_temp = tmp_path / "runner-temp"
    sha = "f" * 40
    env = _github_actions_custody_env(repo_root, runner_temp, sha=sha)
    env["MOLT_TARGET_ROOT"] = str(repo_root / "target-root")
    monkeypatch.setattr(dx, "_git_checkout_head", lambda _root: sha)

    with pytest.raises(dx.DxConfigError, match="cannot own MOLT_TARGET_ROOT"):
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
    monkeypatch.setattr(dx, "_git_checkout_head", lambda _root: sha)

    custody = dx._github_actions_checkout_custody(
        source_root, env, require_exists=False
    )

    assert custody is not None
    assert custody.kind == "github-actions-ephemeral"
    assert custody.source_root == source_root.resolve()
    assert custody.custody_root != custody.source_root
    assert not forbidden_for_role(
        custody.toolchain_root, CustodyPathRole.HOSTED_EXECUTION
    )
    with pytest.raises(dx.DxConfigError, match=r"forbidden D:"):
        dx.canonical_molt_root(source_root, require_exists=False)


@pytest.mark.skipif(os.name != "nt", reason="drive-letter semantics are Windows-only")
def test_verified_windows_ci_keeps_d_toolchain_cache_ephemeral(
    monkeypatch: pytest.MonkeyPatch, tmp_path: Path
) -> None:
    repo_root = tmp_path / "repo"
    repo_root.mkdir()
    sha = "2" * 40
    env = _github_actions_custody_env(repo_root, tmp_path / "runner-temp", sha=sha)
    env["RUNNER_TOOL_CACHE"] = r"D:\hostedtoolcache\windows"
    monkeypatch.setattr(dx, "_git_checkout_head", lambda _root: sha)

    custody = dx.checkout_custody(repo_root, env, require_exists=False)

    assert custody.ephemeral
    assert (
        custody.toolchain_root == custody.custody_root / dx.DEFAULT_TARGET_ROOT_DIRNAME
    )
    assert not forbidden_for_role(
        custody.toolchain_root, CustodyPathRole.HOSTED_EXECUTION
    )


@pytest.mark.skipif(os.name != "nt", reason="drive-letter rehoming is Windows-only")
def test_should_rehome_offvolume_toolchain_root(monkeypatch) -> None:
    assert dx._should_rehome_toolchain_root(r"E:\molt-target", Path(r"D:\Molt"), {})
    # Every D: toolchain path is rehomed when its role is durable.
    assert dx._should_rehome_toolchain_root(
        r"D:\Molt\target-root", Path(r"D:\Molt"), {}
    )
    assert dx._should_rehome_toolchain_root(
        r"D:\custom-toolchains", Path(r"C:\Molt"), {}
    )
    # Explicit operator opt-out may preserve a non-D custom toolchain, never D:.
    assert dx._should_rehome_toolchain_root(
        r"D:\Molt\custom-toolchains",
        Path(r"C:\Molt"),
        {"MOLT_PRESERVE_TARGET_ROOT": "1"},
    )
    assert not dx._should_rehome_toolchain_root(
        r"E:\custom-toolchains",
        Path(r"C:\Molt"),
        {"MOLT_PRESERVE_TARGET_ROOT": "1"},
    )


@pytest.mark.skipif(os.name != "nt", reason="drive-letter rehoming is Windows-only")
def test_canonical_env_rehomes_stale_target_root_and_adds_ruff_cache(
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
        "_default_windows_external_artifact_roots",
        lambda _root, _env=None: (external_root,),
    )
    monkeypatch.setattr(dx, "_is_windows_c_drive_path", lambda _path: False)

    env = RunContext(
        repo_root, session_prefix="test", prefer_external_artifacts=True
    ).canonical_env(
        {"MOLT_EXTERNAL_MIN_FREE_GB": "0", "MOLT_TARGET_ROOT": r"E:\molt-target"},
        create_dirs=True,
    )

    resolved_output = external_root.resolve()
    assert env["MOLT_TARGET_ROOT"] == str(
        custody_root.resolve() / dx.DEFAULT_TARGET_ROOT_DIRNAME
    )
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


def test_uv_project_env_session_scoped_opt_in(tmp_path: Path) -> None:
    """MOLT_UV_PROJECT_ENV_SESSION_SCOPED restores per-session isolation on demand."""
    ctx = RunContext(tmp_path, session_prefix="proof")
    base = {
        "MOLT_EXT_ROOT": str(tmp_path),
        "MOLT_UV_PROJECT_ENV_SESSION_SCOPED": "1",
    }
    env_a = ctx.uv_project_env_dir({**base, "MOLT_SESSION_ID": "sess-aaa-111"})
    env_b = ctx.uv_project_env_dir({**base, "MOLT_SESSION_ID": "sess-bbb-222"})
    assert env_a != env_b
    assert env_a == (tmp_path / "tmp" / "uv-project-envs" / "sess-aaa-111").resolve()


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


def test_auto_janitor_throttled_and_optout(monkeypatch, tmp_path):
    # Stale artifacts are cleaned BY DEFAULT: canonical_env fires a throttled,
    # detached, best-effort janitor sweep. Verify it spawns once, then throttles,
    # and honors the opt-out.
    import molt.dx as dx

    popen_calls = []
    monkeypatch.setattr(dx.subprocess, "Popen", lambda *a, **k: popen_calls.append(a))
    monkeypatch.setattr(dx, "_running_under_pytest", lambda: False)
    monkeypatch.delenv("MOLT_DISABLE_AUTO_JANITOR", raising=False)

    dx._maybe_sweep_stale_artifacts(tmp_path)
    assert len(popen_calls) == 1
    assert (tmp_path / ".molt_janitor_last_run").exists()

    # Immediately again -> throttled (recent marker), no second spawn.
    dx._maybe_sweep_stale_artifacts(tmp_path)
    assert len(popen_calls) == 1

    # Opt-out on a fresh root -> never spawns.
    other = tmp_path / "other"
    other.mkdir()
    monkeypatch.setenv("MOLT_DISABLE_AUTO_JANITOR", "1")
    dx._maybe_sweep_stale_artifacts(other)
    assert len(popen_calls) == 1
    assert not (other / ".molt_janitor_last_run").exists()


def test_auto_janitor_skips_under_pytest(monkeypatch, tmp_path):
    import molt.dx as dx

    popen_calls = []
    monkeypatch.setattr(dx.subprocess, "Popen", lambda *a, **k: popen_calls.append(a))
    monkeypatch.setenv(
        "PYTEST_CURRENT_TEST", "tests/test_dx_run_context.py::test (call)"
    )
    monkeypatch.delenv("MOLT_DISABLE_AUTO_JANITOR", raising=False)

    dx._maybe_sweep_stale_artifacts(tmp_path)

    assert popen_calls == []
    assert not (tmp_path / ".molt_janitor_last_run").exists()


def test_onedrive_paths_rejected_fail_closed():
    # Nothing may ever drift back onto OneDrive — checkout OR artifacts fail closed.
    import pytest as _pytest
    import molt.dx as dx
    from pathlib import Path

    with _pytest.raises(dx.DxConfigError, match="OneDrive"):
        dx._reject_onedrive(Path(r"C:\Users\x\OneDrive\Documents\molt"), "checkout")
    with _pytest.raises(dx.DxConfigError, match="OneDrive"):
        dx._reject_onedrive(Path(r"C:\Users\x\OneDrive\molt\target"), "artifacts")
    # Canonical paths pass.
    dx._reject_onedrive(Path(r"C:\Molt\molt-src"), "checkout")
    dx._reject_onedrive(Path(r"C:\Molt"), "artifacts")
    assert dx._is_onedrive_path(Path(r"C:\Users\x\OneDrive\Documents\molt")) is True
    assert dx._is_onedrive_path(Path(r"C:\Molt\molt-src")) is False
