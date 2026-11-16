"""Guarded external-command and linker symbol-probe authority."""

from __future__ import annotations


from collections.abc import Mapping, Sequence
import contextlib
import os
from pathlib import Path
import subprocess
import tempfile

from tools.command_execution import CommandExecutor


_COMMANDS = CommandExecutor.for_file(__file__, stem="MOLT_WASM_LINK")


def _run_external_tool(
    cmd: Sequence[str],
    *,
    capture_output: bool = True,
    text: bool = True,
    timeout: float | None = None,
    cwd: str | Path | None = None,
    env: Mapping[str, str] | None = None,
) -> subprocess.CompletedProcess[str]:
    """Run typed argv under repository process custody.

    Windows imposes a short command-line limit even when the linker itself can
    consume an arbitrary number of inputs. Materialize only oversized
    ``wasm-ld`` invocations as response files; every other command preserves its
    exact argv for the shared executor.
    """

    guarded_cmd = list(cmd)
    response_path: Path | None = None
    if (
        os.name == "nt"
        and Path(guarded_cmd[0]).stem.casefold() == "wasm-ld"
        and len(subprocess.list2cmdline(guarded_cmd)) >= 30_000
    ):
        handle = tempfile.NamedTemporaryFile(
            mode="w",
            encoding="utf-8",
            suffix=".rsp",
            prefix="molt-wasm-ld-",
            delete=False,
        )
        try:
            response_path = Path(handle.name)
            handle.write(
                "\n".join(
                    subprocess.list2cmdline([argument]) for argument in guarded_cmd[1:]
                )
            )
            handle.write("\n")
        finally:
            handle.close()
        guarded_cmd = [guarded_cmd[0], f"@{response_path}"]
    try:
        return _COMMANDS.run(
            guarded_cmd,
            cwd=cwd,
            env=env,
            capture_output=capture_output,
            text=text,
            timeout=timeout,
        )
    finally:
        if response_path is not None:
            with contextlib.suppress(OSError):
                response_path.unlink()
