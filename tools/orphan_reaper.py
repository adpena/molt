#!/usr/bin/env python3
"""Orphan build-process reaper — standing prevention for the memory-pressure
class where build subtrees leak.

ROOT CAUSE (Windows): killing a process does NOT kill its children. When a build
launcher goes away without tree-reaping — a subagent's `cargo ... | tail` shell,
a stopped/idle subagent, a queue runner that only closes its direct child — the
descendants (rustc, cargo, link, tail, conhost) are orphaned. Orphaned `tail.exe`
in particular hold build pipes open, keeping rustc/cargo alive and reserving many
GB. On 2026-07-03 this accumulated to 42 GB / 98% commit before a manual sweep.

The memory-guard tree-reaps builds that go THROUGH it (ProcessTreeTracker +
force_close_process_group), but direct cargo and stopped-subagent shells escape
it. This reaper is the all-source safety net: it periodically kills processes
that are BOTH (a) a build/pipe tool by name AND (b) ORPHANED (their parent PID is
no longer alive = they're not part of any active build/agent tree).

SAFETY (hard): it only ever touches the names in REAP_NAMES, and only when the
parent is dead. It NEVER touches claude, Codex, python, node, the OS, or any
process whose parent is alive (i.e. an active build). An active `cargo | tail`
keeps its shell parent alive, so its tail is not orphaned and is left alone.

Windows-only (this leak is a Windows process-model artifact); a no-op elsewhere.

Modes:
  orphan_reaper.py sweep [--dry-run]          one-shot; --dry-run only reports
  orphan_reaper.py watch [--interval 90]      periodic sweep (default 90s)
"""
from __future__ import annotations

import argparse
import sys
import time

# Build/pipe tools that are safe to reap WHEN ORPHANED. Deliberately excludes
# python/node (ambiguous — Codex/guard helpers) and every agent/OS name.
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
    "conhost.exe",
}


def _snapshot() -> list[tuple[int, int, str]]:
    """Return (pid, ppid, name) for every process, via Toolhelp32. Windows-only."""
    import ctypes
    from ctypes import wintypes

    TH32CS_SNAPPROCESS = 0x00000002
    k32 = ctypes.WinDLL("kernel32", use_last_error=True)

    class PROCESSENTRY32(ctypes.Structure):
        _fields_ = [
            ("dwSize", wintypes.DWORD),
            ("cntUsage", wintypes.DWORD),
            ("th32ProcessID", wintypes.DWORD),
            ("th32DefaultHeapID", ctypes.POINTER(ctypes.c_ulong)),
            ("th32ModuleID", wintypes.DWORD),
            ("cntThreads", wintypes.DWORD),
            ("th32ParentProcessID", wintypes.DWORD),
            ("pcPriClassBase", ctypes.c_long),
            ("dwFlags", wintypes.DWORD),
            ("szExeFile", ctypes.c_char * 260),
        ]

    snap = k32.CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0)
    if snap == wintypes.HANDLE(-1).value:
        raise ctypes.WinError(ctypes.get_last_error())
    out: list[tuple[int, int, str]] = []
    try:
        entry = PROCESSENTRY32()
        entry.dwSize = ctypes.sizeof(PROCESSENTRY32)
        if not k32.Process32First(snap, ctypes.byref(entry)):
            return out
        while True:
            name = entry.szExeFile.decode("ascii", "replace").lower()
            out.append((int(entry.th32ProcessID), int(entry.th32ParentProcessID), name))
            if not k32.Process32Next(snap, ctypes.byref(entry)):
                break
    finally:
        k32.CloseHandle(snap)
    return out


def _kill(pid: int) -> bool:
    import ctypes

    PROCESS_TERMINATE = 0x0001
    k32 = ctypes.WinDLL("kernel32", use_last_error=True)
    h = k32.OpenProcess(PROCESS_TERMINATE, False, pid)
    if not h:
        return False
    try:
        return bool(k32.TerminateProcess(h, 1))
    finally:
        k32.CloseHandle(h)


def find_orphans(procs: list[tuple[int, int, str]]) -> list[tuple[int, int, str]]:
    live = {pid for pid, _ppid, _name in procs}
    return [
        (pid, ppid, name)
        for (pid, ppid, name) in procs
        if name in REAP_NAMES and ppid not in live and pid != 0
    ]


def sweep(dry_run: bool = False) -> dict[str, int]:
    if not sys.platform.startswith("win"):
        return {}
    orphans = find_orphans(_snapshot())
    reaped: dict[str, int] = {}
    for pid, _ppid, name in orphans:
        if dry_run or _kill(pid):
            reaped[name] = reaped.get(name, 0) + 1
    return reaped


def _report(reaped: dict[str, int], dry_run: bool) -> None:
    verb = "would reap" if dry_run else "reaped"
    total = sum(reaped.values())
    if total == 0:
        print(f"orphan_reaper: no orphaned build processes ({verb} 0)")
        return
    detail = ", ".join(f"{n}={c}" for n, c in sorted(reaped.items(), key=lambda kv: -kv[1]))
    print(f"orphan_reaper: {verb} {total} orphaned build processes [{detail}]")


def main(argv: list[str] | None = None) -> int:
    p = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
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
        sys.stderr.write(f"orphan_reaper: watch every {args.interval}s (dead-parent build tools only)\n")
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
