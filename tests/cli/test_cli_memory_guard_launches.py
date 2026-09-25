from __future__ import annotations

import importlib
import subprocess
from pathlib import Path
from typing import Any

import pytest
import molt.cli as cli
from molt.cli import link_pipeline as cli_link_pipeline
from molt.cli import typecheck as cli_typecheck

BACKEND_EXECUTION = importlib.import_module("molt.cli.backend_execution")
COMMAND_RUNTIME = importlib.import_module("molt.cli.command_runtime")
COMPILER_METADATA = importlib.import_module("molt.cli.compiler_metadata")
LOCKFILES = importlib.import_module("molt.cli.lockfiles")
MLIR_BACKEND = importlib.import_module("molt.cli.mlir_backend")
SETUP_READINESS = importlib.import_module("molt.cli.setup_readiness")
WASM_TOOLCHAIN = importlib.import_module("molt.cli.wasm_toolchain")


def test_uv_lock_check_uses_build_memory_guard(
    monkeypatch,
    tmp_path: Path,
) -> None:
    (tmp_path / "pyproject.toml").write_text("[project]\nname='x'\n")
    (tmp_path / "uv.lock").write_text("# lock\n")
    captured: dict[str, Any] = {}

    def fake_run(cmd: list[str], **kwargs: object) -> subprocess.CompletedProcess[str]:
        captured["cmd"] = cmd
        captured["kwargs"] = kwargs
        return subprocess.CompletedProcess(cmd, 0, "", "")

    monkeypatch.setattr(LOCKFILES.shutil, "which", lambda name: f"/usr/bin/{name}")
    monkeypatch.setattr(LOCKFILES, "_run_completed_command", fake_run)

    assert cli._verify_uv_lock(tmp_path) is None
    assert captured["cmd"] == ["uv", "lock", "--check"]
    assert captured["kwargs"]["memory_guard_prefix"] == "MOLT_BUILD"
    assert captured["kwargs"]["cwd"] == tmp_path


def test_cargo_lock_check_uses_build_memory_guard(
    monkeypatch,
    tmp_path: Path,
) -> None:
    (tmp_path / "Cargo.toml").write_text("[workspace]\nmembers=[]\n")
    (tmp_path / "Cargo.lock").write_text("# lock\n")
    captured: dict[str, Any] = {}

    def fake_run(cmd: list[str], **kwargs: object) -> subprocess.CompletedProcess[str]:
        captured["cmd"] = cmd
        captured["kwargs"] = kwargs
        return subprocess.CompletedProcess(cmd, 0, "", "")

    monkeypatch.setattr(LOCKFILES.shutil, "which", lambda name: f"/usr/bin/{name}")
    monkeypatch.setattr(LOCKFILES, "_run_completed_command", fake_run)

    assert cli._verify_cargo_lock(tmp_path) is None
    assert captured["cmd"] == [
        "cargo",
        "metadata",
        "--locked",
        "--format-version",
        "1",
    ]
    assert captured["kwargs"]["memory_guard_prefix"] == "MOLT_BUILD"
    assert captured["kwargs"]["cwd"] == tmp_path


def test_native_link_command_uses_build_memory_guard(monkeypatch) -> None:
    captured: dict[str, Any] = {}

    def fake_run(cmd: list[str], **kwargs: object) -> subprocess.CompletedProcess[str]:
        captured["cmd"] = cmd
        captured["kwargs"] = kwargs
        return subprocess.CompletedProcess(cmd, 0, "out", "err")

    class FakeMemoryGuard:
        TIMEOUT_RETURN_CODE = 124

    class FakeHarness:
        memory_guard = FakeMemoryGuard()

    monkeypatch.setattr(cli_link_pipeline, "_run_completed_command", fake_run)
    monkeypatch.setattr(
        cli_link_pipeline, "_load_cli_harness_memory_guard", lambda cwd: FakeHarness()
    )

    result = cli_link_pipeline._run_native_link_command(
        link_cmd=["cc", "main.o"],
        json_output=True,
        link_timeout=12.0,
    )

    assert result.returncode == 0
    assert captured["cmd"] == ["cc", "main.o"]
    assert captured["kwargs"]["memory_guard_prefix"] == "MOLT_BUILD"
    assert captured["kwargs"]["timeout"] == 12.0


