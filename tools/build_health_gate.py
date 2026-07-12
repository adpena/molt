#!/usr/bin/env python3
"""Build-health anomaly gate — a deterministic hook that TRIGGERS attention.

WHY THIS EXISTS
---------------
Apparatus catches code metabugs mechanically instead of trusting attention; this
does the same for BUILD-HEALTH signals a human/orchestrator can stare past. Two
real anomalies sat unflagged in witness builds until the operator asked: the
runtime rebuilt TWICE per build (single-compile not routed to the app path), and
the frontend lowering cache was 19% effective (re-lowering 123/152 UNCHANGED numpy
modules, ~202s of pure waste). Both are "configured != effective" (M34) / redundant
-work smells. This gate reads the build diagnostics (+ the build log for the
rebuild count) and prints a LOUD, unmissable attention block when a health
invariant is violated — so the anomaly surfaces on every run instead of when
someone finally asks "why is this so slow?"

It WARNS loudly by default (perf anomalies are not correctness failures, and a
genuine cold first-run legitimately misses the cache). Pass --strict to exit
nonzero on a hard invariant (e.g. redundant runtime rebuilds, which should never
exceed 1 once single-compile is routed everywhere).

Usage:
    python tools/build_health_gate.py --diagnostics build_diagnostics.json [--log build.log] [--strict]
    python tools/build_health_gate.py --diagnostics warm.json --cold-diagnostics cold.json --strict
"""
from __future__ import annotations

import argparse
import json
from pathlib import Path
import re
import sys

try:
    import tomllib
except ModuleNotFoundError:  # pragma: no cover
    import tomli as tomllib  # type: ignore[no-redef]

_THRESHOLDS = Path(__file__).resolve().parent / "build_health_thresholds.toml"
_REBUILD_RE = re.compile(r"Runtime sources changed; rebuilding runtime")


class Anomaly:
    __slots__ = ("invariant", "observed", "expected", "waste", "hint", "hard")

    def __init__(self, invariant, observed, expected, waste, hint, hard):
        self.invariant = invariant
        self.observed = observed
        self.expected = expected
        self.waste = waste
        self.hint = hint
        self.hard = hard


def _load_thresholds() -> dict:
    if _THRESHOLDS.is_file():
        return tomllib.loads(_THRESHOLDS.read_text(encoding="utf-8"))
    return {}


def check(diag: dict, log_text: str | None, th: dict) -> list[Anomaly]:
    out: list[Anomaly] = []

    # 1. Redundant runtime rebuilds — a runtime .rs / link change should rebuild the
    #    runtime ONCE (single-compile serves reloc+shared). >1 = the app path isn't
    #    routed through _ensure_runtime_wasm_both (task #33).
    max_rebuilds = int(th.get("max_runtime_rebuilds", 1))
    rebuilds = None
    if log_text is not None:
        rebuilds = len(_REBUILD_RE.findall(log_text))
    if rebuilds is not None and rebuilds > max_rebuilds:
        out.append(Anomaly(
            "runtime_rebuilds", rebuilds, f"<= {max_rebuilds}", None,
            "the runtime was rebuilt from scratch more than once — single-compile is "
            "not routed to this build path (app split-runtime path, task #33). One "
            "cargo staticlib compile should serve both reloc + shared.",
            hard=True,
        ))

    # 2. Frontend lowering cache under-effective — re-lowering UNCHANGED modules.
    flc = diag.get("frontend_lowering_cache") or {}
    observed = int(flc.get("observed", 0) or 0)
    hit_rate = float(flc.get("hit_rate", 0.0) or 0.0)
    relowered_s = float(flc.get("relowered_s", 0.0) or 0.0)
    floor = float(th.get("frontend_lowering_cache_hit_floor", 0.80))
    min_observed = int(th.get("frontend_lowering_cache_min_observed", 40))
    waste_floor_s = float(th.get("relowered_waste_floor_s", 30.0))
    # Only alarm when there are enough modules AND real time was spent re-lowering
    # (a tiny/first build legitimately has a cold cache).
    if observed >= min_observed and hit_rate < floor and relowered_s >= waste_floor_s:
        out.append(Anomaly(
            "frontend_lowering_cache_hit_rate", f"{hit_rate:.2f} ({flc.get('hits')}/{observed})",
            f">= {floor:.2f}", f"{relowered_s:.0f}s re-lowered",
            "the frontend lowering cache is re-lowering unchanged modules (configured "
            "!= effective, M34). On a warm run where the module sources didn't change, "
            "hit_rate should approach 1.0. A cache-key input is still too sensitive.",
            hard=False,
        ))

    # 3. Optional: a single phase dominating total (correlates with the above).
    phase = diag.get("phase_sec") or {}
    total = float(diag.get("total_sec", 0.0) or 0.0)
    dom_frac = float(th.get("phase_dominance_frac", 0.55))
    if total > 0 and phase:
        top_name, top_val = max(phase.items(), key=lambda kv: kv[1])
        if top_val / total >= dom_frac and total >= float(th.get("phase_dominance_min_total_s", 120)):
            out.append(Anomaly(
                "phase_dominance", f"{top_name}={top_val:.0f}s ({top_val/total:.0%} of {total:.0f}s)",
                f"< {dom_frac:.0%}", None,
                f"one phase ({top_name}) dominates the build — a candidate for "
                "content-address caching if it re-runs on unchanged inputs.",
                hard=False,
            ))

    return out


