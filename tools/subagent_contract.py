#!/usr/bin/env python3
"""Bounded dispatch support; AGENTS.md owns standing project policy.

Routing blocks do not add proof, research or landing obligations. The composer
requires an outcome, deliverable, owners, starting aperture, reserved paths,
extra proof commands, budget and stop condition. Historical doctrine is documented
in the owning apparatus doc.
"""

from __future__ import annotations

__all__ = [
    "BUILD_ENV",
    "CONTRACT_CONSTANT_NAMES",
    "DESIGN_REVIEW",
    "DISTILL",
    "EVIDENCE_REVIEW",
    "GIT_CUSTODY",
    "GROUNDED_PROGRESS",
    "HANDOFF_PRECHECK",
    "KEY_PHRASES",
    "LANDING_CONTRACT",
    "MECHANIZES",
    "MODEL_TIER",
    "NO_ENDING_ON_PROMISES",
    "NO_FAKES",
    "PREMISE_VERIFICATION",
    "PROJECT_IDENTITY",
    "TRIALITY_WIRING",
    "VERIFY_CHANGED_CONTRACT",
    "WEB_AUTHORITY",
    "WHOLE_CRATE_TESTS",
    "standard_contract",
]

# Mandatory identity is outside the selectable routing-block registry.
PROJECT_IDENTITY = """\
Molt is a world-class, early-stage ahead-of-time (AOT) compiler for Python, not an interpreter.
Build it the way leading compilers are built (LLVM and MLIR, Cranelift, rustc, GraalVM Native Image,
mypyc, Codon). Prove facts at compile and build time in the closed world (spec 0216); compiled code
calls the runtime only for work it cannot do itself.

Work the way an early-stage, world-class AOT compiler project works (Bun's early years, Zig, Go
2009-2012, early LLVM): before each decision, ask what such a project would do, and do it. Keep
trunk green and moving with small changes; delete instead of keeping compatibility; prefer
differential end-to-end tests over tests that pin an implementation; add process only for a
measured bottleneck.

Typing is our friend. Identity is typed, never spelled: runtime names are `NameId`; op kinds,
exceptions and intrinsics are generated closed types; facts live on typed SSA values. Never compare,
intern, allocate or look up a name that the compiler or the runtime can know: no
`attr_name_bits_from_bytes`, no byte or str literal of a registered name, no string-keyed fact. When
the typed form does not exist yet, add it at its one authority (for example the runtime name table)
and use it; do not spell the name.

Design data first. Prefer flat arrays indexed by dense typed ids over hash tables, indices over
pointers, structures of arrays over arrays of structures, and hot bits packed into the index or the
value word. A cache is a sign of a slow design: do not add one; redesign so the work is done once at
build time or is cheap enough to repeat. (Content-addressed build caching of whole artifacts is not
such a cache.)

On a hot path, measure instructions retired and runtime calls per iteration before and after."""

# --- The named contract blocks -------------------------------------------------------------
#
# Each constant is one self-contained instruction block. Keep them ASCII (molt runs on a
# Windows cp1252 console; M43's read/write encoding class extends to what we print) and keep
# the KEY_PHRASES fragment verbatim so the anti-rot gate can prove the block still says what
# it must.

GROUNDED_PROGRESS = (
    "GROUNDED PROGRESS: audit each claim against a tool result or matching retained evidence. "
    "Report the assigned outcome, evidence, failures and unknowns concisely; test counts and "
    "activity are not progress."
)

NO_ENDING_ON_PROMISES = (
    "COMPLETE THE ASSIGNMENT: check your last paragraph against the assigned deliverable and "
    "stop condition. Finish that deliverable and return it; a bounded audit or review ends with "
    "its findings. The parent owns integration and resolves reserved-path conflicts and "
    "unsettled semantic decisions."
)

VERIFY_CHANGED_CONTRACT = (
    "VERIFY THE CHANGED CONTRACT: use the assigned proof owner, standard proof families and "
    "listed extras. Reuse matching "
    "evidence; no automatic full-suite run, repeated PASS, benchmark or extra verifier. A "
    "changed input, new failure or uncovered obligation must justify a rerun. Mandatory landing "
    "and release gates remain required once."
)

EVIDENCE_REVIEW = (
    "EVIDENCE REVIEW: review the result against the written specification and its "
    "source/tool/environment/workload identities. Inspect existing receipts first. Independent "
    "review challenges the oracle and changed contract; it does not reproduce a matching PASS "
    "by default."
)

WEB_AUTHORITY = (
    "WEB AUTHORITY: use primary sources for uncertain, changing or load-bearing design facts "
    "needed by this task. Reuse applicable research and cite it. Broader surveys require a "
    "concrete unresolved decision on the critical path."
)