@pytest.mark.parametrize(
    "target", ["wasm32-wasip1", "aarch64-apple-darwin", "x86_64-pc-windows-msvc"]
)
def test_rust_target_readiness_is_source_bound_guarded_and_non_mutating(
    monkeypatch, tmp_path: Path, target: str
) -> None:
    inputs = WASM_TOOLCHAIN.wasm_link_inputs
    source = tmp_path / "compiler source"
    source.mkdir()
    (source / "rust-toolchain.toml").write_text('[toolchain]\nchannel="9.8.7"\n')
    guest = tmp_path / "guest"
    guest.mkdir()
    monkeypatch.chdir(guest)
    rustc = tmp_path / "rustc"
    rustc.write_bytes(b"selected compiler")
    libdir = tmp_path / "selected target lib"
    libdir.mkdir()
    # Another installed compiler must not satisfy the selected compiler's target.
    other = tmp_path / "rustup/toolchains/9.8.7-other/lib/rustlib" / target / "lib"
    other.mkdir(parents=True)
    (other / "libstd-other.rlib").write_bytes(b"other standard library")
    monkeypatch.setenv("RUSTUP_HOME", str(tmp_path / "rustup"))
    monkeypatch.setattr(inputs, "find_executable", lambda *args, **kwargs: rustc)

    def resolve(selected, *, role, root, env):
        assert selected == rustc and role == "rustc" and root == source
        return selected

    monkeypatch.setattr(inputs, "resolve_rustup_proxy", resolve)
    calls: list[tuple[list[str], dict[str, object]]] = []

    def fake_run(cmd: list[str], **kwargs: object) -> subprocess.CompletedProcess[str]:
        calls.append((cmd, kwargs))
        assert cmd == [str(rustc), "--print", "target-libdir", "--target", target]
        assert kwargs["cwd"] == source
        assert kwargs["memory_guard_prefix"] == "MOLT_BUILD"
        return subprocess.CompletedProcess(cmd, 0, str(libdir) + "\n", "")

    monkeypatch.setattr(inputs, "_run_completed_command", fake_run)
    inputs.clear_rust_target_libdir_cache()
    missing = WASM_TOOLCHAIN.rust_target_readiness_error(target, root=source)
    assert missing is not None
    assert f"rustup target add {target} --toolchain 9.8.7" in missing
    assert "No toolchains were installed or changed" in missing
    std = libdir / "libstd-selected.rlib"
    std.write_bytes(b"")
    assert WASM_TOOLCHAIN.rust_target_readiness_error(target, root=source) == missing
    std.write_bytes(b"standard library")
    assert WASM_TOOLCHAIN.rust_target_readiness_error(target, root=source) is None
    std.unlink()
    assert WASM_TOOLCHAIN.rust_target_readiness_error(target, root=source) == missing
    assert len(calls) == 1  # Query cache never caches library availability.


@pytest.mark.parametrize(
    "failure",
    [PermissionError("access denied"), subprocess.TimeoutExpired("rustc", 30)],
)
def test_rust_target_readiness_reports_failed_inspection(
    monkeypatch, tmp_path, failure
):
    def inspect(*args, **kwargs):
        raise failure

    monkeypatch.setattr(WASM_TOOLCHAIN.wasm_link_inputs, "rust_target_libdir", inspect)
    error = WASM_TOOLCHAIN.rust_target_readiness_error("wasm32-wasip1", root=tmp_path)
    assert error is not None
    assert "Cannot inspect Rust target wasm32-wasip1" in error
    assert str(failure) in error
    assert "No toolchains were installed or changed" in error


