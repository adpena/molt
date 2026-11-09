#!/usr/bin/env python3
"""No new fact is carried as a loose string or untyped value.

Owner rule (2026-11-08): "No fact should ever be carried as loose strings or
untyped values, except where absolutely necessary or optimal." This check runs
on the lines a change adds, so new code meets the rule at once while the old
backlog burns down through its own ratchet. Prior art: reviewdog's
``filter-mode=added`` and the diff-scoped analysis of Google's Tricorder.

Two classes of finding:

* **Blocking** (``ruff``): a parameter or return without an annotation
  (ANN001, ANN002, ANN003, ANN201, ANN202), ``Any`` in a signature (ANN401),
  and a ``type: ignore`` without a rule code (PGH003).
* **Advisory** (AST): an identity-shaped name (``kind``, ``role``, ``mode``,
  ``state``, ``status``, ``tier``, ``surface`` ...) annotated as plain ``str``,
  and ``Any`` in a variable annotation. A closed type (a generated ``Literal``
  or an enum from the fact's one authority) is the default; the advisory asks
  for one or for the reason a string is necessary.

``--changed --base SHA`` is the landing gate (blocking findings exit 1).
``tools/hooks/typed_facts.py`` calls ``hook_context`` after every Python edit
and shows every finding to the author; it never blocks.
"""

from __future__ import annotations

import argparse
import ast
import json
import re
import sys
import tomllib
from collections.abc import Iterable, Mapping, Sequence
from dataclasses import dataclass
from enum import Enum
from fnmatch import fnmatch
from pathlib import Path

_REPO_ROOT = Path(__file__).resolve().parents[1]
if str(_REPO_ROOT) not in sys.path:
    sys.path.insert(0, str(_REPO_ROOT))

from tools.import_file import bind_repository_imports  # noqa: E402

ROOT = bind_repository_imports(__file__)

from tools.command_execution import CommandExecutor  # noqa: E402
from tools.git_identity import branch_changed_paths  # noqa: E402

_COMMANDS = CommandExecutor.for_file(__file__, stem="MOLT_DEV")

BLOCKING_RULES: tuple[str, ...] = (
    "ANN001",
    "ANN002",
    "ANN003",
    "ANN201",
    "ANN202",
    "ANN401",
    "PGH003",
)
IDENTITY_NAME = re.compile(
    r"(?:^|_)(?:kind|role|mode|state|status|tier|surface|category|family|phase"
    r"|variant|stage|flavor|level)$"
)
RUFF_TIMEOUT_S = 120.0


class Severity(Enum):
    BLOCKING = "blocking"
    ADVISORY = "advisory"


@dataclass(frozen=True, slots=True)
class Finding:
    path: Path
    line: int
    rule: str
    message: str
    severity: Severity

    def render(self) -> str:
        return f"{self.path.as_posix()}:{self.line}: {self.rule} {self.message}"


def untyped_exclusions(root: Path) -> tuple[str, ...]:
    """The globs ty does not check (``[tool.ty.src] exclude``): one authority."""
    with (root / "pyproject.toml").open("rb") as stream:
        document = tomllib.load(stream)
    ty_src = document.get("tool", {}).get("ty", {}).get("src", {})
    return tuple(str(pattern) for pattern in ty_src.get("exclude", ()))


def generated_outputs(root: Path) -> frozenset[str]:
    """Generator outputs: their generator owns their types."""
    with (root / "tools" / "generator_manifest.toml").open("rb") as stream:
        manifest = tomllib.load(stream)
    return frozenset(
        output
        for generator in manifest.get("generator", ())
        for output in generator.get("outputs", ())
    )


