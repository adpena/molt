#!/usr/bin/env python3
"""Orphan build-process reaper — standing prevention for the memory-pressure
class where *Molt* build subtrees leak.

ROOT CAUSE (Windows): killing a process does NOT kill its children. When a build
launcher goes away without tree-reaping — a subagent's `cargo ... | tail` shell,
a stopped/idle subagent, a queue runner that only closes its direct child — the
descendants (rustc, cargo, link, tail) are orphaned. Orphaned `tail.exe`
in particular hold build pipes open, keeping rustc/cargo alive and reserving many
GB. On 2026-07-03 this accumulated to 42 GB / 98% commit before a manual sweep.

The memory-guard tree-reaps builds that go THROUGH it (ProcessTreeTracker +
force_close_process_group), but direct cargo and stopped-subagent shells escape
it. This reaper is the all-source safety net for that leak class.

SAFETY (hard, owned-only): a dead parent plus a build-tool name is NOT sufficient
to kill — that conflated "a build tool whose launcher exited" with "a leaked
*Molt* build tool" and could collateral-kill an unrelated non-Molt build whose
launcher shell had already exited, a Windows-PID-reuse victim, or a console host.
This reaper reaps a process ONLY when ALL of these hold:

  (a) its executable name is in REAP_NAMES (a build/pipe tool), AND
  (b) it is ORPHANED (its parent PID is no longer alive), AND
  (c) it is proven MOLT-OWNED by the same ``process_sentinel.is_molt_process``
      predicate the rest of the apparatus (memory_guard, process_sentinel,
      backend_daemon_custody) uses — its command line / image path resolves
      under the Molt repo or a Molt artifact root — AND
  (d) at the moment of the kill, its process identity (creation time via
      GetProcessTimes, carried as ``started_at_ns``) still matches the identity
      observed in the snapshot. If the PID was recycled to a different process
      between snapshot and kill (Windows reuses PIDs aggressively), the identity
      mismatches and the kill FAILS CLOSED.

It NEVER touches claude, Codex, python, node, the OS, host-control-plane
processes, any process whose parent is alive (an active build), or any process
not proven Molt-owned. Termination is routed through
``memory_guard._send_pid_signal_if_identity_action``, the shared identity-checked
kill primitive, so host-control-plane lineage and protected-group protections
apply here exactly as they do everywhere else.

Windows-only (this leak is a Windows process-model artifact); a no-op elsewhere.

Modes:
  orphan_reaper.py sweep [--dry-run]          one-shot; --dry-run only reports
  orphan_reaper.py watch [--interval 90]      periodic sweep (default 90s)
"""
from __future__ import annotations

import argparse
from collections.abc import Mapping, Sequence
import ntpath
from pathlib import Path
import signal
import sys
import time

_THIS_FILE = Path(__file__).resolve()
_REPO_ROOT = _THIS_FILE.parents[1]
if str(_REPO_ROOT) not in sys.path:
    sys.path.insert(0, str(_REPO_ROOT))

from tools import memory_guard  # noqa: E402
from tools import process_sentinel  # noqa: E402
from tools.memory_guard import ProcessSample  # noqa: E402

# Build/pipe tools that are safe to reap WHEN ORPHANED **and Molt-owned**.
# Deliberately excludes python/node (ambiguous — Codex/guard helpers) and every
# agent/OS name. ``conhost.exe`` is deliberately NOT here: a console host is
# almost never a leaked Molt build tool and routinely backs a live unrelated
# console, so name-matching it risks killing a live console's host.
REAP_NAMES = {
    "tail.exe",
    "sleep.exe",
    "rustc.exe",
    "cargo.exe",
    "link.exe",
    "cl.exe",
    "lld-link.exe",
    "cc1.exe",
    "cc1plus.exe",
    "mspdbsrv.exe",
    "vctip.exe",
}


def repo_root() -> Path:
    return _REPO_ROOT


def _snapshot() -> dict[int, ProcessSample]:
    """Return ``{pid: ProcessSample}`` for every process. Windows-only.

    Reuses the shared memory-guard Windows sampler, so every sample carries the
    full command line / image path (needed for the Molt-ownership gate) and the
    ``started_at_ns`` process creation time (needed for the PID-reuse identity
    gate). This is the same snapshot authority ``process_sentinel`` scans; the
    reaper does not maintain a private, weaker process view.
    """
    return memory_guard.sample_processes_windows_hard_timeout()


def _exe_name(command: str) -> str:
    """Return the lowercased basename of the executable in ``command``.

    ``command`` is a full command line for cargo/rustc/etc. or a full image path
    otherwise (see ``memory_guard_core.windows_snapshot``). The reaper matches on
    the leading executable's basename against ``REAP_NAMES``.
    """
    text = command.strip()
    if not text:
        return ""
    if text[0] in {'"', "'"}:
        quote = text[0]
        end = text.find(quote, 1)
        first = text[1:end] if end > 0 else text[1:]
    else:
        first = text.split(None, 1)[0]
    return ntpath.basename(first).casefold()


