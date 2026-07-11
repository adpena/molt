#!/usr/bin/env python3
"""Compose a molt subagent spawn prompt from the canonical contract (APPARATUS A7).

The orchestrator composes spawn prompts from ``tools/subagent_contract.py`` GOING
FORWARD -- it does not hand-type the standing directives per dispatch (hand-typed
prompts drift; a drifted prompt silently drops a clause such as "verify the FULL
test surface", which is the exact miss that motivated this apparatus). This tool
makes composing the path of least resistance: a task body plus the named contract
blocks you select by short section key.

Import of the real contract module is REQUIRED -- if it cannot be imported this
tool fails LOUD rather than emitting a fallback prompt, because a silent fallback
would reintroduce the drift class the contract exists to extinguish.

Section keys (map to the named constants in subagent_contract):

    grounded    -> GROUNDED_PROGRESS
    promises    -> NO_ENDING_ON_PROMISES
    verify      -> VERIFY_FULL_SUITE + FRESH_CONTEXT_VERIFIER
    web         -> WEB_AUTHORITY
    landing     -> LANDING_CONTRACT
    fakes       -> NO_FAKES
    git         -> GIT_CUSTODY
    build       -> BUILD_ENV
    distill     -> DISTILL
    model-tier  -> MODEL_TIER
    triality    -> TRIALITY_WIRING
    preflight   -> PREMISE_VERIFICATION
    all         -> every block, in canonical order (the default)

Usage:
    python tools/dispatch_prompt.py --task-file task.md --sections landing,web,git,verify
    python tools/dispatch_prompt.py --task-text "Build X ..."           # all sections
    echo "Build X ..." | python tools/dispatch_prompt.py --sections git,build,verify
"""

from __future__ import annotations

import argparse
import sys
from pathlib import Path

_REPO = Path(__file__).resolve().parents[1]


def _import_contract():
    """Import the real contract module, whether run as a script or under pytest."""
    try:
        from tools import subagent_contract
    except ImportError:
        # Run as `python tools/dispatch_prompt.py`: sys.path[0] is tools/, not the repo
        # root. Put the repo root on the path and retry -- then let any failure propagate
        # LOUD (a missing contract module must never degrade to a hand-typed fallback).
        sys.path.insert(0, str(_REPO))
        from tools import subagent_contract
    return subagent_contract


#: Short section key -> the ordered constant names it expands to.
SECTIONS: dict[str, tuple[str, ...]] = {
    "grounded": ("GROUNDED_PROGRESS",),
    "promises": ("NO_ENDING_ON_PROMISES",),
    "verify": ("VERIFY_FULL_SUITE", "FRESH_CONTEXT_VERIFIER"),
    "web": ("WEB_AUTHORITY",),
    "landing": ("LANDING_CONTRACT",),
    "fakes": ("NO_FAKES",),
    "git": ("GIT_CUSTODY",),
    "build": ("BUILD_ENV",),
    "distill": ("DISTILL",),
    "model-tier": ("MODEL_TIER",),
    "triality": ("TRIALITY_WIRING",),
    "preflight": ("PREMISE_VERIFICATION",),
}


def _resolve_sections(sections: list[str] | None, sc) -> tuple[str, ...]:
    """Turn a list of section keys into an ordered, deduped list of constant names.

    ``None`` or ``["all"]`` selects every block in canonical order. Otherwise each key is
    expanded via :data:`SECTIONS`; an unknown key fails loud (never silently drops a block).
    Regardless of the order the keys were given, blocks come out in canonical order so the
    composed prompt is stable across dispatchers.
    """
    if sections is None or sections == ["all"] or "all" in (sections or ()):
        return sc.CONTRACT_CONSTANT_NAMES
    wanted: set[str] = set()
    for key in sections:
        key = key.strip()
        if not key:
            continue
        if key not in SECTIONS:
            raise KeyError(
                f"unknown section {key!r}; known: {sorted(SECTIONS)} (or 'all')"
            )
        wanted.update(SECTIONS[key])
    # Emit in canonical order for determinism.
    return tuple(n for n in sc.CONTRACT_CONSTANT_NAMES if n in wanted)


def compose(task: str, *, sections: list[str] | None = None) -> str:
    """Assemble a spawn prompt: the task body followed by the selected contract blocks.

    Args:
        task: the task-specific body (what THIS lane must build/verify/land).
        sections: short section keys to include (see :data:`SECTIONS`); ``None`` includes
            every block in canonical order.

    Returns:
        ``"<task>\\n\\n<block>\\n\\n<block>..."``. Raises ``KeyError`` on an unknown section.
    """
    sc = _import_contract()
    names = _resolve_sections(sections, sc)
    contract = sc.standard_contract(names)
    body = task.rstrip()
    return f"{body}\n\n{contract}" if contract else body


def main(argv: list[str] | None = None) -> int:
    ap = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    src = ap.add_mutually_exclusive_group()
    src.add_argument("--task-file", type=Path, help="file holding the task body")
    src.add_argument("--task-text", help="task body inline (default: read stdin)")
    ap.add_argument(
        "--sections",
        default="all",
        help="comma-separated section keys (default: all). e.g. landing,web,git,verify",
    )
    ap.add_argument(
        "--list-sections",
        action="store_true",
        help="print the section keys and the constants each expands to, then exit",
    )
    args = ap.parse_args(argv)

    if args.list_sections:
        for key in SECTIONS:
            print(f"{key:12s} -> {', '.join(SECTIONS[key])}")
        print(f"{'all':12s} -> every block, canonical order")
        return 0

    if args.task_file is not None:
        task = args.task_file.read_text(encoding="utf-8")
    elif args.task_text is not None:
        task = args.task_text
    else:
        task = sys.stdin.read()
    if not task.strip():
        ap.error("empty task body (pass --task-file / --task-text or pipe it on stdin)")

    sections = [s for s in args.sections.split(",") if s.strip()]
    try:
        print(compose(task, sections=sections))
    except KeyError as exc:
        ap.error(str(exc))
    return 0


if __name__ == "__main__":
    sys.exit(main())
