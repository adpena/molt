#!/usr/bin/env python3
"""Canonical molt subagent-dispatch contract, as NAMED CODE CONSTANTS (APPARATUS A7).

Every molt spawn prompt already needs the same blocks by convention (grounded
progress, land-or-blocker, verify-the-full-surface, web-authority, no-fakes, git
custody, build-env, distill, model-tier, triality wiring). When those blocks are
hand-typed per dispatch they DRIFT — a clause silently goes missing and the lane
inherits a gap. The concrete failure this module extinguishes: a recent lane
shipped a change verified only by a NARROW gate, not the full test surface,
because "verify the FULL test surface" was not in its prompt.

This module holds each block as a named constant, documents the M## directive it
mechanizes (see ``MECHANIZES``), and pins the verbatim-grade fragment each block
must keep (see ``KEY_PHRASES``). ``tools/dispatch_prompt.py`` COMPOSES spawn
prompts from these constants (a dispatcher composes, never re-types), and
``tools/check_subagent_contract.py`` is the anti-rot gate: it hardcodes the
required constant names so removing/renaming/blanking any of them fails the gate
(a removed constant cannot self-waive its own check).

Doctrine anchors: docs/agent/APPARATUS_FROM_COMMA_LAB.md sections A7 + 1.10, and
memory M05/M06/M12/M14/M17/M18/M19/M20/M23/M28/M33/M62/M63/M70.

Usage (dispatcher side)::

    from tools.subagent_contract import standard_contract
    prompt = f"{task_body}\n\n{standard_contract()}"

or, to pick a subset of sections::

    from tools.dispatch_prompt import compose
    prompt = compose(task_body, sections=["landing", "web", "git", "verify"])
"""

from __future__ import annotations

__all__ = [
    "BUILD_ENV",
    "CONTRACT_CONSTANT_NAMES",
    "DISTILL",
    "FRESH_CONTEXT_VERIFIER",
    "GIT_CUSTODY",
    "GROUNDED_PROGRESS",
    "KEY_PHRASES",
    "LANDING_CONTRACT",
    "MECHANIZES",
    "MODEL_TIER",
    "NO_ENDING_ON_PROMISES",
    "NO_FAKES",
    "TRIALITY_WIRING",
    "VERIFY_FULL_SUITE",
    "WEB_AUTHORITY",
    "standard_contract",
]

# --- The named contract blocks -------------------------------------------------------------
#
# Each constant is one self-contained instruction block. Keep them ASCII (molt runs on a
# Windows cp1252 console; M43's read/write encoding class extends to what we print) and keep
# the KEY_PHRASES fragment verbatim so the anti-rot gate can prove the block still says what
# it must.

#: The honest-status floor: never promote an unverified claim to a reported result.
#: Also the background-lane restate convention (the extractor reads only message text).
GROUNDED_PROGRESS = (
    "GROUNDED PROGRESS: before you report any result, audit each claim against a tool "
    "result from THIS session; report only what you can point to evidence for, and say "
    "'not yet verified' for anything you cannot. Restate outcomes in your own words even "
    "when a tool already printed them -- the reader saw none of the tool output."
)

#: M12 -- do not end a turn on a plan/promise; do the work or name a real external blocker.
NO_ENDING_ON_PROMISES = (
    "NO ENDING ON PROMISES (M12): before ending your turn, check your last paragraph. If "
    "it is a plan, a question, or a promise about undone work, do that work now with tool "
    "calls instead of ending -- or name the concrete external blocker. Reporting/planning/"
    "handing-off without landing is POISON."
)

#: M05 -- the exact recent miss: verify the FULL surface a change can touch, not one gate.
VERIFY_FULL_SUITE = (
    "VERIFY THE FULL SURFACE (M05): a change is verified only when you run the FULL test "
    "surface it can touch -- `cargo test -p <crate>` for every crate you edited (plus the "
    "differential/E2E path where behavior changed), NOT a single narrow gate that happens "
    "to pass. A green narrow gate over a broken full suite is the exact miss this contract "
    "exists to kill. A PASS is a hypothesis until you reproduce it."
)

