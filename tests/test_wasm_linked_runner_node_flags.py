from __future__ import annotations

import json
import os
import subprocess
import sys
from pathlib import Path
from typing import Any, cast

import pytest

import tests.wasm_linked_runner as wasm_runner


def _require_node_binary() -> str:
    node_bin = wasm_runner._select_node_binary()
    if node_bin is None:
        pytest.skip("Node >= 18 is required for explicit JS-runner coverage.")
    return node_bin


def test_wasm_test_process_uses_memory_guard(monkeypatch, tmp_path: Path) -> None:
    captured: dict[str, Any] = {}

    def fake_guarded_completed_process(cmd, **kwargs):  # type: ignore[no-untyped-def]
        captured["cmd"] = cmd
        captured["kwargs"] = kwargs
        return subprocess.CompletedProcess(cmd, 0, stdout="ok\n", stderr="")

    monkeypatch.setattr(
        wasm_runner.harness_memory_guard,
        "guarded_completed_process",
        fake_guarded_completed_process,
    )

    result = wasm_runner._run_wasm_test_process(
        ["node", "-e", "console.log('ok')"],
        cwd=tmp_path,
        env={"NODE_NO_WARNINGS": "1"},
        timeout=5,
    )

    assert result.returncode == 0
    assert result.stdout == "ok\n"
    assert captured["cmd"] == ["node", "-e", "console.log('ok')"]
    assert captured["kwargs"]["prefix"] == "MOLT_WASM_TEST"
    assert captured["kwargs"]["cwd"] == tmp_path
    assert captured["kwargs"]["timeout"] == 5


def test_wasm_test_process_preserves_timeout_semantics(
    monkeypatch, tmp_path: Path
) -> None:
    def fake_guarded_completed_process(cmd, **kwargs):  # type: ignore[no-untyped-def]
        return subprocess.CompletedProcess(
            cmd,
            wasm_runner.harness_memory_guard.memory_guard.TIMEOUT_RETURN_CODE,
            stdout="partial",
            stderr="memory_guard: timeout after 2.00s\n",
        )

    monkeypatch.setattr(
        wasm_runner.harness_memory_guard,
        "guarded_completed_process",
        fake_guarded_completed_process,
    )

    try:
        wasm_runner._run_wasm_test_process(
            ["node", "runner.js"],
            cwd=tmp_path,
            env={},
            timeout=2,
        )
    except subprocess.TimeoutExpired as exc:
        assert exc.cmd == ["node", "runner.js"]
        assert exc.output == "partial"
        assert exc.stderr == "memory_guard: timeout after 2.00s\n"
    else:  # pragma: no cover - assertion clarity
        raise AssertionError("expected TimeoutExpired")


def test_resolve_molt_wasm_host_binary_prefers_explicit_env(
    monkeypatch,
    tmp_path: Path,
) -> None:
    host_bin = tmp_path / wasm_runner._molt_wasm_host_exe_name()
    host_bin.write_bytes(b"host")
    target_host = (
        tmp_path / "target" / "dev-fast" / wasm_runner._molt_wasm_host_exe_name()
    )
    target_host.parent.mkdir(parents=True)
    target_host.write_bytes(b"target")

    monkeypatch.setenv("MOLT_WASM_HOST_BIN", str(host_bin))
    monkeypatch.setenv("CARGO_TARGET_DIR", str(target_host.parent.parent))

    assert wasm_runner._resolve_molt_wasm_host_binary(tmp_path) == str(host_bin)


def test_resolve_molt_wasm_host_binary_uses_dev_fast_target_dir(
    monkeypatch,
    tmp_path: Path,
) -> None:
    host_bin = tmp_path / "target" / "dev-fast" / wasm_runner._molt_wasm_host_exe_name()
    host_bin.parent.mkdir(parents=True)
    host_bin.write_bytes(b"host")

    monkeypatch.delenv("MOLT_WASM_HOST_BIN", raising=False)
    monkeypatch.setenv("MOLT_WASM_TEST_CARGO_TARGET_DIR", str(host_bin.parent.parent))

    assert wasm_runner._resolve_molt_wasm_host_binary(tmp_path) == str(host_bin)


