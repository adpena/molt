#!/usr/bin/env python3
"""Shared, fail-open infrastructure for the molt Claude Code hook spine.

APPARATUS Wave 1 (docs/agent/APPARATUS_FROM_COMMA_LAB.md, A1). Every hook in
``tools/hooks/`` is wired into ``.claude/settings.json`` and therefore runs on
the operator's OWN sessions. The single non-negotiable invariant is:

    A BUG IN A HOOK MUST NEVER BRICK A SESSION.

So this module gives every hook the same spine:

* ``force_utf8()`` -- reuse the landed cp1252 backstop (M43) so a relayed byte
  never crashes a hook on Windows.
* a PURE, unit-tested ``decide()``/``evaluate()`` surface per hook (defined in
  the hook module, tested separately from the wrapper).
* ``run_fail_open()`` -- the wrapper: ANY exception in the hook body is caught,
  logged to ``.molt/state/<hook>_errors.log`` with a loud-escalation threshold
  (so fail-open is never silent), and the process exits 0 (ALLOW).
* cross-platform (``msvcrt`` on Windows, ``fcntl`` on POSIX) locked append for
  the append-only ledgers.

Nothing here imports a third-party package -- hooks must run under any Python
3.9+ interpreter (host ``python`` on the Windows box, or any fleet python),
because the hook ``command`` in settings.json cannot depend on the molt venv.
"""

from __future__ import annotations

import json
import os
import subprocess
import sys
import time
from pathlib import Path
from typing import Any, Callable

# --- UTF-8 backstop (M43) --------------------------------------------------
# Reuse the landed helper if importable; otherwise degrade to a local copy so a
# hook is never blocked merely because sys.path did not include the repo root.
try:  # pragma: no cover - trivial import shim
    from tools._io_utf8 import force_utf8_stdio as _force_utf8_stdio
except Exception:  # pragma: no cover - fallback path

    def _force_utf8_stdio(*, errors: str = "backslashreplace") -> None:
        for stream in (sys.stdout, sys.stderr):
            reconfigure = getattr(stream, "reconfigure", None)
            if reconfigure is None:
                continue
            try:
                reconfigure(encoding="utf-8", errors=errors)
            except (AttributeError, ValueError, OSError):
                pass


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
            out = subprocess.run(
                ["git", "rev-parse", "--show-toplevel"],
                cwd=str(cwd),
                capture_output=True,
                text=True,
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
                f"{log_path} -- the {hook_name} guard may be silently inert. Investigate.\n"
            )
        sys.stderr.flush()
    except Exception:
        # Logging must never itself brick the hook.
        pass


def run_fail_open(hook_name: str, body: Callable[[], int]) -> None:
    """Run ``body()`` and exit with its code; ANY exception -> log + exit 0.

    This is THE fail-open guarantee. ``body`` may block (print JSON / exit 2) by
    returning that intent, but if it raises, the session is never wedged.
    """
    force_utf8()
    try:
        code = body()
    except SystemExit as se:  # body may sys.exit deliberately (e.g. exit 2 block)
        raise se
    except BaseException as exc:  # noqa: BLE001 - fail-open is the whole point
        log_error(hook_name, exc)
        sys.exit(0)
    sys.exit(int(code) if isinstance(code, int) else 0)


# --- cross-platform locked append ------------------------------------------


class _FileLock:
    """A tiny advisory file lock: msvcrt on Windows, fcntl on POSIX.

    Best-effort: if locking is unavailable the append still happens (a lost lock
    is better than a wedged hook). Used only for low-contention append-only
    ledgers, so a rare interleave is acceptable and never destructive.
    """

    def __init__(self, handle) -> None:  # noqa: ANN001 - file object
        self._handle = handle
        self._locked = False

    def __enter__(self):  # noqa: ANN204
        try:
            if os.name == "nt":
                import msvcrt

                # Lock 1 byte at offset 0; enough for append serialization.
                self._handle.seek(0)
                msvcrt.locking(self._handle.fileno(), msvcrt.LK_LOCK, 1)
            else:
                import fcntl

                fcntl.flock(self._handle.fileno(), fcntl.LOCK_EX)
            self._locked = True
        except Exception:
            self._locked = False
        return self

    def __exit__(self, *exc) -> None:  # noqa: ANN002
        if not self._locked:
            return
        try:
            if os.name == "nt":
                import msvcrt

                self._handle.seek(0)
                msvcrt.locking(self._handle.fileno(), msvcrt.LK_UNLCK, 1)
            else:
                import fcntl

                fcntl.flock(self._handle.fileno(), fcntl.LOCK_UN)
        except Exception:
            pass


