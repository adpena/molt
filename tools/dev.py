#!/usr/bin/env python3
from __future__ import annotations

import os
import platform
import shutil
import subprocess
import sys
import time
from datetime import datetime
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
TEST_PYTHONS = ["3.12", "3.13", "3.14"]


def _log(msg: str) -> None:
    stamp = datetime.now().isoformat(timespec="seconds")
    print(f"[dev.py {stamp}] {msg}")


def _run_with_pty(cmd: list[str], env: dict[str, str]) -> None:
    import os
    import pty

    master_fd, slave_fd = pty.openpty()
    try:
        proc = subprocess.Popen(
            cmd,
            cwd=ROOT,
            env=env,
            stdin=slave_fd,
            stdout=slave_fd,
            stderr=slave_fd,
        )
    finally:
        os.close(slave_fd)

    try:
        while True:
            data = os.read(master_fd, 1024)
            if not data:
                break
            if hasattr(sys.stdout, "buffer"):
                sys.stdout.buffer.write(data)
                sys.stdout.buffer.flush()
            else:
                sys.stdout.write(data.decode(errors="replace"))
                sys.stdout.flush()
    except KeyboardInterrupt:
        proc.terminate()
        raise
    finally:
        os.close(master_fd)

    rc = proc.wait()
    if rc != 0:
        raise subprocess.CalledProcessError(rc, cmd)


def run_uv(
    args: list[str],
    python: str | None = None,
    env: dict[str, str] | None = None,
    tty: bool = False,
) -> None:
    cmd = ["uv", "run"]
    if python:
        cmd.extend(["--python", python])
        if (
            python == "3.14"
            and sys.platform == "darwin"
            and platform.machine().lower() in {"arm64", "aarch64"}
            and shutil.which("python3.14")
        ):
            cmd.append("--no-managed-python")
    cmd.extend(args)
    run_env = (env or os.environ).copy()
    run_env.setdefault("PYTHONUNBUFFERED", "1")
    if tty and os.name == "posix":
        _run_with_pty(cmd, run_env)
    else:
        subprocess.check_call(cmd, cwd=ROOT, env=run_env)


def _apply_dev_trusted(env: dict[str, str]) -> None:
    raw = env.get("MOLT_DEV_TRUSTED", "").strip().lower()
    if raw and raw in {"0", "false", "no", "off"}:
        return
    env.setdefault("MOLT_TRUSTED", "1")


def main() -> None:
    cmd = sys.argv[1:] or ["help"]
    use_tty = "--tty" in cmd or os.environ.get("MOLT_TTY") == "1"
    if use_tty:
        cmd = [arg for arg in cmd if arg != "--tty"]
    if not cmd:
        cmd = ["help"]
    if cmd[0] == "lint":
        run_uv(["ruff", "check", "."], python=TEST_PYTHONS[0], tty=use_tty)
        run_uv(["ruff", "format", "--check", "."], python=TEST_PYTHONS[0], tty=use_tty)
        run_uv(["ty", "check", "src"], python=TEST_PYTHONS[0], tty=use_tty)
        run_uv(
            ["python3", "tools/verified_subset.py", "check"],
            python=TEST_PYTHONS[0],
            tty=use_tty,
        )
    elif cmd[0] == "test":
        env = os.environ.copy()
        _apply_dev_trusted(env)
        src_path = str(ROOT / "src")
        existing = env.get("PYTHONPATH", "")
        env["PYTHONPATH"] = (
            src_path if not existing else f"{src_path}{os.pathsep}{existing}"
        )
        _log(f"PYTHONPATH={env['PYTHONPATH']}")
        for python in TEST_PYTHONS:
            _log(f"tests start (python {python})")
            start = time.monotonic()
            batch_cmd = ["python3", "tools/dev_test_runner.py"]
            if python == TEST_PYTHONS[0]:
                batch_cmd.append("--verified-subset")
            run_uv(batch_cmd, python=python, env=env, tty=use_tty)
            _log(f"tests done (python {python}) in {time.monotonic() - start:.2f}s")
    else:
        print("Usage: tools/dev.py [lint|test]")


if __name__ == "__main__":
    main()
