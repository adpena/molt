from __future__ import annotations

from pathlib import Path

import pytest

import tools.bench_wasm as bench_wasm


def _reset_cache(monkeypatch: pytest.MonkeyPatch) -> None:
    monkeypatch.setattr(bench_wasm, "_NODE_BIN_CACHE", None)


def test_resolve_node_binary_accepts_env_override(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    _reset_cache(monkeypatch)
    monkeypatch.setenv("MOLT_NODE_BIN", "/custom/node")
    monkeypatch.setattr(
        bench_wasm,
        "_node_major_for_binary",
        lambda path: 20 if path == "/custom/node" else None,
    )
    assert bench_wasm.resolve_node_binary() == "/custom/node"


def test_resolve_node_binary_rejects_old_env_override(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    _reset_cache(monkeypatch)
    monkeypatch.setenv("MOLT_NODE_BIN", "/old/node")
    monkeypatch.setattr(bench_wasm, "_node_major_for_binary", lambda _path: 14)
    with pytest.raises(RuntimeError, match="Node >="):
        bench_wasm.resolve_node_binary()


def test_resolve_node_binary_prefers_highest_major(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    _reset_cache(monkeypatch)
    monkeypatch.delenv("MOLT_NODE_BIN", raising=False)
    monkeypatch.setattr(bench_wasm.shutil, "which", lambda _name: "/usr/local/bin/node")
    majors = {
        "/usr/local/bin/node": 14,
        "/opt/homebrew/bin/node": 25,
    }
    monkeypatch.setattr(
        bench_wasm, "_node_major_for_binary", lambda path: majors.get(path)
    )
    assert bench_wasm.resolve_node_binary() == "/opt/homebrew/bin/node"


def test_resolve_node_binary_errors_when_none_valid(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    _reset_cache(monkeypatch)
    monkeypatch.delenv("MOLT_NODE_BIN", raising=False)
    monkeypatch.setattr(bench_wasm.shutil, "which", lambda _name: None)
    monkeypatch.setattr(bench_wasm, "_node_major_for_binary", lambda _path: None)
    with pytest.raises(RuntimeError, match="Node binary not found"):
        bench_wasm.resolve_node_binary()


def test_resolve_runner_node_enforces_stable_wasm_flags(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    monkeypatch.setattr(bench_wasm, "resolve_node_binary", lambda: "/usr/bin/node")
    monkeypatch.delenv("MOLT_WASM_NODE_OPTIONS", raising=False)
    cmd = bench_wasm._resolve_runner(
        "node", tty=False, log=None, node_max_old_space_mb=None
    )
    assert cmd[0] == "/usr/bin/node"
    assert "--no-warnings" in cmd
    assert "--no-wasm-tier-up" in cmd
    assert "--no-wasm-dynamic-tiering" in cmd
    assert "--wasm-num-compilation-tasks=1" in cmd
    assert cmd[-1] == "run_wasm.js"


def test_prepare_wasm_binary_sets_linked_table_base(
    monkeypatch: pytest.MonkeyPatch,
    tmp_path: Path,
) -> None:
    reloc_runtime = tmp_path / "molt_runtime_reloc.wasm"
    reloc_runtime.write_bytes(b"\x00asm")
    monkeypatch.setattr(bench_wasm, "RUNTIME_WASM_RELOC", reloc_runtime)
    monkeypatch.setattr(bench_wasm, "RUNTIME_WASM", tmp_path / "molt_runtime.wasm")
    monkeypatch.setattr(bench_wasm, "_want_linked", lambda: True)
    monkeypatch.setattr(bench_wasm, "_base_env", lambda: {})
    monkeypatch.setattr(bench_wasm, "_python_cmd", lambda: ["python3"])
    monkeypatch.setattr(bench_wasm, "_read_wasm_table_min", lambda _path: 2354)
    captured_env: dict[str, str] = {}

    def _fake_build(
        _python_cmd: list[str],
        env: dict[str, str],
        output_path: Path,
        _script: str,
        *,
        tty: bool,
        log,
    ) -> float:
        del tty, log
        captured_env.update(env)
        output_path.write_bytes(b"\x00asm")
        return 0.01

    def _fake_link(
        _env: dict[str, str],
        input_path: Path,
        *,
        require_linked: bool,
        log,
    ) -> Path:
        del require_linked, log
        linked = input_path.with_name("output_linked.wasm")
        linked.write_bytes(b"\x00asm")
        return linked

    monkeypatch.setattr(bench_wasm, "_build_wasm_output", _fake_build)
    monkeypatch.setattr(bench_wasm, "_link_wasm", _fake_link)

    wasm = bench_wasm.prepare_wasm_binary(
        "tests/benchmarks/bench_sum.py",
        require_linked=False,
        tty=False,
        log=None,
        keep_temp=False,
    )
    assert wasm is not None
    assert captured_env.get("MOLT_WASM_LINK") == "1"
    assert captured_env.get("MOLT_WASM_TABLE_BASE") == "2354"
