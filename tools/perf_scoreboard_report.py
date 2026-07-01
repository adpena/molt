#!/usr/bin/env python3
from __future__ import annotations

import json
import sys
from pathlib import Path

from perf_schema import (
    CLASS_DIMENSIONAL_WIN,
    CLASS_GREEN,
    CLASS_INFRA,
    CLASS_RED_NOISY,
    CLASS_RED_STABLE,
    CLASS_TIE,
    flatten_cells,
    validate_board,
    verdict_fails_gate,
)
from perf_scoreboard_model import (
    SCOREBOARD_DIR,
    VERDICT_BUILD_FAILED,
    VERDICT_CPY_INCOMPAT,
    VERDICT_FAIL_COLD_BUDGET,
    VERDICT_FAIL_ENGINE,
    VERDICT_FAIL_STALE,
    VERDICT_GREEN,
    VERDICT_RUN_BLOCKED,
    VERDICT_RUN_ERROR,
    VERDICT_UNSTABLE,
    VERDICT_WARN_COLD_FLOOR,
    Cell,
    PhaseStats,
    ScoreboardSchemaError,
    _VERDICT_DERIVED_NOTES,
    _budget_ms_for,
    _load_cold_start_budgets,
    _robust_cell_stable,
)


def _validate_board_for_emit(doc: dict, *, context: str) -> None:
    problems = validate_board(doc)
    if problems:
        raise ScoreboardSchemaError(context, problems)


def _write_scoreboard_doc(path: Path, doc: dict, *, context: str) -> None:
    _write_scoreboard_doc_atomic(path, doc, context=context)


def _write_scoreboard_doc_atomic(path: Path, doc: dict, *, context: str) -> None:
    _validate_board_for_emit(doc, context=context)
    path.parent.mkdir(parents=True, exist_ok=True)
    tmp = path.with_suffix(".tmp")
    tmp.write_text(json.dumps(doc, indent=2) + "\n", encoding="utf-8")
    tmp.replace(path)


def _print_schema_error(exc: ScoreboardSchemaError) -> None:
    print(f"[schema] {exc.context} FAILED:", file=sys.stderr)
    for problem in exc.problems:
        print(f"    - {problem}", file=sys.stderr)