def check_warm_pair(cold: dict, warm: dict, th: dict) -> list[Anomaly]:
    cold_cache = cold.get("frontend_lowering_cache") or {}
    warm_cache = warm.get("frontend_lowering_cache") or {}
    cold_observed = int(cold_cache.get("observed", 0) or 0)
    warm_observed = int(warm_cache.get("observed", 0) or 0)
    minimum = int(th.get("frontend_lowering_cache_min_observed", 40))
    floor = float(th.get("frontend_lowering_cache_warm_hit_floor", 0.90))
    anomalies: list[Anomaly] = []
    if cold_observed < minimum or warm_observed < minimum:
        anomalies.append(Anomaly(
            "frontend_lowering_cache_warm_sample_size",
            f"cold={cold_observed}, warm={warm_observed}",
            f"both >= {minimum}",
            None,
            "the effect-attestation pair is too small to prove frontend cache admission",
            hard=True,
        ))
        return anomalies
    if cold_observed != warm_observed:
        anomalies.append(Anomaly(
            "frontend_lowering_cache_warm_module_set",
            f"cold={cold_observed}, warm={warm_observed}",
            "identical observed module counts",
            None,
            "the cold and warm diagnostics do not attest the same module workload",
            hard=True,
        ))
    warm_rate = float(warm_cache.get("hit_rate", 0.0) or 0.0)
    if warm_rate < floor:
        anomalies.append(Anomaly(
            "frontend_lowering_cache_warm_hit_rate",
            f"{warm_rate:.3f} ({warm_cache.get('hits')}/{warm_observed})",
            f">= {floor:.2f}",
            f"{float(warm_cache.get('relowered_s', 0.0) or 0.0):.3f}s re-lowered",
            "a no-change warm rebuild must reuse nearly every module; configured-only caching is a CI failure",
            hard=True,
        ))
    return anomalies


def main(argv: list[str] | None = None) -> int:
    ap = argparse.ArgumentParser(description=__doc__)
    ap.add_argument("--diagnostics", type=Path, required=True)
    ap.add_argument("--cold-diagnostics", type=Path, default=None)
    ap.add_argument("--log", type=Path, default=None)
    ap.add_argument("--strict", action="store_true", help="exit nonzero on a hard invariant")
    args = ap.parse_args(argv)

    if not args.diagnostics.is_file():
        print(f"build_health_gate: no diagnostics at {args.diagnostics}", file=sys.stderr)
        return 3
    try:
        diag = json.loads(args.diagnostics.read_text(encoding="utf-8"))
    except (OSError, ValueError) as exc:
        print(f"build_health_gate: cannot read diagnostics: {exc}", file=sys.stderr)
        return 3
    log_text = None
    if args.log and args.log.is_file():
        log_text = args.log.read_text(encoding="utf-8", errors="replace")

    thresholds = _load_thresholds()
    anomalies = check(diag, log_text, thresholds)
    if args.cold_diagnostics is not None:
        try:
            cold_diag = json.loads(args.cold_diagnostics.read_text(encoding="utf-8"))
        except (OSError, ValueError) as exc:
            print(f"build_health_gate: cannot read cold diagnostics: {exc}", file=sys.stderr)
            return 3
        anomalies.extend(check_warm_pair(cold_diag, diag, thresholds))
    if not anomalies:
        print("build_health_gate: OK — no build-health anomalies")
        return 0

    hard = [a for a in anomalies if a.hard]
    bar = "!" * 72
    print(f"\n{bar}", file=sys.stderr)
    print("!! BUILD-HEALTH ATTENTION — redundant work / configured!=effective anomalies:",
          file=sys.stderr)
    for a in anomalies:
        tag = "HARD" if a.hard else "warn"
        print(f"\n  [{tag}] {a.invariant}: observed {a.observed}, expected {a.expected}",
              file=sys.stderr)
        if a.waste:
            print(f"        waste: {a.waste}", file=sys.stderr)
        print(f"        {a.hint}", file=sys.stderr)
    print(f"{bar}\n", file=sys.stderr)

    if args.strict and hard:
        return 2
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
