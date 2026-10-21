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
from collections.abc import Callable, Mapping, Sequence
from dataclasses import dataclass

CHANGE_REFERENCE = re.compile(rb"\bchange:(I[0-9a-f]{8,40})\b")
_CHANGE_ID = re.compile(r"I[0-9a-f]{40}")
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

    output = run_git(
        [
            "log",
            "--format=%H%x00%(trailers:key=Change-Id,valueonly,separator=%x01)%x00",
            revision_range,
        ]
    )
    owners: dict[str, list[str]] = {}
    for record in output.split("\0\n"):
        if not record.strip():
            continue
        commit, _, values = record.strip("\n").partition("\0")
        for value in values.rstrip("\0").split("\x01"):
            value = value.strip()
            if _CHANGE_ID.fullmatch(value):
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
