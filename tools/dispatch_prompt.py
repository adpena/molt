#!/usr/bin/env python3
"""Compose bounded subagent assignments under the canonical AGENTS.md policy.

Use --task-file or --task-text with --bounds bounds.json. Every DispatchBounds
field is required; owned_paths is the starting aperture (empty means read only),
reserved_paths names other lanes' paths, and proof_commands adds extras to the
standard proof families. Every brief starts with the mandatory project identity;
--list-sections lists only optional routing blocks.
"""

from __future__ import annotations

import argparse
from dataclasses import dataclass
import json
import sys
from pathlib import Path

_REPO_ROOT = Path(__file__).resolve().parents[1]
if str(_REPO_ROOT) not in sys.path:
    sys.path.insert(0, str(_REPO_ROOT))

from tools.import_file import bind_repository_imports  # noqa: E402

bind_repository_imports(__file__)


@dataclass(frozen=True)
class DispatchBounds:
    outcome: str
    deliverable: str
    integration_owner: str
    proof_owner: str
    owned_paths: tuple[str, ...]
    reserved_paths: tuple[str, ...]
    proof_commands: tuple[str, ...]
    budget_minutes: int
    stop_when: str

    def __post_init__(self) -> None:
        for name in (
            "outcome",
            "deliverable",
            "integration_owner",
            "proof_owner",
            "stop_when",
        ):
            value = getattr(self, name)
            if not isinstance(value, str) or not value.strip():
                raise ValueError(f"{name} must be explicit and nonempty")
        if type(self.budget_minutes) is not int or self.budget_minutes <= 0:
            raise ValueError("budget_minutes must be a positive integer")
        for name in ("owned_paths", "reserved_paths", "proof_commands"):
            value = getattr(self, name)
            if not isinstance(value, tuple) or any(
                not isinstance(item, str) or not item.strip() for item in value
            ):
                raise ValueError(
                    f"{name} must be an explicit tuple of nonempty strings"
                )

    @classmethod
    def from_payload(cls, payload: dict) -> DispatchBounds:
        if not isinstance(payload, dict):
            raise ValueError("dispatch bounds must be a JSON object")
        fields = dict(payload)
        for name in ("owned_paths", "reserved_paths", "proof_commands"):
            if not isinstance(fields.get(name), list):
                raise ValueError(f"{name} must be an explicit JSON array")
            fields[name] = tuple(fields[name])
        return cls(**fields)

    def render(self) -> str:
        return "\n".join(
            (
                "Read the assigned checkout's AGENTS.md; it owns standing project policy.",
                "This code is yours: within the outcome below, what it is for and how it is "
                "built are your call, and you will keep building on it. Merge repeated code and "
                "fix related defects you find in the code you touch, with their tests; do not "
                "report them for someone else to fix.",
                f"User outcome: {self.outcome}",
                f"Assigned deliverable: {self.deliverable}",
                f"Integration owner: {self.integration_owner}",
                f"Proof owner: {self.proof_owner}",
                f"Starting aperture: {json.dumps(self.owned_paths) if self.owned_paths else 'read only; inspect retained evidence and return findings'}",
                f"Reserved paths (held by other lanes): {json.dumps(self.reserved_paths)}",
                (
                    "Extend to the canonical owners of each invariant you change, with their "
                    "tests and generated outputs; list each extension in the report. The "
                    "starting aperture bounds discovery, not the end state. Reserved paths "
                    "remain excluded, including their descendants."
                    if self.owned_paths
                    else "This read-only assignment authorizes no source changes."
                ),
                "For implementation work, standard proof families are: "
                "python -B tools/molt_dev.py land --repo <checkout> --precheck-only; "
                "pytest of changed readers at -n 4 or lower; owning differentials on native "
                "and WASM; cargo build/test/clippy for touched crates; generators write/check; "
                "cost runs with /usr/bin/time -l on macOS or the owning platform's equivalent. "
                "Select affected families under the proof plan and reuse matching receipts; "
                "this is not an allowlist or a requirement to run unaffected families.",
                f"Additional proof commands: {json.dumps(self.proof_commands)}",
                "Only the named proof owner may launch proof commands; other workers inspect receipts.",
                f"Work budget: {self.budget_minutes} minutes",
                f"Stop condition: {self.stop_when}",
                "Return at the deliverable or budget, whichever comes first. Close any owned commands canonically.",
                "Return only for a reserved path, an unsettled semantic decision or the budget "
                "when the deliverable remains incomplete; report overturn evidence under the stop condition.",
                "Steps that need no approval: retrying a failed or timed-out command that is safe "
                "to repeat, rerunning gates after a fix, fixing failures and regressions your "
                "change exposes, re-measuring, merging origin/main into your lane, and choosing "
                "between designs inside the settled contract.",
                "Class sweep: report the mechanism of each defect fixed, siblings found by "
                "mechanism, and paths, including unverified cells.",
            )
        )