def print_summary(doc: dict) -> None:
    cells = _flatten_cells(doc)
    by_verdict: dict[str, list[dict]] = {}
    for c in cells:
        by_verdict.setdefault(c.get("verdict", "pending"), []).append(c)

    # --- Authoritative-tree header (council ruling A + B) -------------------
    prov = doc.get("provenance", {})
    authoritative = prov.get("authoritative", True)
    print("\n" + "=" * 100)
    print("CPYTHON FLOOR SCOREBOARD — two-dimensional (warm ≠ cold)")
    print(
        f"  origin/main = {_short(prov.get('origin_sha'))}   "
        f"local HEAD = {_short(prov.get('local_head_sha'))}   "
        f"tool = {_short(prov.get('benchmark_tool_sha'))}"
    )
    print(
        f"  cpython={doc['host']['cpython_baseline']}  "
        f"pypy={doc['host'].get('pypy') or '-'}  "
        f"codon={doc['host'].get('codon') or '-'}"
    )
    # --- Quiescence line (#69 Rule 2) --------------------------------------
    q = prov.get("quiescence") or {}
    if q:
        if q.get("quiet"):
            print(
                f"  QUIESCENT: load={q.get('loadavg_1m')} (<= {q.get('loadavg_threshold')}"
                f", ncpu={q.get('ncpu')})  runnable={q.get('runnable_signal')}  "
                f"builds=0  thermal={'ok' if q.get('thermal_ok') else q.get('thermal_ok')}"
            )
        else:
            print(f"  *** NOT QUIESCENT: {'; '.join(q.get('reasons', []))} ***")
            if prov.get("require_quiescent"):
                print(
                    "      NON-AUTHORITATIVE: machine not quiet; do not optimize "
                    "from this red list (EXPLORATORY only)"
                )
    if authoritative:
        print(
            "  AUTHORITATIVE: tree == origin/main, clean, tool unmodified, machine quiescent"
        )
    else:
        print("  *** WARNING: benchmark is NON-AUTHORITATIVE ***")
        print(f"      reason: {prov.get('authoritative_reason', 'unknown')}")
    print("=" * 100)

    # --- Full table (verdict-ordered) --------------------------------------
    rank = {
        VERDICT_FAIL_STALE: 0,
        VERDICT_FAIL_ENGINE: 1,
        VERDICT_BUILD_FAILED: 1,
        VERDICT_RUN_ERROR: 1,
        VERDICT_UNSTABLE: 2,
        VERDICT_FAIL_COLD_BUDGET: 3,
        VERDICT_WARN_COLD_FLOOR: 4,
        VERDICT_RUN_BLOCKED: 5,
        VERDICT_CPY_INCOMPAT: 5,
        VERDICT_GREEN: 6,
    }
    cells.sort(
        key=lambda c: (
            rank.get(c.get("verdict"), 7),
            -(c.get("warm_speedup") or 0.0),
            c["benchmark"],
        )
    )
    hdr = (
        f"{'VERDICT':<17}{'WARM':>7}  {'COLD':>7}  {'TAXms':>7}  "
        f"{'PYPY':>6}  {'CODON':>6}  {'SIZEKiB':>8}  BENCHMARK [backend/profile]"
    )
    print(hdr)
    print("-" * 100)
    for c in cells:
        print(
            f"{c.get('verdict', '?'):<17}"
            f"{_fmt(c.get('warm_speedup')):>7}  "
            f"{_fmt(c.get('cold_speedup')):>7}  "
            f"{_fmt(c.get('startup_tax_ms'), 0):>7}  "
            f"{_fmt(c.get('pypy_ratio')):>6}  "
            f"{_fmt(c.get('codon_ratio')):>6}  "
            f"{_fmt(c.get('binary_size_kib'), 0):>8}  "
            f"{c['benchmark']} [{c['backend']}/{c['profile']}]"
        )
    print("-" * 100)

    s = doc["summary"]
    print(
        f"TOTAL={s['cells_total']}  GREEN={s['cells_green']}  "
        f"FAIL_ENGINE={s.get('cells_fail_engine', 0)}  "
        f"FAIL_COLD_BUDGET={s.get('cells_fail_cold_budget', 0)}  "
        f"WARN_COLD_FLOOR={s.get('cells_warn_cold_floor', 0)}  "
        f"UNSTABLE={s['cells_unstable']}  BUILD_FAIL={s['cells_build_failed']}  "
        f"RUN_ERROR={s['cells_error']}  CPY_INCOMPAT={s.get('cells_cpython_incompatible', 0)}  "
        f"STALE={s.get('cells_fail_stale', 0)}"
    )

    # --- WARM EXECUTION REDS (the release blockers — IR-fact lane) ---------
    warm_reds = by_verdict.get(VERDICT_FAIL_ENGINE, [])
    print(
        f"\nWARM EXECUTION REDS ({len(warm_reds)}) — execution-engine, RELEASE BLOCKER:"
    )
    print("  (needs an IR FACT, not a local opt — see council triage doctrine)")
    for c in sorted(warm_reds, key=lambda c: c.get("warm_speedup") or 0.0):
        print(
            f"    {_fmt(c.get('warm_speedup'))}x  {c['benchmark']} "
            f"[{c['backend']}/{c['profile']}]  -> {c.get('suspected_missing_fact', '?')}"
        )

    # --- COLD-START BUDGET REDS (startup/runtime/artifact lane) -------------
    cold_reds = by_verdict.get(VERDICT_FAIL_COLD_BUDGET, [])
    print(f"\nCOLD-START BUDGET REDS ({len(cold_reds)}) — startup tax over budget:")
    for c in sorted(cold_reds, key=lambda c: -(c.get("startup_tax_ms") or 0.0)):
        print(
            f"    cold={_fmt(c.get('cold_speedup'))}x  tax={_fmt(c.get('startup_tax_ms'), 0)}ms"
            f" (budget {_fmt(c.get('cold_budget_ms'), 0)}ms)  {c['benchmark']} "
            f"[{c['backend']}/{c['profile']}]  -> {c.get('suspected_startup_component', '?')}"
        )

    # --- WARN_COLD_FLOOR (cold<=1 but warm>1, tax within budget) -----------
    warn_cold = by_verdict.get(VERDICT_WARN_COLD_FLOOR, [])
    if warn_cold:
        print(
            f"\nCOLD-FLOOR WARNINGS ({len(warn_cold)}) — warm>CPython, cold<=CPython "
            "by FIXED startup tax (within budget; NOT a gate fail unless --strict-cold):"
        )
        for c in sorted(warn_cold, key=lambda c: -(c.get("startup_tax_ms") or 0.0))[
            :12
        ]:
            print(
                f"    cold={_fmt(c.get('cold_speedup'))}x  warm={_fmt(c.get('warm_speedup'))}x"
                f"  tax={_fmt(c.get('startup_tax_ms'), 0)}ms  {c['benchmark']} "
                f"[{c['backend']}/{c['profile']}]"
            )
        if len(warn_cold) > 12:
            print(
                f"    ... and {len(warn_cold) - 12} more (full list in JSON verdict_breakdown)"
            )

    # --- BACKEND ERRORS / NON-AUTHORITATIVE --------------------------------
    errs = (
        by_verdict.get(VERDICT_BUILD_FAILED, [])
        + by_verdict.get(VERDICT_RUN_ERROR, [])
        + by_verdict.get(VERDICT_UNSTABLE, [])
    )
    stale = by_verdict.get(VERDICT_FAIL_STALE, [])
    if errs or stale:
        print(f"\nBACKEND ERRORS / NON-AUTHORITATIVE ({len(errs) + len(stale)}):")
        for c in errs:
            origin_rerun = "yes" if not authoritative else "no"
            failure = _molt_failure_summary(c)
            print(
                f"    {c.get('verdict'):<16} {c['benchmark']} [{c['backend']}/{c['profile']}]"
                f"  stale?={'yes' if not authoritative else 'no'}  "
                f"origin_rerun_needed?={origin_rerun}"
                + (f"  ({c.get('note')})" if c.get("note") else "")
                + (f"  failure={failure}" if failure else "")
            )
        for c in stale[:5]:
            print(
                f"    FAIL_STALE       {c['benchmark']} [{c['backend']}/{c['profile']}]"
                "  stale?=yes  origin_rerun_needed?=yes"
            )
        if len(stale) > 5:
            print(
                f"    ... and {len(stale) - 5} more stale cells (whole board non-authoritative)"
            )

    # --- REGRESSIONS FROM LAST GREEN (filled by the baseline-diff caller) ---
    regressions = doc.get("_regressions_from_last_green")
    if regressions:
        print(f"\nREGRESSIONS FROM LAST GREEN ({len(regressions)}):")
        for m in regressions:
            print(f"    {m}")

    # --- GREENS WORTH PROTECTING (>2x — do not reopen a won class) ----------
    greens = by_verdict.get(VERDICT_GREEN, [])
    protected = sorted(
        (c for c in greens if (c.get("warm_speedup") or 0.0) > 2.0),
        key=lambda c: -(c.get("warm_speedup") or 0.0),
    )
    print(f"\nGREENS WORTH PROTECTING ({len(protected)}) — won classes, do NOT reopen:")
    for c in protected:
        print(
            f"    {_fmt(c.get('warm_speedup'))}x  {c['benchmark']} "
            f"[{c['backend']}/{c['profile']}]"
        )

    # --- 5-STATE CLASSIFICATION (#69 --classify) ---------------------------
    summary = doc.get("summary", {})
    if summary.get("classify_active"):
        by_class: dict[str, list[dict]] = {}
        for c in cells:
            cls = c.get("classification")
            if cls:
                by_class.setdefault(cls, []).append(c)
        cb = summary.get("classification_breakdown", {})
        print(
            f"\n5-STATE CLASSIFICATION (#69): RED_STABLE={len(cb.get(CLASS_RED_STABLE, []))}  "
            f"RED_NOISY={len(cb.get(CLASS_RED_NOISY, []))}  TIE={len(cb.get(CLASS_TIE, []))}  "
            f"GREEN={len(cb.get(CLASS_GREEN, []))}  "
            f"DIMENSIONAL_WIN={len(cb.get(CLASS_DIMENSIONAL_WIN, []))}  "
            f"INFRA={len(cb.get(CLASS_INFRA, []))}"
        )
        red_stable = sorted(
            by_class.get(CLASS_RED_STABLE, []),
            key=lambda c: c.get("warm_speedup") or 0.0,
        )
        print(
            f"\n  TRUE WARM REDS — RED_STABLE ({len(red_stable)}) "
            "[quiescent + stable + CI below 1.00 — the ONLY optimize-from set]:"
        )
        for c in red_stable:
            cp = c.get("cycle_profile") or {}
            top = cp.get("top_symbols") or []
            cyc = (
                f" -> CYCLES top: {top[0]['symbol']}"
                if top
                else (f" -> {cp.get('note')}" if cp.get("note") else "")
            )
            print(
                f"    {_fmt(c.get('warm_speedup'))}x  "
                f"CI=[{_fmt(c.get('repeat_ci_lo'))},{_fmt(c.get('repeat_ci_hi'))}]  "
                f"{c['benchmark']} [{c['backend']}/{c['profile']}]{cyc}"
            )
        noisy = by_class.get(CLASS_RED_NOISY, [])
        if noisy:
            print(
                f"\n  RED_NOISY ({len(noisy)}) — warm<1.00 but contaminated/"
                "unstable/CI-straddles — DO NOT optimize (re-measure quiet):"
            )
            for c in sorted(noisy, key=lambda c: c.get("warm_speedup") or 0.0)[:20]:
                print(
                    f"    {_fmt(c.get('warm_speedup'))}x  {c['benchmark']} "
                    f"[{c['backend']}/{c['profile']}]  ({c.get('classification_reason')})"
                )
        ties = by_class.get(CLASS_TIE, [])
        if ties:
            print(f"\n  TIE ({len(ties)}) — CI crosses 1.00 (neither win nor loss):")
            for c in sorted(ties, key=lambda c: c["benchmark"])[:20]:
                print(
                    f"    {_fmt(c.get('warm_speedup'))}x  {c['benchmark']} "
                    f"[{c['backend']}/{c['profile']}]"
                )
        dims = by_class.get(CLASS_DIMENSIONAL_WIN, [])
        if dims:
            print(
                f"\n  DIMENSIONAL_WIN ({len(dims)}) — Rule 4 (no warm flip, dimension improved):"
            )
            for c in dims:
                print(
                    f"    {c['benchmark']} [{c['backend']}/{c['profile']}]  "
                    f"({c.get('classification_reason')})"
                )

    # --- FASTEST NEXT UNLOCK -----------------------------------------------
    unlock = _fastest_next_unlock(warm_reds, cold_reds)
    print(f"\nFASTEST NEXT UNLOCK: {unlock}")

    if doc["benchmarks_deferred"]:
        print(f"\nDEFERRED / CPY-INCOMPATIBLE ({len(doc['benchmarks_deferred'])}):")
        for d in doc["benchmarks_deferred"][:8]:
            print(f"  - {d['benchmark']}: {d['reason']}")
        if len(doc["benchmarks_deferred"]) > 8:
            print(f"  ... and {len(doc['benchmarks_deferred']) - 8} more")
    print("=" * 100 + "\n")


