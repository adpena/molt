#!/usr/bin/env python3
"""Stable change ids: integrate resolves a landing's ``change:I<hex>`` citations.

Gerrit's commit-msg hook gives every commit a ``Change-Id:`` trailer that
survives rebase and amend; Jujutsu keeps a change id apart from the commit
id. Molt's managed commit-msg hook (``.githooks/commit-msg``) adds the same
trailer. A document cites a commit of its own landing as ``change:I`` plus 8
to 40 hex digits of its Change-Id. After the rebase, ``integrate`` rewrites
each citation to that commit's final id and amends the landing's last commit,
so the gates and the push see the final text. A commit that has landed is
cited by its commit id. See docs/design/landing_queue.md, "Stable change ids".
"""

from __future__ import annotations

import re
import subprocess
from collections.abc import Callable, Mapping, Sequence
from dataclasses import dataclass
from typing import Protocol

from tools.molt_dev_common import DriverError

CHANGE_REFERENCE = re.compile(rb"\bchange:(I[0-9a-f]{8,40})\b")
# Citations resolve only in Markdown documents under docs/: code and tests may
# hold citation-shaped text as data.
CITING_ROOT = "docs/"
CITING_SUFFIX = ".md"
SHORT_ID_LENGTH = 9


@dataclass(frozen=True)
class Resolution:
    path: str
    token: str
    commit: str


# The trailer that keeps one change's identity across rebases and amends
# (Gerrit's commit-msg hook; docs/design/landing_queue.md, "Stable change ids").
CHANGE_ID_TRAILER = "Change-Id"
CHANGE_ID = re.compile(r"I[0-9a-f]{40}")


def commit_change_ids(
    run_git: Callable[[list[str]], str], revision_range: str, *, merges: bool = True
) -> list[tuple[str, frozenset[str]]]:
    """Each commit of the range, newest first, with its well-formed Change-Ids."""

    output = run_git(
        [
            "log",
            "-z",
            *(() if merges else ("--no-merges",)),
            f"--format=%H%x1f%(trailers:key={CHANGE_ID_TRAILER},valueonly,separator=%x1e)",
            revision_range,
        ]
    )
    commits: list[tuple[str, frozenset[str]]] = []
    for entry in output.split("\0"):
        entry = entry.strip("\n")
        if not entry:
            continue
        sha, _, trailers = entry.partition("\x1f")
        ids = frozenset(
            value.strip()
            for value in trailers.split("\x1e")
            if CHANGE_ID.fullmatch(value.strip())
        )
        commits.append((sha.strip(), ids))
    return commits


class GitPlumbing(Protocol):
    """The driver's fail-loud `git -C <repo>` runner (`molt_dev.Git`)."""

    def _run(
        self, args: list[str], *, check: bool = True, input_text: str | None = None
    ) -> subprocess.CompletedProcess[str]: ...


@dataclass(frozen=True, slots=True)
class LandingRepresentation:
    """Which of a branch's commits an upstream already carries."""

    merge_base: str | None
    commits: tuple[str, ...]
    unrepresented: tuple[str, ...]

    @property
    def landed(self) -> bool:
        return not self.unrepresented


