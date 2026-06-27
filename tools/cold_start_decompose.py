#!/usr/bin/env python3
"""Cold-start tax decomposition — localize the fixed startup cost (task #62).

The scoreboard's COLD-START BUDGET reds (the ~43 cold-only cells) are a FIXED
startup tax molt pays on every process launch that the warm steady-state does
not. This tool DECOMPOSES that tax into named components so the highest-leverage
one can be attacked — it MEASURES and LOCALIZES, it does NOT implement the
runtime fix (that is a Lane-A-adjacent arc).

Components (process launch -> first user instruction):

    process-launch/dyld   : kernel exec + dynamic-loader work BEFORE main().
                            Isolated with a no-op C binary (pure dyld+exec) and
                            cross-checked with DYLD_PRINT_STATISTICS.
    molt-runtime-init     : molt_runtime_init's 12 phases (the Rust core-init).
                            Measured directly from MOLT_TRACE_RUNTIME_INIT=1
                            per-phase microsecond timing.
    binary-page-in        : faulting the (large) linked binary's pages in. This
                            is the residual of (minimal-molt total) − (dyld) −
                            (runtime-init) and scales with binary SIZE.
    module-init           : eager stdlib module-init for whatever the program
                            imports. The minimal print()-only app pays the
                            essential floor; the delta to a json-importing app
                            isolates per-module init.

Method (council Lane C exit condition) — TWO path modes (this matters):
  * minimal print()-only molt binary  = the pure init floor (no user compute).
  * no-op C binary                    = pure dyld + process launch.
  * MOLT_TRACE_RUNTIME_INIT=1         = the molt_runtime_init phase ladder.
  * DYLD_PRINT_STATISTICS=1           = macOS dyld's own timing (cross-check).
  * SAME-PATH (repeated launches of one path): the REALISTIC repeated cold —
    macOS caches the code-signature validation after the first run, so this is
    the operating point an INSTALLED binary sees. Components attribute this tax.
  * FRESH-PATH (a fresh copy per sample): the WORST-CASE FIRST-EVER launch —
    a freshly-materialized UNSIGNED binary pays macOS codesign/Gatekeeper
    validation on every copy. (no-op C fresh − no-op C same-path) ISOLATES that
    one-time cost; it is reported separately, NOT summed into the realistic tax.

    A measured caveat (this host): the fresh-path codesign cost is ~86ms — large
    enough that a fresh-path-ONLY method (e.g. output_startup_size_audit's) over-
    attributes it to "dyld". The realistic same-path dyld floor is ~2ms. This
    tool reports both so the #62 target is the size-driven page-in lever molt
    controls, not a one-time install cost it does not.

Profiles: native release-fast AND release-output (the shipped artifact — the
council Y1 target is release-output startup_tax < 100ms).
"""

from __future__ import annotations

import argparse
import datetime as dt
import json
import os
import re
import shutil
import statistics
import sys
import tempfile
import time
from dataclasses import asdict, dataclass, field
from pathlib import Path

REPO_ROOT = Path(__file__).resolve().parents[1]
TOOLS_ROOT = REPO_ROOT / "tools"
SRC_ROOT = REPO_ROOT / "src"
for _p in (TOOLS_ROOT, SRC_ROOT):
    if str(_p) not in sys.path:
        sys.path.insert(0, str(_p))

import bench  # noqa: E402
import harness_memory_guard  # noqa: E402

SAFE_RUN = TOOLS_ROOT / "safe_run.py"
DOCS_COLD_START = REPO_ROOT / "docs" / "perf" / "COLD_START.md"

PERFSCORE_SESSION_ID = "perfscore"
COLD_START_GUARD_PREFIX = "MOLT_COLD_START"

# The molt_runtime_init phase ladder (runtime_state.rs trace_runtime_init).
# Order matters: each phase's delta is attributed to the named stage.
RUNTIME_INIT_PHASES = (
    "enter",
    "state_allocated",
    "runtime_reset_for_init",
    "intrinsics_registered",
    "serial_vtable",
    "itertools_vtable",
    "core_gil_vtable",
    "resources",
    "audit",
    "io_mode",
    "capabilities",
    "ok",
)