LANDING_CONTRACT = (
    "LANDING OWNERSHIP: the integration owner carries the authorized change through landing and "
    "retirement. Workers return their assigned artifact and evidence; read-only work does not "
    "create a commit, proof run or second lane merely to show progress."
)

NO_FAKES = (
    "NO FAKES: zero stubs, zero theater, weakened assertions, silent fallbacks or unmeasured "
    "performance claims. Solve the complete owned contract through its canonical authority. "
    "More tests cannot substitute for a coherent design."
)

HANDOFF_PRECHECK = (
    "HANDOFF PRECHECK: before reporting a commit ready, run `python -B tools/molt_dev.py land "
    "--repo <your checkout> --precheck-only` with the slot image. It applies the plan's "
    "repairs (committing any change), then runs the landing's static gates and its try run "
    "(changed-crate Clippy and whole-crate lib tests, changed tests and their direct readers) "
    "within the precheck budget "
    "through main's controller, and queues nothing; a budget refusal exits 1. Run it and your changed tests from the "
    "landing runner's plain environment (`env -u CARGO_TARGET_DIR -u UV_PROJECT_ENVIRONMENT "
    "TMPDIR=<data-volume dir>`), never under a sourced lane env.sh: its exported selectors hide "
    "failures the runner sees. Fix every failure. Commit each green slice at once with exact "
    "paths; uncommitted work dies with the worker."
)

GIT_CUSTODY = (
    "GIT CUSTODY: use the assigned managed checkout and exact owned paths; preserve other work. "
    "NEVER `git add -A` or broadly stage, reset, clean or overwrite. Reuse existing WIP and "
    "lanes; the integration owner owns publication and retirement under AGENTS.md. Generated "
    "outputs of sources you change are in your write scope; regenerate them with "
    "`tools/generators.py write`."
)

BUILD_ENV = (
    "BUILD ENV: resolve the supported CPython 3.12+ interpreter and child environment through "
    "canonical DX/toolchain authorities. Artifact placement must never change source, seal, "
    "worktree, or toolchain custody. Use the proof plan for actual profiles and prerequisites. "
    "Run every driver, probe and helper with the slot image's interpreter, never a system or "
    "bare `python3`; the repository's own Git hooks are exempt."
)

DISTILL = (
    "DISTILL: simplify the canonical mechanism when it advances the assigned outcome. Remove "
    "replaced consumers together; preserve valuable fast paths until measured. Do not pursue a "
    "deletion quota or adjacent cleanup at the expense of the critical path."
)

MODEL_TIER = (
    "MODEL AND DELEGATION: inherit the selected model and effort unless the operator or "
    "AGENTS.md directs otherwise. Never delegate: spawn no subagent, fork, Codex session or "
    "other agent, and hand no part of the outcome to another worker. You own the whole "
    "outcome; only the integrator dispatches. Open no independent proof fanout and no new "
    "integration lane."
)

TRIALITY_WIRING = (
    "KEEP AUTHORITIES ALIGNED: reconcile all three legs where applicable: implementation, "
    "existing proof authority and owning public contract or ledger. Update affected consumers; "
    "do not create a new gate, registry or ledger ritual for every finding."
)

PREMISE_VERIFICATION = (
    "PREMISE VERIFICATION: before your first Write, check the canonical owner, relevant history "
    "and preserved WIP for an existing solution. Reuse or reconcile it. Return reserved-path "
    "conflicts or unsettled semantic decisions to the parent; do not open a competing lane."
)

DESIGN_REVIEW = (
    "DESIGN REVIEW (Elephant Goldfish Model): the egm plugin from "
    "github.com/google/elephant-goldfish-model is the authority for the method; read its files, "
    "not a summary of them. A Goldfish review needs fresh-context reviewer agents, and workers "
    "spawn none, so the integrator runs it. Before implementation, write artifacts/design.md in "
    "your lane against the plugin's resources/document_standards/feature-spec-standard.md: the "
    "measured problem, prior art, the mechanism, its invariants, the test plan and the consumers "
    "it changes, meaning every hand-written list of sibling modules (proof-plan authority inputs "
    "and globs, capture and closure lists, fingerprint role tables), every test fake of a changed "
    "boundary or record, every reader of a new or newly set environment variable, every registry "
    "and generated projection, and any existing runner, cell or tool that already does the job. "
    "List those consumer files in a GoldfishContext section. design.md also carries a skeleton "
    "in the source language (Skeleton Coding, arXiv 2610.38214): signature stubs with "
    "one-paragraph docstrings and placeholder bodies, Requires/Ensures/Raises contracts on the "
    "high-complexity callables, and numbered global invariants cited from those docstrings. "
    "Structural requirements, such as one authority or no duplicate implementation, belong in "
    "those invariants and their generated checks, because output tests cannot see them. Keep "
    "the skeleton short: it states only the decisions that bind the implementation. Commit "
    "design.md, report, and stop: the integrator runs egm-review and egm-resolve on it, "
    "skeleton included, and returns the settled findings and skeleton as a dated Correction in "
    "this brief; you implement against that and keep its types, signatures and owners. A "
    "one-line fix needs no "
    "design.md, and a brief that names a design whose Goldfish round is already recorded is "
    "implemented against that design: write no new design.md and do not stop for review. "
    "After implementation the integrator runs egm-drift-report against the design."
)