#: M05 -- fresh-context verifier subagents beat self-critique; verify against the spec.
FRESH_CONTEXT_VERIFIER = (
    "FRESH-CONTEXT VERIFICATION (M05): independently verify every deliverable, including a "
    "subagent's -- a fresh-context verifier checking against the written specification beats "
    "self-critique from the recollection that produced the work. Reproduce the claimed PASS "
    "yourself before you rely on it."
)

#: M06 -- full web access; verify every spec/API against the PRIMARY source; cite.
WEB_AUTHORITY = (
    "WEB AUTHORITY (M06): you have full WebSearch/WebFetch. Verify every spec, API, ABI "
    "detail, or version claim against the PRIMARY source (the CPython/numpy/Cranelift/WASI "
    "docs or source, never from memory -- especially anything post-cutoff) and cite the "
    "source you checked. An uncited spec claim is an unverified claim."
)

#: M12 -- land a commit/proof/passing test via the queue every turn, or name a real blocker.
LANDING_CONTRACT = (
    "LANDING CONTRACT (M12): every work turn lands something real -- a commit, a proof, or a "
    "passing test via the queue -- or names a concrete external blocker. Land incrementally "
    "so a harness exit cannot lose the work; burn blockers down foundational-first. "
    "Report-without-landing does not count as a turn."
)

#: M05 -- zero fakes: no stubs, theater, weakened asserts, or unmeasured perf claims.
NO_FAKES = (
    "NO FAKES (M05, non-negotiable): zero stubs, zero theater, no weakened/commented-out "
    "asserts, no perf claim you did not measure. A gate that cannot fail certifies nothing; "
    "an assert you relaxed to get green is a silent regression. If you cannot do it for "
    "real this turn, say so and name the blocker instead of shipping a placeholder."
)

#: M17/M18/M19/M20/M23/M24 -- git custody: SSH push, pathspec commits, no destructive git,
#: work in your own worktree.
GIT_CUSTODY = (
    "GIT CUSTODY (M17/M18/M19/M20/M23): work in your OWN `git worktree` off latest "
    "origin/main -- prove `git rev-parse --show-toplevel` is not the shared checkout before "
    "any write. Commit with `git commit -m ... -- <pathspec>` (NEVER `git add -A`/`git add .` "
    "-- that sweeps a sibling lane's staged WIP). Push over SSH (git@github.com; the HTTPS "
    "cred lacks `workflow` scope). NEVER run destructive git (reset --hard / checkout -- / "
    "clean -fd / stash drop) on the shared checkout."
)

#: M14/M28/M29/M62/M66 -- build env: isolated target dir off C:, .venv python, native
#: cpython-abi tests don't set MOLT_SESSION_ID.
BUILD_ENV = (
    "BUILD ENV (M14/M28/M62): the molt CLI needs `.venv/Scripts/python.exe` (Py3.12; the "
    "host python lacks the deps). Route builds through an isolated CARGO_TARGET_DIR with "
    "artifacts OFF the C: drive (MOLT_EXTERNAL_ARTIFACT_ROOTS); the perf gate is `molt build "
    "--release` (default dev is unoptimized); differential is serial "
    "(`tests/molt_diff.py --jobs 1`). Native cpython-abi tests must NOT set MOLT_SESSION_ID."
)

#: The distill philosophy -- delete as much code as possible; the right primitive makes
#: swaths deletable; boxing is poison (M33 keep-vs-rip).
DISTILL = (
    "DISTILL: delete as much code as possible -- the win is the primitive that makes whole "
    "swaths deletable, not the clever addition. Boxing/indirection layered to paper over a "
    "missing primitive is poison; find the primitive instead. Distinguish poison (a fake or "
    "a mask over a primitive -> rip) from misplaced-but-valuable (right code, wrong layer -> "
    "git mv, never delete)."
)

