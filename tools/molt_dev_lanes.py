#!/usr/bin/env python3
"""The lane command family: the driver creates, provisions and retires lanes.

docs/design/landing_queue.md, "Orchestrator-owned lanes". A lane is the branch
``lane/<slug>`` and its worktree. By default the worktree is
``<root>/worktrees/<slug>`` beside the main checkout ``<root>/molt-src``
(`molt.custody_layout`). Its coordination record is
``logs/agents/lane-<slug>/coordination.json`` in the main checkout, written by
`tools/agent_coordination.py`.

- ``lane start`` fetches the base, creates the branch and the worktree,
  writes the record and provisions every checkout input
  (`molt.checkout_provisioning`).
- ``lane retire`` accepts a landed lane (`landing_representation`: every
  commit is on main by Change-Id or patch-id). With ``--abandon`` it accepts
  an unlanded lane after it writes and verifies a Git bundle in
  ``<custody root>/drift-bundles``, where ``tools/drift_harvest.py`` keeps its
  bundles. It removes the worktree through ``molt_dev_retire.cleanup_worktree``, which also
  retires the checkout's daemons and reclaims its checkout state (HF-313).
  Then it deletes the remote and the local branch and closes the record. It
  refuses a dirty worktree, and it refuses to retire the checkout that runs
  it.
- ``lane list`` reports each lane's state.

Every verdict comes from Git plumbing exit codes and object ids (hazard 5).
"""

from __future__ import annotations

import argparse
from collections.abc import Mapping
from concurrent.futures import ThreadPoolExecutor
from dataclasses import asdict, dataclass
from datetime import UTC, datetime
import json
import os
from pathlib import Path
import re
import sys
from types import ModuleType
from typing import Any

from molt import checkout_provisioning
from molt.custody_layout import BUNDLES_DIRNAME, family_worktrees_root
from molt.path_custody import host_path_is_within
from molt.portable_paths import portable_path_component
from molt.process_environment import ProcessEnvironment
from tools.molt_dev_common import (
    EXIT_FAIL,
    EXIT_OK,
    EXIT_USAGE,
    DriverError,
    _fail,
    _ok,
    _run_fast_captured_command,
    _say,
    _step,
    _warn,
)
from tools import molt_dev_retire
from tools.molt_dev_change_ids import landing_representation
from tools.release.git_source_snapshot import immutable_git_environment

LANE_BRANCH_PREFIX = "lane/"
TASK_PREFIX = "lane-"
# Lower-case, so two slugs never name one directory on a case-insensitive
# filesystem, and one Git ref component.
_SLUG = re.compile(r"[a-z0-9](?:[a-z0-9._-]{0,62}[a-z0-9])?")
_WORKTREE_ADD_TIMEOUT_S = 600.0


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

    The lane's ``uv run`` from a plain shell selects ``<lane>/.venv``. An
    inherited ``UV_PROJECT_ENVIRONMENT`` names the driver checkout's
    environment, and syncing it to the lane's lock would rewrite that
    checkout's environment.
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
    from tools.agent_coordination import task_dir

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


# --------------------------------------------------------------------------
# lane start
# --------------------------------------------------------------------------


