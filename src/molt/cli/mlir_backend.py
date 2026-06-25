from __future__ import annotations

import json
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


def _find_mlir_backend_binary(project_root: Path) -> Path | None:
    """Locate the ``molt-backend-mlir`` binary."""
    mlir_crate_dir = project_root / "runtime" / "molt-backend-mlir"
    for profile in ("release", "debug"):
        candidate = mlir_crate_dir / "target" / profile / "molt-backend-mlir"
        if candidate.is_file():
            return candidate
    session_id = _molt_session_id()
    target_dirs = []
    if session_id:
        target_dirs.append(project_root / f"target-{session_id}")
    target_dirs.append(project_root / "target")
    for tdir in target_dirs:
        for profile in ("release", "release-fast", "debug"):
            candidate = tdir / profile / "molt-backend-mlir"
            if candidate.is_file():
                return candidate
    from_path = shutil.which("molt-backend-mlir")
    if from_path is not None:
        return Path(from_path)
    return None


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
    mlir_bin = _find_mlir_backend_binary(project_root)
    if mlir_bin is None:
        msg = (
            "Error: MLIR backend binary not found.\n"
            "\n"
            "The MLIR backend requires LLVM 22 and is built separately:\n"
            "\n"
            "  1. Install LLVM:  brew install llvm\n"
            "  2. Build the MLIR backend:\n"
            "     cargo build --release -p molt-backend-mlir\n"
            "\n"
            "Then retry: molt build --target mlir <file>"
        )
        if json_output:
            return _fail(msg, json_output, command="build")
        print(msg, file=sys.stderr)
        return 1

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
            env=None,
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
