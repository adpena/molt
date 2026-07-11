#!/usr/bin/env python3
"""fast_frontier_cycle.py — the seconds-long native CPython-ABI frontier loop.

Every numpy/scipy *wasm witness* frontier (a silent -1, a wrong answer, a panic,
a trap) lives in platform-independent Rust under `runtime/molt-cpython-abi/`.
The witness takes ~20-30 min to build+run and surface one; the *same* divergence
reproduces as a plain `cargo test` in **seconds**, with a real backtrace and a
debugger. This script drives that loop.

It builds and runs `runtime/molt-cpython-abi/tests/frontier_repro.rs`, whose
`frontier_*` tests each assert CPython-3.12-correct behavior and are `#[ignore]`d
only until the fix lands (default `cargo test` skips them, so gates stay green).

Usage
-----
  python tools/fast_frontier_cycle.py                 # cycle: build + reproduce all frontiers
  python tools/fast_frontier_cycle.py --repro 08      # reproduce just frontier #08 (substring filter)
  python tools/fast_frontier_cycle.py --verify        # run the GREEN control only (harness-is-live check)
  python tools/fast_frontier_cycle.py --list          # list the reproductions
  python tools/fast_frontier_cycle.py --build         # (re)build the test binary only, timed

Environment
-----------
  CARGO_TARGET_DIR   Honored if set; otherwise an isolated `<repo>/target`
                     sibling is NOT forced — cargo's default is used. Set it to
                     a fast off-OneDrive volume (e.g. C:/Molt/cargo-target-XXX)
                     for best incremental times.

Exit status
-----------
  --verify / --build : 0 on success.
  --repro / --cycle  : 0 if the named frontiers still reproduce (tests FAIL as
                       expected); non-zero if a frontier no longer reproduces
                       (i.e. it was fixed — go delete its #[ignore] line!).
"""

from __future__ import annotations

import argparse
import os
import subprocess
import sys
import time
from pathlib import Path

# Windows consoles default to cp1252 and choke on non-ASCII; force UTF-8 so this
# tool never dies on an encode error while relaying subprocess output (recurring
# Windows bug class). One shared primitive backstops it.
try:  # importable whether launched as a script (tools/ on path) or as tools.X
    from _io_utf8 import force_utf8_stdio
except ModuleNotFoundError:
    from tools._io_utf8 import force_utf8_stdio
force_utf8_stdio()

REPO_ROOT = Path(__file__).resolve().parents[1]
CRATE = "molt-lang-cpython-abi"
TEST = "frontier_repro"
WASM_WITNESS_SECONDS = 1800  # ~30 min baseline the wasm witness cycle costs.


def _cargo(args: list[str], capture: bool) -> subprocess.CompletedProcess:
    env = dict(os.environ)
    cmd = ["cargo", *args]
    return subprocess.run(
        cmd,
        cwd=REPO_ROOT,
        env=env,
        text=True,
        encoding="utf-8",
        errors="replace",
        capture_output=capture,
    )


def build() -> float:
    start = time.time()
    proc = _cargo(["test", "-p", CRATE, "--test", TEST, "--no-run"], capture=True)
    secs = time.time() - start
    if proc.returncode != 0:
        sys.stderr.write(proc.stdout + proc.stderr)
        print(f"FRONTIER build FAILED in {secs:.1f}s")
        sys.exit(proc.returncode)
    print(f"FRONTIER build ok  {secs:.1f}s")
    return secs


def verify() -> int:
    """Run the non-ignored GREEN control — proves the harness drives real ABI code."""
    start = time.time()
    proc = _cargo(["test", "-p", CRATE, "--test", TEST], capture=True)
    secs = time.time() - start
    ok = proc.returncode == 0
    print(
        f"FRONTIER verify {'ok' if ok else 'FAILED'}  {secs:.1f}s  (green control: harness is live)"
    )
    if not ok:
        sys.stderr.write(proc.stdout + proc.stderr)
    return proc.returncode


def repro(filt: str | None) -> int:
    """Run the #[ignore]d frontier reproductions. A FAILING test == a reproduced
    frontier (expected). A PASSING test == the frontier was fixed."""
    args = ["test", "-p", CRATE, "--test", TEST, "--"]
    if filt:
        args.append(filt)
    args += ["--ignored", "--nocapture", "--test-threads=1"]
    start = time.time()
    proc = _cargo(args, capture=True)
    secs = time.time() - start
    out = proc.stdout + proc.stderr

    reproduced = [ln.strip() for ln in out.splitlines() if "REPRODUCED" in ln]
    fixed = [ln for ln in out.splitlines() if "test result:" in ln]

    print("-" * 72)
    for ln in reproduced:
        print("  " + ln)
    print("-" * 72)
    for ln in fixed:
        print("  " + ln.strip())

    speedup = WASM_WITNESS_SECONDS / max(secs, 0.1)
    print(
        f"FRONTIER repro  {secs:.1f}s edit->signal  "
        f"(~{speedup:.0f}x faster than the ~30-min wasm witness cycle)"
    )
    # cargo returns non-zero because the #[ignore]d tests FAIL by design (they
    # assert the CPython-correct behavior a frontier violates). That non-zero is
    # the *expected* "still reproduces" signal, so we translate:
    #   at least one REPRODUCED line printed -> success (0).
    #   nothing reproduced + tests passed    -> frontier fixed -> non-zero.
    if reproduced:
        return 0
    if proc.returncode == 0:
        print(
            "  NOTE: no frontier reproduced — was it fixed? Delete its #[ignore] line."
        )
        return 3
    sys.stderr.write(out)
    return proc.returncode


def list_frontiers() -> int:
    proc = _cargo(
        ["test", "-p", CRATE, "--test", TEST, "--", "--list", "--ignored"], capture=True
    )
    for ln in (proc.stdout or "").splitlines():
        if ln.strip().endswith(": test"):
            print("  " + ln.strip())
    return 0


def main() -> int:
    ap = argparse.ArgumentParser(
        description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter
    )
    g = ap.add_mutually_exclusive_group()
    g.add_argument(
        "--build", action="store_true", help="build the frontier test binary only"
    )
    g.add_argument(
        "--verify",
        action="store_true",
        help="run the GREEN control (harness-is-live) only",
    )
    g.add_argument(
        "--repro",
        nargs="?",
        const="",
        metavar="FILTER",
        help="reproduce frontiers (optional name substring)",
    )
    g.add_argument(
        "--list", action="store_true", help="list the frontier reproductions"
    )
    ap.add_argument(
        "--cycle", action="store_true", help="build then reproduce (default)"
    )
    args = ap.parse_args()

    if args.build:
        build()
        return 0
    if args.verify:
        return verify()
    if args.list:
        return list_frontiers()
    if args.repro is not None:
        return repro(args.repro or None)
    # default: full cycle
    build()
    return repro(None)


if __name__ == "__main__":
    raise SystemExit(main())
