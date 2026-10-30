#!/usr/bin/env python3
"""Bounded dispatch support; AGENTS.md owns standing project policy.

Routing blocks do not add proof, research or landing obligations. The composer
requires an outcome, deliverable, owners, write scope, proof commands, budget and
stop condition. Historical doctrine is documented in the owning apparatus doc.
"""

from __future__ import annotations

__all__ = [
    "BUILD_ENV",
    "CONTRACT_CONSTANT_NAMES",
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
    "TRIALITY_WIRING",
    "VERIFY_CHANGED_CONTRACT",
    "WEB_AUTHORITY",
    "standard_contract",
]

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
    "its findings. The parent owns integration and scope expansion."
)

VERIFY_CHANGED_CONTRACT = (
    "VERIFY THE CHANGED CONTRACT: use the assigned proof owner and commands. Reuse matching "
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
    "--repo <your checkout> --precheck-only` with the slot image. It runs the landing's static "
    "gates through main's controller and queues nothing. Run it and your changed tests from the "
    "landing runner's plain environment (`env -u CARGO_TARGET_DIR -u UV_PROJECT_ENVIRONMENT "
    "TMPDIR=<data-volume dir>`), never under a sourced lane env.sh: its exported selectors hide "
    "failures the runner sees. Fix every failure. Commit each green slice at once with exact "
    "paths; uncommitted work dies with the worker."
)

GIT_CUSTODY = (
    "GIT CUSTODY: use the assigned managed checkout and exact owned paths; preserve other work. "
    "NEVER `git add -A` or broadly stage, reset, clean or overwrite. Reuse existing WIP and "
    "lanes; the integration owner owns publication and retirement under AGENTS.md."
)

BUILD_ENV = (
    "BUILD ENV: resolve the supported CPython 3.12+ interpreter and child environment through "
    "canonical DX/toolchain authorities. Artifact placement must never change source, seal, "
    "worktree, or toolchain custody. Use the proof plan for actual profiles and prerequisites."
)

DISTILL = (
    "DISTILL: simplify the canonical mechanism when it advances the assigned outcome. Remove "
    "replaced consumers together; preserve valuable fast paths until measured. Do not pursue a "
    "deletion quota or adjacent cleanup at the expense of the critical path."
)

MODEL_TIER = (
    "MODEL AND CONCURRENCY: inherit the selected model and effort unless the operator or "
    "AGENTS.md directs otherwise. Respect operator concurrency limits. No recursive delegation, "
    "independent proof fanout or new integration lane without the parent assigning it."
)

TRIALITY_WIRING = (
    "KEEP AUTHORITIES ALIGNED: reconcile all three legs where applicable: implementation, "
    "existing proof authority and owning public contract or ledger. Update affected consumers; "
    "do not create a new gate, registry or ledger ritual for every finding."
)

PREMISE_VERIFICATION = (
    "PREMISE VERIFICATION: before your first Write, check the canonical owner, relevant history "
    "and preserved WIP for an existing solution. Reuse or reconcile it. Return ownership "
    "conflicts or new scope to the parent; do not open a competing lane."
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
    "HANDOFF_PRECHECK",
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
    "HANDOFF_PRECHECK": "M05/M12",
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
    "MODEL_TIER": "Respect operator concurrency limits",
    "TRIALITY_WIRING": "all three legs",
    "PREMISE_VERIFICATION": "before your first Write",
    "HANDOFF_PRECHECK": "--precheck-only",
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