def checked_python_paths(root: Path, paths: Iterable[str]) -> tuple[Path, ...]:
    """The existing, type-checked, hand-written Python files among *paths*."""
    exclusions = untyped_exclusions(root)
    generated = generated_outputs(root)
    selected: list[Path] = []
    for raw in paths:
        posix = Path(raw).as_posix()
        if not posix.endswith(".py") or posix in generated:
            continue
        if any(fnmatch(posix, pattern) for pattern in exclusions):
            continue
        if (root / posix).is_file():
            selected.append(Path(posix))
    return tuple(sorted(set(selected), key=Path.as_posix))


_HUNK = re.compile(r"^@@ -\d+(?:,\d+)? \+(\d+)(?:,(\d+))? @@")


def _git(root: Path, *args: str) -> str:
    return _COMMANDS.run(
        ["git", "-C", str(root), *args],
        capture_output=True,
        text=True,
        check=True,
        encoding="utf-8",
    ).stdout


def added_lines(root: Path, merge_base: str, path: Path) -> frozenset[int]:
    """Line numbers *path* adds since *merge_base*, working tree included."""
    diff = _git(root, "diff", "-U0", "--no-color", merge_base, "--", path.as_posix())
    if not diff and not _tracked(root, path):
        # An untracked file: every line is new.
        count = len((root / path).read_text(encoding="utf-8").splitlines())
        return frozenset(range(1, count + 1))
    lines: set[int] = set()
    for line in diff.splitlines():
        match = _HUNK.match(line)
        if match is None:
            continue
        start = int(match.group(1))
        length = 1 if match.group(2) is None else int(match.group(2))
        lines.update(range(start, start + length))
    return frozenset(lines)


def _tracked(root: Path, path: Path) -> bool:
    return (
        _COMMANDS.run(
            ["git", "-C", str(root), "ls-files", "--error-unmatch", path.as_posix()],
            capture_output=True,
            check=False,
        ).returncode
        == 0
    )


def ruff_findings(root: Path, paths: Sequence[Path]) -> tuple[Finding, ...]:
    """The blocking rules' diagnostics for *paths* (all lines)."""
    if not paths:
        return ()
    completed = _COMMANDS.run(
        [
            sys.executable,
            "-m",
            "ruff",
            "check",
            "--select",
            ",".join(BLOCKING_RULES),
            "--output-format",
            "json",
            "--no-fix",
            *(path.as_posix() for path in paths),
        ],
        cwd=root,
        capture_output=True,
        text=True,
        timeout=RUFF_TIMEOUT_S,
        check=False,
        encoding="utf-8",
    )
    if completed.returncode not in (0, 1):
        raise RuntimeError(f"ruff failed: {completed.stderr.strip()}")
    findings: list[Finding] = []
    for item in json.loads(completed.stdout or "[]"):
        filename = Path(str(item["filename"]))
        relative = filename.relative_to(root) if filename.is_absolute() else filename
        findings.append(
            Finding(
                relative,
                int(item["location"]["row"]),
                str(item["code"]),
                str(item["message"]),
                Severity.BLOCKING,
            )
        )
    return tuple(findings)


def _is_plain_str(annotation: ast.expr) -> bool:
    if isinstance(annotation, ast.Name):
        return annotation.id == "str"
    if isinstance(annotation, ast.BinOp) and isinstance(annotation.op, ast.BitOr):
        sides = (annotation.left, annotation.right)
        return any(_is_plain_str(side) for side in sides) and all(
            _is_plain_str(side)
            or (isinstance(side, ast.Constant) and side.value is None)
            for side in sides
        )
    if isinstance(annotation, ast.Constant) and isinstance(annotation.value, str):
        return annotation.value in ("str", "str | None")
    return False


def _mentions_any(annotation: ast.expr) -> bool:
    return any(
        (isinstance(node, ast.Name) and node.id == "Any")
        or (isinstance(node, ast.Attribute) and node.attr == "Any")
        for node in ast.walk(annotation)
    )