_TRACE_RE = re.compile(
    r"\[molt runtime_init\]\s+\+\s*(\d+)us\s+\(d\s*(\d+)us\)\s+(\S+)"
)


# ---------------------------------------------------------------------------
# Probe sources
# ---------------------------------------------------------------------------

MINIMAL_PY = "print('molt-cold-probe')\n"
# A program that imports + uses json, to isolate per-module init from the floor.
JSON_PY = "import json\nprint(len(json.dumps({'a': [1, 2, 3], 'b': 'x' * 16})))\n"
NOOP_C = "int main(void) { return 0; }\n"


@dataclass
class RunStat:
    median_s: float | None = None
    min_s: float | None = None
    n: int = 0
    samples_s: list[float] = field(default_factory=list)


def _canonical_env() -> dict[str, str]:
    base = os.environ.copy()
    base["MOLT_SESSION_ID"] = PERFSCORE_SESSION_ID
    return bench._canonical_bench_env(base)


def _guarded_text_process(
    cmd: list[str],
    *,
    env: dict[str, str],
    timeout_s: float,
    cwd: Path = REPO_ROOT,
) -> harness_memory_guard.GuardedCompletedProcess:
    return harness_memory_guard.guarded_completed_process(
        cmd,
        prefix=COLD_START_GUARD_PREFIX,
        cwd=cwd,
        env=env,
        capture_output=True,
        text=True,
        timeout=timeout_s,
    )


def _safe_run_elapsed(
    cmd: list[str],
    *,
    env: dict[str, str],
    rss_mb: int,
    timeout_s: float,
    label: str,
    extra_env: dict[str, str] | None = None,
) -> tuple[float | None, str]:
    """Run one process through safe_run; return (elapsed_s, captured_stderr)."""
    full = [
        sys.executable,
        str(SAFE_RUN),
        "--json",
        "--rss-mb",
        str(rss_mb),
        "--timeout",
        str(timeout_s),
        "--poll",
        "0.01",
        "--label",
        label,
        "--",
        *cmd,
    ]
    run_env = dict(env)
    if extra_env:
        run_env.update(extra_env)
    try:
        proc = _guarded_text_process(
            full,
            env=run_env,
            timeout_s=timeout_s + 30.0,
        )
    except OSError:
        return None, ""
    elapsed = None
    for line in reversed((proc.stderr or "").splitlines()):
        s = line.strip()
        if s.startswith("SAFE_RUN ") and s[9:].lstrip().startswith("{"):
            try:
                payload = json.loads(s[9:].lstrip())
                if payload.get("status") == "ok":
                    elapsed = payload.get("elapsed_s")
            except json.JSONDecodeError:
                pass
            break
    return (
        float(elapsed) if isinstance(elapsed, (int, float)) else None,
        proc.stderr or "",
    )


def _measure_binary(
    binary: Path,
    *,
    env: dict[str, str],
    samples: int,
    warmup: int,
    rss_mb: int,
    timeout_s: float,
    label: str,
    fresh_each: bool = True,
    extra_env: dict[str, str] | None = None,
) -> RunStat:
    """Median wall time over fresh-path copies (defeats macOS dyld/codesign cache).

    macOS caches dyld fixups + code-signature validation per binary PATH, which
    hides the true cold tax on repeated same-path launches. We copy the binary
    to a fresh path for each sample so every launch pays the cold cost — the
    same technique output_startup_size_audit uses.
    """
    fresh_dir = Path(tempfile.mkdtemp(prefix="coldstart-fresh-"))
    samples_s: list[float] = []
    try:
        idx = 0
        for _ in range(warmup):
            tgt = _fresh(binary, fresh_dir, idx) if fresh_each else binary
            idx += 1
            _safe_run_elapsed(
                [str(tgt)],
                env=env,
                rss_mb=rss_mb,
                timeout_s=timeout_s,
                label=f"{label}-warmup",
                extra_env=extra_env,
            )
        for _ in range(samples):
            tgt = _fresh(binary, fresh_dir, idx) if fresh_each else binary
            idx += 1
            elapsed, _ = _safe_run_elapsed(
                [str(tgt)],
                env=env,
                rss_mb=rss_mb,
                timeout_s=timeout_s,
                label=label,
                extra_env=extra_env,
            )
            if elapsed is not None:
                samples_s.append(elapsed)
    finally:
        shutil.rmtree(fresh_dir, ignore_errors=True)
    if not samples_s:
        return RunStat()
    return RunStat(
        median_s=round(statistics.median(samples_s), 6),
        min_s=round(min(samples_s), 6),
        n=len(samples_s),
        samples_s=[round(s, 6) for s in samples_s],
    )


