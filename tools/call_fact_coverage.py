#!/usr/bin/env python3
"""Call-site fact coverage — the scoreboard for the council's question #4
("what percentage of calls are direct, leaf, no-throw, no-alloc, inlinable?")
and the meter for the op-semantics / CallableTarget ladder (#70–#74, #71).

THE FINDING THIS TOOL MAKES MEASURABLE
--------------------------------------
A world-class compiler can answer "what fraction of call sites are leaf /
direct / inlinable / no-throw / noescape / typed-return?" because those facts
are RECORDED ON THE CALL SITE. molt computes most of them — but discards them
inside the pass that computed them. They are never attached to the call op, so:

  * backends cannot consume them (every call lowers through the generic helper),
  * no tool can measure their coverage, and
  * each is re-derived (or lost) by the next pass.

This tool makes that gap explicit and RATCHETS IT CLOSED: it tracks, per call
fact, whether the fact is ATTACHED to the call op (coverage-measurable, backend-
consumable), OPCODE_STATIC (a generated op_kinds fact), or TRANSIENT (computed-
and-discarded — the missing IR primitive). The `--check` gate fails if the count
of ATTACHED facts ever DECREASES (a fact may not silently un-attach) — a POSITIVE
ratchet, the dual of structural_audit.py's debt ratchet.

The single source of truth is `CALL_FACTS` below: each fact names where it is
computed today and what IR primitive would attach it. The tool VERIFIES that
evidence against the live tree, so the registry cannot rot silently.

MODES
-----
  call_fact_coverage.py                 census + per-opcode may_throw (no build)
  call_fact_coverage.py --json          machine-readable
  call_fact_coverage.py --corpus a.json [b.json ...]   typed-return % from
                                        typed_repr_report JSON dumps (needs a
                                        prior build to produce the dumps)
  call_fact_coverage.py --check         fail if ATTACHED-fact count regressed
  call_fact_coverage.py --update-baseline   re-pin the coverage baseline
"""

from __future__ import annotations

import argparse
import json
import sys
import tomllib
from dataclasses import dataclass, asdict, field
from pathlib import Path

ROOT_DEFAULT = Path(__file__).resolve().parents[1]
BASELINE_REL = "tools/call_fact_coverage_baseline.json"
OP_KINDS_REL = "runtime/molt-tir/src/tir/op_kinds.toml"

# Representation status of a call-site fact, in increasing order of usefulness.
ATTACHED = "ATTACHED"  # recorded on the call op → measurable + lowerable
OPCODE_STATIC = "OPCODE_STATIC"  # a generated op_kinds.toml fact (per-opcode)
TRANSIENT = "TRANSIENT"  # computed in a pass, then DISCARDED — the gap


@dataclass
class CallFact:
    key: str
    label: str
    status: str
    # evidence: a file + a stable SYMBOL the tool greps for (not a line number,
    # which rots) so the registry self-validates against the live tree.
    evidence_file: str
    evidence_symbol: str
    how_to_read: str
    missing_primitive: str
    evidence_ok: bool = field(default=True)


