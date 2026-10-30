#!/usr/bin/env python3
"""The lane command family: the driver creates, provisions and retires lanes.

docs/design/landing_queue.md, "Orchestrator-owned lanes". A lane is the branch
``lane/<slug>`` checked out in a slot worktree. The slots are
``<root>/worktrees/slot-01`` to ``slot-12`` beside the main checkout
``<root>/molt-src`` (`molt.custody_layout`; twelve is the live worktree
limit). A slot outlives its lanes: its Cargo target, sccache keys and checkout
inputs are keyed by its path, so each lane inherits them warm (HF-491). Its
coordination record is ``logs/agents/lane-<slug>/coordination.json`` in the
main checkout, written by `tools/agent_coordination.py`.

- ``lane start`` fetches the base, prefers the caller's registered slot if
  clean, detached and unprotected, otherwise takes the first such slot (or
  adds the next one), switches it to the new branch, writes the record and
  provisions every checkout input (`molt.checkout_provisioning`).
- ``lane retire`` accepts a landed lane (`landing_representation`: every
  commit is on main by Change-Id or patch-id). With ``--abandon`` it accepts
  an unlanded lane after it writes and verifies a Git bundle in
  ``<custody root>/drift-bundles``, where ``tools/drift_harvest.py`` keeps its
  bundles. It releases a slot: the clean slot detaches at the upstream
  commit and keeps its checkout state. A lane recorded before slots existed
  has its own worktree, which it removes through
  ``molt_dev_retire.cleanup_worktree``; that also retires the checkout's
  daemons and reclaims its checkout state (HF-313). Then it deletes the remote
  and the local branch and closes the record. It
  refuses a dirty worktree, and it refuses to retire the checkout that runs
  it.
- ``lane list`` reports each lane's state.

Every verdict comes from Git plumbing exit codes and object ids (hazard 5).
"""

from __future__ import annotations

from collections.abc import Generator
from contextlib import contextmanager

import argparse
from collections.abc import Callable, Mapping
from concurrent.futures import ThreadPoolExecutor
from dataclasses import asdict, dataclass
from datetime import UTC, datetime
import json
import os
from pathlib import Path
import re
import time
from types import ModuleType
from typing import Any

from molt import checkout_provisioning
from molt.checkout_state import is_checkout_state_lock_file
from molt.custody_layout import (
    BUNDLES_DIRNAME,
    LANE_SLOT_LIMIT,
    family_worktrees_root,
    lane_slot_index,
    lane_slot_path,
)
from molt.path_custody import host_path_is_within
from molt.portable_paths import portable_path_component
from molt.file_locks import FileLockTimeout, _acquire_file_lock, _release_file_lock
from molt.process_environment import ProcessEnvironment
from tools.driver_errors import EXIT_FAIL, EXIT_OK, EXIT_USAGE, DriverError  # noqa: E402
from tools.molt_dev_common import (
    _COMMANDS,
    _fail,
    _ok,
    _run_fast_captured_command,
    _say,
    _step,
)  # noqa: E402
from tools import drift_harvest, molt_dev_controller, molt_dev_retire
from tools.molt_dev_change_ids import landing_representation
from tools.git_identity import immutable_git_environment

LANE_BRANCH_PREFIX = "lane/"
TASK_PREFIX = "lane-"
# Lower-case, so two slugs never name one directory on a case-insensitive
# filesystem, and one Git ref component.
_SLUG = re.compile(r"[a-z0-9](?:[a-z0-9._-]{0,62}[a-z0-9])?")
_WORKTREE_ADD_TIMEOUT_S = 600.0
# One start chooses and claims a slot at a time; a claim takes seconds.
_SLOT_LOCK_WAIT_S = 120.0
# The landing runner retires a landed lane in its own process. A start's
# checks hold the slug's lock for tens of milliseconds and an operator's
# `lane retire` for seconds, so the runner waits up to this long for the lock
# instead of recording landed_retirement_failed at the first contention
# (HF-468 review v6).
LANDED_RETIREMENT_LOCK_WAIT_S = 60.0


def _driver() -> ModuleType:
    """The canonical driver; imported late because it imports this module."""
    from tools import molt_dev

    return molt_dev


def lane_slug(value: str) -> str:
    if not _SLUG.fullmatch(value) or ".." in value or value.endswith(".lock"):
        raise DriverError(
            "a lane slug is 1 to 64 lower-case letters, digits, '.', '_' or '-', "
            f"starting and ending with a letter or digit: {value!r}",
            EXIT_USAGE,
        )
    try:
        portable_path_component(value)
    except ValueError as exc:
        raise DriverError(f"lane slug {value!r}: {exc}", EXIT_USAGE) from exc
    return value


def lane_branch(slug: str) -> str:
    return f"{LANE_BRANCH_PREFIX}{slug}"


def lane_task(slug: str) -> str:
    return f"{TASK_PREFIX}{slug}"


def lane_environment(environ: Mapping[str, str]) -> ProcessEnvironment:
    """The environment that provisions a new lane.

    An inherited UV selection belongs to the driver checkout. The lane
    publishes/selects its own immutable generation and records that choice
    through checkout_provisioning's existing per-worktree control projection.
    """
    return ProcessEnvironment(environ).without(
        ("UV_PROJECT_ENVIRONMENT", "VIRTUAL_ENV")
    )


def _worktrees(git: Any) -> list[dict[str, Any]]:
    from tools.agent_coordination import parse_worktree_porcelain

    return parse_worktree_porcelain(
        git._run(["worktree", "list", "--porcelain"]).stdout
    )


def _lane_worktree(worktrees: list[dict[str, Any]], branch: str) -> Path | None:
    for record in worktrees:
        if record["branch"] == branch and not record["detached"]:
            return Path(record["path"])
    return None


def _ref_sha(git: Any, ref: str) -> str | None:
    proc = git._run(
        ["rev-parse", "--verify", "--quiet", f"{ref}^{{commit}}"], check=False
    )
    if proc.returncode == 0:
        return proc.stdout.strip()
    if proc.returncode == 1:
        return None
    raise DriverError(f"git rev-parse {ref} failed: {proc.stderr.strip()}")


