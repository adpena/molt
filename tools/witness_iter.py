#!/usr/bin/env python3
"""tools/witness_iter.py — FAST-WITNESS-ITER: a seconds-per-frontier numpy-init inner loop.

Shrinks the numpy-init edit->verify cycle from the ~30-min full wasm witness to a
native, re-runnable, **seconds-per-frontier** check. It is the repeatable driver
around the native C-extension discovery engine (`runtime/molt-cext-discovery` +
`tools/native_numpy_discovery.sh`), adding the three things a real inner loop
needs on top of the raw engine:

  (a) WARM frontend-lowering cache reuse for the reserved wasm confirmation
      (the persistent, content-addressed `MOLT_CACHE/module_lowering` tier; a
      fresh session no longer re-lowers unchanged numpy modules — cf.
      `--wasm-confirm` and the idempotent-AST-encoding fix this lane landed);
  (b) INCREMENTAL relink of only the changed CPython-ABI object — measured, not
      asserted by feel: `--measure-relink` touches one `molt-cpython-abi` source
      and times a single `cargo build -p molt-cext-discovery` (a crate relink,
      NOT a whole-runtime rebuild);
  (c) the native `PyInit` drive as the inner-loop CORRECTNESS check, with a
      two-sided PASS/RED gate against a committed known-good frontier baseline —
      so a real ABI regression turns the loop RED (a runner that cannot fail on a
      real break is theater, M05).

The full wasm witness is reserved for FINAL confirmation only (`--wasm-confirm`).

Host requirement
----------------
The native drive needs a Unix host with real `dlopen`/`RTLD_GLOBAL` (Linux or
macOS) — NOT native Windows (no flat namespace). On Windows this runner
auto-dispatches into WSL (`MOLT_WITNESS_WSL_DISTRO`, default `MoltCodonUbuntu`);
translate/point it at a WSL-local checkout with `MOLT_WITNESS_WSL_REPO` for fast
builds (a `/mnt/c` 9p build is slow). Inside Linux/macOS it runs directly.

Usage
-----
    # inner loop: build (incremental) -> symbol-gap -> drive PyInit -> assert
    python tools/witness_iter.py                       # module _multiarray_umath
    python tools/witness_iter.py --module _multiarray_umath

    # establish / refresh the known-good frontier baseline (prints observed sig)
    python tools/witness_iter.py --record

    # whole-witness static symbol sweep only (fastest; no PyInit)
    python tools/witness_iter.py --symbols

    # also time an incremental ABI relink (evidences lever (b))
    python tools/witness_iter.py --measure-relink

    # reserved final confirmation: full wasm witness with warm lowering cache (a)
    python tools/witness_iter.py --wasm-confirm

Exit code: 0 = PASS (reached the known-good frontier), non-zero = RED (regression
or engine error). `--json` emits the machine-readable result on stdout.
"""

from __future__ import annotations

import argparse
import json
import os
import platform
import re
import shutil
import subprocess
import sys
import time
from dataclasses import asdict, dataclass, field
from pathlib import Path

# ── Known-good frontier baselines (the committed authority) ───────────────────
# Each entry encodes the EXPECTED far frontier a clean, all-fixes-landed tree
# reaches when the engine drives that extension's PyInit natively. It is a
# two-sided gate (the #39 pattern): `required` markers MUST appear (proof the
# drive advanced past every landed ABI fix), `forbidden` markers must NOT appear
# (each is the exact signature of a reverted landed fix), and `symbol_gap` pins
# the static Py* symbol wall. Reverting any landed frontier fix flips one of
# these -> RED. Refresh the observed side with `--record`; keep this dict as the
# code-reviewed authority.
DEFAULT_BASELINES: dict[str, dict] = {
    "_multiarray_umath": {
        # numpy 1.26.4 links 301 Py* symbols; the ABI resolves all but the 5
        # `_SizeT` variadic aliases numpy does not reference during init
        # (Tier A / A.3 in NATIVE_DISCOVERY_FRONTIERS.md).
        "symbol_gap": 5,
        # Known-good far frontier: numpy's C init has cleared module + type setup
        # and the datetime CAPI capsule (B.1, landed 09c8d2337), and now hits the
        # AOT import wall importing its pure-Python sibling `numpy.exceptions`.
        "required": [r"numpy\.exceptions"],
        # Each forbidden marker is the signature of a REVERTED landed fix:
        #  * datetime CAPI capsule (09c8d2337): PyCapsule_Import silent failure.
        "forbidden": [r"silent-failure\s+PyCapsule_Import\(datetime\.datetime_CAPI\)"],
        "note": (
            "reaches the numpy.exceptions AOT import wall (past singletons, "
            "symbol gaps, and the datetime.datetime_CAPI capsule)"
        ),
    },
}