def _short(sha: str | None) -> str:
    if not sha:
        return "-"
    return sha[:12]


def _fastest_next_unlock(warm_reds: list[dict], cold_reds: list[dict]) -> str:
    """One structural fact / one file lane / one gate — the highest-leverage next move.

    Prefer the WORST warm red (engine reds outrank cold reds per ruling A); a
    warm red the most benchmarks share is the fastest class to retire.
    """
    if warm_reds:
        worst = min(warm_reds, key=lambda c: c.get("warm_speedup") or 1e9)
        return (
            f"heal {worst['benchmark']} [{worst['backend']}/{worst['profile']}] "
            f"({_fmt(worst.get('warm_speedup'))}x) — fact: "
            f"{worst.get('suspected_missing_fact', '?')}"
        )
    if cold_reds:
        worst = max(cold_reds, key=lambda c: c.get("startup_tax_ms") or 0.0)
        return (
            f"cold-start: {worst['benchmark']} tax={_fmt(worst.get('startup_tax_ms'), 0)}ms "
            f"— attack {worst.get('suspected_startup_component', '?')}"
        )
    return "no reds — protect the greens; widen the suite for the next class"


def _molt_failure_summary(cell: dict) -> str | None:
    status = cell.get("molt_failure_status")
    detail = cell.get("molt_failure_detail")
    message = cell.get("molt_failure_message")
    parts = [str(value) for value in (status, detail) if value]
    if message:
        text = str(message).replace("\n", " ")
        parts.append(text[:160] + ("..." if len(text) > 160 else ""))
    return " | ".join(parts) if parts else None


