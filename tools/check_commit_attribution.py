#!/usr/bin/env python3
"""Reject Claude attribution in commit messages.

Repository policy: no commit or pull request is attributed to Claude or
Anthropic (no `Co-Authored-By: Claude ...` trailer, no "Generated with Claude
Code" footer, no `noreply@anthropic.com` address). This checker is the one
authority for that rule; the `commit-msg` hook and CI both run it.

Modes (exactly one):
  --message-file PATH   check one message file (the git `commit-msg` hook)
  --range BASE..HEAD    check every commit in a revision range
  --github-event        check the commits of the current GitHub Actions event
  --all                 audit every commit reachable from any branch or tag

Standard library only, so CI can run it with any Python 3.10+ before the
project environment exists.
"""

from __future__ import annotations

import argparse
import json
import os
import re
import subprocess
import sys
from collections.abc import Sequence
from dataclasses import dataclass
from pathlib import Path

ATTRIBUTION_PATTERNS: tuple[re.Pattern[str], ...] = tuple(
    re.compile(pattern, re.IGNORECASE)
    for pattern in (
        r"^\s*co-authored-by:.*\b(claude|anthropic)\b",
        r"noreply@anthropic\.com",
        r"generated with \[?claude code",
        r"claude\.com/claude-code",
    )
)
ZERO_SHA = re.compile(r"^0+$")


@dataclass(frozen=True)
class Violation:
    commit: str | None
    line: str


def attribution_lines(message: str) -> list[str]:
    """Return the message lines that attribute the commit to Claude."""
    return [
        line.strip()
        for line in message.splitlines()
        if not line.startswith("#")
        and any(pattern.search(line) for pattern in ATTRIBUTION_PATTERNS)
    ]


def _git(args: Sequence[str], *, cwd: Path) -> str:
    result = subprocess.run(
        ["git", *args],
        cwd=cwd,
        capture_output=True,
        text=True,
        encoding="utf-8",
        errors="replace",
        check=False,
    )
    if result.returncode != 0:
        raise SystemExit(
            f"check_commit_attribution: git {' '.join(args)} failed "
            f"(exit {result.returncode}): {result.stderr.strip()}"
        )
    return result.stdout


def check_revisions(revisions: Sequence[str], *, cwd: Path) -> list[Violation]:
    """Check every commit `git log <revisions>` selects, in one git process."""
    violations: list[Violation] = []
    output = _git(["log", "--format=%H%n%B%x00", *revisions], cwd=cwd)
    for record in output.split("\x00"):
        commit, _, message = record.strip("\n").partition("\n")
        if commit:
            violations.extend(
                Violation(commit, line) for line in attribution_lines(message)
            )
    return violations


def github_event_revisions(
    event_name: str, event: dict[str, object], *, cwd: Path
) -> list[str]:
    """Return the `git rev-list` arguments for one GitHub Actions event."""

    def sha(mapping: object, key: str) -> str:
        value = mapping.get(key) if isinstance(mapping, dict) else None
        if not isinstance(value, str) or not value:
            raise SystemExit(f"check_commit_attribution: event has no {key!r}")
        return value

    if event_name == "pull_request":
        pull = event.get("pull_request")
        base = sha(pull.get("base") if isinstance(pull, dict) else None, "sha")
        head = sha(pull.get("head") if isinstance(pull, dict) else None, "sha")
        return [f"{base}..{head}"]
    if event_name == "merge_group":
        group = event.get("merge_group")
        return [f"{sha(group, 'base_sha')}..{sha(group, 'head_sha')}"]
    if event_name == "push":
        after = sha(event, "after")
        before = event.get("before")
        if isinstance(before, str) and before and not ZERO_SHA.match(before):
            return [f"{before}..{after}"]
        # A new ref: every commit not already on a remote-tracking branch.
        return [after, "--not", "--remotes"]
    head = _git(["rev-parse", "HEAD"], cwd=cwd).strip()
    return [f"{head}^!"]


def _report(violations: Sequence[Violation]) -> int:
    if not violations:
        return 0
    print(
        "Commit messages must not attribute work to Claude or Anthropic "
        "(policy: no Co-Authored-By: Claude trailer, no 'Generated with Claude "
        "Code' footer, no noreply@anthropic.com).",
        file=sys.stderr,
    )
    for violation in violations:
        where = violation.commit[:12] if violation.commit else "commit message"
        print(f"  {where}: {violation.line}", file=sys.stderr)
    print(
        "Remove those lines (git commit --amend for the last commit) and retry.",
        file=sys.stderr,
    )
    return 1


def main(argv: Sequence[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__.split("\n\n")[0])
    mode = parser.add_mutually_exclusive_group(required=True)
    mode.add_argument("--message-file", type=Path)
    mode.add_argument("--range", dest="revision_range")
    mode.add_argument("--github-event", action="store_true")
    mode.add_argument("--all", action="store_true")
    parser.add_argument("--repo", type=Path, default=Path.cwd())
    args = parser.parse_args(argv)

    if args.message_file is not None:
        message = args.message_file.read_text(encoding="utf-8", errors="replace")
        return _report([Violation(None, line) for line in attribution_lines(message)])
    if args.revision_range is not None:
        revisions = [args.revision_range]
    elif args.all:
        revisions = ["--branches", "--tags"]
    else:
        event_path = os.environ.get("GITHUB_EVENT_PATH", "")
        event_name = os.environ.get("GITHUB_EVENT_NAME", "")
        if not event_path or not event_name:
            raise SystemExit(
                "check_commit_attribution: --github-event needs GITHUB_EVENT_NAME "
                "and GITHUB_EVENT_PATH"
            )
        event = json.loads(Path(event_path).read_text(encoding="utf-8"))
        revisions = github_event_revisions(event_name, event, cwd=args.repo)
    return _report(check_revisions(revisions, cwd=args.repo))


if __name__ == "__main__":
    raise SystemExit(main())