def _dirty_paths(git: Any, ignore_globs: tuple[str, ...]) -> list[str]:
    """Tracked changes and untracked files a removal would lose."""
    from tools.dirty_tree_policy import is_ignored

    return sorted(
        f"{xy} {path}"
        for xy, path in git.status_porcelain()
        if not is_ignored(path, ignore_globs)
    )


def _main_checkout(worktrees: list[dict[str, Any]]) -> Path:
    if not worktrees:
        raise DriverError("git worktree list named no main worktree")
    return Path(worktrees[0]["path"])


def _default_root(main: Path) -> Path:
    root = family_worktrees_root(main)
    if root is None:
        raise DriverError(
            f"the main checkout {main} is not <root>/molt-src, so it has no "
            "declared lane root (docs/agent/ORCHESTRATION.md, Canonical paths); "
            "pass --root",
            EXIT_USAGE,
        )
    return root


def _record_path(main: Path, slug: str) -> Path:
    from tools.coordination_records import task_dir

    return task_dir(main, lane_task(slug)) / "coordination.json"


def _record_status(main: Path, slug: str) -> str | None:
    path = _record_path(main, slug)
    try:
        payload = json.loads(path.read_text(encoding="utf-8"))
    except FileNotFoundError:
        return None
    except (OSError, ValueError) as exc:
        raise DriverError(f"coordination record {path} is unreadable: {exc}") from exc
    return str(payload.get("status") or "") if isinstance(payload, dict) else None


def _managed_record(main: Path, slug: str) -> dict[str, Any]:
    path = _record_path(main, slug)
    try:
        record = json.loads(path.read_text(encoding="utf-8"))
    except (OSError, ValueError) as exc:
        raise DriverError(f"managed lane custody unavailable at {path}: {exc}") from exc
    if (
        not isinstance(record, dict)
        or record.get("task") != lane_task(slug)
        or not isinstance(record.get("lane"), dict)
    ):
        raise DriverError(f"managed lane custody is invalid at {path}")
    return record


def _hand_removal(git: Any, branch: str) -> str:
    """The Git commands that remove ``branch`` by hand, naming the worktree
    that holds it, for a branch no active lane record manages."""
    holder = _lane_worktree(_worktrees(git), branch)
    return (
        "inspect it, then "
        + (f"`git worktree remove {holder}` and " if holder is not None else "")
        + f"`git branch -D {branch}`"
    )


# The Cache Directory Tagging Specification (https://bford.info/cachedir/):
# a directory whose CACHEDIR.TAG begins with this line holds only data its
# writer regenerates. Cargo, Ruff, pytest and mypy tag their caches.
_CACHEDIR_SIGNATURE = b"Signature: 8a477f597d28d172789f06886806bc55"
# setuptools' `egg_info.writers` entry points, its SOURCES.txt manifest and
# bdist_egg's zip-safety flags: everything `egg_info` writes for a project.
_EGG_INFO_FILES = frozenset(
    {
        "PKG-INFO",
        "SOURCES.txt",
        "dependency_links.txt",
        "eager_resources.txt",
        "entry_points.txt",
        "namespace_packages.txt",
        "not-zip-safe",
        "requires.txt",
        "top_level.txt",
        "zip-safe",
    }
)


# PEP 3147: CPython names a cache file `<module>.<tag>[.opt-N].pyc`, pytest's
# assertion rewriter `<module>.<tag>-pytest-<version>.pyc`, and importlib
# publishes each through a temporary `<file>.<id>`.
_BYTECODE_NAME = re.compile(r".+\.pyc(?:\.[0-9]+)?")


def _bytecode_file(name: str) -> bool:
    return _BYTECODE_NAME.fullmatch(name) is not None


def _only_files(directory: Path, keep: Callable[[str], bool]) -> bool:
    try:
        entries = list(os.scandir(directory))
    except OSError:
        return False
    return all(
        entry.is_file(follow_symlinks=False) and keep(entry.name) for entry in entries
    )


def _output_verdict(path: Path) -> bool | list[Path]:
    """One path's verdict, or a plain directory's entries to judge instead."""
    if path.is_symlink() or path.is_junction():
        return False
    if path.is_file():
        from molt.artifact_publication import is_publication_lock_file

        # Stable, empty lock metadata leaves only with its whole checkout
        # under retirement custody; a lock is never unlinked to clean a lane.
        return (
            (path.parent.name == "__pycache__" and _bytecode_file(path.name))
            or is_checkout_state_lock_file(path)
            or is_publication_lock_file(path)
        )
    if not path.is_dir():
        return False
    if path.name == "__pycache__":
        return _only_files(path, _bytecode_file)
    if path.suffix == ".egg-info":
        return _only_files(path, _EGG_INFO_FILES.__contains__)
    try:
        with (path / "CACHEDIR.TAG").open("rb") as tag:
            if tag.read(len(_CACHEDIR_SIGNATURE)) == _CACHEDIR_SIGNATURE:
                return True
    except OSError:
        pass
    try:
        return [Path(entry.path) for entry in os.scandir(path)]
    except OSError:
        return False


def _regenerable_output(path: Path) -> bool:
    """An ignored path that holds no signal, judged through its whole tree.

    - bytecode in a PEP 3147 ``__pycache__``, or the whole directory when it
      holds only bytecode files. The import system reads a cache file only
      beside its source, so the directory is cache, as PEP 3147 says; this
      releases the one trace of a module that was never committed and then
      deleted. A ``.pyc`` outside ``__pycache__`` imports without its source
      and stays signal;
    - a directory tagged by the Cache Directory Tagging Specification;
    - a setuptools ``*.egg-info`` that holds only the files ``egg_info``
      writes; the editable build ``uv sync`` runs writes one into the checkout;
    - an empty checkout-state or publication lock, which leaves only with
      its checkout;
    - a plain directory whose every entry is one of these, or that is empty.
      Git names an untracked directory beside each ignored path inside it,
      so such a directory is judged by what it holds, never by its name.

    An explicit stack walks the tree, so no depth raises `RecursionError`.
    """
    pending = [path]
    while pending:
        verdict = _output_verdict(pending.pop())
        if verdict is False:
            return False
        if isinstance(verdict, list):
            pending.extend(verdict)
    return True


