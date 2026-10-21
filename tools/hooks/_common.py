#!/usr/bin/env python3
"""Shared, fail-open infrastructure for the molt Claude Code hooks.

``tools/hooks/wiring.py`` declares the hooks and generates
``.claude/settings.json``; each runs on the operator's own sessions. The one
non-negotiable invariant: A BUG IN A HOOK MUST NEVER BRICK A SESSION. So this
module gives every hook the same spine:

* ``force_utf8()`` -- reuse the landed cp1252 backstop (M43) so a relayed byte
  never crashes a hook on Windows.
* ``run_fail_open()`` -- the wrapper: ANY exception in the hook body is caught,
  logged to ``.molt/state/<hook>_errors.log`` with a loud-escalation threshold
  (so fail-open is never silent), and the process exits 0.

Every hook runs under the checkout's own interpreter: each settings command
starts it through ``tools/hooks/run.sh``, which selects the uv environment's
Python. Claude Code runs hook commands with the app's PATH, which need not
carry ``python`` (the macOS desktop app has none).
"""

from __future__ import annotations

import json
import os
import sqlite3
import sys
import time
from pathlib import Path
from typing import Any, Callable

from tools.command_execution import CommandExecutor  # type: ignore

_COMMANDS = CommandExecutor.for_file(__file__)

# --- UTF-8 backstop (M43) --------------------------------------------------
from tools._io_utf8 import force_utf8_stdio as _force_utf8_stdio  # noqa: E402


def force_utf8() -> None:
    """Idempotent, never-raising UTF-8 reconfigure of stdout/stderr."""
    try:
        _force_utf8_stdio()
    except Exception:
        pass


# --- Repo / state-dir resolution -------------------------------------------

# Number of logged errors after which fail-open escalates loudly.
ERROR_ESCALATION_THRESHOLD = 3


def repo_root(cwd: str | os.PathLike[str] | None = None) -> Path:
    """Resolve the repo root, preferring the harness-provided project dir.

    Order: ``$CLAUDE_PROJECT_DIR`` -> ``git rev-parse --show-toplevel`` from
    ``cwd`` -> this file's ancestor (``tools/hooks/_common.py`` -> repo root).
    Always returns *some* directory; never raises.
    """
    env_dir = os.environ.get("CLAUDE_PROJECT_DIR")
    if env_dir:
        p = Path(env_dir)
        if p.is_dir():
            return p
    if cwd:
        try:
            out = _COMMANDS.run(
                ["git", "rev-parse", "--show-toplevel"],
                cwd=str(cwd),
                capture_output=True,
                text=True,
                encoding="utf-8",
                errors="replace",
                timeout=5,
            )
            top = out.stdout.strip()
            if out.returncode == 0 and top:
                return Path(top)
        except Exception:
            pass
    # tools/hooks/_common.py -> parents[2] == repo root
    return Path(__file__).resolve().parents[2]


def state_dir(root: Path | None = None) -> Path:
    """``.molt/state`` under the repo root; created on demand, never raises."""
    base = (root or repo_root()) / ".molt" / "state"
    try:
        base.mkdir(parents=True, exist_ok=True)
    except Exception:
        pass
    return base


# --- stdin / hook input ----------------------------------------------------


def read_hook_input() -> dict[str, Any]:
    """Read the JSON the harness passes on stdin. Fail-open to ``{}``."""
    try:
        raw = sys.stdin.read()
    except Exception:
        return {}
    if not raw or not raw.strip():
        return {}
    try:
        data = json.loads(raw)
        return data if isinstance(data, dict) else {}
    except Exception:
        return {}


# --- error logging + loud escalation ---------------------------------------


def log_error(hook_name: str, exc: BaseException, root: Path | None = None) -> None:
    """Append a hook error to ``.molt/state/<hook>_errors.log`` and, past the
    escalation threshold, shout on stderr so fail-open is never silent."""
    try:
        sd = state_dir(root)
        log_path = sd / f"{hook_name}_errors.log"
        stamp = time.strftime("%Y-%m-%dT%H:%M:%S")
        line = f"{stamp}\t{type(exc).__name__}\t{exc}\n"
        with open(log_path, "a", encoding="utf-8") as fh:
            fh.write(line)
        # Count lines cheaply to decide on escalation.
        try:
            with open(log_path, "r", encoding="utf-8", errors="replace") as fh:
                count = sum(1 for _ in fh)
        except Exception:
            count = 1
        # Always emit a concise stderr note (non-silent); escalate past threshold.
        sys.stderr.write(
            f"[molt-hook:{hook_name}] fail-open after error: {type(exc).__name__}: {exc}\n"
        )
        if count >= ERROR_ESCALATION_THRESHOLD:
            sys.stderr.write(
                f"[molt-hook:{hook_name}] ALERT: {count} fail-open errors logged to "
                f"{log_path} -- the {hook_name} hook keeps failing. Investigate.\n"
            )
        sys.stderr.flush()
    except Exception:
        # Logging must never itself brick the hook.
        pass


def run_fail_open(hook_name: str, body: Callable[[], int]) -> None:
    """Run ``body()`` and exit with its code; ANY exception -> log + exit 0.

    This is THE fail-open guarantee: ``body`` returns its exit code or calls
    ``sys.exit`` deliberately, and if it raises, the session is never wedged.
    """
    force_utf8()
    try:
        code = body()
    except SystemExit as se:  # a deliberate sys.exit passes through
        raise se
    except BaseException as exc:  # noqa: BLE001 - fail-open is the whole point
        log_error(hook_name, exc)
        sys.exit(0)
    sys.exit(int(code) if isinstance(code, int) else 0)


def proof_queue_active_count(root: Path) -> int | None:
    """Active proof_queue runs (queued, dispatched or running), or None.

    Reads the queue's own state in-process (``state.active_run_count``). None
    means the state could not be read; the digest reports it as unknown.
    """
    from tools.proof_queue_pkg import state

    try:
        return state.active_run_count(state.default_db_path(root, os.environ))
    except (OSError, sqlite3.Error):
        return None