def test_mlir_backend_pipeline_uses_tempfile_memory_guard(
    monkeypatch,
    tmp_path: Path,
) -> None:
    captured: dict[str, Any] = {}
    output = tmp_path / "out.mlir"
    backend = tmp_path / "molt-backend-mlir"

    def fake_tempfiles(
        cmd: list[str], **kwargs: object
    ) -> subprocess.CompletedProcess[bytes]:
        captured["cmd"] = cmd
        captured["kwargs"] = kwargs
        output.write_text("module {}\n")
        return subprocess.CompletedProcess(cmd, 0, b"", b"")

    monkeypatch.setattr(
        MLIR_BACKEND,
        "_find_mlir_backend_binary",
        lambda root: backend,
    )
    monkeypatch.setattr(
        MLIR_BACKEND,
        "_run_subprocess_captured_to_tempfiles",
        fake_tempfiles,
    )

    rc = cli._run_mlir_backend_pipeline(
        ir={"functions": []},
        output_artifact=output,
        project_root=tmp_path,
        json_output=False,
        verbose=False,
    )

    assert rc == 0
    assert captured["cmd"] == [str(backend), "--output", str(output)]
    assert captured["kwargs"]["cwd"] == tmp_path
    assert captured["kwargs"]["timeout"] == 120
    assert captured["kwargs"]["progress_label"] == "MLIR backend"


def test_ty_check_uses_cli_memory_guard(monkeypatch, tmp_path: Path) -> None:
    captured: dict[str, Any] = {}

    def fake_run(cmd: list[str], **kwargs: object) -> subprocess.CompletedProcess[str]:
        captured["cmd"] = cmd
        captured["kwargs"] = kwargs
        return subprocess.CompletedProcess(cmd, 0, "ok", "")

    monkeypatch.setattr(cli_typecheck, "_run_completed_command", fake_run)

    ok, output = cli_typecheck._run_ty_check(tmp_path)

    assert ok is True
    assert output == "ok"
    assert captured["cmd"] == [
        "uv",
        "run",
        "ty",
        "check",
        str(tmp_path),
        "--output-format",
        "concise",
    ]
    assert captured["kwargs"]["memory_guard_prefix"] == "MOLT_CLI"
    assert captured["kwargs"]["timeout"] == 30.0


def test_ty_check_timeout_env_reaches_cli_memory_guard(
    monkeypatch,
    tmp_path: Path,
) -> None:
    captured: dict[str, Any] = {}

    def fake_run(cmd: list[str], **kwargs: object) -> subprocess.CompletedProcess[str]:
        captured["cmd"] = cmd
        captured["kwargs"] = kwargs
        return subprocess.CompletedProcess(cmd, 0, "ok", "")

    monkeypatch.setenv("MOLT_TY_TIMEOUT", "1.5")
    monkeypatch.setattr(cli_typecheck, "_run_completed_command", fake_run)

    ok, output = cli_typecheck._run_ty_check(tmp_path)

    assert ok is True
    assert output == "ok"
    assert captured["kwargs"]["timeout"] == 1.5


def test_ty_check_timeout_returns_guarded_failure(
    monkeypatch,
    tmp_path: Path,
) -> None:
    def fake_run(cmd: list[str], **kwargs: object) -> subprocess.CompletedProcess[str]:
        raise subprocess.TimeoutExpired(cmd, timeout=2.0)

    monkeypatch.setenv("MOLT_TY_TIMEOUT", "2")
    monkeypatch.setattr(cli_typecheck, "_run_completed_command", fake_run)

    ok, output = cli_typecheck._run_ty_check(tmp_path)

    assert ok is False
    assert "ty check timed out after 2.0s" in output
    assert "guarded hints" in output