def _fresh(binary: Path, fresh_dir: Path, idx: int) -> Path:
    tgt = fresh_dir / f"{binary.stem}_{idx}{binary.suffix}"
    shutil.copy2(binary, tgt)
    tgt.chmod(0o755)
    return tgt


# ---------------------------------------------------------------------------
# molt_runtime_init phase breakdown (MOLT_TRACE_RUNTIME_INIT=1)
# ---------------------------------------------------------------------------


def _measure_runtime_init_phases(
    binary: Path,
    *,
    env: dict[str, str],
    samples: int,
    rss_mb: int,
    timeout_s: float,
) -> dict[str, float]:
    """Per-phase median microseconds from the MOLT_TRACE_RUNTIME_INIT ladder.

    The trace prints ``[molt runtime_init] +<total>us (d<delta>us) <stage>`` on
    stderr. We collect the per-phase delta across runs and report the median
    per stage; the sum is the total molt_runtime_init cost.
    """
    per_phase: dict[str, list[int]] = {p: [] for p in RUNTIME_INIT_PHASES}
    trace_env = dict(env)
    trace_env["MOLT_TRACE_RUNTIME_INIT"] = "1"
    for _ in range(samples):
        _elapsed, stderr = _safe_run_elapsed(
            [str(binary)],
            env=env,
            rss_mb=rss_mb,
            timeout_s=timeout_s,
            label="runtime-init-trace",
            extra_env={"MOLT_TRACE_RUNTIME_INIT": "1"},
        )
        for m in _TRACE_RE.finditer(stderr):
            delta = int(m.group(2))
            stage = m.group(3)
            if stage in per_phase:
                per_phase[stage].append(delta)
    out: dict[str, float] = {}
    for stage, deltas in per_phase.items():
        if deltas:
            out[stage] = round(statistics.median(deltas) / 1000.0, 4)  # ms
    return out


# ---------------------------------------------------------------------------
# dyld timing (DYLD_PRINT_STATISTICS=1) + no-op C baseline
# ---------------------------------------------------------------------------


def _build_noop_c() -> Path | None:
    cc = shutil.which("cc") or shutil.which("clang")
    if cc is None:
        return None
    tmp = Path(tempfile.mkdtemp(prefix="coldstart-noopc-"))
    src = tmp / "noop.c"
    src.write_text(NOOP_C, encoding="utf-8")
    out = tmp / "noop"
    try:
        res = _guarded_text_process(
            [cc, "-O2", "-o", str(out), str(src)],
            env=os.environ.copy(),
            timeout_s=60,
        )
    except OSError:
        return None
    return out if res.returncode == 0 and out.exists() else None


def _parse_dyld_total_ms(stderr: str) -> float | None:
    """Extract dyld's reported 'total time' from DYLD_PRINT_STATISTICS output."""
    # Format: "  total time: 3.45 milliseconds (100.0%)"
    m = re.search(r"total time:\s+([\d.]+)\s+milliseconds", stderr)
    if m:
        return float(m.group(1))
    # Older dyld prints microseconds for sub-lines; total may be in seconds.
    m = re.search(r"total time:\s+([\d.]+)\s+seconds", stderr)
    if m:
        return float(m.group(1)) * 1000.0
    return None


def _measure_dyld_ms(
    binary: Path, *, env: dict[str, str], samples: int, timeout_s: float
) -> float | None:
    vals: list[float] = []
    for _ in range(samples):
        run_env = dict(env)
        run_env["DYLD_PRINT_STATISTICS"] = "1"
        try:
            proc = _guarded_text_process(
                [str(binary)],
                env=run_env,
                timeout_s=timeout_s,
            )
        except OSError:
            continue
        ms = _parse_dyld_total_ms(proc.stderr or "")
        if ms is not None:
            vals.append(ms)
    return round(statistics.median(vals), 4) if vals else None