def cmd_lane_start(args: argparse.Namespace) -> int:
    from tools import agent_coordination

    driver = _driver()
    slug = lane_slug(args.slug)
    branch = lane_branch(slug)
    repo = Path(args.repo).resolve()
    git = driver.Git(repo)
    main = _main_checkout(_worktrees(git))
    root = Path(args.root).expanduser().resolve() if args.root else _default_root(main)
    worktree = root / slug
    if _ref_sha(git, f"refs/heads/{branch}") is not None:
        raise DriverError(
            f"{branch} already exists; retire it (lane retire {slug}) or choose "
            "another slug"
        )
    if worktree.exists() or worktree.is_symlink():
        raise DriverError(f"{worktree} already exists; choose another slug or --root")
    if _record_status(main, slug) in agent_coordination.ACTIVE_STATUSES:
        raise DriverError(
            f"{_record_path(main, slug)} is an active record without its branch; "
            "close it before reusing the slug"
        )

    _step(f"fetch {args.remote}/{args.branch}")
    git.fetch(args.remote, args.branch)
    base_ref = args.base or f"{args.remote}/{args.branch}"
    base = git.rev_parse(f"{base_ref}^{{commit}}")

    _step(f"create {branch} at {git.short(base)} in {worktree}")
    root.mkdir(parents=True, exist_ok=True)
    env = immutable_git_environment(os.environ)
    env["GIT_TERMINAL_PROMPT"] = "0"
    proc = _run_fast_captured_command(
        [
            "git",
            "-C",
            str(repo),
            "worktree",
            "add",
            "--no-track",
            "-b",
            branch,
            str(worktree),
            base,
        ],
        env=ProcessEnvironment(env),
        timeout=_WORKTREE_ADD_TIMEOUT_S,
    )
    if proc.returncode != 0:
        raise DriverError(
            f"git worktree add failed (exit {proc.returncode}): {proc.stderr.strip()}"
        )
    _ok(f"created {worktree}")

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
            }
        },
    )
    _ok(f"recorded {_record_path(main, slug)} ({record['status']})")

    _step(f"provision {worktree}")
    try:
        done = checkout_provisioning.ensure_all(worktree, lane_environment(os.environ))
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


def cmd_lane_retire(args: argparse.Namespace) -> int:
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
    if tip is None:
        raise DriverError(f"no lane branch {branch}")
    worktree = _lane_worktree(worktrees, branch)
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

    if worktree is not None:
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

    disposition = "landed" if landing.landed else "abandoned"
    if _record_status(main, slug) is None:
        _warn(f"{branch} has no coordination record at {_record_path(main, slug)}")
    else:
        agent_coordination.update_task(
            main,
            lane_task(slug),
            status=disposition,
            fields={
                "lane_retired": {
                    "at_utc": datetime.now(UTC).isoformat(timespec="seconds"),
                    "tip": tip,
                    "disposition": disposition,
                    "bundle": None if bundle is None else str(bundle),
                }
            },
            note=f"tip={tip}" + ("" if bundle is None else f" bundle={bundle}"),
        )
        _ok(f"closed {_record_path(main, slug)} ({disposition})")
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
    for name in checkout_provisioning.CHECKOUT_INPUT_NAMES:
        spec = checkout_provisioning.checkout_input(name, worktree, environment)
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
    for name in checkout_provisioning.CHECKOUT_INPUT_NAMES:
        spec = checkout_provisioning.checkout_input(name, checkout)
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


def add_lane_arguments(
    lane: argparse.ArgumentParser,
    *,
    add_repo_arg: Any,
    add_remote_branch: Any,
) -> None:
    """The ``lane start|retire|list`` subcommands under ``lane``."""
    commands = lane.add_subparsers(dest="lane_command", required=True)

    start = commands.add_parser(
        "start", help="create lane/<slug> and its worktree, record and provision it"
    )
    start.add_argument("slug")
    start.add_argument(
        "--base", help="base ref (default: the fetched <remote>/<branch>)"
    )
    start.add_argument(
        "--root",
        help="directory for the worktree (default: <root>/worktrees beside "
        "<root>/molt-src)",
    )
    add_repo_arg(start)
    add_remote_branch(start)
    start.set_defaults(func=cmd_lane_start)

    retire = commands.add_parser(
        "retire", help="remove a landed (or bundled, abandoned) lane completely"
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
    retire.set_defaults(func=cmd_lane_retire)

    listing = commands.add_parser("list", help="each lane's state")
    listing.add_argument("--json", action="store_true")
    listing.add_argument(
        "--no-fetch", action="store_true", help="judge against the current upstream ref"
    )
    add_repo_arg(listing)
    add_remote_branch(listing)
    listing.set_defaults(func=cmd_lane_list)


if __name__ == "__main__":
    sys.exit("run lane commands through tools/molt_dev.py lane")