# THE CALL-FACT REGISTRY — single source of truth. Derived from the backend
# representation map (tir/call_graph.rs, passes/inliner.rs, passes/escape_analysis.rs,
# passes/effects.rs, op_kinds.toml, tir/types.rs). When a fact graduates from
# TRANSIENT to ATTACHED (someone records it on the call op via a CallFacts record),
# flip `status` here and the coverage ratchet rewards it.
CALL_FACTS: list[CallFact] = [
    CallFact(
        key="direct_target",
        label="direct target known (devirtualized)",
        status=ATTACHED,
        evidence_file="runtime/molt-tir/src/tir/call_graph.rs",
        evidence_symbol="StaticDirect",
        how_to_read="Call op carries `s_value` attr = static callee name; "
        "CallEdge::StaticDirect in the call graph",
        missing_primitive="(attached) — but as an attr string, not a typed "
        "CallableTarget; #71 makes it a typed variant",
    ),
    CallFact(
        key="typed_return",
        label="return Repr/TirType known (not DynBox)",
        status=ATTACHED,
        evidence_file="runtime/molt-tir/src/tir/types.rs",
        evidence_symbol="DynBox",
        how_to_read="result ValueId's TirType in function.value_types; observable "
        "in typed_repr_report JSON opcodes.call.result_reprs",
        missing_primitive="(attached) — measurable via --corpus",
    ),
    CallFact(
        key="no_throw",
        label="call proven not to raise",
        status=ATTACHED,
        evidence_file="runtime/molt-tir/src/tir/call_facts.rs",
        evidence_symbol="no_throw",
        how_to_read="CallFacts.no_throw (Proven iff opcode-static-no-throw ∨ "
        "resolved-callee-no-handlers ∨ allowlisted-builtin; else Unknown)",
        missing_primitive="(attached, CallFacts Phase 1a) — consumer: exception "
        "normal-edge fast path (ties doc 45) is the deferred 1b",
    ),
    CallFact(
        key="leaf",
        label="callee makes no further calls (leaf)",
        status=ATTACHED,
        evidence_file="runtime/molt-tir/src/tir/call_facts.rs",
        evidence_symbol="leaf",
        how_to_read="CallFacts.leaf (Proven/False from !makes_any_call for a "
        "resolved callee; Unknown for opaque) — now on the call site",
        missing_primitive="(attached, CallFacts Phase 1a) — consumer: frame-elision "
        "/ no-spill leaf-call lowering is a follow-up",
    ),
    CallFact(
        key="inlinable",
        label="callee eligible to inline",
        status=ATTACHED,
        evidence_file="runtime/molt-tir/src/tir/call_facts.rs",
        evidence_symbol="inlinable",
        how_to_read="CallFacts.inlinable (Eligible|WhyNot from classify_inline_"
        "eligibility — same gate is_inlineable reduces to; Unknown if opaque)",
        missing_primitive="(attached, CallFacts Phase 1a) — consumer: inliner reading "
        "the side-table instead of recomputing is the deferred 1b",
    ),
    CallFact(
        key="noescape_args",
        label="arguments do not escape",
        status=TRANSIENT,
        evidence_file="runtime/molt-tir/src/tir/passes/escape_analysis.rs",
        evidence_symbol="EscapeState",
        how_to_read="escape_analysis::analyze() yields per-ValueId EscapeState; "
        "the call's arg-escape summary is not attached to the call",
        missing_primitive="CallFacts.args_noescape mask — enables stack-promotion "
        "of arg temporaries + borrow-not-own arg passing",
    ),
    CallFact(
        key="no_alloc",
        label="call performs no heap allocation",
        status=TRANSIENT,
        evidence_file="runtime/molt-tir/src/tir/passes/escape_analysis.rs",
        evidence_symbol="StackAlloc",
        how_to_read="escape pass rewrites NoEscape Alloc→StackAlloc per value; "
        "there is no per-call 'callee allocates?' summary on the call",
        missing_primitive="CallFacts.no_alloc bit (callee alloc-free ∨ all results "
        "stack-promotable) — enables alloc-free call fast paths",
    ),
]


def _verify_evidence(root: Path) -> None:
    """Self-validation: each fact's cited symbol must still exist in its file, or
    the registry has rotted and the tool says so (never silently)."""
    for fact in CALL_FACTS:
        p = root / fact.evidence_file
        try:
            fact.evidence_ok = p.is_file() and fact.evidence_symbol in p.read_text(
                errors="replace"
            )
        except OSError:
            fact.evidence_ok = False


def _call_opcode_may_throw(root: Path) -> dict[str, bool]:
    """Per-opcode may_throw for the call opcodes, read from the authoritative
    registry (not hardcoded)."""
    data = tomllib.loads((root / OP_KINDS_REL).read_text())
    out: dict[str, bool] = {}
    for o in data.get("opcode", []):
        name = o.get("name", "")
        if any(k in name.lower() for k in ("call", "invoke", "ord_at")):
            out[name] = bool(o.get("may_throw", True))
    return out


# --- corpus coverage (typed-return %, from typed_repr_report JSON) ----------


def _corpus_typed_return(json_paths: list[Path]) -> dict:
    """Parse typed_repr_report JSON dumps; compute typed-return coverage for the
    call opcodes = (non-dynbox result reprs) / (total result reprs)."""
    call_ops = {"call", "call_method", "call_builtin"}
    total = 0
    typed = 0
    boxed = 0
    funcs_seen = 0
    for jp in json_paths:
        try:
            doc = json.loads(jp.read_text())
        except (OSError, json.JSONDecodeError) as e:
            print(f"WARN: skipping {jp}: {e}", file=sys.stderr)
            continue
        for fn in doc.get("functions", []):
            funcs_seen += 1
            opcodes = (fn.get("stats", {}) or {}).get("opcodes", {}) or {}
            for opname, st in opcodes.items():
                if opname.lower() not in call_ops:
                    continue
                reprs = st.get("result_reprs", {}) or {}
                for repr_name, n in reprs.items():
                    total += n
                    if repr_name.lower() not in ("dynbox", "dyn", "boxed"):
                        typed += n
                boxed += st.get("boxed_result_values", 0) or 0
    return {
        "functions": funcs_seen,
        "call_result_reprs_total": total,
        "call_result_reprs_typed": typed,
        "typed_return_pct": round(100.0 * typed / total, 1) if total else None,
        "boxed_result_values": boxed,
    }


def census(root: Path) -> dict:
    _verify_evidence(root)
    may_throw = _call_opcode_may_throw(root)
    attached = sum(1 for f in CALL_FACTS if f.status == ATTACHED)
    transient = sum(1 for f in CALL_FACTS if f.status == TRANSIENT)
    stale = [f.key for f in CALL_FACTS if not f.evidence_ok]
    return {
        "n_facts": len(CALL_FACTS),
        "attached": attached,
        "opcode_static": sum(1 for f in CALL_FACTS if f.status == OPCODE_STATIC),
        "transient": transient,
        "representation_coverage_pct": round(100.0 * attached / len(CALL_FACTS), 1),
        "stale_evidence": stale,
        "call_opcode_may_throw": may_throw,
        "facts": [asdict(f) for f in CALL_FACTS],
    }


