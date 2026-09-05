from __future__ import annotations

import json
import os
import sys
from pathlib import Path

import pytest

from molt.dx import development_artifact_env
from molt.target_python import TargetPythonVersion, require_verified_subset_target
from tests.native_process_guard import run_native_test_process
from tests.wasm_linked_runner import (
    build_wasm_linked,
    require_wasm_toolchain,
    run_wasm_linked,
)
from tools.compat.comparison import Outputs, compare_outputs


ROOT = Path(__file__).resolve().parents[1]
FIXTURE = ROOT / "tests" / "fixtures" / "pending_call_probe"
PROGRAM = FIXTURE / "finally_pending_observer.py"


def _cpython_oracle_env(env: dict[str, str]) -> dict[str, str]:
    # -I isolates Python imports, not setuptools/sysconfig or C compiler input.
    # Use this interpreter's native build configuration, never an inherited
    # Molt/WASM cross-build override. Preserve PATH and all guard/custody limits;
    # MSVC discovery owns constructing its own INCLUDE/LIB environment.
    build_overrides = {
        "CC",
        "CXX",
        "CPP",
        "CFLAGS",
        "CPPFLAGS",
        "CXXFLAGS",
        "LDFLAGS",
        "LDSHARED",
        "LDCXXSHARED",
        "BLDSHARED",
        "AR",
        "ARFLAGS",
        "RANLIB",
        "CPATH",
        "C_INCLUDE_PATH",
        "CPLUS_INCLUDE_PATH",
        "OBJC_INCLUDE_PATH",
        "INCLUDE",
        "LIB",
        "LIBPATH",
        "DISTUTILS_USE_SDK",
        "MSSDK",
        "SDKROOT",
        "ARCHFLAGS",
        "MACOSX_DEPLOYMENT_TARGET",
        "_PYTHON_SYSCONFIGDATA_NAME",
        "_PYTHON_SYSCONFIGDATA_PATH",
        "_PYTHON_HOST_PLATFORM",
        "_PYTHON_PROJECT_BASE",
        "PYTHONPATH",
        "PYTHONHOME",
        "PYTHONUSERBASE",
        "PYTHONSTARTUP",
    }
    return {
        key: value for key, value in env.items() if key.upper() not in build_overrides
    }


def _run_cpython_oracle(artifact_root: Path, env: dict[str, str]) -> Outputs:
    oracle_root = artifact_root / "cpython"
    oracle_env = _cpython_oracle_env(env)
    build = run_native_test_process(
        [
            sys.executable,
            "-I",
            str(FIXTURE / "build_cpython.py"),
            str(oracle_root),
        ],
        cwd=ROOT,
        env=oracle_env,
        timeout=900,
    )
    assert build.returncode == 0, build.stderr
    # Isolated CPython loads only the extension just built against its own
    # headers. Neither Molt's stdlib nor a globally installed probe is an oracle.
    oracle = run_native_test_process(
        [
            sys.executable,
            "-I",
            "-c",
            "import runpy, sys; sys.path.insert(0, sys.argv[1]); "
            "runpy.run_path(sys.argv[2], run_name='__main__')",
            str(oracle_root / "lib"),
            str(PROGRAM),
        ],
        cwd=ROOT,
        env=oracle_env,
        timeout=60,
    )
    assert oracle.returncode == 0, oracle.stderr
    return Outputs(oracle.stdout, oracle.stderr, oracle.returncode)


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
    target_python: TargetPythonVersion,
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
            "--python-version",
            target_python.short,
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
    if sys.implementation.name != "cpython":
        pytest.skip("the pending-call oracle requires CPython")
    target_python = require_verified_subset_target(
        TargetPythonVersion(*sys.version_info[:3])
    )
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
    oracle = _run_cpython_oracle(artifact_root, env)
    for key, value in env.items():
        monkeypatch.setenv(key, value)

    _build_extension(artifact_root, env, target="native", target_python=target_python)
    _build_extension(artifact_root, env, target="wasm", target_python=target_python)

    native_path = artifact_root / ("observer.exe" if os.name == "nt" else "observer")
    native_build = run_native_test_process(
        [
            sys.executable,
            "-m",
            "molt.cli",
            "build",
            "--build-profile",
            "dev",
            "--python-version",
            target_python.short,
            "--output",
            str(native_path),
            str(PROGRAM),
        ],
        cwd=ROOT,
        env=env,
        timeout=900,
    )
    assert native_build.returncode == 0, native_build.stderr
    native = run_native_test_process([str(native_path)], cwd=ROOT, env=env, timeout=60)
    assert native.returncode == 0, native.stderr

    wasm_path = build_wasm_linked(
        ROOT,
        PROGRAM,
        tmp_path / "wasm",
        extra_args=["--python-version", target_python.short],
    )
    wasm = run_wasm_linked(ROOT, wasm_path)
    assert wasm.returncode == 0, wasm.stderr

    for backend, result in (("native", native), ("wasm", wasm)):
        actual = Outputs(result.stdout, result.stderr, result.returncode)
        verdict = compare_outputs(oracle, actual, stderr_mode="exception")
        assert verdict.equal, (
            f"{backend} vs CPython {target_python.short}: {verdict.detail}\n"
            f"oracle={oracle!r}\nactual={actual!r}"
        )