# ---------------------------------------------------------------------------
# Build the probe binaries via the molt CLI
# ---------------------------------------------------------------------------


def _build_molt_probe(
    src_text: str, *, build_profile: str, stem: str, log: list[str]
) -> Path | None:
    """Compile a tiny .py probe to a native binary via the molt CLI."""
    work = Path(tempfile.mkdtemp(prefix=f"coldstart-{stem}-"))
    py = work / f"{stem}.py"
    py.write_text(src_text, encoding="utf-8")
    out_dir = work / "out"
    out_dir.mkdir()
    cmd = [
        *bench._molt_build_cmd(build_profile),
        "--target",
        "native",
        "--trusted",
        "--json",
        "--rebuild",
        "--out-dir",
        str(out_dir),
        str(py),
    ]
    env = _canonical_env()
    start = time.perf_counter()
    try:
        res = harness_memory_guard.guarded_completed_process(
            cmd,
            prefix=COLD_START_GUARD_PREFIX,
            env=env,
            capture_output=True,
            text=True,
            timeout=600.0,
        )
    except Exception as exc:  # noqa: BLE001
        log.append(f"{stem}: build exception {exc!r}")
        return None
    log.append(f"{stem}: build {time.perf_counter() - start:.1f}s rc={res.returncode}")
    if res.returncode != 0:
        log.append((res.stderr or res.stdout or "")[-600:])
        return None
    try:
        payload = json.loads((res.stdout or "{}").strip() or "{}")
    except json.JSONDecodeError:
        payload = {}
    out_str = payload.get("data", {}).get("output") or payload.get("output")
    candidates = [Path(out_str)] if out_str else []
    candidates += [
        p for p in out_dir.rglob("*") if p.is_file() and os.access(p, os.X_OK)
    ]
    for c in candidates:
        if c.exists() and os.access(c, os.X_OK):
            return c
    log.append(f"{stem}: no executable artifact found")
    return None


# ---------------------------------------------------------------------------
# Decomposition per profile
# ---------------------------------------------------------------------------


@dataclass
class ProfileBreakdown:
    profile: str
    # FRESH-PATH = a freshly-materialized copy per sample. Defeats the page
    # cache BUT pays macOS first-launch code-signature/Gatekeeper validation of
    # an unsigned binary every time — the WORST-CASE first-ever launch.
    minimal_total_fresh_ms: float | None = None
    noop_c_fresh_ms: float | None = None
    # SAME-PATH = repeated launches of one stable path. Signature validation is
    # cached after the first run, so this is the REALISTIC repeated-cold launch
    # of an installed binary (the operating point a deployed molt binary sees).
    minimal_total_samepath_ms: float | None = None
    noop_c_samepath_ms: float | None = None
    json_total_samepath_ms: float | None = None
    # Legacy aliases (= the realistic same-path numbers; the scoreboard's COLD
    # column is a single first-run, closest to same-path-first).
    minimal_total_ms: float | None = None
    json_total_ms: float | None = None
    noop_c_ms: float | None = None
    dyld_stat_ms: float | None = None
    minimal_binary_kib: float | None = None
    runtime_init_phases_ms: dict[str, float] = field(default_factory=dict)
    runtime_init_total_ms: float | None = None
    components_ms: dict[str, float] = field(default_factory=dict)
    notes: list[str] = field(default_factory=list)