def test_run_wasm_linked_uses_molt_wasm_host(
    monkeypatch,
    tmp_path: Path,
) -> None:
    wasm_path = tmp_path / "output_linked.wasm"
    wasm_path.write_bytes(b"\x00asm")
    host_bin = tmp_path / wasm_runner._molt_wasm_host_exe_name()
    host_bin.write_bytes(b"host")
    monkeypatch.setenv("MOLT_WASM_HOST_BIN", str(host_bin))
    recorded: dict[str, Any] = {}

    def _fake_run(*args, **kwargs):  # type: ignore[no-untyped-def]
        recorded["args"] = list(args[0])
        recorded["env"] = dict(kwargs["env"])
        return subprocess.CompletedProcess(args[0], 0, "", "")

    monkeypatch.setattr(wasm_runner, "_run_wasm_test_process", _fake_run)
    result = wasm_runner.run_wasm_linked(tmp_path, wasm_path)
    assert result.returncode == 0
    cmd = cast(list[str], recorded["args"])
    assert cmd == [str(host_bin), str(wasm_path)]
    env = cast(dict[str, str], recorded["env"])
    assert "NODE_NO_WARNINGS" not in env
    assert "MOLT_WASM_TEST_CHILD_RLIMIT_GB" not in env


def test_run_wasm_linked_preserves_inherited_child_rlimit_by_default(
    monkeypatch,
    tmp_path: Path,
) -> None:
    wasm_path = tmp_path / "output_linked.wasm"
    wasm_path.write_bytes(b"\x00asm")
    host_bin = tmp_path / wasm_runner._molt_wasm_host_exe_name()
    host_bin.write_bytes(b"host")
    monkeypatch.setenv("MOLT_WASM_HOST_BIN", str(host_bin))
    monkeypatch.setenv("MOLT_WASM_TEST_CHILD_RLIMIT_GB", "16")
    recorded: dict[str, Any] = {}

    def _fake_run(*args, **kwargs):  # type: ignore[no-untyped-def]
        recorded["env"] = dict(kwargs["env"])
        return subprocess.CompletedProcess(args[0], 0, "", "")

    monkeypatch.setattr(wasm_runner, "_run_wasm_test_process", _fake_run)
    result = wasm_runner.run_wasm_linked(tmp_path, wasm_path)

    assert result.returncode == 0
    env = cast(dict[str, str], recorded["env"])
    assert env.get("MOLT_WASM_TEST_CHILD_RLIMIT_GB") == "16"


def test_run_wasm_linked_preserves_explicit_env_overrides(
    monkeypatch,
    tmp_path: Path,
) -> None:
    wasm_path = tmp_path / "output_linked.wasm"
    wasm_path.write_bytes(b"\x00asm")
    host_bin = tmp_path / wasm_runner._molt_wasm_host_exe_name()
    host_bin.write_bytes(b"host")
    monkeypatch.setenv("MOLT_WASM_HOST_BIN", str(host_bin))
    recorded: dict[str, Any] = {}

    def _fake_run(*args, **kwargs):  # type: ignore[no-untyped-def]
        recorded.setdefault("envs", []).append(dict(kwargs["env"]))
        return subprocess.CompletedProcess(args[0], 0, "", "")

    monkeypatch.setattr(wasm_runner, "_run_wasm_test_process", _fake_run)

    result = wasm_runner.run_wasm_linked(
        tmp_path,
        wasm_path,
        env_overrides={
            "MOLT_RUNTIME_WASM": "",
            "MOLT_WASM_HOST_DEBUG": "1",
            "MOLT_WASM_TEST_CHILD_RLIMIT_GB": "0",
        },
    )
    assert result.returncode == 0

    envs = cast(list[dict[str, str]], recorded["envs"])
    assert envs[0].get("MOLT_RUNTIME_WASM") == ""
    assert envs[0].get("MOLT_WASM_HOST_DEBUG") == "1"
    assert envs[0].get("MOLT_WASM_TEST_CHILD_RLIMIT_GB") == "0"


def test_run_wasm_linked_scrubs_stale_direct_mode_env(
    monkeypatch,
    tmp_path: Path,
) -> None:
    wasm_path = tmp_path / "output_linked.wasm"
    wasm_path.write_bytes(b"\x00asm")
    host_bin = tmp_path / wasm_runner._molt_wasm_host_exe_name()
    host_bin.write_bytes(b"host")
    monkeypatch.setenv("MOLT_WASM_HOST_BIN", str(host_bin))
    monkeypatch.setenv("MOLT_WASM_DIRECT_LINK", "1")
    monkeypatch.setenv("MOLT_WASM_PREFER_LINKED", "0")
    monkeypatch.setenv("MOLT_WASM_LINKED_PATH", "/tmp/stale-linked.wasm")
    monkeypatch.setenv("MOLT_WASM_TABLE_BASE", "123")
    monkeypatch.setenv("MOLT_RUNTIME_WASM", "/tmp/stale-runtime.wasm")
    recorded: dict[str, Any] = {}

    def _fake_run(*args, **kwargs):  # type: ignore[no-untyped-def]
        recorded["env"] = dict(kwargs["env"])
        return subprocess.CompletedProcess(args[0], 0, "", "")

    monkeypatch.setattr(wasm_runner, "_run_wasm_test_process", _fake_run)
    result = wasm_runner.run_wasm_linked(tmp_path, wasm_path)
    assert result.returncode == 0
    env = cast(dict[str, str], recorded["env"])
    assert "MOLT_WASM_DIRECT_LINK" not in env
    assert "MOLT_WASM_PREFER_LINKED" not in env
    assert "MOLT_WASM_LINKED_PATH" not in env
    assert "MOLT_WASM_TABLE_BASE" not in env
    assert "MOLT_RUNTIME_WASM" not in env


