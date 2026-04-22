from __future__ import annotations

import os
import subprocess
import sys
import textwrap
from pathlib import Path

import pytest

from tests.wasm_linked_runner import (
    _read_timeout_seconds,
    build_wasm_linked,
    require_wasm_toolchain,
    run_wasm_linked,
)
from tests.test_wasm_split_runtime import _run_split_worker_live


def _build_file_wasm(
    root: Path,
    source_file: Path,
    out_dir: Path,
    *,
    split_runtime: bool,
) -> subprocess.CompletedProcess[str]:
    env = os.environ.copy()
    env["PYTHONPATH"] = str(root / "src")
    env["MOLT_BACKEND_DAEMON"] = "0"
    target_dir, diff_target_dir = _wasm_importlib_package_bootstrap_target_dirs(
        root, env
    )
    target_dir.mkdir(parents=True, exist_ok=True)
    diff_target_dir.mkdir(parents=True, exist_ok=True)
    env["CARGO_TARGET_DIR"] = str(target_dir)
    env["MOLT_DIFF_CARGO_TARGET_DIR"] = str(diff_target_dir)
    env.setdefault(
        "MOLT_SESSION_ID",
        f"test-wasm-importlib-package-bootstrap-{source_file.parent.name}",
    )
    env.setdefault("CARGO_BUILD_JOBS", "1")
    env.setdefault("MOLT_BUILD_LOCK_TIMEOUT", "45")
    env.setdefault("MOLT_CARGO_TIMEOUT", "900")
    env.setdefault("MOLT_WASM_DISABLE_SCCACHE", "1")
    env.setdefault("MOLT_MIDEND_MAX_ROUNDS", "2")
    env.setdefault("MOLT_CSE_MAX_ITERS", "6")
    env.setdefault("MOLT_MIDEND_IDEMPOTENCE_CHECK", "0")
    env.setdefault("MOLT_MIDEND_FAIL_OPEN", "1")
    env.setdefault("MOLT_MIDEND_DISABLE", "1")
    tmp_root = root / "tmp"
    tmp_root.mkdir(parents=True, exist_ok=True)
    env["TMPDIR"] = str(tmp_root)
    env["MOLT_HOME"] = str(root)
    env["MOLT_CACHE"] = str(root / ".molt_cache")
    env["MOLT_WASM_RUNTIME_DIR"] = str(root / "wasm")
    env["MOLT_EXT_ROOT"] = str(root)

    cmd = [
        sys.executable,
        "-m",
        "molt.cli",
        "build",
        str(source_file),
        "--build-profile",
        "dev",
        "--target",
        "wasm",
        "--no-cache",
        "--out-dir",
        str(out_dir),
    ]
    if split_runtime:
        cmd.append("--split-runtime")
    else:
        cmd.append("--require-linked")

    build_timeout = _read_timeout_seconds("MOLT_WASM_TEST_BUILD_TIMEOUT_SEC", 900.0)
    try:
        return subprocess.run(
            cmd,
            cwd=root,
            env=env,
            capture_output=True,
            text=True,
            timeout=build_timeout,
        )
    except subprocess.TimeoutExpired as exc:
        stderr = exc.stderr or ""
        stdout = exc.stdout or ""
        raise AssertionError(
            "WASM module build timed out "
            f"after {build_timeout:.1f}s for {source_file}; "
            f"CARGO_TARGET_DIR={env['CARGO_TARGET_DIR']}\n"
            f"stdout:\n{stdout}\n\nstderr:\n{stderr}"
        ) from exc


def _wasm_importlib_package_bootstrap_target_dirs(
    root: Path,
    env: dict[str, str],
) -> tuple[Path, Path]:
    default_target_dir = (
        root / "target" / "pytest" / "test_wasm_importlib_package_bootstrap"
    )
    raw_target = env.get("CARGO_TARGET_DIR", "").strip()
    target_dir = Path(raw_target).expanduser() if raw_target else default_target_dir
    raw_diff_target = env.get("MOLT_DIFF_CARGO_TARGET_DIR", "").strip()
    diff_target_dir = (
        Path(raw_diff_target).expanduser() if raw_diff_target else target_dir
    )
    return target_dir, diff_target_dir


def test_wasm_importlib_package_bootstrap_target_dir_respects_explicit_env_override() -> (
    None
):
    env = {
        "CARGO_TARGET_DIR": "/tmp/molt-package-target",
        "MOLT_DIFF_CARGO_TARGET_DIR": "/tmp/molt-package-diff-target",
    }

    target_dir, diff_target_dir = _wasm_importlib_package_bootstrap_target_dirs(
        Path("/repo"), env
    )

    assert target_dir == Path("/tmp/molt-package-target")
    assert diff_target_dir == Path("/tmp/molt-package-diff-target")