WHOLE_CRATE_TESTS = (
    "WHOLE-CRATE TESTS: before reporting ready, run the whole library tests of every touched "
    "Rust crate and its reverse dependencies, `--lib --no-fail-fast`, in each profile and "
    "feature cell the proof plan declares for it: `python -B tools/check_changed_crate_tests.py "
    "--base origin/main` computes and runs them, and the handoff precheck runs it too. A "
    "focused test filter is not this proof, and a declared crate that runs 0 tests fails. When "
    "a root Cargo input reaches the whole workspace, say so and leave the full run to the queue."
)

# --- Registry (consumed by the anti-rot gate + the tests) ----------------------------------

CONTRACT_CONSTANT_NAMES: tuple[str, ...] = (
    "GROUNDED_PROGRESS",
    "NO_ENDING_ON_PROMISES",
    "VERIFY_CHANGED_CONTRACT",
    "EVIDENCE_REVIEW",
    "WEB_AUTHORITY",
    "LANDING_CONTRACT",
    "NO_FAKES",
    "GIT_CUSTODY",
    "BUILD_ENV",
    "DISTILL",
    "MODEL_TIER",
    "TRIALITY_WIRING",
    "PREMISE_VERIFICATION",
    "DESIGN_REVIEW",
    "HANDOFF_PRECHECK",
    "WHOLE_CRATE_TESTS",
)

MECHANIZES: dict[str, str] = {
    "GROUNDED_PROGRESS": "M05",
    "NO_ENDING_ON_PROMISES": "M12",
    "VERIFY_CHANGED_CONTRACT": "M05",
    "EVIDENCE_REVIEW": "M05",
    "WEB_AUTHORITY": "M06",
    "LANDING_CONTRACT": "M12",
    "NO_FAKES": "M05",
    "GIT_CUSTODY": "M17/M18/M19/M20/M23",
    "BUILD_ENV": "M14/M28/M62",
    "DISTILL": "M33",
    "MODEL_TIER": "M70",
    "TRIALITY_WIRING": "M63",
    "PREMISE_VERIFICATION": "M22/M24",
    "DESIGN_REVIEW": "M22/M24 (owner 2026-11-04: Elephant Goldfish review before implementation)",
    "HANDOFF_PRECHECK": "M05/M12",
    "WHOLE_CRATE_TESTS": "M05 (HF-701: HF-205 and HF-764 regressions hid behind focused tests)",
}

KEY_PHRASES: dict[str, str] = {
    "GROUNDED_PROGRESS": "audit each claim against a tool result",
    "NO_ENDING_ON_PROMISES": "check your last paragraph",
    "VERIFY_CHANGED_CONTRACT": "Reuse matching evidence",
    "EVIDENCE_REVIEW": "against the written specification",
    "WEB_AUTHORITY": "primary sources",
    "LANDING_CONTRACT": "integration owner",
    "NO_FAKES": "zero stubs, zero theater",
    "GIT_CUSTODY": "NEVER `git add -A`",
    "BUILD_ENV": "never change source, seal, worktree, or toolchain custody",
    "DISTILL": "advances the assigned outcome",
    "MODEL_TIER": "Never delegate",
    "TRIALITY_WIRING": "all three legs",
    "PREMISE_VERIFICATION": "before your first Write",
    "DESIGN_REVIEW": "Elephant Goldfish",
    "HANDOFF_PRECHECK": "--precheck-only",
    "WHOLE_CRATE_TESTS": "--lib --no-fail-fast",
}


def standard_contract(names: tuple[str, ...] | None = None) -> str:
    """Compose the contract text for a dispatcher prompt.

    Args:
        names: the constant names to include, in the order given (deduped). ``None`` (the
            default) composes every block in :data:`CONTRACT_CONSTANT_NAMES` order.

    Returns:
        The selected blocks joined by blank lines. Raises ``KeyError`` if a requested name
        is not a known contract constant (a typo must fail loud, never silently drop a block).
    """
    if names is None:
        names = CONTRACT_CONSTANT_NAMES
    seen: set[str] = set()
    blocks: list[str] = []
    for name in names:
        if name in seen:
            continue
        if name not in CONTRACT_CONSTANT_NAMES:
            raise KeyError(
                f"{name!r} is not a contract constant; known: {CONTRACT_CONSTANT_NAMES}"
            )
        seen.add(name)
        blocks.append(globals()[name])
    return "\n\n".join(blocks)