# Aggregate whole-witness static symbol-gap (native_witness_symbol_sweep.sh):
# the complete Tier-A frontier for the field_solve.py compute surface.
DEFAULT_SYMBOL_SWEEP_MAX_GAP = 14

WSL_DISTRO = os.environ.get("MOLT_WITNESS_WSL_DISTRO", "MoltCodonUbuntu")

# ── Machine-readable engine markers ───────────────────────────────────────────
RE_GAP = re.compile(r"\bGAP=(\d+)\b")
RE_NEEDS = re.compile(r"numpy needs (\d+) Py\* symbols; harness exports (\d+)")
RE_OK = re.compile(r"===MOLT_DISCOVERY_OK:")
RE_LOADERR = re.compile(r"===MOLT_DISCOVERY_FRONTIER \(LoadError\): (.*)")
RE_FRONTIER_DISPLAY = re.compile(r"===MOLT_DISCOVERY_FRONTIER_DISPLAY: (.*)")
RE_EXC = re.compile(r"===MOLT_DISCOVERY_EXC(?:_[A-Z_]+)?: (.*)")
RE_PANIC = re.compile(r"===MOLT_FRONTIER_PANIC===")
RE_DRIVER_RC = re.compile(r"== driver exit code: (-?\d+)")
RE_TRACE = re.compile(r"\[MOLT_TRACE_CAPI\]\s+(call|silent-failure)\s+(.*)")
RE_AGG_GAP = re.compile(r"\((\d+) unique symbols\)")


@dataclass
class Fingerprint:
    """Structured, machine-checkable signature of one native drive."""

    module: str
    symbol_gap: int | None = None
    numpy_needs: int | None = None
    harness_exports: int | None = None
    pyinit_ok: bool = False
    driver_rc: int | None = None
    panic: bool = False
    loaderror: str | None = None
    frontier_display: str | None = None
    exc_lines: list[str] = field(default_factory=list)
    trace_calls: list[str] = field(default_factory=list)
    silent_failures: list[str] = field(default_factory=list)
    reached_frontier: str | None = None  # human-readable furthest point

    def haystack(self) -> str:
        """All frontier-bearing text, for required/forbidden marker matching."""
        parts: list[str] = []
        if self.loaderror:
            parts.append(self.loaderror)
        if self.frontier_display:
            parts.append(self.frontier_display)
        parts.extend(self.exc_lines)
        parts.extend(self.trace_calls)
        parts.extend(f"silent-failure {s}" for s in self.silent_failures)
        if self.reached_frontier:
            parts.append(self.reached_frontier)
        return "\n".join(parts)


def log(msg: str) -> None:
    print(msg, flush=True)