def decompose_profile(
    profile: str,
    *,
    samples: int,
    warmup: int,
    rss_mb: int,
    timeout_s: float,
    noop_c: Path | None,
) -> ProfileBreakdown:
    build_flag = "release" if profile in ("release-fast", "release-output") else "dev"
    log: list[str] = []
    bd = ProfileBreakdown(profile=profile)
    env = _canonical_env()
    if profile == "release-output":
        env["MOLT_STDLIB_PROFILE"] = env.get("MOLT_STDLIB_PROFILE", "full")

    minimal = _build_molt_probe(
        MINIMAL_PY, build_profile=build_flag, stem="minimal", log=log
    )
    if minimal is None:
        bd.notes.append("minimal probe build failed; profile skipped")
        bd.notes.extend(log)
        return bd
    bd.minimal_binary_kib = round(minimal.stat().st_size / 1024, 1)

    json_bin = _build_molt_probe(
        JSON_PY, build_profile=build_flag, stem="json", log=log
    )

    # --- FRESH-PATH totals (worst-case first launch; incl macOS codesign) --
    minimal_fresh = _measure_binary(
        minimal,
        env=env,
        samples=samples,
        warmup=warmup,
        rss_mb=rss_mb,
        timeout_s=timeout_s,
        label="minimal-fresh",
        fresh_each=True,
    )
    bd.minimal_total_fresh_ms = (
        round(minimal_fresh.median_s * 1000, 3) if minimal_fresh.median_s else None
    )

    # --- SAME-PATH totals (realistic repeated cold; signature cached) ------
    minimal_same = _measure_binary(
        minimal,
        env=env,
        samples=samples,
        warmup=warmup,
        rss_mb=rss_mb,
        timeout_s=timeout_s,
        label="minimal-samepath",
        fresh_each=False,
    )
    bd.minimal_total_samepath_ms = (
        round(minimal_same.median_s * 1000, 3) if minimal_same.median_s else None
    )
    bd.minimal_total_ms = bd.minimal_total_samepath_ms  # realistic alias

    if json_bin is not None:
        json_same = _measure_binary(
            json_bin,
            env=env,
            samples=samples,
            warmup=warmup,
            rss_mb=rss_mb,
            timeout_s=timeout_s,
            label="json-samepath",
            fresh_each=False,
        )
        bd.json_total_samepath_ms = (
            round(json_same.median_s * 1000, 3) if json_same.median_s else None
        )
        bd.json_total_ms = bd.json_total_samepath_ms

    # --- dyld / process launch baselines (no-op C, both path modes) --------
    if noop_c is not None:
        noop_fresh = _measure_binary(
            noop_c,
            env=env,
            samples=samples,
            warmup=warmup,
            rss_mb=rss_mb,
            timeout_s=timeout_s,
            label="noop-c-fresh",
            fresh_each=True,
        )
        bd.noop_c_fresh_ms = (
            round(noop_fresh.median_s * 1000, 3) if noop_fresh.median_s else None
        )
        noop_same = _measure_binary(
            noop_c,
            env=env,
            samples=samples,
            warmup=warmup,
            rss_mb=rss_mb,
            timeout_s=timeout_s,
            label="noop-c-samepath",
            fresh_each=False,
        )
        bd.noop_c_samepath_ms = (
            round(noop_same.median_s * 1000, 3) if noop_same.median_s else None
        )
        bd.noop_c_ms = bd.noop_c_samepath_ms  # realistic alias
    bd.dyld_stat_ms = _measure_dyld_ms(
        minimal, env=env, samples=samples, timeout_s=timeout_s
    )

    # --- molt_runtime_init phase ladder ------------------------------------
    phases = _measure_runtime_init_phases(
        minimal, env=env, samples=max(samples, 5), rss_mb=rss_mb, timeout_s=timeout_s
    )
    bd.runtime_init_phases_ms = phases
    if phases:
        # 'enter' delta is ~0 (clock starts there); sum the real phases.
        bd.runtime_init_total_ms = round(
            sum(v for k, v in phases.items() if k != "enter"), 4
        )

    bd.components_ms = _attribute_components(bd)
    bd.notes.extend(log)
    return bd


