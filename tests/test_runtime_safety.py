from __future__ import annotations

import os
import shutil

from pathlib import Path
from molt.checkout_state import scratch_state_root
import subprocess

import molt.dx as molt_dx
import pytest

from tools import runtime_safety
from tests.executable_test_support import write_mock_executable
from tests.process_guard_common import run_guarded_test_process

# Cargo runs in this file are fakes; build capacity is not under test.
pytestmark = pytest.mark.usefixtures("admitted_build_capacity")


def test_fuzz_workspace_for_runtime_target() -> None:
    workspace = runtime_safety._fuzz_workspace_for_target("string_ops")
    assert workspace == Path("runtime/molt-runtime/fuzz") or workspace == (
        runtime_safety.RUNTIME_FUZZ_DIR
    )


def test_fuzz_workspace_for_root_target() -> None:
    workspace = runtime_safety._fuzz_workspace_for_target("fuzz_ir_parse")
    assert workspace == Path("fuzz") or workspace == runtime_safety.ROOT_FUZZ_DIR


def test_fuzz_workspace_for_unknown_target_raises() -> None:
    with pytest.raises(SystemExit, match="unknown fuzz target"):
        runtime_safety._fuzz_workspace_for_target("definitely_missing_target")


@pytest.mark.usefixtures("developer_host_context")
@pytest.mark.parametrize("returncode", [0, 101])
def test_run_clippy_preserves_warning_policy_and_result(
    monkeypatch, tmp_path: Path, returncode: int
) -> None:
    calls: list[list[str]] = []

    def fake_guarded_completed_process(cmd, **kwargs):
        calls.append(list(cmd))
        assert kwargs["capture_output"] is True
        assert kwargs["env"]["CARGO_BUILD_WARNINGS"] == "allow"
        return subprocess.CompletedProcess(
            cmd, returncode, stdout="", stderr="clippy diagnostic\n"
        )

    # The command must carry the policy even when the inherited Cargo setting
    # permits warnings. Cargo's real warning/cache behavior has separate proofs.
    monkeypatch.setenv("CARGO_BUILD_WARNINGS", "allow")
    monkeypatch.setattr(
        runtime_safety.harness_memory_guard,
        "guarded_completed_process",
        fake_guarded_completed_process,
    )
    expected = [
        "cargo",
        "clippy",
        "--config",
        'build.warnings="deny"',
        "-p",
        "molt-runtime",
        "--all-targets",
    ]
    if returncode:
        with pytest.raises(subprocess.CalledProcessError) as raised:
            runtime_safety.run_clippy(tmp_path)
        assert raised.value.returncode == returncode
        assert raised.value.cmd == expected
        assert raised.value.stderr == "clippy diagnostic\n"
    else:
        runtime_safety.run_clippy(tmp_path)

    assert calls == [expected]
    assert (tmp_path / "runtime_clippy.log").read_text(
        encoding="utf-8"
    ) == "clippy diagnostic\n"


@pytest.mark.skipif(os.name == "nt", reason="deployment checklist is a POSIX script")
@pytest.mark.parametrize("failed_command", [None, "test", "clippy", "check"])
def test_pre_deploy_cargo_checks_use_exit_status(
    tmp_path: Path, failed_command: str | None
) -> None:
    bash = shutil.which("bash")
    if bash is None:
        pytest.skip("bash is required for the deployment checklist")

    root = tmp_path / "project"
    scripts = root / "deploy" / "scripts"
    cloudflare = root / "deploy" / "cloudflare"
    mcp = root / "deploy" / "mcp"
    for directory in (scripts, cloudflare, mcp, root / "docs" / "deployment"):
        directory.mkdir(parents=True)
    source = runtime_safety.ROOT / "deploy" / "scripts" / "pre_deploy_check.sh"
    script = write_mock_executable(scripts / source.name, source.read_bytes())
    (cloudflare / "wrangler.toml").write_text('name = "test"\n', encoding="utf-8")
    (mcp / "ocr_tool.json").write_text("{}\n", encoding="utf-8")
    for name in ("worker.js", "ocr_api.js", "x402.js", "monitoring.js"):
        (cloudflare / name).write_text("// syntax fixture\n", encoding="utf-8")

    mock_bin = tmp_path / "bin"
    mock_bin.mkdir()
    write_mock_executable(
        mock_bin / "cargo",
        b"#!/bin/sh\n"
        b'printf "%s\\n" "$@" >> "$CARGO_CALLS"\n'
        b'printf "\\n" >> "$CARGO_CALLS"\n'
        b'if [ "$1" = "$FAILED_CARGO_COMMAND" ]; then\n'
        b'    echo "Finished passed"\n'
        b"    exit 101\n"
        b"fi\n"
        b"exit 0\n",
    )
    for name in ("node", "git"):
        write_mock_executable(mock_bin / name, b"#!/bin/sh\nexit 0\n")
    calls_path = tmp_path / "cargo-calls.txt"
    env = dict(os.environ)
    env.update(
        PATH=str(mock_bin) + os.pathsep + env.get("PATH", ""),
        CARGO_CALLS=str(calls_path),
        FAILED_CARGO_COMMAND=failed_command or "",
    )
    result = run_guarded_test_process(
        [bash, str(script)],
        cwd=root,
        env=env,
        capture_output=True,
        text=True,
        check=False,
    )

    assert result.returncode == (1 if failed_command else 0), (
        result.stdout,
        result.stderr,
    )
    calls = [
        call.splitlines()
        for call in calls_path.read_text(encoding="utf-8").strip().split("\n\n")
    ]
    # All three commands must run even when an earlier check fails; the
    # independent argv oracle also preserves every package/feature/target cell.
    assert calls == [
        ["test", "-p", "molt-gpu", "--all-features"],
        [
            "clippy",
            "--config",
            'build.warnings="deny"',
            "-p",
            "molt-gpu",
            "--all-features",
        ],
        [
            "check",
            "-p",
            "molt-gpu",
            "--target",
            "wasm32-unknown-unknown",
            "--no-default-features",
            "--features",
            "cpu-backend,wasm-backend",
        ],
    ]
    if failed_command:
        assert "1 CHECK(S) FAILED" in result.stdout
        assert "ALL CHECKS PASSED" not in result.stdout
    else:
        assert "ALL CHECKS PASSED" in result.stdout


