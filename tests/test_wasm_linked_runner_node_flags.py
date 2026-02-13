from __future__ import annotations

import json
import subprocess
from pathlib import Path
from typing import Any, cast

import tests.wasm_linked_runner as wasm_runner


def test_run_wasm_linked_uses_stable_node_flags(
    monkeypatch,
    tmp_path: Path,
) -> None:
    wasm_path = tmp_path / "output_linked.wasm"
    wasm_path.write_bytes(b"\x00asm")
    monkeypatch.setattr(wasm_runner, "_select_node_binary", lambda: "/usr/bin/node")
    recorded: dict[str, Any] = {}

    def _fake_run(*args, **kwargs):  # type: ignore[no-untyped-def]
        recorded["args"] = list(args[0])
        recorded["env"] = dict(kwargs["env"])
        return subprocess.CompletedProcess(args[0], 0, "", "")

    monkeypatch.setattr(wasm_runner.subprocess, "run", _fake_run)
    result = wasm_runner.run_wasm_linked(tmp_path, wasm_path)
    assert result.returncode == 0
    cmd = cast(list[str], recorded["args"])
    assert cmd[0] == "/usr/bin/node"
    assert "--no-warnings" in cmd
    assert "--no-wasm-tier-up" in cmd
    assert "--no-wasm-dynamic-tiering" in cmd
    assert "--wasm-num-compilation-tasks=1" in cmd
    assert cmd[-2:] == [str(tmp_path / "run_wasm.js"), str(wasm_path)]
    env = cast(dict[str, str], recorded["env"])
    assert env.get("NODE_NO_WARNINGS") == "1"


def test_run_wasm_linked_env_overrides_can_opt_out_of_node_warning_suppression(
    monkeypatch,
    tmp_path: Path,
) -> None:
    wasm_path = tmp_path / "output_linked.wasm"
    wasm_path.write_bytes(b"\x00asm")
    monkeypatch.setattr(wasm_runner, "_select_node_binary", lambda: "/usr/bin/node")
    recorded: dict[str, Any] = {}

    def _fake_run(*args, **kwargs):  # type: ignore[no-untyped-def]
        recorded["env"] = dict(kwargs["env"])
        return subprocess.CompletedProcess(args[0], 0, "", "")

    monkeypatch.setattr(wasm_runner.subprocess, "run", _fake_run)
    result = wasm_runner.run_wasm_linked(
        tmp_path,
        wasm_path,
        env_overrides={"NODE_NO_WARNINGS": "0"},
    )
    assert result.returncode == 0
    env = cast(dict[str, str], recorded["env"])
    assert env.get("NODE_NO_WARNINGS") == "0"


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


def test_linked_wasm_exports_table_base_setter_when_available(
    tmp_path: Path,
) -> None:
    root = Path(__file__).resolve().parents[1]
    wasm_runner.require_wasm_toolchain()
    src = root / "examples" / "hello.py"
    output_wasm = wasm_runner.build_wasm_linked(root, src, tmp_path)
    node_bin = wasm_runner._select_node_binary()
    assert node_bin is not None
    probe = subprocess.run(
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
        capture_output=True,
        text=True,
        check=False,
    )
    assert probe.returncode == 0, probe.stderr
    names = json.loads(probe.stdout)
    assert "molt_set_wasm_table_base" in names
