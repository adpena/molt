from __future__ import annotations

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