def _flatten_cells(doc: dict) -> list[dict]:
    return [dict(cell) for cell in flatten_cells(doc)]


def _fmt(v: float | None, places: int = 2) -> str:
    if v is None:
        return "-"
    if places == 0:
        return f"{v:.0f}"
    return f"{v:.{places}f}"


def diff_against_baseline(
    doc: dict, baseline_path: Path
) -> tuple[list[str], list[str]]:
    """Return (newly_red, regressed_still_green) message lists vs a prior board."""
    try:
        prior = json.loads(baseline_path.read_text(encoding="utf-8"))
    except (OSError, json.JSONDecodeError) as exc:
        return ([f"baseline unreadable: {exc}"], [])

    prior_cells = {_cell_key(c): c for c in _flatten_cells(prior)}
    newly_red: list[str] = []
    regressed: list[str] = []
    for c in _flatten_cells(doc):
        key = _cell_key(c)
        old = prior_cells.get(key)
        if old is None:
            continue
        new_ratio = c.get("warm_speedup")
        old_ratio = old.get("warm_speedup")
        # "Newly gating" = a green-or-warn cell that became a hard gate fail.
        was_green = not verdict_fails_gate(str(old.get("verdict", "")))
        now_fails = verdict_fails_gate(
            str(c.get("verdict", "")),
            fail_stale=False,
        )
        if now_fails and was_green:
            newly_red.append(
                f"{key}: NEWLY {c.get('verdict', 'RED')}  "
                f"{_fmt(old_ratio)} -> {_fmt(new_ratio)}"
            )
        elif (
            new_ratio is not None
            and old_ratio is not None
            and not now_fails
            and new_ratio < old_ratio * 0.95  # >5% slower but still passing
        ):
            regressed.append(
                f"{key}: regressed-but-passing  {_fmt(old_ratio)} -> {_fmt(new_ratio)} "
                f"({(new_ratio / old_ratio - 1) * 100:+.1f}%)"
            )
    return newly_red, regressed