def test_backend_daemon_spawn_uses_guard_context_and_sentinel(
    monkeypatch,
    tmp_path: Path,
) -> None:
    COMPILER_METADATA._rustc_version.cache_clear()
    backend = tmp_path / "molt-backend"
    backend.write_text("backend")
    socket_path = tmp_path / "daemon.sock"
    captured: dict[str, Any] = {}
    sentinel_events: list[str] = []

    class FakeSentinel:
        def __exit__(self, exc_type, exc, tb) -> None:
            sentinel_events.append("exit")

    class FakeContext:
        env = {"PATH": "/usr/bin", "MOLT_EXT_ROOT": str(tmp_path)}

        def run(
            self,
            command: list[str],
            **kwargs: object,
        ) -> subprocess.CompletedProcess[str]:
            captured.setdefault("run_calls", []).append(
                {"command": command, "kwargs": kwargs}
            )
            return subprocess.CompletedProcess(command, 0, "rustc test\n", "")

        def process_group_kwargs(self) -> dict[str, object]:
            return {"start_new_session": True, "preexec_fn": lambda: None}

        def start_repo_sentinel(self, **kwargs: object) -> FakeSentinel:
            captured["sentinel_kwargs"] = kwargs
            sentinel_events.append("start")
            return FakeSentinel()

    class FakeHarness:
        class HarnessExecutionContext:
            @classmethod
            def from_env(cls, prefix, env, *, repo_root):  # type: ignore[no-untyped-def]
                context = {
                    "prefix": prefix,
                    "env": env,
                    "repo_root": repo_root,
                }
                captured.setdefault("contexts", []).append(context)
                captured["context"] = context
                return FakeContext()

    class FakeProc:
        pid = 4321

        def poll(self) -> None:
            return None

    def fake_popen(cmd: list[str], **kwargs: object) -> FakeProc:
        captured["popen_cmd"] = cmd
        captured["popen_kwargs"] = kwargs
        return FakeProc()

    monkeypatch.setattr(
        BACKEND_EXECUTION, "_load_cli_harness_memory_guard", lambda cwd: FakeHarness()
    )
    monkeypatch.setattr(
        COMMAND_RUNTIME, "_load_cli_harness_memory_guard", lambda cwd: FakeHarness()
    )
    monkeypatch.setattr(BACKEND_EXECUTION.subprocess, "Popen", fake_popen)
    monkeypatch.setattr(
        BACKEND_EXECUTION,
        "_backend_daemon_wait_until_ready",
        lambda *a, **k: (True, None),
    )
    monkeypatch.setattr(
        BACKEND_EXECUTION, "_unix_socket_path_exceeds_limit", lambda path: False
    )
    monkeypatch.setattr(
        BACKEND_EXECUTION,
        "_backend_daemon_process_command",
        lambda pid: f"{backend} --daemon --socket {socket_path}",
    )

    ok = cli._start_backend_daemon(
        backend,
        socket_path,
        cargo_profile="dev-fast",
        project_root=tmp_path,
        target_triple=None,
        config_digest="a" * 64,
        startup_timeout=1.0,
        json_output=True,
        warnings=[],
    )

    assert ok is True
    assert captured["context"]["prefix"] == "MOLT_BUILD"
    assert captured["contexts"][0]["prefix"] == "MOLT_BUILD"
    assert captured["sentinel_kwargs"]["label"] == "backend_daemon_start"
    assert captured["sentinel_kwargs"]["drain_on_exit"] is False
    assert captured["popen_cmd"] == [
        str(backend),
        "--daemon",
        "--socket",
        str(socket_path),
    ]
    assert captured["popen_kwargs"]["start_new_session"] is True
    assert callable(captured["popen_kwargs"]["preexec_fn"])
    assert sentinel_events == ["start", "exit"]
    COMPILER_METADATA._rustc_version.cache_clear()