def find_orphans(
    samples: Mapping[int, ProcessSample],
    *,
    root: Path | None = None,
    self_pid: int | None = None,
) -> list[ProcessSample]:
    """Select Molt-owned, orphaned build tools that are safe to reap.

    A sample is selected only when its executable name is in ``REAP_NAMES``, its
    parent PID is no longer alive (orphaned), and ``is_molt_process`` proves it
    is Molt-owned. A dead-parent build tool that is NOT Molt-owned (an unrelated
    build whose launcher exited, a PID-reuse victim, a console host) is never
    selected. This is a pure function of the snapshot — no OS calls, no kills —
    so the ownership contract is directly testable.
    """
    resolved_root = repo_root() if root is None else root
    live = set(samples)
    orphans: list[ProcessSample] = []
    for sample in samples.values():
        if sample.pid == 0:
            continue
        if self_pid is not None and sample.pid == self_pid:
            continue
        if _exe_name(sample.command) not in REAP_NAMES:
            continue
        if sample.ppid in live:
            continue
        if not process_sentinel.is_molt_process(
            sample, root=resolved_root, self_pid=self_pid
        ):
            continue
        orphans.append(sample)
    return orphans


def _reap_one(sample: ProcessSample, *, grace: float) -> bool:
    """Terminate one selected orphan, re-validating identity at the kill.

    Routes through the shared identity-checked kill primitive: it re-samples the
    process table and refuses to signal if the PID's identity (creation time via
    ``started_at_ns``) no longer matches the snapshot, or if the PID now belongs
    to host-control-plane lineage or a protected group. A recycled PID therefore
    fails closed. Because the command is part of ``process_identity``, an identity
    match guarantees this is the same process ``find_orphans`` already proved
    Molt-owned. Returns True only if SIGTERM was actually delivered.
    """
    identity = memory_guard.process_identity(sample)
    action = memory_guard._send_pid_signal_if_identity_action(
        sample.pid,
        identity,
        signal.SIGTERM,
        sampler=_snapshot,
    )
    if action.result != "sent":
        return False
    if grace > 0:
        time.sleep(grace)
    memory_guard._send_pid_signal_if_identity_action(
        sample.pid,
        identity,
        memory_guard.fallback_kill_signal(),
        sampler=_snapshot,
    )
    return True


def sweep(dry_run: bool = False, *, grace: float = 0.5) -> dict[str, int]:
    if not sys.platform.startswith("win"):
        return {}
    root = repo_root()
    self_pid = _current_pid()
    orphans = find_orphans(_snapshot(), root=root, self_pid=self_pid)
    reaped: dict[str, int] = {}
    for sample in orphans:
        name = _exe_name(sample.command) or f"pid:{sample.pid}"
        if dry_run:
            reaped[name] = reaped.get(name, 0) + 1
        elif _reap_one(sample, grace=grace):
            reaped[name] = reaped.get(name, 0) + 1
    return reaped


def _current_pid() -> int:
    import os

    return os.getpid()


def _report(reaped: dict[str, int], dry_run: bool) -> None:
    verb = "would reap" if dry_run else "reaped"
    total = sum(reaped.values())
    if total == 0:
        print(f"orphan_reaper: no orphaned Molt build processes ({verb} 0)")
        return
    detail = ", ".join(
        f"{n}={c}" for n, c in sorted(reaped.items(), key=lambda kv: -kv[1])
    )
    print(
        f"orphan_reaper: {verb} {total} orphaned Molt build processes [{detail}]"
    )


def main(argv: Sequence[str] | None = None) -> int:
    p = argparse.ArgumentParser(
        description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter
    )
    sub = p.add_subparsers(dest="cmd", required=True)
    ps = sub.add_parser("sweep", help="one-shot sweep")
    ps.add_argument("--dry-run", action="store_true", help="report without killing")
    pw = sub.add_parser("watch", help="periodic sweep")
    pw.add_argument("--interval", type=int, default=90)
    args = p.parse_args(argv)

    if not sys.platform.startswith("win"):
        print("orphan_reaper: non-Windows, nothing to do")
        return 0

    if args.cmd == "sweep":
        _report(sweep(dry_run=args.dry_run), args.dry_run)
        return 0
    if args.cmd == "watch":
        sys.stderr.write(
            f"orphan_reaper: watch every {args.interval}s "
            "(dead-parent, Molt-owned build tools only)\n"
        )
        sys.stderr.flush()
        while True:
            try:
                reaped = sweep(dry_run=False)
                if sum(reaped.values()):
                    _report(reaped, False)
                    sys.stdout.flush()
            except Exception as exc:  # never let the net die
                sys.stderr.write(f"orphan_reaper: sweep error (continuing): {exc}\n")
                sys.stderr.flush()
            time.sleep(args.interval)
    return 2


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
