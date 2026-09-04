from __future__ import annotations

import json
import os
import sys
from pathlib import Path

import pytest

from molt.dx import development_artifact_env
from tests.native_process_guard import run_native_test_process
from tests.wasm_linked_runner import (
    build_wasm_linked,
    require_wasm_toolchain,
    run_wasm_linked,
)


ROOT = Path(__file__).resolve().parents[1]
FIXTURE = ROOT / "tests" / "fixtures" / "pending_call_probe"
PROGRAM = FIXTURE / "finally_pending_observer.py"
EXPECTED_OUTPUT = "\n".join(
    (
        "plain TypeError plain replacement ValueError plain original",
        "marked RuntimeError pending replacement LookupError marked original",
    )
)


def _test_env(artifact_root: Path) -> dict[str, str]:
    base = dict(os.environ)
    base.pop("MOLT_SESSION_ID", None)
    base.pop("MOLT_SESSION_ID_GENERATED", None)
    # Extension discovery and global build custody are separate authorities.
    # Keeping MOLT_EXT_ROOT at the canonical DX root prevents nested extension
    # paths from becoming Cargo/runtime staging roots on Windows.
    base.pop("MOLT_EXT_ROOT", None)
    env = development_artifact_env(
        ROOT,
        base,
        session_prefix="finally-pending-observer",
        create_dirs=True,
    )
    env["PYTHONPATH"] = str(ROOT / "src")
    env["MOLT_MODULE_ROOTS"] = str(artifact_root)
    env["MOLT_EXTERNAL_STATIC_PACKAGES"] = "pending_call_probe"
    env["MOLT_HERMETIC_MODULE_ROOTS"] = "1"
    env.setdefault("MOLT_BACKEND_DAEMON", "0")
    env.setdefault("MOLT_BUILD_LOCK_TIMEOUT", "45")
    env.setdefault("MOLT_CARGO_TIMEOUT", "900")
    return env


def _build_extension(
    artifact_root: Path,
    env: dict[str, str],
    *,
    target: str,
) -> None:
    build = run_native_test_process(
        [
            sys.executable,
            "-m",
            "molt.cli",
            "extension",
            "build",
            "--project",
            str(FIXTURE),
            "--out-dir",
            str(artifact_root),
            "--target",
            target,
            "--deterministic",
        ],
        cwd=ROOT,
        env=env,
        timeout=900,
    )
    assert build.returncode == 0, build.stderr

    suffix = ".molt.wasm" if target == "wasm" else ".molt.a"
    artifact = artifact_root / "pending_call_probe" / f"_native{suffix}"
    assert artifact.is_file(), f"extension artifact missing: {artifact}"
    manifest_path = artifact.with_name(f"{artifact.name}.extension_manifest.json")
    manifest = json.loads(manifest_path.read_text(encoding="utf-8"))
    assert manifest["extension"] == artifact.name
    assert manifest["abi_tier"] == "cpython-abi"
    assert manifest["runtime_linkage"] == "static_link"
    assert manifest["capabilities"] == []
    assert manifest["effects"] == ["write"]
    assert manifest["determinism"] == "deterministic"
    assert manifest["python_exports"] == [
        "pending_call_probe._native.arm_runtime_error"
    ]
    assert manifest["callable_exports"] == [
        {
            "module": "pending_call_probe._native",
            "name": "arm_runtime_error",
            "binding": "module_attr",
            "abi": "molt.object_callargs_v1",
            "effects": ["write"],
            "deterministic": True,
        }
    ]


@pytest.mark.slow
def test_finally_pending_observer_native_wasm_parity(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    require_wasm_toolchain()

    custody_env = development_artifact_env(ROOT, os.environ)
    artifact_root = (
        Path(custody_env["MOLT_EXT_ROOT"])
        / "tmp"
        / "finally-pending-observer"
        / f"{os.getpid()}-{tmp_path.name}"
    )
    artifact_root.mkdir(parents=True, exist_ok=True)
    env = _test_env(artifact_root)
    for key, value in env.items():
        monkeypatch.setenv(key, value)

    _build_extension(artifact_root, env, target="native")
    _build_extension(artifact_root, env, target="wasm")

    native = run_native_test_process(
        [
            sys.executable,
            "-m",
            "molt.cli",
            "run",
            "--profile",
            "dev",
            str(PROGRAM),
        ],
        cwd=ROOT,
        env=env,
        timeout=900,
    )
    assert native.returncode == 0, native.stderr

    wasm_path = build_wasm_linked(ROOT, PROGRAM, tmp_path / "wasm")
    wasm = run_wasm_linked(ROOT, wasm_path)
    assert wasm.returncode == 0, wasm.stderr

    native_output = native.stdout.strip()
    wasm_output = wasm.stdout.strip()
    assert native_output == EXPECTED_OUTPUT
    assert wasm_output == EXPECTED_OUTPUT
    assert native_output == wasm_output
