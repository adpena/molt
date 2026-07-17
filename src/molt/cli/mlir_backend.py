from __future__ import annotations

import json
import os
from pathlib import Path
import shlex
import shutil
import subprocess
import sys
from typing import Any

from molt.cli.cache_keys import _json_ir_default
from molt.cli.command_runtime import _run_subprocess_captured_to_tempfiles
from molt.cli.output import emit_json as _emit_json
from molt.cli.output import fail as _fail
from molt.cli.output import json_payload as _json_payload
from molt.cli.runtime_paths import _molt_session_id
from molt.llvm_toolchain import LlvmToolchainConfigError, mlir_toolchain_environment


def _mlir_backend_executable_name(*, os_name: str | None = None) -> str:
    return "molt-backend-mlir.exe" if (os_name or os.name) == "nt" else "molt-backend-mlir"


def _find_mlir_backend_binary(project_root: Path) -> Path | None:
    """Locate the ``molt-backend-mlir`` binary."""
    mlir_crate_dir = project_root / "runtime" / "molt-backend-mlir"
    executable_name = _mlir_backend_executable_name()
    for profile in ("release", "debug"):
        candidate = mlir_crate_dir / "target" / profile / executable_name
        if candidate.is_file():
            return candidate
    session_id = _molt_session_id()
    target_dirs = []
    if session_id:
        target_dirs.append(project_root / f"target-{session_id}")
    target_dirs.append(project_root / "target")
    for tdir in target_dirs:
        for profile in ("release", "release-fast", "debug"):
            candidate = tdir / profile / executable_name
            if candidate.is_file():
                return candidate
    from_path = shutil.which("molt-backend-mlir")
    if from_path is not None:
        return Path(from_path)
    return None


def _ensure_mlir_backend_binary(project_root: Path) -> tuple[Path | None, str | None]:
    """Resolve or build the standalone MLIR backend with one toolchain family."""

    existing = _find_mlir_backend_binary(project_root)
    if existing is not None:
        return existing, None

    manifest = project_root / "runtime" / "molt-backend-mlir" / "Cargo.toml"
    if not manifest.is_file():
        return None, (
            "MLIR backend binary is absent and this installation does not contain "
            "the source manifest; reinstall a distribution that includes the MLIR backend"
        )
    cargo = shutil.which("cargo")
    if cargo is None:
        return None, "MLIR backend is not built and cargo is unavailable on PATH"
    try:
        env = mlir_toolchain_environment(project_root)
    except LlvmToolchainConfigError as exc:
        return None, str(exc)

    command = [
        cargo,
        "build",
        "--locked",
        "--release",
        "--manifest-path",
        str(manifest),
    ]
    try:
        result = _run_subprocess_captured_to_tempfiles(
            command,
            cwd=project_root,
            env=env,
            timeout=1800,
            progress_label="MLIR backend build",
        )
    except (OSError, subprocess.TimeoutExpired) as exc:
        return None, f"MLIR backend build could not complete: {exc}"
    if result.returncode != 0:
        stderr = result.stderr.decode("utf-8", errors="replace").strip()
        detail = stderr[-4000:] if stderr else "cargo returned no diagnostic"
        return None, f"MLIR backend build failed:\n{detail}"

    built = _find_mlir_backend_binary(project_root)
    if built is None:
        return None, "MLIR backend build succeeded but published no discoverable binary"
    return built, None


def _run_mlir_backend_pipeline(
    *,
    ir: dict[str, Any],
    output_artifact: Path,
    project_root: Path,
    json_output: bool,
    verbose: bool,
    emit_llvm: bool = False,
) -> int:
    """Run the standalone MLIR backend binary and write the emitted artifact."""
    mlir_bin, build_error = _ensure_mlir_backend_binary(project_root)
    if mlir_bin is None:
        msg = (
            "Error: MLIR backend binary not found.\n"
            "\n"
            "The MLIR backend requires Molt's complete LLVM/MLIR toolchain:\n"
            "\n"
            "  python -m tools.bootstrap_llvm\n"
            "\n"
            "Molt builds the standalone backend on first use."
        )
        if build_error:
            msg += f"\n\n{build_error}"
        if json_output:
            return _fail(msg, json_output, command="build")
        print(msg, file=sys.stderr)
        return 1

    try:
        mlir_env = mlir_toolchain_environment(
            project_root,
            environ=os.environ.copy(),
            require=False,
        )
    except LlvmToolchainConfigError as exc:
        return _fail(str(exc), json_output, command="build")

    cmd: list[str] = [str(mlir_bin), "--output", str(output_artifact)]
    if emit_llvm:
        cmd.append("--emit-llvm")

    ir_bytes = json.dumps(ir, separators=(",", ":"), default=_json_ir_default).encode(
        "utf-8"
    )

    if verbose and not json_output:
        print(f"MLIR backend: {shlex.join(cmd)}", file=sys.stderr)
        print(
            f"  IR size: {len(ir_bytes)} bytes, "
            f"functions: {len(ir.get('functions', []))}",
            file=sys.stderr,
        )

    try:
        result = _run_subprocess_captured_to_tempfiles(
            cmd,
            input=ir_bytes,
            cwd=project_root,
            env=mlir_env,
            timeout=120,
            progress_label="MLIR backend",
        )
    except FileNotFoundError:
        return _fail(
            f"MLIR backend binary not executable: {mlir_bin}",
            json_output,
            command="build",
        )
    except subprocess.TimeoutExpired:
        return _fail(
            "MLIR backend timed out after 120 seconds",
            json_output,
            command="build",
        )

    stderr_text = result.stderr.decode("utf-8", errors="replace").strip()
    if stderr_text and (verbose or result.returncode != 0):
        print(stderr_text, file=sys.stderr)

    if result.returncode != 0:
        return _fail(
            f"MLIR backend failed (exit {result.returncode})",
            json_output,
            command="build",
        )

    if json_output:
        data: dict[str, Any] = {
            "target": "mlir",
            "output": str(output_artifact),
            "consumer_output": str(output_artifact),
            "artifacts": {"mlir": str(output_artifact)},
        }
        payload = _json_payload("build", "ok", data=data)
        _emit_json(payload, json_output)
    else:
        print(f"Wrote MLIR output: {output_artifact}", file=sys.stderr)

    return 0