# ── Platform dispatch: on Windows, re-exec inside WSL ─────────────────────────
def maybe_dispatch_to_wsl(argv: list[str]) -> "int | None":
    """If on native Windows, re-exec this runner inside WSL and return its rc.

    Returns None when already on a Unix host (caller proceeds natively).
    """
    if platform.system() != "Windows":
        return None
    if "--in-wsl" in argv:  # guard against loops
        return None
    if not shutil.which("wsl.exe") and not shutil.which("wsl"):
        log("FATAL: native Windows has no dlopen/RTLD_GLOBAL and WSL is unavailable.")
        log("       Run this on Linux/macOS, or install WSL. See the module docstring.")
        return 2

    repo = repo_root()
    wsl_repo = os.environ.get("MOLT_WITNESS_WSL_REPO")
    if wsl_repo:
        target_repo = wsl_repo
    else:
        # Translate C:\Molt\... -> /mnt/c/Molt/... via wslpath.
        try:
            target_repo = subprocess.check_output(
                ["wsl.exe", "-d", WSL_DISTRO, "-e", "wslpath", "-a", str(repo)],
                text=True,
            ).strip()
        except Exception as exc:  # noqa: BLE001
            log(f"FATAL: could not translate repo path into WSL: {exc}")
            return 2

    inner = argv + ["--in-wsl"]
    inner_cmd = " ".join(_shq(a) for a in ["python3", "tools/witness_iter.py", *inner])
    # A per-distro isolated target dir off the Windows tree keeps builds fast and
    # avoids clobbering a native Windows target.
    fast_target = os.environ.get(
        "MOLT_WITNESS_WSL_TARGET", "/root/.molt-witness-target"
    )
    bash = (
        f"cd {_shq(target_repo)} && "
        f"export CARGO_TARGET_DIR={_shq(fast_target)} && "
        f"export MOLT_STALE_ORPHAN_CLEANUP=0 MOLT_DISABLE_AUTO_JANITOR=1 && "
        f"source /root/.cargo/env 2>/dev/null; {inner_cmd}"
    )
    log(f"== dispatching into WSL distro '{WSL_DISTRO}' (repo {target_repo}) ...")
    proc = subprocess.run(["wsl.exe", "-d", WSL_DISTRO, "-e", "bash", "-lc", bash])
    return proc.returncode


def _shq(s: str) -> str:
    """Minimal POSIX shell single-quote."""
    return "'" + s.replace("'", "'\"'\"'") + "'"


def repo_root() -> Path:
    return Path(__file__).resolve().parent.parent


# ── Engine invocation ─────────────────────────────────────────────────────────
def _env_for_drive() -> dict[str, str]:
    env = dict(os.environ)
    env.setdefault("MOLT_STALE_ORPHAN_CLEANUP", "0")
    env.setdefault("MOLT_DISABLE_AUTO_JANITOR", "1")
    env.setdefault("MOLT_TRACE_CAPI", "1")
    env.setdefault("RUST_BACKTRACE", "1")
    return env


def run_native_drive(module: str, profile: str) -> tuple[Fingerprint, str, float]:
    """Invoke native_numpy_discovery.sh: build (incremental) + symbol-gap + drive.

    Returns (fingerprint, combined_output, wall_seconds). `wall_seconds` is the
    full edit->frontier cycle (incremental build + static sweep + PyInit drive) —
    the number this lane measures against the ~1800 s wasm witness.
    """
    repo = repo_root()
    script = repo / "tools" / "native_numpy_discovery.sh"
    if not script.is_file():
        raise SystemExit(f"FATAL: engine script missing: {script}")
    env = _env_for_drive()
    env["MOLT_DISCOVERY_PROFILE"] = profile
    t0 = time.monotonic()
    proc = subprocess.run(
        ["bash", str(script), module],
        cwd=str(repo),
        env=env,
        capture_output=True,
        text=True,
    )
    wall = time.monotonic() - t0
    out = (proc.stdout or "") + (proc.stderr or "")
    fp = parse_drive_output(module, out)
    if fp.driver_rc is None:
        fp.driver_rc = proc.returncode
    return fp, out, wall


