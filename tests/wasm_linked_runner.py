from __future__ import annotations

from molt.process_environment import ProcessEnvironmentBuilder

import os
import shutil
import subprocess
import sys
import tempfile
from collections.abc import Mapping, Sequence
from dataclasses import dataclass
from pathlib import Path

import pytest

from molt.capability_manifest import resolve_runtime_policy_from_env
from molt.cli import wasm_toolchain
from molt.cli.wasm_host import resolve_molt_wasm_host_binary
from molt.dx import (
    ARTIFACT_ROOT_ENV,
    artifact_root,
    configured_artifact_root,
    development_artifact_env,
    generated_session_id,
    project_cargo_target_dir,
    scratch_root,
)
from molt.node_runtime import NodeRuntimeError, resolve_node_runtime
from molt.wasm_artifact import wasm_runtime_manifest_path
from tests import process_guard_common


def _wasm_test_lane() -> str:
    configured = os.environ.get("MOLT_WASM_TEST_LANE", "").strip()
    if configured:
        return configured
    worker = os.environ.get("PYTEST_XDIST_WORKER", "").strip()
    if worker:
        return f"worker_{worker}"
    return "local"


def _linked_test_artifact_root(root: Path, out_dir: Path) -> Path:
    configured = configured_artifact_root(os.environ, relative_to=root)
    if configured is not None:
        return configured
    selected = artifact_root(root)
    if not _same_location(selected, root):
        return selected
    # A checkout that is its own artifact root keeps linked-WASM runtime and
    # cache state beside the test outputs instead of the checkout's wasm root.
    return out_dir.parent


def _same_location(lhs: Path, rhs: Path) -> bool:
    try:
        return lhs.samefile(rhs)
    except OSError:
        pass
    return os.path.normcase(os.fspath(lhs.resolve())) == os.path.normcase(
        os.fspath(rhs.resolve())
    )


def _select_out_dir(default: Path, root: Path) -> Path:
    selected = artifact_root(root)
    use_external = os.environ.get("MOLT_WASM_TEST_USE_EXTERNAL", "").strip().lower()
    allow_external = use_external not in {"0", "false", "no", "off"}
    if allow_external and not _same_location(selected, root):
        if default.is_relative_to(selected):
            return default
        base = scratch_root(root)
        try:
            base.mkdir(parents=True, exist_ok=True)
            return Path(tempfile.mkdtemp(prefix="molt_wasm_", dir=base))
        except OSError:
            pass
    if default.is_relative_to(root):
        return default
    base = root / "build" / "wasm"
    base.mkdir(parents=True, exist_ok=True)
    return Path(tempfile.mkdtemp(prefix="molt_wasm_", dir=base))


def _run_wasm_test_process(
    args: Sequence[str],
    *,
    cwd: Path,
    env: Mapping[str, str] | None = None,
    timeout: float | None = None,
    capture_output: bool = True,
    text: bool = True,
    check: bool = False,
    input: str | bytes | None = None,
) -> subprocess.CompletedProcess[str]:
    # Direct Node and Rust hosts consume exact runtime grants; named profiles
    # belong to the same policy projection used by the CLI embedding owner.
    process_env = ProcessEnvironmentBuilder(os.environ if env is None else env)
    process_env.update(resolve_runtime_policy_from_env(process_env).to_env_vars())
    return process_guard_common.run_guarded_test_process(
        list(args),
        prefix="MOLT_WASM_TEST",
        cwd=cwd,
        env=process_env,
        capture_output=capture_output,
        text=text,
        timeout=timeout,
        check=check,
        input=input,
    )


def require_node_binary() -> str:
    """Return the shared Node selection, skipping only when Node is optional."""
    root = Path(__file__).resolve().parents[1]
    try:
        node = resolve_node_runtime(source_root=root, guard_prefix="MOLT_WASM_TEST")
    except NodeRuntimeError as exc:
        if os.environ.get("MOLT_NODE_BIN", "").strip():
            pytest.fail(f"explicit MOLT_NODE_BIN is unusable: {exc}")
        pytest.skip(f"Node is required for explicit JS-runner coverage: {exc}")
    return str(node.path)