def _cell_key(c: dict) -> str:
    return f"{c['benchmark']} [{c['backend']}/{c['profile']}]"


def _latest_baseline(exclude: Path | None = None) -> Path | None:
    """The most recent committed board, EXCLUDING in-progress ``.partial.json``
    checkpoints and an optional explicit path (the board being written now).

    Without the ``.partial`` exclusion a diff would compare a board against its
    own mid-sweep checkpoint (a near-self-diff that hides every regression).
    """
    if not SCOREBOARD_DIR.exists():
        return None
    exclude_resolved = exclude.resolve() if exclude is not None else None
    candidates = [
        p
        for p in sorted(SCOREBOARD_DIR.glob("cpython_*.json"))
        if not p.name.endswith(".partial.json")
        and (exclude_resolved is None or p.resolve() != exclude_resolved)
    ]
    return candidates[-1] if candidates else None


def _gate_exit_code(
    doc: dict,
    *,
    no_gate: bool,
    strict_cold: bool = False,
    allow_nonauthoritative: bool = False,
) -> int:
    """The two-dimensional gate (council ruling A).

    Nonzero iff any FAIL_ENGINE / FAIL_COLD_BUDGET / BUILD_FAILED / RUN_ERROR /
    UNSTABLE. WARN_COLD_FLOOR fails ONLY with ``--strict-cold``. FAIL_STALE
    fails UNLESS ``--allow-nonauthoritative`` (local-debug opt-out). The single
    source of truth shared by run / merge / rebuild-summary.
    """
    if no_gate:
        return 0
    s = doc.get("summary", {})
    if s.get("gate_fails"):
        return 1
    if strict_cold and s.get("cells_warn_cold_floor", 0) > 0:
        return 1
    if (not allow_nonauthoritative) and s.get("cells_fail_stale", 0) > 0:
        return 1
    return 0


