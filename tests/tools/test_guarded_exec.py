from __future__ import annotations

import importlib.util
from pathlib import Path
from types import SimpleNamespace


REPO_ROOT = Path(__file__).resolve().parents[2]
GUARDED_EXEC = REPO_ROOT / "tools" / "guarded_exec.py"


def _load_guarded_exec():
    spec = importlib.util.spec_from_file_location(
        "molt_tools_guarded_exec", GUARDED_EXEC
    )
    assert spec is not None
    assert spec.loader is not None
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


def _install_fake_context(module, monkeypatch):
    captured: dict[str, object] = {}

    class FakeContext:
        @classmethod
        def from_env(cls, prefix, env, *, repo_root):
            captured["prefix"] = prefix
            captured["env"] = dict(env)
            captured["repo_root"] = repo_root
            return cls()

        def run(self, command, *, cwd, env, capture_output, timeout):
            captured["command"] = list(command)
            captured["cwd"] = cwd
            captured["run_env"] = dict(env)
            captured["capture_output"] = capture_output
            captured["timeout"] = timeout
            return SimpleNamespace(returncode=0, stderr="")

    monkeypatch.setattr(
        module.harness_memory_guard,
        "HarnessExecutionContext",
        FakeContext,
        raising=True,
    )
    return captured


def test_guarded_exec_uses_family_timeout_env(monkeypatch) -> None:
    module = _load_guarded_exec()
    captured = _install_fake_context(module, monkeypatch)
    monkeypatch.setenv("MOLT_WASM_TEST_TIMEOUT_SEC", "123.5")

    rc = module.main(["--prefix", "MOLT_WASM_TEST", "--", "python3", "-c", "pass"])

    assert rc == 0
    assert captured["timeout"] == 123.5
    assert captured["command"] == ["python3", "-c", "pass"]


def test_guarded_exec_cli_timeout_overrides_family_env(monkeypatch) -> None:
    module = _load_guarded_exec()
    captured = _install_fake_context(module, monkeypatch)
    monkeypatch.setenv("MOLT_WASM_TEST_TIMEOUT_SEC", "123.5")

    rc = module.main(
        [
            "--prefix",
            "MOLT_WASM_TEST",
            "--timeout",
            "7",
            "--",
            "python3",
            "-c",
            "pass",
        ]
    )

    assert rc == 0
    assert captured["timeout"] == 7


def test_guarded_exec_timeout_env_remains_fallback(monkeypatch) -> None:
    module = _load_guarded_exec()
    captured = _install_fake_context(module, monkeypatch)
    monkeypatch.delenv("MOLT_WASM_TEST_TIMEOUT_SEC", raising=False)
    monkeypatch.delenv("MOLT_TEST_PROCESS_TIMEOUT_SEC", raising=False)
    monkeypatch.setenv("CUSTOM_TIMEOUT_SEC", "88")

    rc = module.main(
        [
            "--prefix",
            "MOLT_WASM_TEST",
            "--timeout-env",
            "CUSTOM_TIMEOUT_SEC",
            "--",
            "python3",
            "-c",
            "pass",
        ]
    )

    assert rc == 0
    assert captured["timeout"] == 88


def test_guarded_exec_family_timeout_can_disable_fallback(monkeypatch) -> None:
    module = _load_guarded_exec()
    captured = _install_fake_context(module, monkeypatch)
    monkeypatch.setenv("MOLT_WASM_TEST_TIMEOUT_SEC", "0")
    monkeypatch.setenv("CUSTOM_TIMEOUT_SEC", "88")

    rc = module.main(
        [
            "--prefix",
            "MOLT_WASM_TEST",
            "--timeout-env",
            "CUSTOM_TIMEOUT_SEC",
            "--",
            "python3",
            "-c",
            "pass",
        ]
    )

    assert rc == 0
    assert captured["timeout"] is None


def test_guarded_exec_preflights_backend_llvm_toolchain(monkeypatch, capsys) -> None:
    module = _load_guarded_exec()
    captured = _install_fake_context(module, monkeypatch)
    monkeypatch.setattr(
        module,
        "_llvm_backend_unavailable_message",
        lambda _root: "LLVM backend requires LLVM 22.1 with llvm-config.",
        raising=True,
    )

    rc = module.main(
        [
            "--prefix",
            "MOLT_TEST_SUITE",
            "--",
            "cargo",
            "test",
            "-p",
            "molt-backend",
            "--features",
            "native-backend llvm",
            "--lib",
        ]
    )

    assert rc == 2
    assert "command" not in captured
    err = capsys.readouterr().err
    assert "guarded_exec preflight" in err
    assert "LLVM backend requires LLVM 22.1" in err


def test_guarded_exec_does_not_preflight_tir_all_features(monkeypatch) -> None:
    module = _load_guarded_exec()
    captured = _install_fake_context(module, monkeypatch)
    monkeypatch.setattr(
        module,
        "_llvm_backend_unavailable_message",
        lambda _root: "should not be queried",
        raising=True,
    )

    rc = module.main(
        [
            "--prefix",
            "MOLT_TEST_SUITE",
            "--",
            "cargo",
            "clippy",
            "-p",
            "molt-tir",
            "--all-features",
        ]
    )

    assert rc == 0
    assert captured["command"] == [
        "cargo",
        "clippy",
        "-p",
        "molt-tir",
        "--all-features",
    ]
