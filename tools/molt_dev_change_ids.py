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
from pathlib import Path
from typing import Protocol

from tools.git_identity import changed_paths, require_git_object_id
from tools.molt_dev_common import CandidateFailure, DriverError, _ok, _say, _step

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
    run_git: Callable[[list[str]], str], revision_range: str
) -> list[tuple[str, frozenset[str]]]:
    """Each commit of the range, newest first, with its well-formed Change-Ids."""

    output = run_git(
        [
            "log",
            "-z",
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

    def head_sha(self) -> str: ...

    def short(self, sha: str) -> str: ...


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

    Historical queue landings replayed lanes and amended their last commit,
    so an ordinary landed tip need not be an ancestor of upstream. It is represented
    when its ``Change-Id:`` trailer appears on an upstream commit (Gerrit's
    rule), or when its patch-id matches one (``git cherry``'s rule). Both
    searches cover only upstream's commits since the merge base. A merge can
    introduce resolution changes absent from either parent: only its actual
    ancestry on upstream represents it, never a Change-Id or patch-id match.
    An empty ordinary commit needs a matching Change-Id or empty upstream commit.

    Cost: one ``log`` of ``upstream..tip``, which is empty when the tip is on
    upstream. Otherwise also one ``rev-list`` of its merges, one ``merge-base``,
    one ``log`` of upstream since the merge base when an ordinary lane commit
    carries a Change-Id, and one ``cherry`` when an ordinary commit has no
    matching Change-Id. A Change-Id is ``I`` and 40 hex digits, the managed
    commit-msg hook's format.
    """
    commits = commit_change_ids(
        lambda arguments: git._run(arguments).stdout, f"{upstream}..{tip}"
    )
    if not commits:
        return LandingRepresentation(None, (), ())
    shas = tuple(sha for sha, _ids in commits)
    # The range already excludes ancestors of upstream. Every remaining merge
    # must stay pending even if an ordinary upstream commit repeats its trailer.
    merges = frozenset(
        git._run(["rev-list", "--min-parents=2", f"{upstream}..{tip}"]).stdout.split()
    )
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
    if any(ids for sha, ids in commits if sha not in merges):
        upstream_ids = frozenset(
            change
            for _commit, ids in commit_change_ids(
                lambda arguments: git._run(arguments).stdout,
                f"{merge_base}..{upstream}",
            )
            for change in ids
        )
    pending = [sha for sha, ids in commits if sha in merges or not ids & upstream_ids]
    if any(sha not in merges for sha in pending):
        cherry = git._run(["cherry", upstream, tip]).stdout
        on_upstream = {
            line[2:].strip() for line in cherry.splitlines() if line.startswith("- ")
        }
        pending = [sha for sha in pending if sha in merges or sha not in on_upstream]
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


def resolve_change_references(
    git: GitPlumbing,
    repo: Path,
    upstream: str,
    selected_commit: str,
    *,
    dry: bool,
    assert_source: Callable[[GitPlumbing, str], None],
) -> str:
    """Rewrite the landing's change citations and amend its last commit."""

    def run_git(arguments: list[str]) -> str:
        return git._run(arguments).stdout

    touched = changed_paths(upstream, selected_commit, run_git=run_git)
    documents = {
        path: (repo / path).read_bytes()
        for path in citing_paths(touched)
        if (repo / path).is_file()
    }
    if not any(CHANGE_REFERENCE.search(text) for text in documents.values()):
        return selected_commit
    _step("resolve change citations")
    try:
        rewritten, resolutions = resolve(
            documents,
            change_ids(run_git, f"{upstream}..{selected_commit}"),
            tip=selected_commit,
            abbreviate=lambda commit: git._run(
                ["rev-parse", f"--short={SHORT_ID_LENGTH}", commit]
            ).stdout.strip(),
        )
    except ValueError as exc:
        raise CandidateFailure(f"{exc}\nexecuted=0") from exc
    for resolution in resolutions:
        _ok(f"{resolution.path}: change:{resolution.token} -> {resolution.commit}")
    if dry:
        _say("    would amend the last commit with the resolved citations")
        return selected_commit
    for path, content in rewritten.items():
        (repo / path).write_bytes(content)
    git._run(["add", "--", *rewritten])
    # The message is unchanged and already passed its commit-msg hook.
    git._run(["commit", "--amend", "--no-edit", "--no-verify", "--quiet"])
    amended = require_git_object_id(git.head_sha(), label="resolved integration commit")
    assert_source(git, amended)
    _ok(f"amended {git.short(selected_commit)} -> {git.short(amended)}")
    return amended