def _import_contract():
    """Import the real contract module; the bootstrap makes `tools` resolvable."""
    from tools import subagent_contract

    return subagent_contract


#: Short section key -> the ordered constant names it expands to.
SECTIONS: dict[str, tuple[str, ...]] = {
    "grounded": ("GROUNDED_PROGRESS",),
    "promises": ("NO_ENDING_ON_PROMISES",),
    "verify": ("VERIFY_CHANGED_CONTRACT", "EVIDENCE_REVIEW"),
    "web": ("WEB_AUTHORITY",),
    "landing": ("LANDING_CONTRACT",),
    "fakes": ("NO_FAKES",),
    "git": ("GIT_CUSTODY",),
    "build": ("BUILD_ENV",),
    "distill": ("DISTILL",),
    "model-tier": ("MODEL_TIER",),
    "triality": ("TRIALITY_WIRING",),
    "preflight": ("PREMISE_VERIFICATION",),
    "egm": ("DESIGN_REVIEW", "TYPED_FACTS"),
    "typed": ("TYPED_FACTS",),
    "handoff": ("HANDOFF_PRECHECK", "WHOLE_CRATE_TESTS"),
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


def compose(
    task: str, *, bounds: DispatchBounds, sections: list[str] | None = None
) -> str:
    """Prefix project identity, then the task and selected routing blocks.

    Args:
        task: the task-specific body.
        bounds: explicit assignment and stopping rules, never inferred from prose.
        sections: short section keys to include (see :data:`SECTIONS`); ``None`` includes
            every block in canonical order.

    Returns:
        ``"<identity>\\n\\n<task>\\n\\n<block>..."``. Raises ``KeyError`` on an unknown section.
    """
    if not isinstance(bounds, DispatchBounds):
        raise ValueError("dispatch requires validated DispatchBounds")
    if not task.strip():
        raise ValueError("dispatch requires a nonempty task")
    sc = _import_contract()
    names = _resolve_sections(sections, sc)
    contract = sc.standard_contract(names)
    body = (
        f"{sc.PROJECT_IDENTITY}\n\n{task.rstrip()}\n\n{bounds.render()}\n\n"
        "For an upstream merge, use the selected image's Python: "
        "python -B tools/generated_merge.py merge origin/main. "
        "It regenerates eligible generated conflicts after sources merge; "
        "source conflicts still require resolution."
    )
    return f"{body}\n\n{contract}" if contract else body


#: Header of a brief dispatched as a file (``--out``): corrections reach a running worker.
BRIEF_HEADER = (
    "This file is your brief. Re-read it before each commit and before each long command. "
    'The integrator may append corrections at the end, under a dated "Correction" heading. '
    "A correction overrides the text above it. "
    "Put every scratch clone, build target and measurement output under $TMPDIR, "
    "your lane's scratch root (set by the lane's env.sh); lane retire deletes it, "
    "so record what matters in your report."
)


def launch_prompt(brief: Path) -> str:
    """The one-line launch prompt that points a worker at its live brief file."""
    return (
        f"Read and follow the brief at {brief}, including any dated Correction at its end. "
        "It is a live file: re-read it before each commit and before each long command."
    )


def main(argv: list[str] | None = None) -> int:
    ap = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    src = ap.add_mutually_exclusive_group()
    src.add_argument("--task-file", type=Path, help="file holding the task body")
    src.add_argument("--task-text", help="task body inline (default: read stdin)")
    ap.add_argument(
        "--bounds", type=Path, help="JSON object containing all DispatchBounds fields"
    )
    ap.add_argument(
        "--sections",
        default="all",
        help="comma-separated section keys (default: all). e.g. landing,web,git,verify",
    )
    ap.add_argument(
        "--out",
        type=Path,
        help="write the brief to this file with the live-brief header and print the launch prompt",
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

    if args.bounds is None:
        ap.error("--bounds is required for every dispatch")

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
        bounds = DispatchBounds.from_payload(
            json.loads(args.bounds.read_text(encoding="utf-8"))
        )
        if args.out is not None:
            task = f"{BRIEF_HEADER}\n\n{task}"
        prompt = compose(task, bounds=bounds, sections=sections)
        if args.out is None:
            print(prompt)
        else:
            args.out.write_text(f"{prompt}\n", encoding="utf-8")
            print(launch_prompt(args.out))
    except (KeyError, TypeError, ValueError, OSError) as exc:
        ap.error(str(exc))
    return 0


if __name__ == "__main__":
    sys.exit(main())