def _lane_trees(
    repo: Path,
) -> list[
    tuple[checkout_provisioning.CheckoutInputName, checkout_provisioning.LockedTree]
]:
    """The declared inputs a lane owns in its own tree, as the lane's commands
    select them (`lane_environment`): the trees a handoff judges and `land`
    makes current. Repository-scope inputs such as the Git hooks are not."""
    trees = []
    for name in checkout_provisioning.CHECKOUT_INPUT_NAMES:
        spec = checkout_provisioning.checkout_input(
            name,
            repo,
            lane_environment(os.environ),
            guard_source_root=_COMMANDS.repo_root,
        )
        if (
            isinstance(spec, checkout_provisioning.LockedTree)
            and spec.scope == "checkout"
            and not spec.tree.is_symlink()
            and host_path_is_within(spec.tree.resolve(), repo.resolve())
            and spec.tree.resolve() != repo.resolve()
            and checkout_provisioning.declares(spec)
        ):
            trees.append((name, spec))
    return trees


def _unharvested_ignored_paths(git: Any) -> tuple[list[str], list[str]]:
    """Ignored content that no lock or writer regenerates is signal.

    Returns the stale declared inputs and the unharvested paths. Git ignores
    are not a source/evidence disposal policy. A declared checkout input
    belongs to the provisioning authority alone: it is reproducible only while
    current, which the authority verifies by lock stamp and sentinel without
    launching tools or reading the tree's payloads, and a stale one is held
    whatever its writer tagged. Outside the declared trees, only
    `_regenerable_output` paths are disposable; it lists their entries and
    reads one 43-byte tag per directory, never a payload.
    """
    declared = [
        (
            name,
            spec.tree.resolve(),
            checkout_provisioning.input_state(spec) == "current",
        )
        for name, spec in _lane_trees(git.repo)
    ]
    paths = [
        path
        for path in git._run(
            [
                "ls-files",
                "--others",
                "--ignored",
                "--exclude-standard",
                "--directory",
                "-z",
            ]
        ).stdout.split("\0")
        if path
    ]
    stale: list[str] = []
    unknown: list[str] = []
    for path in paths:
        named = git.repo / path
        trees = [
            (name, current)
            for name, tree, current in declared
            if host_path_is_within(named.resolve(), tree)
        ]
        if trees:
            stale.extend(name for name, current in trees if not current)
            continue
        if _regenerable_output(named):
            continue
        from tools.molt_dev_storage import indexed_payload

        if indexed_payload(named):
            continue
        unknown.append(path)
    return sorted(set(stale)), unknown


def provision_lane(worktree: Path) -> tuple[checkout_provisioning.Ensured, ...]:
    """Publish/select the lane's declared inputs through their existing owners."""
    return checkout_provisioning.ensure_all(
        worktree,
        lane_environment(os.environ),
        guard_source_root=_COMMANDS.repo_root,
    )


def validate_lane_handoff(
    git: Any,
    branch: str,
    worktree: Path,
    tip: str,
    *,
    ignore_globs: tuple[str, ...] = (),
    provision: bool = False,
) -> tuple[checkout_provisioning.Ensured, ...]:
    """Bind cooperative queue custody to the existing managed lane record.

    With ``provision``, once the lane's custody checks pass, make the trees it
    owns current before judging its content (`land`: a merged dependency
    refresh leaves the lane's environment stale). Returns what it provisioned.
    """
    slug = lane_slug(branch.removeprefix(LANE_BRANCH_PREFIX))
    worktrees = _worktrees(git)
    main = _main_checkout(worktrees)
    record = _managed_record(main, slug)
    lane = record["lane"]
    if (
        lane.get("branch") != branch
        or lane.get("worktree") != str(worktree.resolve())
        or record.get("repo_root") != str(worktree.resolve())
        or record.get("branch") != branch
        or worktree.resolve() == main.resolve()
    ):
        raise DriverError(f"managed lane record does not bind {branch} to {worktree}")
    if _ref_sha(git, f"refs/heads/{branch}") != tip:
        raise DriverError(
            f"{branch} advanced or disappeared after enrollment; reconcile its tip"
        )
    registrations = [row for row in worktrees if row["branch"] == branch]
    if len(registrations) != 1:
        raise DriverError(
            f"{branch} has {len(registrations)} registrations; shared custody must be reconciled"
        )
    listed = registrations[0]
    if Path(listed["path"]).resolve() != worktree.resolve():
        raise DriverError(
            f"{branch} no longer occupies its enrolled worktree {worktree}"
        )
    if listed["locked"]:
        raise DriverError(f"{worktree} is locked; its owner must release custody first")
    done: tuple[checkout_provisioning.Ensured, ...] = ()
    if provision:
        try:
            inputs = {name for name, _spec in _lane_trees(worktree)}
            environment = lane_environment(os.environ)
            python_input = checkout_provisioning.checkout_input(
                "uv-environment",
                worktree,
                environment,
                guard_source_root=_COMMANDS.repo_root,
            )
            if checkout_provisioning.declares(python_input):
                inputs.add("uv-environment")
            done = checkout_provisioning.ensure_all(
                worktree,
                environment,
                [
                    name
                    for name in checkout_provisioning.CHECKOUT_INPUT_NAMES
                    if name in inputs
                ],
                guard_source_root=_COMMANDS.repo_root,
            )
        except checkout_provisioning.ProvisioningError as exc:
            raise DriverError(
                f"{worktree} inputs could not be provisioned: {exc}"
            ) from exc
    source = _driver().Git(worktree)
    dirty = _dirty_paths(source, ignore_globs)
    if dirty:
        raise DriverError(
            f"{worktree} is dirty; harvest or commit first: " + ", ".join(dirty)
        )
    try:
        stale, unknown = _unharvested_ignored_paths(source)
    except (checkout_provisioning.ProvisioningError, ValueError) as exc:
        raise DriverError(
            f"{worktree} generated metadata custody is invalid: {exc}"
        ) from exc
    if stale:
        raise DriverError(
            f"{worktree} holds stale checkout inputs ({', '.join(stale)}); make "
            f"them current with: uv run python -m molt.checkout_provisioning "
            f"ensure --checkout {worktree} {' '.join(stale)}"
        )
    if unknown:
        from tools.molt_dev_storage import harvest_lane_payload

        harvested = harvest_lane_payload(
            worktree, lane_task(slug), unknown, lane_environment(os.environ)
        )
        for item in harvested:
            _ok(f"preserved lane payload {item['source']} at {item['index']}")
    return done