def wasm_test_cargo_target_dir(
    root: Path, env: Mapping[str, str] | None = None
) -> Path:
    """The Cargo target WASM tests build into and find molt-wasm-host in.

    It is the checkout's persistent target, `molt.dx.project_cargo_target_dir`
    without session scope: an explicit ``CARGO_TARGET_DIR`` wins (the pytest
    run context, a CI job, a queued proof's leased generation), else the
    checkout's own target under the artifact root. Each build pins its own
    session, so a session-scoped target would start cold in every test, and a
    target shared by checkouts would rebuild whichever sources built last.
    xdist workers of one checkout share it: Cargo's build lock and the build
    locks addressed by the target serialize them.
    """
    view = os.environ if env is None else env
    return project_cargo_target_dir(root, view, session_scoped=False)


def wasm_ld_unavailable_reason() -> str | None:
    """Why the WASM toolchain authority selects no wasm-ld, or None."""
    try:
        wasm_toolchain.resolve_wasm_linker()
    except wasm_toolchain.WasmLinkerContractError as exc:
        return str(exc)
    return None


def selected_wasm_ld() -> Path:
    """The wasm-ld the WASM toolchain authority selects, or a precise skip."""
    try:
        return wasm_toolchain.resolve_wasm_linker().path
    except wasm_toolchain.WasmLinkerContractError as exc:
        pytest.skip(f"wasm-ld is required: {exc}")


def require_wasm_build_toolchain() -> None:
    if shutil.which("cargo") is None:
        pytest.skip("cargo is required for wasm parity test")
    selected_wasm_ld()


def require_wasm_toolchain() -> None:
    root = Path(__file__).resolve().parents[1]
    host_bin = resolve_molt_wasm_host_binary(
        root, cargo_profile="dev-fast", target_dir=wasm_test_cargo_target_dir(root)
    )
    if host_bin is None:
        pytest.skip(
            "prebuilt molt-wasm-host dev-fast binary is required for linked wasm "
            "parity test (set MOLT_WASM_HOST_BIN or CARGO_TARGET_DIR)."
        )
    require_wasm_build_toolchain()


def wasm_test_build_env(
    root: Path,
    *,
    base_env: Mapping[str, str] | None = None,
    linked: bool | None = None,
    session_prefix: str = "test-wasm",
    session_id: str | None = None,
    create_dirs: bool = True,
) -> ProcessEnvironmentBuilder:
    """Return shared WASM test policy without overriding Cargo/DX authority.

    The pinned build session scopes scratch, never the Cargo target: the
    target is `wasm_test_cargo_target_dir`, so builds stay warm across tests.
    """
    base = ProcessEnvironmentBuilder(os.environ if base_env is None else base_env)
    base["CARGO_TARGET_DIR"] = str(wasm_test_cargo_target_dir(root, base))
    inherited_session_id = base.get("MOLT_SESSION_ID", "").strip()
    if generated_session_id(base):
        inherited_session_id = ""
    resolved_session_id = (
        session_id or inherited_session_id or f"{session_prefix}-{_wasm_test_lane()}"
    )
    env = ProcessEnvironmentBuilder(
        development_artifact_env(
            root,
            base,
            session_prefix=session_prefix,
            session_id=resolved_session_id,
            create_dirs=create_dirs,
        )
    )
    if linked is not None:
        env["MOLT_WASM_LINKED"] = "1" if linked else "0"
    # Build-lock waiters keep the lock's default budget, one bounded cold
    # build: xdist workers share the checkout's target, and a shorter budget
    # fails the waiter behind a legitimate cold backend build.
    env.setdefault("MOLT_CARGO_TIMEOUT", "900")
    env.setdefault("MOLT_BACKEND_DAEMON", "0")
    return env


@dataclass(frozen=True)
class WasmTestBuildCommand:
    argv: tuple[str, ...]
    cwd: Path
    environment: Mapping[str, str]
    output: Path | None