def advisory_findings(
    root: Path, path: Path, lines: frozenset[int]
) -> tuple[Finding, ...]:
    """Identity-shaped ``str`` and variable ``Any`` on *lines* of *path*."""
    try:
        tree = ast.parse(
            (root / path).read_text(encoding="utf-8"), filename=path.as_posix()
        )
    except SyntaxError:
        return ()
    findings: list[Finding] = []
    for node in ast.walk(tree):
        if isinstance(node, ast.AnnAssign) and node.lineno in lines:
            name = (
                node.target.id
                if isinstance(node.target, ast.Name)
                else node.target.attr
                if isinstance(node.target, ast.Attribute)
                else ""
            )
            if IDENTITY_NAME.search(name) and _is_plain_str(node.annotation):
                findings.append(_identity_finding(path, node.lineno, name))
            elif _mentions_any(node.annotation):
                findings.append(
                    Finding(
                        path,
                        node.lineno,
                        "TYPED-ANY",
                        f"`{name or 'value'}` is annotated with Any; name its real type",
                        Severity.ADVISORY,
                    )
                )
        elif isinstance(node, ast.arg) and node.lineno in lines and node.annotation:
            if IDENTITY_NAME.search(node.arg) and _is_plain_str(node.annotation):
                findings.append(_identity_finding(path, node.lineno, node.arg))
    return tuple(findings)


def _identity_finding(path: Path, line: int, name: str) -> Finding:
    return Finding(
        path,
        line,
        "TYPED-IDENTITY",
        f"`{name}` carries an identity as plain str; use the closed type its "
        "authority generates (Literal or enum), or state why a str is necessary",
        Severity.ADVISORY,
    )


def check(root: Path, base: str, paths: Sequence[Path]) -> tuple[Finding, ...]:
    """Every finding on the lines *paths* add since their merge base with *base*."""
    merge_base = _git(root, "merge-base", base, "HEAD").strip()
    added = {path: added_lines(root, merge_base, path) for path in paths}
    findings = [
        finding
        for finding in ruff_findings(root, [path for path in paths if added[path]])
        if finding.line in added.get(finding.path, frozenset())
    ]
    for path in paths:
        findings.extend(advisory_findings(root, path, added[path]))
    return tuple(sorted(findings, key=lambda f: (f.path.as_posix(), f.line, f.rule)))


def hook_context(root: Path, event: Mapping[str, object]) -> str | None:
    """The author-facing context for one PostToolUse event, or None."""
    tool_input = event.get("tool_input")
    if not isinstance(tool_input, Mapping):
        return None
    raw = tool_input.get("file_path")
    if not isinstance(raw, str):
        return None
    path = Path(raw)
    try:
        relative = (
            path.resolve().relative_to(root.resolve()) if path.is_absolute() else path
        )
    except ValueError:
        return None
    paths = checked_python_paths(root, [relative.as_posix()])
    if not paths:
        return None
    findings = check(root, "HEAD", paths)
    if not findings:
        return None
    return (
        "Typed facts (owner rule 2026-11-08: no fact as a loose string or untyped "
        "value). The lines you just wrote carry:\n"
        + "\n".join(f"  {finding.render()}" for finding in findings)
        + "\nFix them now: blocking ones fail the landing gate."
    )


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__.split("\n\n", 1)[0])
    parser.add_argument(
        "--changed",
        action="store_true",
        required=True,
        help="check the branch against --base",
    )
    parser.add_argument("--base", required=True, help="the integration base commit")
    args = parser.parse_args(argv)
    changed = branch_changed_paths(
        args.base, run_git=lambda git_args: _git(ROOT, *git_args)
    )
    findings = check(ROOT, args.base, checked_python_paths(ROOT, changed))
    blocking = [f for f in findings if f.severity is Severity.BLOCKING]
    for finding in findings:
        print(f"{finding.severity.value}: {finding.render()}")
    if blocking:
        print(
            f"check_typed_facts: {len(blocking)} new untyped fact(s); annotate them "
            "(no Any, no bare type: ignore)",
            file=sys.stderr,
        )
        return 1
    print(f"check_typed_facts: OK ({len(findings)} advisory)")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