# --------------------------------------------------------------------------
# lane start
# --------------------------------------------------------------------------


#: A lane untouched this long is named when the work-in-progress limit refuses
#: a new one (design 56 section 2.9, item 5).
IDLE_LANE_HOURS = 48.0


def work_in_progress_refusal(repo: Path, *, now: float) -> str | None:
    """Why a new lane may not start, or None.

    A new lane may not take the live worktree count past ``drift_harvest``'s
    limit. The refusal names the idle worktrees that land or retire first;
    only a refusal pays for that classification.
    """
    live = drift_harvest.live_worktree_count(repo)
    limit = drift_harvest.MAX_LIVE_WORKTREES
    if live < limit:
        return None
    idle = [
        row
        for row in drift_harvest.classify(IDLE_LANE_HOURS, now, root=repo)
        if row["state"] in ("SIGNAL", "SUPERSEDED")
    ]
    lines = [
        f"{live} live worktrees: a new lane would pass the limit of {limit}. "
        "Land or retire one first; `lane retire <slug> --abandon` keeps a "
        "verified bundle."
    ]
    lines += [
        f"  idle over {IDLE_LANE_HOURS:.0f} h ({row['state'].lower()}): "
        f"{row['branch'] or '(detached)'} {row['path']}"
        for row in idle
    ]
    return "\n".join(lines)


@contextmanager
def _lane_slug_lock(repo: Path, slug: str, *, wait_s: float = 0.0) -> Generator[None]:
    """Hold ``slug``'s lock, or refuse: a start or retirement of it is running.

    `lane start` holds it from its checks through provisioning and `lane
    retire` for its whole run, so neither meets the other's half-made or
    half-removed lane (HF-468 reviews v4 and v5). A command refuses at once;
    the landing runner's retirement waits up to ``wait_s``.
    """
    common = _driver()._git_common_directory(repo)
    if common is None:
        raise DriverError(f"cannot resolve the Git common directory of {repo}")
    path = common / "molt" / "lane-start" / f"{slug}.lock"
    path.parent.mkdir(parents=True, exist_ok=True)
    refusal = (
        f"another `lane start {slug}` or `lane retire {slug}` is running in "
        "this repository"
        + (f" (waited {wait_s:g} s)" if wait_s > 0 else "")
        + "; wait for it, or choose another slug"
    )
    try:
        handle = _acquire_file_lock(path, timeout_s=wait_s, timeout_message=refusal)
    except FileLockTimeout as exc:
        raise DriverError(str(exc)) from exc
    try:
        yield
    finally:
        _release_file_lock(handle)


@contextmanager
def _lane_slots_lock(repo: Path) -> Generator[None]:
    """Hold the slot table while one start chooses and claims its slot."""
    common = _driver()._git_common_directory(repo)
    if common is None:
        raise DriverError(f"cannot resolve the Git common directory of {repo}")
    path = common / "molt" / "lane-start" / "slots.lock"
    path.parent.mkdir(parents=True, exist_ok=True)
    try:
        handle = _acquire_file_lock(
            path,
            timeout_s=_SLOT_LOCK_WAIT_S,
            timeout_message="another `lane start` held the slot table for "
            f"{_SLOT_LOCK_WAIT_S:g} s",
        )
    except FileLockTimeout as exc:
        raise DriverError(str(exc)) from exc
    try:
        yield
    finally:
        _release_file_lock(handle)


def _choose_slot(
    worktrees: list[dict[str, Any]],
    root: Path,
    ignore_globs: tuple[str, ...],
    *,
    preferred_repo: Path,
) -> tuple[Path, bool]:
    """Prefer the resolved caller's admissible registered slot, then path order.

    Reuse requires a clean, detached, unlocked and non-prunable registration.
    Every registered slot keeps its number reserved even when unavailable;
    an unregistered path with a slot's name is also left untouched. Otherwise
    add the first unused slot name. Detach is the availability commit point:
    the previous lane may still be finishing its ref/record retirement.
    """
    taken: set[int] = set()
    skipped: list[str] = []
    for row in sorted(
        worktrees,
        key=lambda row: (
            Path(row["path"]).resolve() != preferred_repo,
            str(row["path"]),
        ),
    ):
        path = Path(row["path"])
        index = lane_slot_index(path, root)
        if index is None:
            continue
        taken.add(index)
        if row["locked"] or row["prunable"]:
            reason = "locked" if row["locked"] else "prunable"
            skipped.append(f"{path} ({reason} registration)")
            continue
        if not row["detached"]:
            continue
        dirty = _dirty_paths(_driver().Git(path), ignore_globs)
        if dirty:
            skipped.append(f"{path} ({', '.join(dirty[:3])})")
            continue
        return path, True
    for index in range(1, LANE_SLOT_LIMIT + 1):
        path = lane_slot_path(root, index)
        if index not in taken and not (path.exists() or path.is_symlink()):
            return path, False
    raise DriverError(
        f"all {LANE_SLOT_LIMIT} lane slots under {root} hold lanes"
        + (f"; unavailable slots: {'; '.join(skipped)}" if skipped else "")
        + ". Retire a landed lane first."
    )