def format_human(c: dict, corpus: dict | None) -> str:
    L = [
        "Call-site fact coverage (council Q4)",
        "=" * 60,
        f"call facts named: {c['n_facts']}   "
        f"ATTACHED (measurable+lowerable): {c['attached']}   "
        f"TRANSIENT (computed-and-discarded): {c['transient']}",
        f"call-site representation coverage: {c['representation_coverage_pct']}%  "
        f"(the missing IR primitive is a `CallFacts` record on the call op)",
        "",
        f"{'fact':<26} {'status':<13} {'where computed / how to read'}",
        "-" * 90,
    ]
    for f in c["facts"]:
        flag = "" if f["evidence_ok"] else "  ⚠STALE-EVIDENCE"
        L.append(
            f"{f['label'][:25]:<26} {f['status']:<13} "
            f"{Path(f['evidence_file']).name}:{f['evidence_symbol']}{flag}"
        )
        L.append(f"{'':<26} {'':<13} → missing: {f['missing_primitive'][:70]}")
    L.append("")
    L.append(
        "per-OPCODE may_throw (all call opcodes throw → no-throw MUST be a "
        "per-call-site fact, not an opcode fact):"
    )
    for op, mt in c["call_opcode_may_throw"].items():
        L.append(f"  {op:<14} may_throw={mt}")
    if corpus:
        L.append("")
        L.append("CORPUS (typed-return, from typed_repr_report JSON):")
        L.append(
            f"  functions={corpus['functions']}  "
            f"call result-reprs={corpus['call_result_reprs_total']}  "
            f"typed={corpus['call_result_reprs_typed']}  "
            f"typed_return={corpus['typed_return_pct']}%  "
            f"boxed_results={corpus['boxed_result_values']}"
        )
    if c["stale_evidence"]:
        L.append("")
        L.append(
            f"⚠ STALE EVIDENCE for facts {c['stale_evidence']} — the registry "
            "cites a symbol no longer in the tree; update CALL_FACTS."
        )
    return "\n".join(L)


def main(argv: list[str] | None = None) -> int:
    ap = argparse.ArgumentParser(
        description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter
    )
    ap.add_argument("--root", type=Path, default=ROOT_DEFAULT)
    ap.add_argument(
        "--corpus",
        type=Path,
        nargs="*",
        default=None,
        help="typed_repr_report JSON dump(s) for typed-return coverage",
    )
    ap.add_argument("--json", action="store_true")
    ap.add_argument(
        "--check",
        action="store_true",
        help="fail if ATTACHED-fact count regressed or evidence is stale",
    )
    ap.add_argument("--update-baseline", action="store_true")
    args = ap.parse_args(argv)

    root: Path = args.root.resolve()
    c = census(root)
    corpus = _corpus_typed_return(args.corpus) if args.corpus else None
    baseline_path = root / BASELINE_REL

    if args.update_baseline:
        payload = {"attached": c["attached"], "transient": c["transient"]}
        if corpus and corpus["typed_return_pct"] is not None:
            payload["typed_return_pct"] = corpus["typed_return_pct"]
        baseline_path.write_text(json.dumps(payload, indent=2, sort_keys=True) + "\n")
        print(f"baseline updated: {baseline_path}")
        return 0

    if args.json:
        out = {"census": c}
        if corpus:
            out["corpus"] = corpus
        print(json.dumps(out, indent=2))
        return 0

    if args.check:
        # evidence rot is always a failure (the census is lying about the tree).
        if c["stale_evidence"]:
            print(
                f"CALL-FACT EVIDENCE STALE: {c['stale_evidence']} — update "
                "CALL_FACTS in tools/call_fact_coverage.py",
                file=sys.stderr,
            )
            return 1
        if not baseline_path.is_file():
            print(
                f"ERROR: no baseline at {baseline_path}; run --update-baseline",
                file=sys.stderr,
            )
            return 2
        base = json.loads(baseline_path.read_text())
        if c["attached"] < base.get("attached", 0):
            print(
                f"CALL-FACT REPRESENTATION REGRESSED: attached "
                f"{base['attached']} -> {c['attached']} (a fact un-attached from "
                "the call op).",
                file=sys.stderr,
            )
            return 1
        if (
            corpus
            and "typed_return_pct" in base
            and corpus["typed_return_pct"] is not None
        ):
            if corpus["typed_return_pct"] + 0.05 < base["typed_return_pct"]:
                print(
                    f"TYPED-RETURN COVERAGE REGRESSED: "
                    f"{base['typed_return_pct']}% -> {corpus['typed_return_pct']}%",
                    file=sys.stderr,
                )
                return 1
        print(
            f"call-fact coverage OK (attached={c['attached']}/{c['n_facts']}, "
            f"evidence fresh)"
        )
        return 0

    print(format_human(c, corpus))
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