def test_backend_daemon_request_uses_request_scoped_sentinel(
    monkeypatch,
    tmp_path: Path,
) -> None:
    captured: dict[str, Any] = {}
    sentinel_events: list[str] = []

    class FakeSentinel:
        def __exit__(self, exc_type, exc, tb) -> None:
            sentinel_events.append("exit")

    class FakeContext:
        def start_repo_sentinel(self, **kwargs: object) -> FakeSentinel:
            captured["sentinel_kwargs"] = kwargs
            sentinel_events.append("start")
            return FakeSentinel()

    class FakeHarness:
        class HarnessExecutionContext:
            @classmethod
            def from_env(cls, prefix, env, *, repo_root):  # type: ignore[no-untyped-def]
                captured["context"] = {
                    "prefix": prefix,
                    "env": env,
                    "repo_root": repo_root,
                }
                return FakeContext()

    class FakeSocket:
        def __enter__(self) -> "FakeSocket":
            return self

        def __exit__(self, exc_type, exc, tb) -> None:
            return None

        def settimeout(self, value: float) -> None:
            captured["timeout"] = value

        def connect(self, path: str) -> None:
            captured["socket_path"] = path

        def sendall(self, data: bytes) -> None:
            captured["data"] = data

        def shutdown(self, how: int) -> None:
            captured["shutdown"] = how

        def recv_into(self, view: memoryview) -> int:
            payload = b'{"ok": true, "jobs": [{"output_written": false}]}\n'
            view[: len(payload)] = payload
            return len(payload)

    monkeypatch.setattr(
        BACKEND_EXECUTION, "_load_cli_harness_memory_guard", lambda cwd: FakeHarness()
    )
    monkeypatch.setattr(BACKEND_EXECUTION.socket, "AF_UNIX", 1, raising=False)
    monkeypatch.setattr(
        BACKEND_EXECUTION.socket, "socket", lambda *args, **kwargs: FakeSocket()
    )

    daemon_identity = cli._BackendDaemonIdentity(
        pid=1234,
        socket_path=tmp_path / "daemon.sock",
        project_root=tmp_path,
        cargo_profile="dev-fast",
        config_digest=None,
        backend_bin=tmp_path / "molt-backend",
        created_at=1_700_000_000.0,
    )

    response, err = cli._backend_daemon_request_bytes(
        tmp_path / "daemon.sock",
        b'{"version": 1}\n',
        timeout=None,
        daemon_identity=daemon_identity,
        project_root=tmp_path,
    )

    assert err is None
    assert response == {"ok": True, "jobs": [{"output_written": False}]}
    assert captured["context"]["prefix"] == "MOLT_BUILD"
    assert captured["context"]["repo_root"] == tmp_path
    assert captured["sentinel_kwargs"]["label"] == "backend_daemon_request_1234"
    assert captured["sentinel_kwargs"]["drain_on_exit"] is False
    assert sentinel_events == ["start", "exit"]


def test_git_source_commands_use_build_memory_guard(
    monkeypatch,
    tmp_path: Path,
) -> None:
    deps = importlib.import_module("molt.cli.deps")

    calls: list[tuple[list[str], dict[str, object]]] = []

    def fake_run(cmd: list[str], **kwargs: object) -> subprocess.CompletedProcess[str]:
        calls.append((cmd, kwargs))
        if cmd[:3] == ["git", "clone", "--filter=blob:none"]:
            repo_dir = Path(cmd[-1])
            repo_dir.mkdir(parents=True)
            (repo_dir / "pkg").mkdir()
            (repo_dir / "pkg" / "module.py").write_text("x = 1\n")
        if cmd[:2] == ["git", "ls-remote"]:
            return subprocess.CompletedProcess(cmd, 0, "abc123\trefs/heads/main\n", "")
        if cmd[-1] == "HEAD":
            return subprocess.CompletedProcess(cmd, 0, "abc123\n", "")
        if cmd[-1] == "HEAD^{tree}":
            return subprocess.CompletedProcess(cmd, 0, "tree123\n", "")
        return subprocess.CompletedProcess(cmd, 0, "", "")

    monkeypatch.setattr(deps, "_run_completed_command", fake_run)

    resolved, err = deps._resolve_git_ref(
        "https://example.invalid/repo.git",
        "main",
        project_root=tmp_path,
    )
    assert err is None
    assert resolved == "abc123"

    dest = tmp_path / "vendor" / "pkg"
    dest.parent.mkdir()
    commit, tree = deps._clone_git_source(
        "https://example.invalid/repo.git",
        "abc123",
        dest,
        project_root=tmp_path,
        subdirectory="pkg",
    )

    assert commit == "abc123"
    assert tree == "tree123"
    assert (dest / "module.py").read_text() == "x = 1\n"
    assert calls
    for _cmd, kwargs in calls:
        assert kwargs["memory_guard_prefix"] == "MOLT_BUILD"
        assert kwargs["cwd"] == tmp_path
        assert kwargs["capture_output"] is True