def _undo_lane_start(
    git: Any,
    repo: Path,
    worktree: Path,
    branch: str,
    base: str,
    env: Mapping[str, str],
    *,
    reused: bool = False,
) -> str:
    """Undo what a lane start that stopped before its record created.

    A lane without its record is one `lane retire` refuses. Nothing here
    forces: the worktree goes only while Git finds it clean, and the branch
    only once no worktree holds it and only while it still points at
    ``base``. It stops at the first refusal and says what it kept and how to
    finish by hand.
    """

    def git_in_repo(*argv: str):
        return _run_fast_captured_command(
            ["git", "-C", str(repo), *argv],
            env=ProcessEnvironment(env),
            timeout=_WORKTREE_ADD_TIMEOUT_S,
        )

    resolved = worktree.resolve()
    registered = any(
        Path(record["path"]).resolve() == resolved for record in _worktrees(git)
    )
    removed: list[str] = []
    kept: list[str] = []
    if reused:
        # A reused slot stays: it detaches at the base again (its tree is clean
        # unless this start failed mid-switch), and the branch goes below.
        proc = _run_fast_captured_command(
            ["git", "-C", str(worktree), "switch", "--quiet", "--detach", base],
            env=ProcessEnvironment(env),
            timeout=_WORKTREE_ADD_TIMEOUT_S,
        )
        if proc.returncode != 0:
            return (
                f"kept {worktree} on {branch}: git switch --detach refused "
                f"({proc.stderr.strip()}); inspect the slot, then "
                f"`git -C {worktree} switch --detach {base[:12]}` and "
                f"`git branch -D {branch}`"
            )
    elif registered:
        proc = git_in_repo("worktree", "remove", str(worktree))
        if proc.returncode != 0:
            return (
                f"kept {worktree} and {branch}: git worktree remove refused "
                f"({proc.stderr.strip()}); inspect the worktree, then "
                f"`git worktree remove --force {worktree}` and "
                f"`git branch -D {branch}`"
            )
        removed.append(str(worktree))
    elif worktree.exists():
        # Not registered, so `git worktree add` did not make it; this start's
        # checks refused an existing path, so another process made it.
        kept.append(
            f"{worktree}: it is no registered worktree, so this start did not make it"
        )
    holder = _lane_worktree(_worktrees(git), branch)
    if holder is not None:
        kept.append(f"{branch}: {holder} holds it")
    elif _ref_sha(git, f"refs/heads/{branch}") is not None:
        proc = git_in_repo("update-ref", "-d", f"refs/heads/{branch}", base)
        if proc.returncode != 0:
            kept.append(
                f"{branch}: it no longer points at {base[:12]} "
                f"({proc.stderr.strip()}); keep what it holds "
                f"(`git log {base[:12]}..{branch}`), then `git branch -D {branch}`"
            )
        else:
            removed.append(branch)
    done = f"removed {' and '.join(removed)}" if removed else "removed nothing"
    return done + "".join(f"; kept {item}" for item in kept)


def cmd_lane_start(args: argparse.Namespace) -> int:
    from tools import agent_coordination, coordination_records

    driver = _driver()
    # A pinned claim must name the controller this process runs from; the
    # record keeps which driver started the lane.
    controller = molt_dev_controller.integration_controller(args)
    slug = lane_slug(args.slug)
    branch = lane_branch(slug)
    repo = Path(args.repo).resolve()
    git = driver.Git(repo)
    main = _main_checkout(_worktrees(git))
    root = Path(args.root).expanduser().resolve() if args.root else _default_root(main)
    ignore_globs = tuple(driver.DEFAULT_IGNORE_GLOBS)
    # The slug's lock, from the checks through provisioning: a second start or
    # a retirement of the slug refuses, so no start's undo meets another
    # start's worktree or branch, and no retirement removes a lane that is
    # still provisioning (reviews v4 and v5).
    with _lane_slug_lock(repo, slug), _lane_slots_lock(repo):
        if _ref_sha(git, f"refs/heads/{branch}") is not None:
            status = _record_status(main, slug)
            if status in coordination_records.ACTIVE_STATUSES:
                raise DriverError(
                    f"{branch} already exists; retire it (lane retire {slug}) or "
                    "choose another slug"
                )
            found = (
                "no lane record (a start that stopped, or a branch made by "
                "hand), so `lane retire` cannot take it"
                if status is None
                else f"a closed lane record ({_record_path(main, slug)}: "
                f"{status or 'no status'})"
            )
            raise DriverError(
                f"{branch} exists with {found}; {_hand_removal(git, branch)}, or "
                "choose another slug"
            )
        if _record_status(main, slug) in coordination_records.ACTIVE_STATUSES:
            raise DriverError(
                f"{_record_path(main, slug)} is an active record without its branch; "
                "close it before reusing the slug"
            )
        refusal = work_in_progress_refusal(repo, now=time.time())
        if refusal is not None:
            raise DriverError(refusal)

        _step(f"fetch {args.remote}/{args.branch}")
        git.fetch(args.remote, args.branch)
        base_ref = args.base or f"{args.remote}/{args.branch}"
        base = git.rev_parse(f"{base_ref}^{{commit}}")

        worktree, reused = _choose_slot(
            _worktrees(git), root, ignore_globs, preferred_repo=repo
        )
        if not reused and (worktree.exists() or worktree.is_symlink()):
            raise DriverError(
                f"{worktree} exists but is no registered slot; reconcile it"
            )
        _step(
            f"create {branch} at {git.short(base)} in "
            f"{'reused ' if reused else 'new '}slot {worktree}"
        )
        root.mkdir(parents=True, exist_ok=True)
        env = immutable_git_environment(os.environ)
        env["GIT_TERMINAL_PROMPT"] = "0"
        try:
            argv = (
                ["git", "-C", str(worktree), "switch", "--quiet", "--no-track"]
                + ["-c", branch, base]
                if reused
                else ["git", "-C", str(repo), "worktree", "add", "--no-track"]
                + ["-b", branch, str(worktree), base]
            )
            proc = _run_fast_captured_command(
                argv,
                env=ProcessEnvironment(env),
                timeout=_WORKTREE_ADD_TIMEOUT_S,
            )
            if proc.returncode != 0:
                raise DriverError(
                    f"git {'switch' if reused else 'worktree add'} failed "
                    f"(exit {proc.returncode}): {proc.stderr.strip()}"
                )
            _ok(f"{'switched' if reused else 'created'} {worktree}")
            record = agent_coordination.create_task(
                registry_root=main,
                source_root=worktree,
                task=lane_task(slug),
                lane=branch,
                extra={
                    "lane": {
                        "slug": slug,
                        "branch": branch,
                        "base_ref": base_ref,
                        "base_commit": base,
                        "worktree": str(worktree),
                        "slot": True,
                        "controller": controller,
                    }
                },
            )
        except BaseException as exc:
            # `git worktree add` can fail after it made the branch (a hook, the
            # deadline, an interrupt), and the record can fail after the add;
            # either way undo what this command created before it reports.
            undone = _undo_lane_start(
                git, repo, worktree, branch, base, env, reused=reused
            )
            if isinstance(exc, Exception):
                raise DriverError(f"cannot start {branch}: {exc}; {undone}") from exc
            _fail(f"interrupted; {undone}")
            raise
        _ok(f"recorded {_record_path(main, slug)} ({record['status']})")

        _step(f"provision {worktree}")
        try:
            done = provision_lane(worktree)
        except checkout_provisioning.ProvisioningError as exc:
            _fail(str(exc))
            _say(
                f"    the lane exists; finish provisioning with: cd {worktree} && "
                "uv run python -m molt.checkout_provisioning ensure"
            )
            return EXIT_FAIL
        for item in done:
            action = "provisioned" if item.provisioned else "current"
            _ok(f"{item.name}: {action} ({item.seconds:.2f} s)")
        print(worktree, flush=True)
        return EXIT_OK