def test_miri_tmp_root_defaults_to_scratch_outside_the_checkout(monkeypatch) -> None:
    for key in ("MOLT_DIFF_TMPDIR", "TMPDIR", "MOLT_EXT_ROOT"):
        monkeypatch.delenv(key, raising=False)

    miri = runtime_safety._miri_tmp_root({})
    assert miri.parts[-2:] == ("runtime_safety", "miri")
    assert not miri.is_relative_to(runtime_safety.ROOT.resolve())


def test_run_miri_preserves_explicit_tmpdir(monkeypatch, tmp_path: Path) -> None:
    captured: dict[str, object] = {}

    def fake_run(cmd, env=None, cwd=None, log_path=None):
        captured["cmd"] = cmd
        captured["env"] = env
        captured["cwd"] = cwd
        captured["log_path"] = log_path

    monkeypatch.setenv("TMPDIR", str(tmp_path / "explicit-tmp"))
    monkeypatch.setattr(runtime_safety, "_run", fake_run)

    runtime_safety.run_miri(None)

    assert captured["env"]["TMPDIR"] == str(tmp_path / "explicit-tmp")
    assert captured["cmd"][:3] == [
        "cargo",
        f"+{runtime_safety.RUST_NIGHTLY}",
        "miri",
    ]


def test_run_miri_defaults_to_canonical_tmp_root(monkeypatch) -> None:
    captured: dict[str, object] = {}

    def fake_run(cmd, env=None, cwd=None, log_path=None):
        captured["env"] = env

    for key in ("MOLT_DIFF_TMPDIR", "TMPDIR", "MOLT_EXT_ROOT"):
        monkeypatch.delenv(key, raising=False)
    monkeypatch.setattr(runtime_safety, "_run", fake_run)

    runtime_safety.run_miri(None)

    miri = Path(captured["env"]["TMPDIR"])
    # The process environment, as run_miri sees it; under CI it names the
    # ephemeral custody root, which places scratch.
    assert miri == runtime_safety._miri_tmp_root(dict(os.environ))
    assert not miri.is_relative_to(runtime_safety.ROOT.resolve())


@pytest.mark.usefixtures("developer_host_context")
def test_run_with_log_uses_memory_guard_and_canonical_env(
    monkeypatch,
    tmp_path: Path,
) -> None:
    captured: dict[str, object] = {}

    def fake_guarded_completed_process(cmd, **kwargs):
        captured["cmd"] = cmd
        captured["kwargs"] = kwargs
        return subprocess.CompletedProcess(cmd, 0, stdout="out\n", stderr="err\n")

    monkeypatch.setattr(
        runtime_safety.harness_memory_guard,
        "guarded_completed_process",
        fake_guarded_completed_process,
    )
    log_path = tmp_path / "runtime.log"
    # An artifact root outside the source tree: where a checkout is its own
    # root (a hosted runner's checkout), scratch moves out of the tree instead.
    env = {
        "PATH": "/usr/bin",
        "MOLT_EXT_ROOT": str(tmp_path / "Molt"),
        "MOLT_SESSION_ID": "runtime-safety-test",
        "RUSTFLAGS": "-Z sanitizer=address",
    }

    runtime_safety._run(["cargo", "test"], env=env, log_path=log_path)

    assert captured["cmd"] == ["cargo", "test"]
    assert captured["kwargs"]["prefix"] == "MOLT_RUNTIME_SAFETY"
    assert captured["kwargs"]["capture_output"] is True
    run_env = captured["kwargs"]["env"]
    artifact_root = Path(run_env["MOLT_EXT_ROOT"])
    assert run_env["CARGO_TARGET_DIR"] == str(
        molt_dx.cargo_target_dir_for_artifact_root(
            artifact_root, "runtime-safety-test", source_root=runtime_safety.ROOT
        )
    )
    assert run_env["MOLT_DIFF_CARGO_TARGET_DIR"] == run_env["CARGO_TARGET_DIR"]
    assert run_env["MOLT_CACHE"] == str(artifact_root / ".molt_cache")
    assert run_env["MOLT_DIFF_ROOT"] == str(
        scratch_state_root(
            artifact_root / "tmp", owner=runtime_safety.ROOT, create=False
        )
        / "diff"
    )
    assert run_env["MOLT_DIFF_TMPDIR"] == str(
        scratch_state_root(
            artifact_root / "tmp", owner=runtime_safety.ROOT, create=False
        )
    )
    assert run_env["UV_CACHE_DIR"] == str(artifact_root / ".uv-cache")
    assert run_env["TMPDIR"] == str(
        scratch_state_root(
            artifact_root / "tmp", owner=runtime_safety.ROOT, create=False
        )
    )
    assert run_env["MOLT_SESSION_ID"] == "runtime-safety-test"
    assert run_env["RUSTFLAGS"] == "-Z sanitizer=address"
    assert log_path.read_text(encoding="utf-8") == "out\nerr\n"
