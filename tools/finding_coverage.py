#!/usr/bin/env python3
"""Project every executable expectation onto the open findings it names.

Three record sites carry the typed finding identity of
`tests/findings/finding_ids.py`, each beside the test it owns:

* a pytest finding test under `tests/findings/` (`@finding("HF-n")`);
* a differential program (`# MOLT_META: ... finding=HF-n`, a pinned expected
  failure that `tests/molt_diff.py` runs);
* an expectation in `tools/suite_honesty/differential_expectations.json`
  (the `finding` of a `fail` dimension: a differential program, a compliance
  case or a CPython regrtest case).

This tool reads all three and reports, per finding and per family
(`tests/findings/families.py`), how many expected failures remain: the
burn-down measure. `--check` fails when an expectation names a finding that is
not open, when a member of an enforced family has no expectation, or when the
family's local proof lane (`findings.<family>.native` in tools/proof_plan.toml)
does not run exactly the family's finding programs.

Usage:
    python tools/finding_coverage.py                 # per-family report
    python tools/finding_coverage.py --check         # enforce family coverage
    python tools/finding_coverage.py --json          # machine-readable report
"""

from __future__ import annotations

import argparse
import importlib
import json
import sys
import tomllib
from collections.abc import Iterable, Iterator, Mapping
from dataclasses import asdict, dataclass
from pathlib import Path
from typing import Literal

_REPO_ROOT = Path(__file__).resolve().parents[1]
if str(_REPO_ROOT) not in sys.path:
    sys.path.insert(0, str(_REPO_ROOT))

from tools.import_file import bind_repository_imports  # noqa: E402

ROOT = bind_repository_imports(__file__)

from tests import findings  # noqa: E402
from tests.findings.families import FAMILY_FINDINGS  # noqa: E402
from tests.findings.finding_ids import OPEN_FINDING_IDS  # noqa: E402
from tools.compat import test_policy  # noqa: E402

Channel = Literal["pytest", "program", "manifest"]
EXPECTATIONS_PATH = Path("tools/suite_honesty/differential_expectations.json")
PROOF_PLAN_PATH = Path("tools/proof_plan.toml")
DIFFERENTIAL_ROOT = Path("tests/differential")
# The manifest blocks whose `fail` dimensions may name a finding.
MANIFEST_BLOCKS = ("tests", "compliance", "regrtest")


@dataclass(frozen=True, slots=True, order=True)
class Citation:
    """One expected failure and the finding that owns it."""

    finding: str
    channel: Channel
    test: str


def pytest_citations(root: Path = ROOT) -> Iterator[Citation]:
    """Finding marks of the pytest tests under tests/findings/.

    The marks are read from the imported test functions, as pytest reads them,
    including the marks of `pytest.param` cases.
    """
    for path in sorted((root / "tests" / "findings").glob("test_*.py")):
        module = importlib.import_module(f"tests.findings.{path.stem}")
        relative = path.relative_to(root).as_posix()
        for name, value in sorted(vars(module).items()):
            if not name.startswith("test") or not callable(value):
                continue
            for mark in getattr(value, "pytestmark", ()):
                if mark.name == findings.MARK_NAME:
                    yield Citation(mark.args[0], "pytest", f"{relative}::{name}")
                elif mark.name == "parametrize":
                    for case in mark.args[1]:
                        for case_mark in getattr(case, "marks", ()):
                            if case_mark.name == findings.MARK_NAME:
                                yield Citation(
                                    case_mark.args[0],
                                    "pytest",
                                    f"{relative}::{name}[{case.id}]",
                                )


def program_citations(root: Path = ROOT) -> Iterator[Citation]:
    """Differential programs whose pinned expected failure names a finding."""
    for path in sorted((root / DIFFERENTIAL_ROOT).rglob("*.py")):
        metadata = test_policy.parse_metadata(path)
        if metadata.finding is not None:
            yield Citation(
                metadata.finding, "program", path.relative_to(root).as_posix()
            )


def manifest_citations(manifest: Mapping[str, object]) -> Iterator[Citation]:
    """`fail` dimensions of the expectation manifest that name a finding."""
    for block in MANIFEST_BLOCKS:
        entries = manifest.get(block, {})
        if not isinstance(entries, Mapping):
            continue
        for test, entry in sorted(entries.items()):
            dimensions = (
                entry.get("dimensions", {}) if isinstance(entry, Mapping) else {}
            )
            for dimension, status in sorted(dimensions.items()):
                if isinstance(status, Mapping) and status.get("finding"):
                    yield Citation(
                        str(status["finding"]), "manifest", f"{test} [{dimension}]"
                    )