# --------------------------------------------------------------------------
# lane retire
# --------------------------------------------------------------------------


def _remote_lane_tip(git: Any, remote: str, ref: str) -> str | None:
    proc = git._run(["ls-remote", "--heads", remote, ref], check=False)
    if proc.returncode != 0:
        raise DriverError(
            f"git ls-remote {remote} {ref} failed (exit {proc.returncode}): "
            f"{proc.stderr.strip()}"
        )
    for line in proc.stdout.splitlines():
        sha, _, name = line.partition("\t")
        if name.strip() == ref:
            return sha.strip()
    return None


def _bundle_root(main: Path) -> Path:
    from molt.dx import DxConfigError, canonical_molt_root

    try:
        return canonical_molt_root(main) / BUNDLES_DIRNAME
    except DxConfigError as exc:
        raise DriverError(f"no bundle location for {main}: {exc}") from exc


def _bundle_heads(git: Any, path: Path) -> set[str]:
    proc = git._run(["bundle", "list-heads", str(path)], check=False)
    if proc.returncode != 0:
        return set()
    return {line.strip() for line in proc.stdout.splitlines() if line.strip()}


def write_lane_bundle(
    git: Any, *, main: Path, slug: str, tip: str, upstream: str
) -> Path:
    """Bundle ``upstream..lane/<slug>`` and verify it before anything is removed."""
    ref = f"refs/heads/{lane_branch(slug)}"
    expected = f"{tip} {ref}"
    directory = _bundle_root(main)
    path = directory / f"{lane_task(slug)}-{tip[:12]}.bundle"
    if path.exists():
        if expected in _bundle_heads(git, path):
            git._run(["bundle", "verify", "--quiet", str(path)])
            return path
        raise DriverError(f"{path} exists but does not hold {expected}")
    directory.mkdir(parents=True, exist_ok=True)
    staging = path.with_name(f".{path.name}.{os.getpid()}.tmp")
    try:
        git._run(["bundle", "create", "--quiet", str(staging), ref, f"^{upstream}"])
        git._run(["bundle", "verify", "--quiet", str(staging)])
        if expected not in _bundle_heads(git, staging):
            raise DriverError(f"bundle {staging} does not hold {expected}")
        os.replace(staging, path)
    finally:
        staging.unlink(missing_ok=True)
    return path


def _verify_retirement_absence(
    git: Any, branch: str, worktree: Path, remote: str, *, slot: bool
) -> None:
    """A retired lane leaves no branch, no remote ref and no worktree holding
    it; a slot's worktree stays, released for the next lane."""
    ref = f"refs/heads/{branch}"
    if (
        (not slot and (worktree.exists() or worktree.is_symlink()))
        or any(
            row["branch"] == branch
            or (not slot and Path(row["path"]).resolve() == worktree)
            for row in _worktrees(git)
        )
        or _ref_sha(git, ref) is not None
        or _remote_lane_tip(git, remote, ref) is not None
    ):
        raise DriverError(f"{branch} retirement still has resources; reconcile custody")


def _complete_retirement(main: Path, slug: str, receipt: dict[str, Any]) -> None:
    from tools import agent_coordination

    agent_coordination.update_task(
        main,
        lane_task(slug),
        status=receipt["disposition"],
        fields={
            "lane_retired": {
                **receipt,
                "at_utc": datetime.now(UTC).isoformat(timespec="seconds"),
            }
        },
        note=f"tip={receipt['tip']}",
    )
    _ok(f"closed {_record_path(main, slug)} ({receipt['disposition']})")


def cmd_lane_retire(
    args: argparse.Namespace,
    *,
    expected_tip: str | None = None,
    expected_worktree: Path | None = None,
    lock_wait_s: float = 0.0,
) -> int:
    # A pinned claim must name the controller this process runs from.
    molt_dev_controller.integration_controller(args)
    with _lane_slug_lock(
        Path(args.repo).resolve(), lane_slug(args.slug), wait_s=lock_wait_s
    ):
        return _retire_lane(
            args, expected_tip=expected_tip, expected_worktree=expected_worktree
        )