def test_build_wasm_linked_treats_symlinked_ext_root_as_repo_local(
    monkeypatch,
    tmp_path: Path,
) -> None:
    root = Path(__file__).resolve().parents[1]
    alias_root = tmp_path / "repo-alias"
    alias_root.symlink_to(root, target_is_directory=True)
    src = tmp_path / "probe.py"
    src.write_text("print('hi')\n")
    recorded: dict[str, Any] = {}

    def _fake_run(*args, **kwargs):  # type: ignore[no-untyped-def]
        recorded["env"] = dict(kwargs["env"])
        out_dir = Path(args[0][args[0].index("--out-dir") + 1])
        out_dir.mkdir(parents=True, exist_ok=True)
        (out_dir / "output_linked.wasm").write_bytes(b"\x00asm")
        return subprocess.CompletedProcess(args[0], 0, "", "")

    monkeypatch.setenv("MOLT_EXT_ROOT", str(alias_root))
    monkeypatch.setattr(wasm_runner, "_run_wasm_test_process", _fake_run)
    output = wasm_runner.build_wasm_linked(root, src, tmp_path)
    assert output.exists()
    env = cast(dict[str, str], recorded["env"])
    assert env["CARGO_TARGET_DIR"].startswith(str(root / "target" / "pytest_wasm"))


def test_build_wasm_linked_marks_repo_local_output_as_output_not_required_external(
    monkeypatch,
    tmp_path: Path,
) -> None:
    root = tmp_path / "repo"
    root.mkdir()
    src = tmp_path / "probe.py"
    src.write_text("print('hi')\n")
    recorded: dict[str, Any] = {}

    def _fake_run(*args, **kwargs):  # type: ignore[no-untyped-def]
        recorded["args"] = list(args[0])
        recorded["env"] = dict(kwargs["env"])
        out_dir = Path(args[0][args[0].index("--out-dir") + 1])
        out_dir.mkdir(parents=True, exist_ok=True)
        (out_dir / "output_linked.wasm").write_bytes(b"\x00asm")
        return subprocess.CompletedProcess(args[0], 0, "", "")

    monkeypatch.delenv("MOLT_EXT_ROOT", raising=False)
    monkeypatch.delenv("MOLT_REQUIRE_EXTERNAL_ARTIFACTS", raising=False)
    monkeypatch.setattr(wasm_runner, "_run_wasm_test_process", _fake_run)
    output = wasm_runner.build_wasm_linked(root, src, tmp_path)

    assert output.exists()
    env = cast(dict[str, str], recorded["env"])
    assert Path(env["MOLT_EXT_ROOT"]).is_relative_to(root / "build" / "wasm")
    assert Path(env["CARGO_TARGET_DIR"]).is_relative_to(
        Path(env["MOLT_EXT_ROOT"]) / "target"
    )
    assert "MOLT_REQUIRE_EXTERNAL_ARTIFACTS" not in env


def test_wasm_test_target_dir_uses_stable_local_lane(
    monkeypatch,
    tmp_path: Path,
) -> None:
    root = tmp_path / "repo"
    root.mkdir()
    out_dir = tmp_path / "out"
    monkeypatch.delenv("MOLT_WASM_TEST_LANE", raising=False)
    monkeypatch.delenv("PYTEST_XDIST_WORKER", raising=False)

    first = wasm_runner._wasm_test_target_dir(root, out_dir, root)
    second = wasm_runner._wasm_test_target_dir(root, out_dir, root)

    assert first == root / "target" / "pytest_wasm" / "local"
    assert second == first


def test_wasm_test_target_dir_preserves_explicit_and_worker_lanes(
    monkeypatch,
    tmp_path: Path,
) -> None:
    root = tmp_path / "repo"
    root.mkdir()
    out_dir = tmp_path / "out"

    monkeypatch.setenv("PYTEST_XDIST_WORKER", "gw2")
    monkeypatch.delenv("MOLT_WASM_TEST_LANE", raising=False)
    worker_target = wasm_runner._wasm_test_target_dir(root, out_dir, root)
    assert worker_target == root / "target" / "pytest_wasm" / "worker_gw2"

    monkeypatch.setenv("MOLT_WASM_TEST_LANE", "custom-lane")
    explicit_target = wasm_runner._wasm_test_target_dir(root, out_dir, root)
    assert explicit_target == root / "target" / "pytest_wasm" / "custom-lane"


