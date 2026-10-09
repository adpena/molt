"""The ``molt`` console entry: fix the hash seed before the CLI loads.

Build output must not depend on string hash order, so every molt process runs
under one ``PYTHONHASHSEED``. The interpreter has already randomized string
hashes before any of this code runs, so a process without the seed restarts
itself. This module restarts first and imports the CLI only in the final
process, which then loads it once instead of twice.
"""

from __future__ import annotations

import os
import shutil
import subprocess
import sys
from collections.abc import Mapping
from pathlib import Path

from molt._host_exit import process_returncode_for_direct_os_exit

HASH_SEED_OVERRIDE_ENV = "MOLT_HASH_SEED"
HASH_SEED_SENTINEL_ENV = "MOLT_HASH_SEED_APPLIED"


def _is_windows_process_model() -> bool:
    return os.name == "nt"


def _flush_standard_streams() -> None:
    for stream in (sys.stdout, sys.stderr):
        try:
            stream.flush()
        except (OSError, ValueError):
            pass


def hash_seed_reexec_argv() -> list[str] | None:
    """The argv that restarts this process under the active interpreter."""
    orig_argv = list(getattr(sys, "orig_argv", ()) or ())
    if len(orig_argv) >= 2 and orig_argv[1] not in {"-c", "-"}:
        return [sys.executable, *orig_argv[1:]]
    if not sys.argv or sys.argv[0] in {"", "-c", "-"}:
        return None
    argv0 = sys.argv[0]
    has_sep = os.sep in argv0 or (os.altsep is not None and os.altsep in argv0)
    if has_sep or Path(argv0).exists() or shutil.which(argv0):
        return [sys.executable, *sys.argv]
    return None


def _reexec_with_hash_seed(env: Mapping[str, str]) -> None:
    argv = hash_seed_reexec_argv()
    if argv is None:
        return
    if _is_windows_process_model():
        try:
            completed = subprocess.run(argv, env=dict(env), check=False)
        except OSError as exc:
            print(
                f"molt: failed to restart with PYTHONHASHSEED: {exc}", file=sys.stderr
            )
            _flush_standard_streams()
            raise SystemExit(127) from exc
        # A normal interpreter exit, never os._exit: the launcher's atexit
        # handlers (the proof-queue child custody hook's terminal handshake
        # among them) must run, or every guarded molt invocation ends with an
        # incomplete child-custody receipt.
        _flush_standard_streams()
        raise SystemExit(
            process_returncode_for_direct_os_exit(completed.returncode, windows=True)
        )
    os.execvpe(argv[0], argv, env)


def ensure_hash_seed() -> None:
    """Restart under the configured hash seed unless it is already applied."""
    desired = os.environ.get(HASH_SEED_OVERRIDE_ENV, "0").strip() or "0"
    if desired.lower() in {"off", "disable", "random"}:
        return
    if os.environ.get("PYTHONHASHSEED") == desired:
        return
    if os.environ.get(HASH_SEED_SENTINEL_ENV) == "1":
        print(
            "molt: deterministic PYTHONHASHSEED restart did not apply "
            f"(expected {desired!r}, got {os.environ.get('PYTHONHASHSEED')!r}).",
            file=sys.stderr,
        )
        _flush_standard_streams()
        raise SystemExit(127)
    env = os.environ.copy()
    env["PYTHONHASHSEED"] = desired
    env[HASH_SEED_SENTINEL_ENV] = "1"
    _reexec_with_hash_seed(env)


def main() -> int:
    ensure_hash_seed()
    from molt.cli import main as cli_main

    return cli_main()