def _finalize_with_board_context(
    cells: list[Cell], doc_like: dict, *, allow_nonauthoritative: bool = False
) -> None:
    """Re-finalize stored cells using budgets + the board's own authoritative flag.

    For rebuild/merge we re-run the classifier so a stored board reflects the
    CURRENT verdict logic. The cold-start budget comes from the live budget
    file; the authoritative flag comes from the stored provenance (a stored
    board does not re-derive authoritativeness — it was already stamped).
    ``allow_nonauthoritative`` mirrors the run path: a non-authoritative board's
    cells classify on their REAL numbers (not FAIL_STALE) so a reader can
    re-derive verdicts for local analysis; the board's stored
    ``authoritative=false`` is untouched. We also RE-DERIVE ``stable`` from the
    stored per-runtime stats so a board measured by an older tool picks up the
    current robust-stability rule without re-running any benchmark.
    """
    budgets = _load_cold_start_budgets()
    stored_auth = doc_like.get("provenance", {}).get("authoritative", True)
    effective_auth = stored_auth or allow_nonauthoritative
    for cell in cells:
        # Drop any verdict-DERIVED note (FAIL_STALE / robustness) before
        # re-deriving so a stale note from a prior finalize (e.g. a board that
        # was once stamped FAIL_STALE) does not leak into the new verdict.
        if cell.note in _VERDICT_DERIVED_NOTES or (
            cell.note and cell.note.startswith("non-authoritative tree")
        ):
            cell.note = None
        _rederive_stability(cell)
        cell.finalize(
            budget_ms=_budget_ms_for(budgets, cell.backend, cell.profile),
            authoritative=effective_auth,
        )


def _rederive_stability(cell: Cell) -> None:
    """Recompute ``cell.stable`` from the stored molt/cpython stats dicts.

    ``finalize`` does not recompute stability (it is set at measurement time),
    so a rebuild-summary/merge must re-derive it to apply the current robust
    rule. No-op if the stored stats are absent.
    """
    if not cell.molt_stats or not cell.cpython_stats:
        return
    molt = _phasestats_from_dict(cell.molt_stats)
    cpy = _phasestats_from_dict(cell.cpython_stats)
    if molt is None or cpy is None:
        return
    cell.stable = _robust_cell_stable(molt, cpy)


def _phasestats_from_dict(d: dict) -> PhaseStats | None:
    import dataclasses

    if not isinstance(d, dict):
        return None
    known = {f.name for f in dataclasses.fields(PhaseStats)}
    return PhaseStats(**{k: v for k, v in d.items() if k in known})


def _proc_summary(procs: object) -> str:
    """One-line summary of a build-process list (pid:exe pairs)."""
    if not isinstance(procs, list) or not procs:
        return "0"
    parts = []
    for p in procs[:6]:
        if isinstance(p, dict):
            cmd = (p.get("cmd") or "").split()
            parts.append(f"{p.get('pid')}:{cmd[0] if cmd else '?'}")
    suffix = ", ".join(parts)
    extra = f" (+{len(procs) - 6} more)" if len(procs) > 6 else ""
    return f"{len(procs)} [{suffix}{extra}]"