def landing_representation(
    git: GitPlumbing, tip: str, upstream: str
) -> LandingRepresentation:
    """The one landed predicate: every commit of `upstream..tip` is on upstream.

    A queue replays a lane onto main and may amend its last commit, so a
    landed tip need not be an ancestor of upstream. A commit is represented
    when its ``Change-Id:`` trailer appears on an upstream commit (Gerrit's
    rule), or when its patch-id matches one (``git cherry``'s rule). Both
    searches cover only upstream's commits since the merge base. Merge commits
    carry no patch of their own and are skipped, as ``git cherry`` skips them;
    an empty commit needs a matching Change-Id or an empty upstream commit.

    Cost: one ``log`` of ``upstream..tip``, which is empty when the tip is on
    upstream. Otherwise also one ``merge-base``, one ``log`` of upstream since
    the merge base when a lane commit carries a Change-Id, and one ``cherry``
    when a commit has no matching Change-Id. A Change-Id is ``I`` and 40 hex
    digits, the managed commit-msg hook's format.
    """
    commits = commit_change_ids(
        lambda arguments: git._run(arguments).stdout, f"{upstream}..{tip}", merges=False
    )
    if not commits:
        return LandingRepresentation(None, (), ())
    shas = tuple(sha for sha, _ids in commits)
    base = git._run(["merge-base", upstream, tip], check=False)
    if base.returncode == 1:
        return LandingRepresentation(None, shas, shas)  # Unrelated histories.
    if base.returncode != 0:
        raise DriverError(
            f"git merge-base {upstream} {tip} failed (exit {base.returncode}): "
            f"{base.stderr.strip()}"
        )
    merge_base = base.stdout.strip()
    upstream_ids: frozenset[str] = frozenset()
    if any(ids for _sha, ids in commits):
        upstream_ids = frozenset(
            change
            for _commit, ids in commit_change_ids(
                lambda arguments: git._run(arguments).stdout,
                f"{merge_base}..{upstream}",
            )
            for change in ids
        )
    pending = [sha for sha, ids in commits if not ids & upstream_ids]
    if pending:
        cherry = git._run(["cherry", upstream, tip]).stdout
        on_upstream = {
            line[2:].strip() for line in cherry.splitlines() if line.startswith("- ")
        }
        pending = [sha for sha in pending if sha not in on_upstream]
    return LandingRepresentation(merge_base, shas, tuple(pending))


def citing_paths(touched: Sequence[str]) -> list[str]:
    return [
        path
        for path in touched
        if path.startswith(CITING_ROOT) and path.endswith(CITING_SUFFIX)
    ]


def change_ids(
    run_git: Callable[[list[str]], str], revision_range: str
) -> dict[str, str]:
    """Change-Id -> commit for every commit of the range that carries one."""

    owners: dict[str, list[str]] = {}
    for commit, ids in commit_change_ids(run_git, revision_range):
        for value in ids:
            owners.setdefault(value, []).append(commit)
    duplicated = sorted(
        change for change, commits in owners.items() if len(commits) > 1
    )
    if duplicated:
        raise ValueError(
            "one Change-Id names several commits of this landing: "
            + ", ".join(duplicated)
        )
    return {change: commits[0] for change, commits in owners.items()}


def resolve(
    documents: Mapping[str, bytes],
    owners: Mapping[str, str],
    *,
    tip: str,
    abbreviate: Callable[[str], str],
) -> tuple[dict[str, bytes], list[Resolution]]:
    """Rewrite every citation, or refuse the landing with every unresolved one."""

    errors: list[str] = []
    resolutions: list[Resolution] = []
    rewritten: dict[str, bytes] = {}
    for path, content in documents.items():

        def replace(match: re.Match[bytes], path: str = path) -> bytes:
            token = match.group(1).decode("ascii")
            matches = [change for change in owners if change.startswith(token)]
            if len(matches) != 1:
                errors.append(
                    f"{path}: change:{token} names "
                    + (
                        "no commit of this landing"
                        if not matches
                        else "several changes"
                    )
                )
                return match.group(0)
            commit = owners[matches[0]]
            if commit == tip:
                errors.append(
                    f"{path}: change:{token} cites the landing's last commit, whose id "
                    "the citation itself changes; cite it from a later landing"
                )
                return match.group(0)
            short = abbreviate(commit)
            resolutions.append(Resolution(path=path, token=token, commit=short))
            return short.encode("ascii")

        updated = CHANGE_REFERENCE.sub(replace, content)
        if updated != content:
            rewritten[path] = updated
    if errors:
        raise ValueError(
            "unresolved change citations; cite landed commits by commit id:\n  "
            + "\n  ".join(errors)
        )
    return rewritten, resolutions