def _retire_lane(
    args: argparse.Namespace,
    *,
    expected_tip: str | None,
    expected_worktree: Path | None,
) -> int:
    from tools import agent_coordination

    driver = _driver()
    slug = lane_slug(args.slug)
    branch = lane_branch(slug)
    ref = f"refs/heads/{branch}"
    repo = Path(args.repo).resolve()
    git = driver.Git(repo)
    worktrees = _worktrees(git)
    main = _main_checkout(worktrees)
    upstream = f"{args.remote}/{args.branch}"
    ignore_globs = tuple(driver.DEFAULT_IGNORE_GLOBS) + tuple(args.ignore or ())

    tip = _ref_sha(git, ref)
    if tip is not None and not _record_path(main, slug).exists():
        raise DriverError(
            f"{branch} has no lane record (a start that stopped, or a branch made "
            "by hand), and `lane retire` removes only a lane it manages; "
            + _hand_removal(git, branch)
        )
    record = _managed_record(main, slug)
    recorded_worktree = Path(record["lane"].get("worktree", "")).resolve()
    slot = record["lane"].get("slot") is True
    if (
        record["lane"].get("branch") != branch
        or record.get("branch") != branch
        or record.get("repo_root") != str(recorded_worktree)
        or recorded_worktree == main.resolve()
    ):
        raise DriverError(f"managed lane custody is invalid for {branch}")
    if (
        expected_worktree is not None
        and recorded_worktree != expected_worktree.resolve()
    ):
        raise DriverError(f"{branch} custody changed after enrollment")
    if tip is None:
        receipt = record.get("lane_retired") or record.get("lane_retirement")
        if (
            not isinstance(receipt, dict)
            or not isinstance(receipt.get("tip"), str)
            or receipt.get("worktree") != str(recorded_worktree)
            or receipt.get("disposition") not in {"landed", "abandoned"}
            or (expected_tip is not None and receipt["tip"] != expected_tip)
        ):
            raise DriverError(
                f"no lane branch {branch}; reconcile its retirement receipt"
            )
        _verify_retirement_absence(
            git, branch, recorded_worktree, args.remote, slot=slot
        )
        if receipt["disposition"] == "abandoned":
            bundle = receipt.get("bundle")
            if not bundle:
                raise DriverError(f"{branch} retirement bundle is unavailable")
            git._run(["bundle", "verify", "--quiet", str(bundle)])
            if f"{receipt['tip']} {ref}" not in _bundle_heads(git, Path(bundle)):
                raise DriverError(
                    f"{branch} retirement bundle does not hold its exact tip/ref"
                )
        _complete_retirement(main, slug, receipt)
        return EXIT_OK
    if expected_tip is not None and tip != expected_tip:
        raise DriverError(f"{branch} advanced after enrollment; reconcile its tip")
    worktree = _lane_worktree(worktrees, branch)
    if worktree is not None:
        validate_lane_handoff(git, branch, worktree, tip, ignore_globs=ignore_globs)
    elif recorded_worktree.exists():
        raise DriverError(
            f"{recorded_worktree} exists outside its managed Git registration"
        )
    if worktree is not None:
        for label, path in (
            # This module and the driver run from one checkout.
            ("this driver", Path(__file__)),
            ("the current directory", Path.cwd()),
        ):
            if host_path_is_within(path.resolve(), worktree.resolve()):
                raise DriverError(
                    f"{label} is inside {worktree}; run lane retire from another "
                    "checkout, for example the main checkout"
                )
        dirty = _dirty_paths(driver.Git(worktree), ignore_globs)
        if dirty:
            raise DriverError(
                f"{worktree} is dirty; commit or remove these first: "
                + ", ".join(dirty)
            )

    _step(f"fetch {upstream}")
    git.fetch(args.remote, args.branch)
    landing = landing_representation(git, tip, upstream)
    if not landing.landed and not args.abandon:
        raise DriverError(
            f"{branch} is not landed: {len(landing.unrepresented)} commit(s) are "
            f"not on {upstream} by Change-Id or patch-id: "
            + ", ".join(git.short(sha) for sha in landing.unrepresented)
            + ". Land it, or pass --abandon to bundle and retire it."
        )

    remote_tip = _remote_lane_tip(git, args.remote, ref)
    if remote_tip is not None and remote_tip != tip:
        git._run(["fetch", "--quiet", args.remote, ref])
        if not (
            git.is_ancestor(remote_tip, tip)
            or landing_representation(git, remote_tip, upstream).landed
        ):
            raise DriverError(
                f"{args.remote}/{branch} holds {git.short(remote_tip)}, which is "
                f"neither in the local {branch} nor landed; reconcile it first"
            )

    bundle: Path | None = None
    if not landing.landed:
        _step(f"bundle {upstream}..{branch}")
        bundle = write_lane_bundle(
            git, main=main, slug=slug, tip=tip, upstream=upstream
        )
        _ok(f"bundle {bundle} holds {len(landing.commits)} commit(s)")
        print(bundle, flush=True)

    retirement = {
        "at_utc": datetime.now(UTC).isoformat(timespec="seconds"),
        "tip": tip,
        "worktree": str(recorded_worktree),
        "disposition": "landed" if landing.landed else "abandoned",
        "bundle": None if bundle is None else str(bundle),
    }
    # Durable intent precedes deletion. It is not a terminal task status: a
    # failed final record write can resume from these exact identities.
    agent_coordination.update_task(
        main,
        lane_task(slug),
        status="running",
        fields={"lane_retirement": retirement},
        note=f"retiring tip={tip}",
    )
    if worktree is not None and slot:
        # The checks above proved the slot clean and the lane landed or
        # bundled, so detaching it at the upstream commit loses nothing.
        _step(f"release slot {worktree}")
        rc = molt_dev_retire.release_slot(driver.Git(worktree), worktree, upstream)
        if rc != EXIT_OK:
            return rc
    elif worktree is not None:
        _step(f"remove worktree {worktree}")
        rc = molt_dev_retire.cleanup_worktree(
            driver.Git(worktree),
            worktree,
            upstream,
            ignore_globs,
            force_sha=None if landing.landed else tip,
        )
        if rc != EXIT_OK:
            return rc

    if remote_tip is not None:
        _step(f"delete {args.remote}/{branch}")
        git._run(
            [
                "push",
                "--quiet",
                f"--force-with-lease={ref}:{remote_tip}",
                args.remote,
                f":{ref}",
            ]
        )
        # A push exit code is not proof (hazard 2): ask the remote again.
        if _remote_lane_tip(git, args.remote, ref) is not None:
            raise DriverError(f"{args.remote} still has {ref} after the delete push")
        _ok(f"deleted {args.remote}/{branch}")

    _step(f"delete {branch}")
    git._run(["update-ref", "-d", ref, tip])
    _ok(f"deleted {branch} ({git.short(tip)})")

    _verify_retirement_absence(git, branch, recorded_worktree, args.remote, slot=slot)
    _complete_retirement(main, slug, retirement)
    return EXIT_OK


# --------------------------------------------------------------------------
# lane list
# --------------------------------------------------------------------------


@dataclass(frozen=True)
class LaneState:
    slug: str
    branch: str
    tip: str
    base: str | None
    ahead: int
    behind: int
    landed: bool
    worktree: str | None
    dirty: list[str] | None
    inputs: dict[str, str] | None
    record_status: str | None

    @property
    def provisioned(self) -> bool | None:
        if self.inputs is None:
            return None
        return all(state == "current" for state in self.inputs.values())


def _checkout_input_states(worktree: Path) -> dict[str, str]:
    environment = lane_environment(os.environ)
    states: dict[str, str] = {}
    for name in checkout_provisioning.DEFAULT_CHECKOUT_INPUT_NAMES:
        spec = checkout_provisioning.checkout_input(
            name, worktree, environment, guard_source_root=_COMMANDS.repo_root
        )
        if spec.scope != "checkout":
            continue
        try:
            states[name] = checkout_provisioning.input_state(spec)
        except checkout_provisioning.ProvisioningError as exc:
            states[name] = f"error: {exc}"
    return states