def append_jsonl(path: Path, record: dict[str, Any]) -> None:
    """Append one JSON record (with a ``ts`` if absent) under a file lock.

    Never raises: an audit ledger that cannot be written must not brick a hook.
    """
    try:
        record = dict(record)
        record.setdefault("ts", time.strftime("%Y-%m-%dT%H:%M:%S"))
        path.parent.mkdir(parents=True, exist_ok=True)
        line = json.dumps(record, ensure_ascii=True)
        # Open in append+read so the lock can seek(0) on Windows.
        with open(path, "a+", encoding="utf-8") as fh:
            with _FileLock(fh):
                fh.seek(0, os.SEEK_END)
                fh.write(line + "\n")
                fh.flush()
    except Exception:
        pass


def read_jsonl(path: Path) -> list[dict[str, Any]]:
    """Read a jsonl ledger, skipping malformed lines. Never raises."""
    out: list[dict[str, Any]] = []
    try:
        if not path.exists():
            return out
        with open(path, "r", encoding="utf-8", errors="replace") as fh:
            for line in fh:
                line = line.strip()
                if not line:
                    continue
                try:
                    obj = json.loads(line)
                    if isinstance(obj, dict):
                        out.append(obj)
                except Exception:
                    continue
    except Exception:
        return out
    return out


# --- git helpers (all best-effort, short-timeout, never raise) --------------


def _git(root: Path, *args: str, timeout: float = 5.0) -> subprocess.CompletedProcess:
    return subprocess.run(
        ["git", *args],
        cwd=str(root),
        capture_output=True,
        text=True,
        timeout=timeout,
    )


def git_head(root: Path) -> str | None:
    try:
        r = _git(root, "rev-parse", "HEAD")
        head = r.stdout.strip()
        return head or None
    except Exception:
        return None


def is_linked_worktree(root: Path) -> bool:
    """True when ``root`` is a linked worktree (git-dir != common-dir) or a
    temp-index plumbing session -- both are SAFE (never the shared checkout)."""
    try:
        if os.environ.get("GIT_INDEX_FILE"):
            return True
        gd = _git(root, "rev-parse", "--git-dir").stdout.strip()
        cd = _git(root, "rev-parse", "--git-common-dir").stdout.strip()
        if not gd or not cd:
            return False
        return os.path.abspath(os.path.join(str(root), gd)) != os.path.abspath(
            os.path.join(str(root), cd)
        )
    except Exception:
        return False


def worktree_count(root: Path) -> int | None:
    try:
        r = _git(root, "worktree", "list", "--porcelain")
        return sum(1 for line in r.stdout.splitlines() if line.startswith("worktree "))
    except Exception:
        return None


def origin_is_https(root: Path) -> bool:
    try:
        r = _git(root, "remote", "get-url", "origin")
        url = r.stdout.strip().lower()
        return url.startswith("https://") or url.startswith("http://")
    except Exception:
        return False


def proof_queue_active_count(root: Path, timeout: float = 5.0) -> int | None:
    """Number of ACTIVE proof_queue rows, or None if undeterminable.

    Parses ``python tools/proof_queue.py status`` (which has no ``--json`` mode)
    -- its ``active:`` section lists ``- none`` when idle. None means "could not
    tell"; callers decide the fail-open direction (the guard treats None as
    not-live so it never blocks a build on uncertainty; the landing gate treats
    None as in-flight so it never nags on uncertainty).
    """
    try:
        r = subprocess.run(
            [sys.executable, "tools/proof_queue.py", "status"],
            cwd=str(root),
            capture_output=True,
            text=True,
            timeout=timeout,
        )
        if r.returncode != 0 or not r.stdout.strip():
            return None
        active: list[str] = []
        in_active = False
        for line in r.stdout.splitlines():
            s = line.strip()
            if not s:
                continue
            low = s.lower().rstrip(":")
            if low == "active" and s.endswith(":"):
                in_active = True
                continue
            if in_active:
                if s.endswith(":") and not s.startswith("-"):
                    break  # next section header (e.g. "recent:")
                if s.startswith("-"):
                    item = s[1:].strip()
                    if item and item.lower() != "none":
                        active.append(item)
                else:
                    break
        return len(active)
    except Exception:
        return None