def parse_drive_output(module: str, out: str) -> Fingerprint:
    fp = Fingerprint(module=module)
    for line in out.splitlines():
        m = RE_GAP.search(line)
        if m:
            fp.symbol_gap = int(m.group(1))
        m = RE_NEEDS.search(line)
        if m:
            fp.numpy_needs = int(m.group(1))
            fp.harness_exports = int(m.group(2))
        if RE_OK.search(line):
            fp.pyinit_ok = True
        m = RE_LOADERR.search(line)
        if m:
            fp.loaderror = m.group(1).strip()
        m = RE_FRONTIER_DISPLAY.search(line)
        if m:
            fp.frontier_display = m.group(1).strip()
        m = RE_EXC.search(line)
        if m:
            fp.exc_lines.append(m.group(1).strip())
        if RE_PANIC.search(line):
            fp.panic = True
        m = RE_DRIVER_RC.search(line)
        if m:
            fp.driver_rc = int(m.group(1))
        m = RE_TRACE.search(line)
        if m:
            kind, target = m.group(1), m.group(2).strip()
            fp.trace_calls.append(f"{kind} {target}")
            if kind == "silent-failure":
                fp.silent_failures.append(target)
    # Human-readable furthest point reached.
    if fp.pyinit_ok:
        fp.reached_frontier = "PyInit returned a module (full init OK)"
    elif fp.exc_lines:
        fp.reached_frontier = fp.exc_lines[-1]
    elif fp.frontier_display:
        fp.reached_frontier = fp.frontier_display
    elif fp.loaderror:
        fp.reached_frontier = fp.loaderror
    elif fp.trace_calls:
        fp.reached_frontier = fp.trace_calls[-1]
    return fp


# ── PASS/RED gate ─────────────────────────────────────────────────────────────
@dataclass
class Verdict:
    passed: bool
    reasons: list[str] = field(default_factory=list)


def evaluate(fp: Fingerprint, baseline: dict) -> Verdict:
    reasons: list[str] = []
    ok = True

    # Engine-health: a panic or a bad-arg rc is never a valid frontier.
    if fp.panic:
        ok = False
        reasons.append("engine PANIC during drive (see backtrace)")
    if fp.driver_rc in (64, 65, 66, 67, -3):
        ok = False
        reasons.append(f"engine harness error (driver rc={fp.driver_rc})")

    exp_gap = baseline.get("symbol_gap")
    if exp_gap is not None:
        if fp.symbol_gap is None:
            ok = False
            reasons.append("symbol-gap not reported by engine")
        elif fp.symbol_gap != exp_gap:
            ok = False
            reasons.append(
                f"symbol GAP {fp.symbol_gap} != known-good {exp_gap} "
                f"({'more' if (fp.symbol_gap or 0) > exp_gap else 'fewer'} missing Py* symbols)"
            )

    hay = fp.haystack()
    for pat in baseline.get("required", []):
        if not re.search(pat, hay):
            ok = False
            reasons.append(f"required known-good marker ABSENT: /{pat}/")
    for pat in baseline.get("forbidden", []):
        if re.search(pat, hay):
            ok = False
            reasons.append(f"regression marker PRESENT (reverted-fix signature): /{pat}/")

    if ok:
        reasons.append(
            f"reached known-good frontier: {fp.reached_frontier or '(unknown)'}"
        )
    return Verdict(passed=ok, reasons=reasons)


# ── Incremental relink measurement (lever b) ──────────────────────────────────
def measure_incremental_relink(profile: str) -> float | None:
    """Touch one molt-cpython-abi source, time a single harness relink.

    Evidences lever (b): a single-frontier ABI edit rebuilds ONLY the changed
    crate object and relinks the cdylib — it does NOT rebuild molt-runtime. The
    returned seconds is the marginal cost of a one-file ABI change.
    """
    repo = repo_root()
    # Pick a stable, always-present ABI source (the datetime CAPI authority).
    candidates = [
        repo / "runtime" / "molt-cpython-abi" / "src" / "api" / "datetime.rs",
        repo / "runtime" / "molt-cpython-abi" / "src" / "bridge.rs",
        repo / "runtime" / "molt-cpython-abi" / "src" / "lib.rs",
    ]
    src = next((c for c in candidates if c.is_file()), None)
    if src is None:
        log("   (measure-relink: no molt-cpython-abi source found; skipping)")
        return None
    os.utime(src, None)  # touch -> mark the ABI object dirty
    env = _env_for_drive()
    build_dir = repo / "runtime"
    cmd = ["cargo", "build", "-p", "molt-cext-discovery"]
    if profile != "dev":
        cmd += ["--profile", profile]
    t0 = time.monotonic()
    proc = subprocess.run(cmd, cwd=str(build_dir), env=env, capture_output=True, text=True)
    wall = time.monotonic() - t0
    if proc.returncode != 0:
        log("   (measure-relink: relink build FAILED)")
        log((proc.stdout or "") + (proc.stderr or ""))
        return None
    return wall