def lane_state(
    driver: ModuleType,
    git: Any,
    *,
    slug: str,
    tip: str,
    upstream: str,
    worktree: Path | None,
    main: Path,
    ignore_globs: tuple[str, ...],
) -> LaneState:
    counts = git._run(["rev-list", "--left-right", "--count", f"{upstream}...{tip}"])
    behind, ahead = (int(part) for part in counts.stdout.split())
    base = git._run(["merge-base", upstream, tip], check=False)
    landed = landing_representation(git, tip, upstream).landed
    dirty = inputs = None
    if worktree is not None and worktree.is_dir():
        dirty = _dirty_paths(driver.Git(worktree), ignore_globs)
        inputs = _checkout_input_states(worktree)
    return LaneState(
        slug=slug,
        branch=lane_branch(slug),
        tip=tip,
        base=base.stdout.strip() if base.returncode == 0 else None,
        ahead=ahead,
        behind=behind,
        landed=landed,
        worktree=None if worktree is None else str(worktree),
        dirty=dirty,
        inputs=inputs,
        record_status=_record_status(main, slug),
    )


def _repository_input_states(checkout: Path) -> dict[str, str]:
    states: dict[str, str] = {}
    for name in checkout_provisioning.DEFAULT_CHECKOUT_INPUT_NAMES:
        spec = checkout_provisioning.checkout_input(
            name, checkout, guard_source_root=_COMMANDS.repo_root
        )
        if spec.scope != "repository":
            continue
        try:
            states[name] = checkout_provisioning.input_state(spec)
        except checkout_provisioning.ProvisioningError as exc:
            states[name] = f"error: {exc}"
    return states


def cmd_lane_list(args: argparse.Namespace) -> int:
    driver = _driver()
    repo = Path(args.repo).resolve()
    git = driver.Git(repo)
    upstream = f"{args.remote}/{args.branch}"
    if not args.no_fetch:
        git.fetch(args.remote, args.branch)
    worktrees = _worktrees(git)
    main = _main_checkout(worktrees)
    ignore_globs = tuple(driver.DEFAULT_IGNORE_GLOBS)
    refs = git._run(
        ["for-each-ref", "--format=%(refname)%00%(objectname)", "refs/heads/lane/"]
    ).stdout
    lanes = []
    for line in refs.splitlines():
        name, _, tip = line.partition("\0")
        slug = name.removeprefix(f"refs/heads/{LANE_BRANCH_PREFIX}")
        lanes.append((slug, tip, _lane_worktree(worktrees, lane_branch(slug))))
    with ThreadPoolExecutor(max_workers=max(1, min(8, len(lanes)))) as executor:
        states = list(
            executor.map(
                lambda lane: lane_state(
                    driver,
                    git,
                    slug=lane[0],
                    tip=lane[1],
                    upstream=upstream,
                    worktree=lane[2],
                    main=main,
                    ignore_globs=ignore_globs,
                ),
                lanes,
            )
        )
    repository = _repository_input_states(repo)
    if args.json:
        payload = {
            "upstream": upstream,
            "repository_inputs": repository,
            "lanes": [
                {**asdict(state), "provisioned": state.provisioned} for state in states
            ],
        }
        print(json.dumps(payload, indent=2, sort_keys=True))
        return EXIT_OK
    for name, state in repository.items():
        print(f"{name}: {state} (installer of {repo})")
    if not states:
        print("no lanes")
    for state in states:
        if state.worktree is None:
            where = "no worktree"
        else:
            dirty = "clean" if not state.dirty else f"dirty({len(state.dirty)})"
            stale = sorted(
                name
                for name, value in (state.inputs or {}).items()
                if value != "current"
            )
            provisioned = (
                "provisioned" if not stale else "unprovisioned(" + ",".join(stale) + ")"
            )
            where = f"{dirty} {provisioned} {state.worktree}"
        print(
            f"{state.branch} {state.tip[:9]} base={(state.base or '?')[:9]} "
            f"+{state.ahead}/-{state.behind} "
            f"{'landed' if state.landed else 'unlanded'} {where}"
            + (f" record={state.record_status}" if state.record_status else "")
        )
    return EXIT_OK


def _add_controller_arguments(parser: argparse.ArgumentParser) -> None:
    """Lane start and retire run main's driver (molt_dev_controller)."""
    parser.add_argument(
        "--candidate-controller",
        action="store_true",
        help="run this checkout's own driver instead of main's pinned controller",
    )
    molt_dev_controller.add_pinned_controller_arguments(parser)


def add_lane_arguments(
    lane: argparse.ArgumentParser,
    *,
    add_repo_arg: Any,
    add_remote_branch: Any,
) -> None:
    """The ``lane start|retire|list`` subcommands under ``lane``."""
    commands = lane.add_subparsers(dest="lane_command", required=True)

    start = commands.add_parser(
        "start",
        help="create lane/<slug> in a free slot worktree, record and provision it",
    )
    start.add_argument("slug")
    start.add_argument(
        "--base", help="base ref (default: the fetched <remote>/<branch>)"
    )
    start.add_argument(
        "--root",
        help="directory of the slot worktrees (default: <root>/worktrees beside "
        "<root>/molt-src)",
    )
    add_repo_arg(start)
    add_remote_branch(start)
    _add_controller_arguments(start)
    start.set_defaults(func=cmd_lane_start)

    retire = commands.add_parser(
        "retire",
        help="retire a landed (or bundled, abandoned) lane and release its slot",
    )
    retire.add_argument("slug")
    retire.add_argument(
        "--abandon",
        action="store_true",
        help="retire an unlanded lane after writing a verified Git bundle",
    )
    retire.add_argument("--ignore", action="append", help="extra ignore glob")
    add_repo_arg(retire)
    add_remote_branch(retire)
    _add_controller_arguments(retire)
    retire.set_defaults(func=cmd_lane_retire)

    listing = commands.add_parser("list", help="each lane's state")
    listing.add_argument("--json", action="store_true")
    listing.add_argument(
        "--no-fetch", action="store_true", help="judge against the current upstream ref"
    )
    add_repo_arg(listing)
    add_remote_branch(listing)
    listing.set_defaults(func=cmd_lane_list)