def wasm_linked_build_command(
    root: Path,
    src: Path,
    out_dir: Path,
    *,
    extra_args: list[str] | None = None,
) -> WasmTestBuildCommand:
    out_dir = _select_out_dir(out_dir, root)
    test_artifact_root = _linked_test_artifact_root(root, out_dir)
    env = wasm_test_build_env(
        root,
        linked=True,
        session_id=f"test-wasm-{_wasm_test_lane()}-{src.stem}-{out_dir.name}",
    )
    env.setdefault("MOLT_MIDEND_MAX_ROUNDS", "2")
    env.setdefault("MOLT_CSE_MAX_ITERS", "6")
    env[ARTIFACT_ROOT_ENV] = str(test_artifact_root)
    tmp_root = scratch_root(root, env)
    tmp_root.mkdir(parents=True, exist_ok=True)
    env["TMPDIR"] = str(tmp_root)
    env["MOLT_HOME"] = str(test_artifact_root)
    env["MOLT_CACHE"] = str(test_artifact_root / ".molt_cache")
    env["MOLT_WASM_RUNTIME_DIR"] = str(test_artifact_root / "wasm")
    args = [
        sys.executable,
        "-m",
        "molt.cli",
        "build",
        str(src),
        "--build-profile",
        "dev",
        "--target",
        "wasm",
        "--require-linked",
        "--out-dir",
        str(out_dir),
    ]
    if extra_args:
        args.extend(extra_args)
    return WasmTestBuildCommand(tuple(args), root, env, out_dir / "output_linked.wasm")


def build_wasm_linked(
    root: Path,
    src: Path,
    out_dir: Path,
    *,
    extra_args: list[str] | None = None,
) -> Path:
    command = wasm_linked_build_command(root, src, out_dir, extra_args=extra_args)
    try:
        build = _run_wasm_test_process(
            list(command.argv),
            cwd=command.cwd,
            env=command.environment,
        )
    except subprocess.TimeoutExpired as exc:
        stderr = exc.stderr or ""
        stdout = exc.stdout or ""
        raise AssertionError(
            "WASM build timed out "
            f"after {exc.timeout}s for {src}; "
            f"CARGO_TARGET_DIR={command.environment['CARGO_TARGET_DIR']}\n"
            f"stdout:\n{stdout}\n\nstderr:\n{stderr}"
        ) from exc
    assert build.returncode == 0, build.stderr
    output_wasm = command.output
    assert output_wasm is not None and output_wasm.exists(), (
        "linked wasm output missing"
    )
    return output_wasm


def run_wasm_linked(
    root: Path, wasm_path: Path, *, env_overrides: dict[str, str] | None = None
) -> subprocess.CompletedProcess[str]:
    host_bin = resolve_molt_wasm_host_binary(
        root, cargo_profile="dev-fast", target_dir=wasm_test_cargo_target_dir(root)
    )
    if host_bin is None:
        raise AssertionError(
            "prebuilt molt-wasm-host dev-fast binary is required for linked wasm "
            "execution (set MOLT_WASM_HOST_BIN or CARGO_TARGET_DIR)."
        )
    env = os.environ.copy()
    for key in (
        "MOLT_WASM_TABLE_BASE",
        "MOLT_RUNTIME_WASM",
        "MOLT_WASM_MANIFEST_PATH",
    ):
        env.pop(key, None)
    if env_overrides:
        env.update(env_overrides)
    manifest = wasm_runtime_manifest_path(wasm_path).resolve()
    host_args = [host_bin, str(manifest)]
    try:
        return _run_wasm_test_process(
            host_args,
            # WASI preopens dot. Relative writes and tempfile's valid cwd
            # fallback belong to this case's existing product directory.
            cwd=manifest.parent,
            env=env,
        )
    except subprocess.TimeoutExpired as exc:
        stderr = exc.stderr or ""
        stdout = exc.stdout or ""
        raise AssertionError(
            "WASM execution timed out "
            f"after {exc.timeout}s for {wasm_path}\n"
            f"stdout:\n{stdout}\n\nstderr:\n{stderr}"
        ) from exc