def _attribute_components(bd: ProfileBreakdown) -> dict[str, float]:
    """Attribute the REALISTIC (same-path) cold total to named components.

    Two distinct macOS costs were conflated by a naive fresh-path-only method:

    macos-codesign-first-launch = (no-op C fresh − no-op C same-path). PURE
                            macOS first-launch overhead (code-signature /
                            Gatekeeper validation of a freshly-materialized
                            UNSIGNED binary). Paid ONCE per binary identity, NOT
                            on every launch of an installed/signed binary —
                            isolated on the no-op C so it carries NO molt or
                            binary-size signal. Reported separately, NOT summed
                            into the realistic same-path tax.
    process-launch/dyld   = no-op C SAME-PATH time (pure exec + dyld fixups,
                            signature cached). The true repeated-launch floor.
    molt-runtime-init     = the summed molt_runtime_init phase ladder.
    binary-page-in+entry+teardown = residual (minimal same-path − dyld −
                            runtime-init): faulting the linked binary's pages +
                            mimalloc init + entry-module setup + teardown. Scales
                            with binary SIZE — the #62 lever for an installed
                            binary.
    module-init (json)    = json same-path − minimal same-path (per-module init).
    """
    comp: dict[str, float] = {}
    total = bd.minimal_total_samepath_ms
    dyld = (
        bd.noop_c_samepath_ms if bd.noop_c_samepath_ms is not None else bd.dyld_stat_ms
    )
    rinit = bd.runtime_init_total_ms

    # The one-time codesign/Gatekeeper cost, isolated on the no-op C so it
    # carries no binary-size signal. Reported but NOT part of the same-path tax.
    if bd.noop_c_fresh_ms is not None and bd.noop_c_samepath_ms is not None:
        comp["macos-codesign-first-launch (one-time/install)"] = round(
            max(bd.noop_c_fresh_ms - bd.noop_c_samepath_ms, 0.0), 3
        )
    if dyld is not None:
        comp["process-launch/dyld"] = round(dyld, 3)
    if rinit is not None:
        comp["molt-runtime-init"] = round(rinit, 3)
    if total is not None:
        residual = total
        if dyld is not None:
            residual -= dyld
        if rinit is not None:
            residual -= rinit
        comp["binary-page-in+entry+teardown"] = round(max(residual, 0.0), 3)
    if (
        bd.json_total_samepath_ms is not None
        and bd.minimal_total_samepath_ms is not None
    ):
        comp["module-init (per json import)"] = round(
            max(bd.json_total_samepath_ms - bd.minimal_total_samepath_ms, 0.0), 3
        )
    return comp


# Components that are NOT part of the realistic repeated-launch tax (excluded
# from the "highest leverage for an installed binary" pick).
_NON_RECURRING_COMPONENTS = frozenset(
    {
        "module-init (per json import)",
        "macos-codesign-first-launch (one-time/install)",
    }
)


def _highest_leverage(breakdowns: list[ProfileBreakdown]) -> str:
    """The single component to attack for an INSTALLED binary (the #62 target).

    Excludes the one-time codesign cost (paid at install, not per launch) and
    the per-import module-init delta — the realistic repeated-launch tax is
    dyld + binary-page-in, and binary-page-in is the size-driven lever molt
    actually controls.
    """
    # Prefer release-output (the shipped artifact, council Y1 target).
    bd = next(
        (b for b in breakdowns if b.profile == "release-output" and b.components_ms),
        None,
    ) or next((b for b in breakdowns if b.components_ms), None)
    if bd is None:
        return "no breakdown available (probe builds failed)"
    items = [
        (k, v)
        for k, v in bd.components_ms.items()
        if k not in _NON_RECURRING_COMPONENTS
    ]
    if not items:
        return "no attributable components"
    k, v = max(items, key=lambda kv: kv[1])
    return (
        f"{k} = {v:.2f}ms ({bd.profile}) — the #62 attack target "
        "(size-driven; converges with the binary-size/tree-shaking arc)"
    )


# ---------------------------------------------------------------------------
# CLI
# ---------------------------------------------------------------------------