# ── Whole-witness static symbol sweep ─────────────────────────────────────────
def run_symbol_sweep(profile: str) -> tuple[int | None, str, float]:
    repo = repo_root()
    script = repo / "tools" / "native_witness_symbol_sweep.sh"
    if not script.is_file():
        raise SystemExit(f"FATAL: sweep script missing: {script}")
    env = _env_for_drive()
    env["MOLT_DISCOVERY_PROFILE"] = profile
    t0 = time.monotonic()
    proc = subprocess.run(
        ["bash", str(script)], cwd=str(repo), env=env, capture_output=True, text=True
    )
    wall = time.monotonic() - t0
    out = (proc.stdout or "") + (proc.stderr or "")
    agg = None
    m = RE_AGG_GAP.search(out)
    if m:
        agg = int(m.group(1))
    return agg, out, wall


# ── Reserved final confirmation: full wasm witness with warm lowering cache ───
def run_wasm_confirm() -> int:
    """Run the full wasm witness for FINAL confirmation, with the warm
    frontend-lowering cache (a) wired + hit-rate attested.

    Heavy (~30 min). The inner native loop is the fast path; this is the reserved
    ground-truth. Set MOLT_WITNESS_CMD to the witness build/run command; the
    persistent shared lowering cache (MOLT_CACHE/module_lowering) makes a fresh
    session reuse unchanged numpy modules instead of re-lowering them.
    """
    repo = repo_root()
    witness_cmd = os.environ.get("MOLT_WITNESS_CMD")
    cache_root = os.environ.get(
        "MOLT_WITNESS_CACHE", str(repo / "target" / "witness-warm-cache")
    )
    ctx_log = str(repo / "target" / "witness-lowering-ctx.jsonl")
    Path(cache_root).mkdir(parents=True, exist_ok=True)
    env = _env_for_drive()
    # (a) Persistent, shared, content-addressed frontend-lowering cache: a fresh
    # witness session hydrates unchanged numpy modules from here (no re-lower).
    env["MOLT_CACHE"] = cache_root
    env.pop("MOLT_DISABLE_FRONTEND_LOWERING_CACHE", None)
    # Attest the warm hit-rate: the standing lowering-context profiler.
    env["MOLT_TRACE_LOWERING_CTX"] = ctx_log
    log(f"== wasm-confirm: warm lowering cache = {cache_root}")
    log(f"== wasm-confirm: lowering-ctx trace  = {ctx_log}")
    if not witness_cmd:
        log(
            "== NOTE: set MOLT_WITNESS_CMD to your witness build/run command to "
            "execute the full confirmation. The warm-cache env above is wired; "
            "re-run with MOLT_WITNESS_CMD set."
        )
        return 0
    log(f"== running witness: {witness_cmd}")
    proc = subprocess.run(witness_cmd, cwd=str(repo), env=env, shell=True)
    _report_lowering_hit_rate(ctx_log)
    return proc.returncode


def _report_lowering_hit_rate(ctx_log: str) -> None:
    p = Path(ctx_log)
    if not p.is_file():
        return
    total = hits = 0
    for line in p.read_text(encoding="utf-8", errors="replace").splitlines():
        try:
            rec = json.loads(line)
        except Exception:  # noqa: BLE001
            continue
        total += 1
        if rec.get("shared_hit") or rec.get("hit"):
            hits += 1
    if total:
        log(f"== warm lowering-cache hit rate: {hits}/{total} = {hits / total:.3f}")