def test_build_wasm_linked_does_not_mutate_process_runtime_env(
    monkeypatch,
    tmp_path: Path,
) -> None:
    root = Path(__file__).resolve().parents[1]
    src = tmp_path / "probe.py"
    src.write_text("print('hi')\n")
    monkeypatch.delenv("MOLT_RUNTIME_WASM", raising=False)

    def _fake_run(*args, **kwargs):  # type: ignore[no-untyped-def]
        out_dir = Path(args[0][args[0].index("--out-dir") + 1])
        out_dir.mkdir(parents=True, exist_ok=True)
        (out_dir / "output_linked.wasm").write_bytes(b"\x00asm")
        return subprocess.CompletedProcess(args[0], 0, "", "")

    monkeypatch.setattr(wasm_runner, "_run_wasm_test_process", _fake_run)
    output = wasm_runner.build_wasm_linked(root, src, tmp_path)
    assert output.exists()
    assert "MOLT_RUNTIME_WASM" not in os.environ


def test_run_wasm_linked_does_not_require_runtime_sidecar_when_linked(
    tmp_path: Path,
) -> None:
    root = Path(__file__).resolve().parents[1]
    wasm_runner.require_wasm_toolchain()
    src = root / "examples" / "hello.py"
    output_wasm = wasm_runner.build_wasm_linked(root, src, tmp_path)
    result = wasm_runner.run_wasm_linked(
        root,
        output_wasm,
        env_overrides={"MOLT_RUNTIME_WASM": ""},
    )
    assert result.returncode == 0, result.stderr
    assert result.stdout.strip().endswith("42")


def test_run_wasm_linked_bench_sum_has_no_table_signature_trap(
    tmp_path: Path,
) -> None:
    root = Path(__file__).resolve().parents[1]
    wasm_runner.require_wasm_toolchain()
    src = root / "tests" / "benchmarks" / "bench_sum.py"
    output_wasm = wasm_runner.build_wasm_linked(root, src, tmp_path)
    result = wasm_runner.run_wasm_linked(
        root,
        output_wasm,
        env_overrides={"MOLT_RUNTIME_WASM": ""},
    )
    assert result.returncode == 0, result.stderr
    assert result.stdout.strip().endswith("49999995000000")
    assert "null function or function signature mismatch" not in result.stderr


def test_run_wasm_direct_bootstraps_split_runtime_before_main(
    tmp_path: Path,
) -> None:
    root = Path(__file__).resolve().parents[1]
    wasm_runner.require_wasm_build_toolchain()
    node_bin = _require_node_binary()
    src = tmp_path / "direct_bootstrap.py"
    src.write_text("import abc\nprint('after')\n", encoding="utf-8")

    env = os.environ.copy()
    env["PYTHONPATH"] = str(root / "src")
    build = wasm_runner._run_wasm_test_process(
        [
            sys.executable,
            "-m",
            "molt.cli",
            "build",
            str(src),
            "--build-profile",
            "dev",
            "--profile",
            "browser",
            "--target",
            "wasm",
            "--split-runtime",
            "--out-dir",
            str(tmp_path),
        ],
        cwd=root,
        env=env,
        capture_output=True,
        text=True,
        check=False,
        timeout=900,
    )
    assert build.returncode == 0, build.stderr

    result = wasm_runner._run_wasm_test_process(
        [node_bin, "wasm/run_wasm.js", str(tmp_path / "app.wasm")],
        cwd=root,
        env=os.environ.copy(),
        capture_output=True,
        text=True,
        timeout=20,
        check=False,
    )
    assert result.returncode == 0, result.stderr
    assert result.stdout.strip().splitlines() == ["after"]


def test_linked_wasm_exports_table_base_setter_when_available(
    tmp_path: Path,
) -> None:
    root = Path(__file__).resolve().parents[1]
    wasm_runner.require_wasm_toolchain()
    src = root / "examples" / "hello.py"
    output_wasm = wasm_runner.build_wasm_linked(root, src, tmp_path)
    node_bin = _require_node_binary()
    probe = wasm_runner._run_wasm_test_process(
        [
            node_bin,
            "-e",
            (
                "const fs=require('fs');"
                "const p=process.argv[1];"
                "WebAssembly.compile(fs.readFileSync(p)).then((m)=>{"
                "const names=WebAssembly.Module.exports(m).map((e)=>e.name);"
                "console.log(JSON.stringify(names));"
                "}).catch((err)=>{console.error(String(err));process.exit(1);});"
            ),
            str(output_wasm),
        ],
        cwd=root,
        capture_output=True,
        text=True,
        check=False,
    )
    assert probe.returncode == 0, probe.stderr
    names = json.loads(probe.stdout)
    assert "molt_set_wasm_table_base" in names