def test_wasm_importlib_package_bootstrap_target_dir_defaults_to_repo_pytest_target() -> (
    None
):
    root = Path("/repo")

    target_dir, diff_target_dir = _wasm_importlib_package_bootstrap_target_dirs(
        root, {}
    )

    assert (
        target_dir
        == root / "target" / "pytest" / "test_wasm_importlib_package_bootstrap"
    )
    assert diff_target_dir == target_dir


def _write_probe_package(root: Path) -> Path:
    pkg = root / "probe_pkg"
    (pkg / "subpkg").mkdir(parents=True)
    (pkg / "__init__.py").write_text("PACKAGE = 'probe_pkg'\n", encoding="utf-8")
    (pkg / "subpkg" / "__init__.py").write_text("", encoding="utf-8")
    (pkg / "subpkg" / "leaf.py").write_text(
        textwrap.dedent(
            """\
            def leaf_value():
                return "leaf"
            """
        ),
        encoding="utf-8",
    )
    (pkg / "sibling.py").write_text(
        textwrap.dedent(
            """\
            from .subpkg.leaf import leaf_value


            def sibling_value():
                return f"sibling:{leaf_value()}"
            """
        ),
        encoding="utf-8",
    )
    (pkg / "__main__.py").write_text(
        textwrap.dedent(
            """\
            import os
            import sys

            from . import sibling
            from .subpkg.leaf import leaf_value

            print(__package__)
            print(sibling.sibling_value())
            print(leaf_value())
            print(sys.modules["os"] is os)
            print(sys.modules["sys"] is sys)
            """
        ),
        encoding="utf-8",
    )
    return pkg / "__main__.py"


@pytest.mark.slow
def test_wasm_module_entrypoint_package_relative_imports_linked(
    tmp_path: Path,
) -> None:
    require_wasm_toolchain()
    root = Path(__file__).resolve().parents[1]
    source_file = _write_probe_package(tmp_path)

    out_dir = tmp_path / "linked_out"
    out_dir.mkdir()
    output_wasm = build_wasm_linked(root, source_file, out_dir)

    run = run_wasm_linked(root, output_wasm)
    assert run.returncode == 0, run.stderr
    assert run.stdout.splitlines() == [
        "probe_pkg",
        "sibling:leaf",
        "leaf",
        "True",
        "True",
    ]


@pytest.mark.slow
def test_wasm_module_entrypoint_package_relative_imports_split_direct(
    tmp_path: Path,
) -> None:
    require_wasm_toolchain()
    root = Path(__file__).resolve().parents[1]
    source_file = _write_probe_package(tmp_path)

    out_dir = tmp_path / "split_out"
    out_dir.mkdir()
    build = _build_file_wasm(root, source_file, out_dir, split_runtime=True)
    assert build.returncode == 0, (
        f"split module build failed (rc={build.returncode}).\n"
        f"stdout:\n{build.stdout[-2000:]}\n"
        f"stderr:\n{build.stderr[-2000:]}"
    )

    run = run_wasm_linked(
        root,
        out_dir / "app.wasm",
        env_overrides={
            "MOLT_WASM_DIRECT_LINK": "1",
            "MOLT_WASM_PREFER_LINKED": "0",
            "MOLT_RUNTIME_WASM": str(out_dir / "molt_runtime.wasm"),
        },
    )
    assert run.returncode == 0, run.stderr
    assert run.stdout.splitlines() == [
        "probe_pkg",
        "sibling:leaf",
        "leaf",
        "True",
        "True",
    ]


@pytest.mark.slow
def test_wasm_module_entrypoint_package_relative_imports_split_worker_host(
    tmp_path: Path,
) -> None:
    require_wasm_toolchain()
    root = Path(__file__).resolve().parents[1]
    source_file = _write_probe_package(tmp_path)

    out_dir = tmp_path / "worker_out"
    out_dir.mkdir()
    build = _build_file_wasm(root, source_file, out_dir, split_runtime=True)
    assert build.returncode == 0, (
        f"linked module build failed (rc={build.returncode}).\n"
        f"stdout:\n{build.stdout[-2000:]}\n"
        f"stderr:\n{build.stderr[-2000:]}"
    )

    status, body, logs = _run_split_worker_live(out_dir, "/")
    assert status == 200, (
        f"split-runtime worker returned HTTP {status}.\n"
        f"body:\n{body[-2000:]}\n"
        f"logs:\n{logs[-4000:]}"
    )
    assert body.splitlines() == [
        "probe_pkg",
        "sibling:leaf",
        "leaf",
        "True",
        "True",
    ]