# ── Reporting ─────────────────────────────────────────────────────────────────
def print_summary(
    fp: Fingerprint,
    verdict: Verdict,
    wall: float,
    relink: float | None,
    baseline: dict,
) -> None:
    log("")
    log("======================================================================")
    log(f"  FAST-WITNESS-ITER  —  module {fp.module}")
    log("======================================================================")
    log(f"  symbol GAP        : {fp.symbol_gap}  (known-good {baseline.get('symbol_gap')})")
    log(f"  PyInit outcome    : {'OK (module)' if fp.pyinit_ok else 'frontier (NULL/err)'}")
    log(f"  driver rc         : {fp.driver_rc}")
    log(f"  reached frontier  : {fp.reached_frontier}")
    log(f"  inner-loop wall   : {wall:.2f} s   (vs ~1800 s full wasm witness "
        f"=> ~{1800 / wall:.0f}x faster)")
    if relink is not None:
        log(f"  incremental relink: {relink:.2f} s   (one ABI object -> cdylib; "
            f"NOT a whole-runtime rebuild)")
    log("  ------------------------------------------------------------------")
    verdict_txt = "PASS" if verdict.passed else "RED"
    log(f"  VERDICT           : {verdict_txt}")
    for r in verdict.reasons:
        log(f"      - {r}")
    log("======================================================================")


def main(argv: list[str]) -> int:
    rc = maybe_dispatch_to_wsl(argv)
    if rc is not None:
        return rc

    ap = argparse.ArgumentParser(
        prog="witness_iter.py",
        description="FAST-WITNESS-ITER: seconds-per-frontier numpy-init inner loop.",
    )
    ap.add_argument("--module", default="_multiarray_umath",
                    help="extension module to drive (default: _multiarray_umath)")
    ap.add_argument("--profile", default=os.environ.get("MOLT_DISCOVERY_PROFILE", "dev"),
                    help="cargo profile for the harness (default: dev)")
    ap.add_argument("--record", action="store_true",
                    help="print the observed frontier signature to refresh the baseline")
    ap.add_argument("--symbols", action="store_true",
                    help="whole-witness static symbol sweep only (no PyInit)")
    ap.add_argument("--measure-relink", action="store_true",
                    help="also time an incremental ABI relink (evidences lever b)")
    ap.add_argument("--wasm-confirm", action="store_true",
                    help="reserved: run the full wasm witness with warm lowering cache")
    ap.add_argument("--baseline", default=None,
                    help="path to a JSON baseline overriding the committed defaults")
    ap.add_argument("--json", action="store_true", help="emit machine-readable result JSON")
    ap.add_argument("--in-wsl", action="store_true", help=argparse.SUPPRESS)
    args = ap.parse_args(argv)

    if platform.system() == "Windows":
        log("FATAL: native Windows lacks dlopen/RTLD_GLOBAL; use WSL/Linux/macOS.")
        return 2

    if args.wasm_confirm:
        return run_wasm_confirm()

    if args.symbols:
        agg, out, wall = run_symbol_sweep(args.profile)
        log(out)
        exp = DEFAULT_SYMBOL_SWEEP_MAX_GAP
        passed = agg is not None and agg <= exp
        log(f"== aggregate symbol GAP = {agg} (known-good <= {exp}); "
            f"{'PASS' if passed else 'RED'}  [{wall:.2f}s]")
        if args.json:
            print(json.dumps({"aggregate_symbol_gap": agg, "max": exp,
                              "passed": passed, "wall_s": wall}))
        return 0 if passed else 1

    # Inner native loop.
    baseline = DEFAULT_BASELINES.get(args.module, {})
    if args.baseline:
        baseline = json.loads(Path(args.baseline).read_text(encoding="utf-8")).get(
            args.module, baseline
        )

    fp, out, wall = run_native_drive(args.module, args.profile)
    log(out)

    relink = None
    if args.measure_relink:
        relink = measure_incremental_relink(args.profile)

    if args.record:
        observed = asdict(fp)
        log("")
        log("== OBSERVED frontier signature (use to refresh DEFAULT_BASELINES) ==")
        log(json.dumps(observed, indent=2))
        log(f"== inner-loop wall: {wall:.2f} s")
        if args.json:
            print(json.dumps({"observed": observed, "wall_s": wall}))
        return 0

    verdict = evaluate(fp, baseline)
    print_summary(fp, verdict, wall, relink, baseline)
    if args.json:
        print(json.dumps({
            "module": args.module,
            "fingerprint": asdict(fp),
            "verdict": {"passed": verdict.passed, "reasons": verdict.reasons},
            "inner_loop_wall_s": wall,
            "incremental_relink_s": relink,
        }))
    return 0 if verdict.passed else 1


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