#: M70 -- model-tier delegation: Fable orchestrates + does the hard novel work; Opus takes
#: hard-specified lanes; Sonnet takes mechanical/doc lanes.
MODEL_TIER = (
    "MODEL TIER (M70): Fable orchestrates and does the novel/hard work itself; delegate "
    "hard-but-specified lanes to Opus and mechanical/doc lanes to Sonnet. Keep concurrent "
    "Fable subagents at most one, very rarely -- over-spawning the top tier burns the token "
    "budget and re-trips the rate limit. Saturate independent P0s with concurrent lanes; "
    "never single-lane-and-sleep."
)

#: M63 -- triality: a real fix lands in all three legs together (a gate/registry + a
#: memory/ledger row alongside the code). Align with the Wave-2 A3 triality drift gate.
TRIALITY_WIRING = (
    "TRIALITY WIRING (M63): a finding is 'known' only when it agrees across all three legs "
    "-- the code/DAG, the gate/registry that enforces it, and the memory/ledger row that "
    "records it. Land the gate (or registry ratchet) and the ledger/CLAIMS row in the SAME "
    "commit batch as the code; a fix that moves only one leg is drift the A3 triality gate "
    "is built to catch."
)

# --- Registry (consumed by the anti-rot gate + the tests) ----------------------------------

#: Every named contract block this module guarantees. The anti-rot gate hardcodes the same
#: names independently so emptying THIS tuple cannot self-waive the gate.
CONTRACT_CONSTANT_NAMES: tuple[str, ...] = (
    "GROUNDED_PROGRESS",
    "NO_ENDING_ON_PROMISES",
    "VERIFY_FULL_SUITE",
    "FRESH_CONTEXT_VERIFIER",
    "WEB_AUTHORITY",
    "LANDING_CONTRACT",
    "NO_FAKES",
    "GIT_CUSTODY",
    "BUILD_ENV",
    "DISTILL",
    "MODEL_TIER",
    "TRIALITY_WIRING",
)

#: The M## directive(s) each constant mechanizes -- so every block documents its provenance
#: and the gate can prove no block lost its anchor.
MECHANIZES: dict[str, str] = {
    "GROUNDED_PROGRESS": "M05",
    "NO_ENDING_ON_PROMISES": "M12",
    "VERIFY_FULL_SUITE": "M05",
    "FRESH_CONTEXT_VERIFIER": "M05",
    "WEB_AUTHORITY": "M06",
    "LANDING_CONTRACT": "M12",
    "NO_FAKES": "M05",
    "GIT_CUSTODY": "M17/M18/M19/M20/M23",
    "BUILD_ENV": "M14/M28/M62",
    "DISTILL": "M33",
    "MODEL_TIER": "M70",
    "TRIALITY_WIRING": "M63",
}

#: The verbatim-grade fragment each block must keep. A block may be reworded, but if it drops
#: its key phrase it has lost its load-bearing instruction and the gate fails.
KEY_PHRASES: dict[str, str] = {
    "GROUNDED_PROGRESS": "audit each claim against a tool result",
    "NO_ENDING_ON_PROMISES": "check your last paragraph",
    "VERIFY_FULL_SUITE": "cargo test -p <crate>",
    "FRESH_CONTEXT_VERIFIER": "against the written specification",
    "WEB_AUTHORITY": "Verify every spec, API",
    "LANDING_CONTRACT": "every work turn lands something real",
    "NO_FAKES": "zero stubs, zero theater",
    "GIT_CUSTODY": "NEVER `git add -A`",
    "BUILD_ENV": "OFF the C: drive",
    "DISTILL": "delete as much code as possible",
    "MODEL_TIER": "Fable orchestrates",
    "TRIALITY_WIRING": "all three legs",
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