def _print_provenance(provenance: dict) -> None:
    """Emit the FULL provenance block (#69 --print-provenance).

    Prints every field a reader needs to certify (or reject) a board's
    authority: the origin/candidate SHAs, dirty/daemon/cache identity, the
    cold/warm + repeat/variance posture, AND the council's NEW quiescence
    fields (``active_molt_processes``, ``active_cargo_or_rustc_processes``,
    ``loadavg_1m``, ``ncpu``, ``runnable_signal``). This is the human-auditable
    twin of the JSON provenance block; it does not re-measure anything.
    """
    p = provenance
    q = p.get("quiescence") or {}
    print("\n" + "=" * 100)
    print("PROVENANCE (full) — #69 measurement-hygiene block")
    print("=" * 100)
    # --- Tree / artifact identity (council ruling A) -----------------------
    print("  [tree identity]")
    print(f"    origin_sha (origin/main)     = {p.get('origin_sha')}")
    print(f"    candidate_sha (local HEAD)   = {p.get('local_head_sha')}")
    print(f"    merge_base_sha               = {p.get('merge_base_sha')}")
    print(f"    dirty_tree                   = {p.get('dirty_tree')}")
    print(f"    diverges_from_origin         = {p.get('diverges_from_origin')}")
    print(f"    benchmark_tool_sha (on-disk) = {p.get('benchmark_tool_sha')}")
    print(f"    benchmark_tool_last_commit   = {p.get('benchmark_tool_last_commit')}")
    print(f"    benchmark_tool_modified      = {p.get('benchmark_tool_modified')}")
    print("  [backend_binary_identity (daemon / stale-cache guard)]")
    bbi = p.get("backend_binary_identity") or {}
    if bbi:
        for lane, ident in sorted(bbi.items()):
            print(f"    {lane:<24} = {ident}")
    else:
        print("    (none recorded)")
    print(f"    stdlib_cache_key             = {p.get('stdlib_cache_key')}")
    # --- Quiescence (#69 Rule 2) — the NEW fields, named explicitly ---------
    print("  [quiescence (#69 Rule 2)]")
    print(f"    require_quiescent            = {p.get('require_quiescent')}")
    print(f"    quiescent                    = {p.get('quiescent')}")
    print(
        f"    active_molt_processes        = {_proc_summary(p.get('active_molt_processes'))}"
    )
    print(
        "    active_cargo_or_rustc_processes = "
        f"{_proc_summary(p.get('active_cargo_or_rustc_processes'))}"
    )
    print(f"    loadavg_1m                   = {p.get('loadavg_1m')}")
    print(f"    loadavg_threshold            = {q.get('loadavg_threshold')}")
    print(f"    ncpu                         = {p.get('ncpu')}")
    print(f"    runnable_signal              = {p.get('runnable_signal')}")
    print(
        f"    thermal_ok                   = {q.get('thermal_ok')}  ({q.get('thermal_note')})"
    )
    if q.get("reasons"):
        print(f"    NON-QUIET reasons            = {'; '.join(q.get('reasons', []))}")
    # --- Authority verdict --------------------------------------------------
    print("  [authority]")
    print(f"    authoritative                = {p.get('authoritative')}")
    print(f"    authoritative_reason         = {p.get('authoritative_reason')}")
    print("=" * 100)


def _checkpoint(
    path: Path,
    cells: list[Cell],
    benchmarks_run: list[str],
    benchmarks_deferred: list[dict],
    cpython_version: str,
    samples: int,
    warmup: int,
    *,
    provenance: dict | None = None,
    cpython_identity: dict | None = None,
    pypy_version: str | None = None,
    codon_version: str | None = None,
) -> None:
    from perf_scoreboard import build_scoreboard_doc

    doc = build_scoreboard_doc(
        cells,
        benchmarks_run=benchmarks_run,
        benchmarks_deferred=benchmarks_deferred,
        cpython_version=cpython_version,
        samples=samples,
        warmup=warmup,
        provenance=provenance,
        cpython_identity=cpython_identity,
        pypy_version=pypy_version,
        codon_version=codon_version,
    )
    _write_scoreboard_doc_atomic(path, doc, context=f"checkpoint {path}")