def citations(root: Path = ROOT) -> tuple[Citation, ...]:
    manifest = json.loads((root / EXPECTATIONS_PATH).read_text(encoding="utf-8"))
    return tuple(
        sorted(
            {
                *pytest_citations(root),
                *program_citations(root),
                *manifest_citations(manifest),
            }
        )
    )


@dataclass(frozen=True, slots=True)
class FamilyCoverage:
    family: str
    members: tuple[str, ...]
    expected_failures: int
    covered: tuple[str, ...]
    uncovered: tuple[str, ...]


@dataclass(frozen=True, slots=True)
class Coverage:
    citations: tuple[Citation, ...]
    families: tuple[FamilyCoverage, ...]
    problems: tuple[str, ...]

    def by_finding(self) -> dict[str, list[Citation]]:
        grouped: dict[str, list[Citation]] = {}
        for citation in self.citations:
            grouped.setdefault(citation.finding, []).append(citation)
        return grouped


def family_lane_id(family: str) -> str:
    """The named proof lane that runs one family's finding programs."""
    return f"findings.{family.replace('_', '-')}.native"


def lane_cases(root: Path = ROOT) -> dict[str, tuple[str, ...]]:
    """The differential cases of every named proof lane, by lane id."""
    plan = tomllib.loads((root / PROOF_PLAN_PATH).read_text(encoding="utf-8"))
    return {
        lane["id"]: tuple(lane.get("differential_cases", ()))
        for lane in plan.get("named_lane", [])
    }


def project(
    cited: Iterable[Citation],
    families: Mapping[str, tuple[str, ...]] = FAMILY_FINDINGS,
    lanes: Mapping[str, tuple[str, ...]] | None = None,
) -> Coverage:
    cited = tuple(cited)
    problems = [
        f"{citation.test} ({citation.channel}): {problem}"
        for citation in cited
        if (problem := findings.finding_problem(citation.finding))
    ]
    owned = {citation.finding for citation in cited}
    reports = []
    for family, members in sorted(families.items()):
        for member in members:
            if member not in OPEN_FINDING_IDS:
                problems.append(f"family {family}: {findings.finding_problem(member)}")
        covered = tuple(member for member in members if member in owned)
        uncovered = tuple(member for member in members if member not in owned)
        problems.extend(
            f"family {family}: open finding {member} has no finding test, "
            "finding program or regrtest expectation"
            for member in uncovered
        )
        programs = sorted(
            citation.test
            for citation in cited
            if citation.channel == "program" and citation.finding in members
        )
        if lanes is not None and programs:
            lane = family_lane_id(family)
            listed = sorted(lanes.get(lane, ()))
            if listed != programs:
                problems.append(
                    f"family {family}: named lane {lane} must run exactly its "
                    f"finding programs; missing {sorted(set(programs) - set(listed))}, "
                    f"extra {sorted(set(listed) - set(programs))}"
                )
        reports.append(
            FamilyCoverage(
                family=family,
                members=tuple(members),
                expected_failures=sum(
                    1 for citation in cited if citation.finding in members
                ),
                covered=covered,
                uncovered=uncovered,
            )
        )
    return Coverage(cited, tuple(reports), tuple(problems))


def render(coverage: Coverage) -> str:
    lines = []
    grouped = coverage.by_finding()
    for family in coverage.families:
        lines.append(
            f"{family.family}: {family.expected_failures} expected failures over "
            f"{len(family.covered)}/{len(family.members)} open findings"
        )
        for member in family.members:
            owned = grouped.get(member, [])
            channels = ", ".join(
                f"{channel} {sum(1 for c in owned if c.channel == channel)}"
                for channel in ("pytest", "program", "manifest")
                if any(c.channel == channel for c in owned)
            )
            lines.append(f"  {member}: {len(owned)} ({channels or 'none'})")
    familied = {member for family in coverage.families for member in family.members}
    outside = sorted(set(grouped) - familied)
    if outside:
        lines.append(
            "outside enforced families: "
            + ", ".join(f"{finding} {len(grouped[finding])}" for finding in outside)
        )
    for problem in coverage.problems:
        lines.append(f"problem: {problem}")
    return "\n".join(lines)


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__.split("\n\n", 1)[0])
    parser.add_argument("--check", action="store_true", help="enforce coverage")
    parser.add_argument("--json", action="store_true", help="emit JSON")
    args = parser.parse_args(argv)
    coverage = project(citations(), lanes=lane_cases())
    if args.json:
        print(
            json.dumps(
                {
                    "families": [asdict(family) for family in coverage.families],
                    "citations": [asdict(citation) for citation in coverage.citations],
                    "problems": list(coverage.problems),
                },
                indent=2,
                sort_keys=True,
            )
        )
    else:
        print(render(coverage))
    return 1 if args.check and coverage.problems else 0


if __name__ == "__main__":
    raise SystemExit(main())