def main(argv: list[str]) -> int:
    ap = argparse.ArgumentParser(description="Cold-start tax decomposition (task #62).")
    ap.add_argument(
        "--profile",
        action="append",
        default=None,
        choices=["release-fast", "release-output", "dev-fast"],
        help="profile(s) to decompose (default: release-fast release-output)",
    )
    ap.add_argument("--samples", type=int, default=12)
    ap.add_argument("--warmup", type=int, default=2)
    ap.add_argument("--rss-mb", type=int, default=2048)
    ap.add_argument("--timeout", type=float, default=30.0)
    ap.add_argument("--out", default=None, help="output JSON path")
    ns = ap.parse_args(argv)

    profiles = ns.profile or ["release-fast", "release-output"]
    noop_c = _build_noop_c()
    if noop_c is None:
        print("[cold-start] WARNING: could not build no-op C baseline (no cc/clang)")

    breakdowns: list[ProfileBreakdown] = []
    for profile in profiles:
        print(f"[cold-start] decomposing {profile} ...", file=sys.stderr, flush=True)
        bd = decompose_profile(
            profile,
            samples=ns.samples,
            warmup=ns.warmup,
            rss_mb=ns.rss_mb,
            timeout_s=ns.timeout,
            noop_c=noop_c,
        )
        breakdowns.append(bd)

    doc = {
        "kind": "cold_start_decomposition",
        "generated_at": dt.datetime.now(dt.timezone.utc).isoformat(),
        "git_rev": bench._git_rev() or "unknown",
        "platform": sys.platform,
        "method": {
            "fresh_path": "fresh binary copy per sample — worst-case FIRST launch (incl macOS codesign/Gatekeeper of an unsigned binary)",
            "same_path": "repeated launches of one path — REALISTIC repeated cold (signature cached); the operating point of an installed binary",
            "minimal_probe": "print()-only molt binary = pure init floor",
            "noop_c": "process-launch + dyld baseline; (fresh - same) isolates the one-time codesign cost",
            "runtime_init_trace": "MOLT_TRACE_RUNTIME_INIT=1 per-phase microseconds",
            "dyld_statistics": "DYLD_PRINT_STATISTICS=1 dyld total time",
            "note": "the scoreboard COLD column is a single first-run; components attribute the SAME-PATH realistic tax, with codesign reported separately as one-time.",
        },
        "highest_leverage_component": _highest_leverage(breakdowns),
        "profiles": [asdict(b) for b in breakdowns],
    }
    out_path = (
        Path(ns.out)
        if ns.out
        else REPO_ROOT / "bench" / "scoreboard" / "cold_start_decomposition.json"
    )
    out_path.parent.mkdir(parents=True, exist_ok=True)
    out_path.write_text(json.dumps(doc, indent=2) + "\n", encoding="utf-8")
    _print_report(doc)
    print(f"\n[cold-start] JSON -> {out_path}", file=sys.stderr)
    return 0


def _print_report(doc: dict) -> None:
    print("\n" + "=" * 88)
    print("COLD-START TAX DECOMPOSITION (task #62)")
    print(f"git_rev={doc['git_rev']}  platform={doc['platform']}")
    print("=" * 88)
    for b in doc["profiles"]:
        print(
            f"\n[{b['profile']}]  minimal_binary={_fmt(b.get('minimal_binary_kib'))}KiB"
        )
        print(
            f"  SAME-PATH (realistic cold): minimal={_fmt(b.get('minimal_total_samepath_ms'))}ms  "
            f"json={_fmt(b.get('json_total_samepath_ms'))}ms  "
            f"noop_C={_fmt(b.get('noop_c_samepath_ms'))}ms"
        )
        print(
            f"  FRESH-PATH (1st launch, +codesign): minimal={_fmt(b.get('minimal_total_fresh_ms'))}ms  "
            f"noop_C={_fmt(b.get('noop_c_fresh_ms'))}ms"
        )
        comps = b.get("components_ms", {})
        if comps:
            print("  startup_tax_ms by component (REALISTIC same-path):")
            for k, v in sorted(comps.items(), key=lambda kv: -kv[1]):
                print(f"    {v:>8.3f} ms  {k}")
        ri = b.get("runtime_init_phases_ms", {})
        if ri:
            print(
                f"  molt_runtime_init total={_fmt(b.get('runtime_init_total_ms'))}ms; "
                "top phases:"
            )
            for k, v in sorted(ri.items(), key=lambda kv: -kv[1])[:5]:
                print(f"    {v:>8.4f} ms  {k}")
    print(f"\nHIGHEST-LEVERAGE COMPONENT: {doc['highest_leverage_component']}")
    print("=" * 88 + "\n")


def _fmt(v: float | None) -> str:
    return "-" if v is None else f"{v:.3f}"


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
