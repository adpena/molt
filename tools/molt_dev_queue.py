#!/usr/bin/env python3
"""The landing queue: one runner owns main; agents enqueue and continue.

bors-ng tests a batch of approved changes merged onto main and bisects a
failed batch; Zuul and the GitHub merge queue test each change on top of the
changes ahead of it. ``molt_dev.py land`` writes one request and returns. The
runner, always main's pinned driver, replays every ready lane onto main's tip
in its own sandbox worktree, runs the union of the batch's selected gates
once through ``integrate`` and publishes the tested commit. A failed batch
splits into halves until the failing lane stands alone; a lane that does not
apply leaves the batch. The runner lands one batch per process and then hands
off to main's newest controller, so every batch runs under the driver of the
main it lands on. See docs/design/landing_queue.md, "A queue with speculative
batches and bisection".
"""

from __future__ import annotations

import argparse
import json
import os
import shutil
import tempfile
import time
from collections.abc import Callable, Sequence
from types import ModuleType
from typing import Any
from dataclasses import asdict, dataclass
from datetime import UTC, datetime
from pathlib import Path

from tools.molt_dev_controller import integration_controller, run_from_pinned_controller
from tools.molt_dev_detached import python_command
from molt import process_model
from tools.molt_dev_common import (
    EXIT_FAIL,
    EXIT_OK,
    CandidateFailure,
    DriverError,
    PublicationRace,
    RebaseConflict,
    _fail,
    _ok,
    _step,
    _warn,
)


def _driver() -> ModuleType:
    """The canonical driver; imported late because it imports this module."""
    from tools import molt_dev

    return molt_dev


REQUEST_SCHEMA = "molt.landing-request.v1"
RESULT_SCHEMA = "molt.landing-result.v1"
PLAN_SCHEMA = "molt.landing-plan.v1"
# A batch larger than this costs more in bisection than one more gate run.
MAX_BATCH = 8
# Retention: finished runner logs beyond the newest RUNNER_LOGS_KEPT, and
# results older than RESULT_DAYS, retire when a runner starts.
RUNNER_LOGS_KEPT = 20
RESULT_DAYS = 30
_AWAIT_POLL_S = 10.0


@dataclass(frozen=True)
class LandingRequest:
    id: str
    head: str
    worktree: str
    branch: str | None
    enqueued_at: str
    # Retire the lane (worktree, branches, record, state) once it lands.
    retire: bool = False

    @classmethod
    def load(cls, path: Path) -> "LandingRequest":
        data = json.loads(path.read_text(encoding="utf-8"))
        if data.get("schema") != REQUEST_SCHEMA:
            raise ValueError(f"{path} is not a {REQUEST_SCHEMA} record")
        return cls(
            id=str(data["id"]),
            head=str(data["head"]),
            worktree=str(data["worktree"]),
            branch=data["branch"],
            enqueued_at=str(data["enqueued_at"]),
            retire=bool(data["retire"]),
        )


@dataclass(frozen=True)
class QueuePaths:
    root: Path

    @classmethod
    def of(cls, common_dir: Path) -> "QueuePaths":
        return cls(common_dir / "molt" / "landing-queue")

    @property
    def requests(self) -> Path:
        return self.root / "requests"

    @property
    def results(self) -> Path:
        return self.root / "results"

    @property
    def plan(self) -> Path:
        return self.root / "plan.json"

    @property
    def runner_lock(self) -> Path:
        return self.root / "runner.lock"

    @property
    def runners(self) -> Path:
        return self.root / "runners"

    @property
    def sandbox(self) -> Path:
        return self.root.parent / "landing-sandbox"


def _now() -> str:
    return datetime.now(UTC).isoformat()


def _write_json(path: Path, data: object) -> None:
    """Publish one record whole: a reader sees the old file or the new one."""

    path.parent.mkdir(parents=True, exist_ok=True)
    handle, staging = tempfile.mkstemp(prefix=f".{path.name}.", dir=path.parent)
    try:
        with os.fdopen(handle, "w", encoding="utf-8") as stream:
            json.dump(data, stream, indent=2, sort_keys=True)
            stream.write("\n")
        os.replace(staging, path)
    except BaseException:
        Path(staging).unlink(missing_ok=True)
        raise


def enqueue(
    paths: QueuePaths,
    *,
    head: str,
    worktree: Path,
    branch: str | None,
    retire: bool = False,
) -> LandingRequest:
    stamp = datetime.now(UTC).strftime("%Y%m%dT%H%M%S%fZ")
    request = LandingRequest(
        id=f"{stamp}-{head[:12]}",
        head=head,
        worktree=str(worktree),
        branch=branch,
        enqueued_at=_now(),
        retire=retire,
    )
    _write_json(
        paths.requests / f"{request.id}.json",
        {"schema": REQUEST_SCHEMA, **asdict(request)},
    )
    return request


def pending(paths: QueuePaths) -> list[LandingRequest]:
    """Requests in queue order; the id starts with its UTC enqueue time."""

    if not paths.requests.is_dir():
        return []
    return [
        LandingRequest.load(path)
        for path in sorted(paths.requests.glob("*.json"))
        if not path.name.startswith(".")
    ]


def record_result(
    paths: QueuePaths,
    request: LandingRequest,
    *,
    status: str,
    detail: str,
    landed: str | None = None,
) -> None:
    """Close one request: its result first, then its removal from the queue."""

    _write_json(
        paths.results / f"{request.id}.json",
        {
            "schema": RESULT_SCHEMA,
            "request": asdict(request),
            "status": status,
            "landed": landed,
            "detail": detail,
            "recorded_at": _now(),
        },
    )
    (paths.requests / f"{request.id}.json").unlink(missing_ok=True)


def read_result(paths: QueuePaths, request_id: str) -> dict | None:
    path = paths.results / f"{request_id}.json"
    if not path.is_file():
        return None
    return json.loads(path.read_text(encoding="utf-8"))


# --------------------------------------------------------------------------
# Batches and bisection
# --------------------------------------------------------------------------


def next_batch(
    paths: QueuePaths, ready: Sequence[LandingRequest]
) -> list[LandingRequest]:
    """The batch to test next: the bisection plan's head, else the queue's head.

    The plan survives the runner's hand-off between processes. A planned id
    that is no longer pending (landed, failed or withdrawn) drops out of it.
    """

    by_id = {request.id: request for request in ready}
    groups: list[list[str]] = []
    if paths.plan.is_file():
        data = json.loads(paths.plan.read_text(encoding="utf-8"))
        if data.get("schema") == PLAN_SCHEMA:
            groups = [
                [request_id for request_id in group if request_id in by_id]
                for group in data["groups"]
            ]
    groups = [group for group in groups if group]
    if groups:
        _write_plan(paths, groups)
        return [by_id[request_id] for request_id in groups[0]]
    paths.plan.unlink(missing_ok=True)
    return list(ready[:MAX_BATCH])


def _write_plan(paths: QueuePaths, groups: list[list[str]]) -> None:
    if groups:
        _write_json(paths.plan, {"schema": PLAN_SCHEMA, "groups": groups})
    else:
        paths.plan.unlink(missing_ok=True)


def _remaining_groups(
    paths: QueuePaths, batch: Sequence[LandingRequest]
) -> list[list[str]]:
    if not paths.plan.is_file():
        return []
    groups = json.loads(paths.plan.read_text(encoding="utf-8"))["groups"]
    ids = [request.id for request in batch]
    return groups[1:] if groups and groups[0] == ids else groups


def bisect(
    paths: QueuePaths,
    batch: Sequence[LandingRequest],
    failed: Sequence[LandingRequest],
) -> None:
    """Split the lanes of a failed batch: the first half runs next.

    ``batch`` is the planned group that ran; ``failed`` is the part of it that
    applied and failed, after conflicting lanes left it.
    """

    middle = len(failed) // 2
    halves = [
        [request.id for request in failed[:middle]],
        [request.id for request in failed[middle:]],
    ]
    _write_plan(paths, halves + _remaining_groups(paths, batch))


def retire_batch_plan(paths: QueuePaths, batch: Sequence[LandingRequest]) -> None:
    _write_plan(paths, _remaining_groups(paths, batch))


# --------------------------------------------------------------------------
# One batch
# --------------------------------------------------------------------------


@dataclass(frozen=True)
class Candidate:
    included: tuple[LandingRequest, ...]
    conflicted: tuple[tuple[LandingRequest, str], ...]


GitRunner = Callable[[Sequence[str]], "GitResult"]


@dataclass(frozen=True)
class GitResult:
    returncode: int
    stdout: str
    stderr: str


def build_candidate(
    run_git: GitRunner, upstream: str, batch: Sequence[LandingRequest]
) -> Candidate:
    """Replay each lane onto upstream in queue order in the sandbox.

    A lane that does not apply on top of the lanes ahead of it is reported and
    left out; the rest keep their order. The sandbox belongs to the queue, so
    the runner resets it to upstream before each batch.
    """

    def git(*args: str) -> GitResult:
        result = run_git(list(args))
        if result.returncode != 0:
            raise DriverError(
                f"git {' '.join(args)} failed in the landing sandbox: "
                f"{result.stderr.strip() or result.stdout.strip()}"
            )
        return result

    git("checkout", "--quiet", "--detach", "--force", upstream)
    # Ignored files (.venv, target/, node_modules) stay warm between batches.
    git("clean", "-ffd", "--quiet")
    included: list[LandingRequest] = []
    conflicted: list[tuple[LandingRequest, str]] = []
    for request in batch:
        base = git("merge-base", request.head, upstream).stdout.strip()
        commits = git("rev-list", "--reverse", f"{base}..{request.head}").stdout.split()
        if not commits:
            conflicted.append((request, "has no commits beyond main"))
            continue
        replay = run_git(["cherry-pick", "--empty=drop", *commits])
        if replay.returncode != 0:
            run_git(["cherry-pick", "--abort"])
            conflicted.append(
                (
                    request,
                    "does not apply on main and the lanes ahead of it: "
                    + (replay.stderr.strip() or replay.stdout.strip()),
                )
            )
            continue
        included.append(request)
    return Candidate(included=tuple(included), conflicted=tuple(conflicted))


def retire_old_records(paths: QueuePaths, *, now: float) -> None:
    """Bound the queue's own records; a live runner and a pending request stay."""

    if paths.runners.is_dir():
        finished = sorted(
            (entry for entry in paths.runners.iterdir() if (entry / "rc").is_file()),
            key=lambda entry: (entry / "rc").stat().st_mtime,
            reverse=True,
        )
        for entry in finished[RUNNER_LOGS_KEPT:]:
            shutil.rmtree(entry)
    if paths.results.is_dir():
        for result in paths.results.glob("*.json"):
            if now - result.stat().st_mtime > RESULT_DAYS * 86400:
                result.unlink()


# --------------------------------------------------------------------------
# Commands: land, queue run, queue status
# --------------------------------------------------------------------------


def _queue_paths(repo: Path):

    drv = _driver()
    common_dir = drv._git_common_directory(repo)
    if common_dir is None:
        raise DriverError("cannot resolve the Git common directory for the queue")
    return QueuePaths.of(common_dir)


def cmd_land(args: argparse.Namespace) -> int:

    drv = _driver()
    repo = Path(args.repo).resolve()
    git = drv.Git(repo)
    head = drv.require_git_object_id(git.head_sha(), label="landing source")
    drv._assert_integration_source(git, head)
    git.fetch(args.remote, args.branch)
    upstream = git.rev_parse(f"{args.remote}/{args.branch}")
    if git.is_ancestor(head, upstream):
        _ok(f"{git.short(head)} is already on {args.remote}/{args.branch}")
        return EXIT_OK
    paths = _queue_paths(repo)
    branch = git.symbolic_head()
    if args.retire and not (branch or "").startswith("lane/"):
        raise DriverError(
            "--retire needs a lane/<slug> branch (`molt_dev.py lane start`)"
        )
    if not args.no_precheck:
        from tools.molt_dev_precheck import precheck_landing

        precheck_landing(
            git,
            repo,
            head=head,
            upstream=upstream,
            session_id=os.environ.get("MOLT_SESSION_ID", "devtool"),
        )
    request = enqueue(
        paths, head=head, worktree=repo, branch=branch, retire=args.retire
    )
    position = [item.id for item in pending(paths)].index(request.id) + 1
    _ok(f"queued {request.id} (position {position}); the runner lands it")
    main_checkout = drv._main_worktree(git)
    if main_checkout is None:
        raise DriverError(
            "cannot find the main worktree to run the landing runner from"
        )
    # The runner outlives every lane it retires, so it runs from the main
    # worktree; it only fetches there.
    _start_queue_runner(paths, main_checkout, args)
    if not args.wait:
        return EXIT_OK
    return _await_landing(paths, request.id)


def _start_queue_runner(paths, repo: Path, args: argparse.Namespace) -> None:
    """Start a runner unless one holds the runner lock; the lock decides."""
    drv = _driver()
    probe = drv._try_acquire_file_lock(paths.runner_lock)
    if probe is None:
        _ok("a landing runner is active; it takes this request")
        return
    drv._release_file_lock(probe)
    # A unique name: a runner that is exiting never blocks the next one.
    name = "runner-" + datetime.now(UTC).strftime("%Y%m%dT%H%M%S%fZ")
    detached = drv.build_parser().parse_args(
        [
            "detached-run",
            "--name",
            name,
            "--state-dir",
            str(paths.runners),
            "--cwd",
            str(repo),
            "--",
            # The driver itself: this module is a library, not an entry point.
            *python_command(str(Path(drv.__file__).resolve())),
            "queue",
            "run",
            "--repo",
            str(repo),
            "--remote",
            args.remote,
            "--branch",
            args.branch,
            *(["--candidate-controller"] if args.candidate_controller else []),
        ]
    )
    drv.cmd_detached_run(detached)


def _runner_exited_without(paths: QueuePaths, request_id: str) -> Path | None:
    """The finished runner's state dir when no runner can still land the request.

    A runner holds its lock while it works and its supervisor writes the exit
    status last (the detached-run protocol), so a free lock, a finished newest
    runner and a still-queued request mean nothing will land it. A supervisor
    that died before it wrote ``rc`` (killed, or dead at startup) finished
    too: the process its ``pid`` file names is no longer alive.
    """
    drv = _driver()
    if not (paths.requests / f"{request_id}.json").is_file():
        return None
    probe = drv._try_acquire_file_lock(paths.runner_lock)
    if probe is None:
        return None
    drv._release_file_lock(probe)
    runners = sorted(paths.runners.iterdir()) if paths.runners.is_dir() else []
    if not runners:
        return None
    newest = runners[-1]
    if not ((newest / "rc").is_file() or _supervisor_died(newest)):
        return None
    if read_result(paths, request_id) is not None:
        return None
    return newest


def _supervisor_died(state: Path) -> bool:
    """True when the runner's supervisor is gone, whether or not it wrote ``rc``.

    The launcher publishes ``pid`` once the supervisor runs; before that the
    runner is still starting. The supervisor is the launching ``land``
    process's child, so after it dies it stays an unreaped zombie of the
    process that waits here: the probe counts a zombie dead and never reaps.
    """
    try:
        pid = int((state / "pid").read_text(encoding="utf-8"))
    except (OSError, ValueError):
        return False
    return not process_model.pid_alive(pid)


def _await_landing(paths, request_id: str) -> int:
    while (result := read_result(paths, request_id)) is None:
        exited = _runner_exited_without(paths, request_id)
        if exited is not None:
            how = (
                "exited"
                if (exited / "rc").is_file()
                else "died before it wrote an exit status"
            )
            _fail(
                f"{request_id} is still queued, but its runner {how} "
                f"(see {exited / 'run.log'}); `molt_dev.py queue run` restarts it"
            )
            return EXIT_FAIL
        time.sleep(_AWAIT_POLL_S)
    status = result["status"]
    if status == "landed":
        _ok(f"{request_id} landed in {result['landed'][:12]}")
        return EXIT_OK
    _fail(f"{request_id} {status}: {result['detail']}")
    return EXIT_FAIL


def _sandbox_git(paths, git, upstream: str):
    """The queue's own worktree; created once, so its build state stays warm."""
    drv = _driver()
    sandbox = paths.sandbox
    if not (sandbox / ".git").exists():
        paths.sandbox.parent.mkdir(parents=True, exist_ok=True)
        git._run(["worktree", "add", "--detach", "--quiet", str(sandbox), upstream])
    listed = git._run(["worktree", "list", "--porcelain", "-z"]).stdout.split("\0")
    if f"worktree {sandbox.resolve()}" not in listed:
        raise DriverError(f"{sandbox} is not a worktree of this repository")
    return drv.Git(sandbox.resolve())


def _land_one_batch(args: argparse.Namespace, git, paths) -> None:

    drv = _driver()
    git.fetch(args.remote, args.branch)
    upstream = drv.require_git_object_id(
        git.rev_parse(f"{args.remote}/{args.branch}"), label="queue upstream"
    )
    ready = pending(paths)
    for request in ready:
        if git.is_ancestor(request.head, upstream):
            record_result(
                paths, request, status="landed", landed=upstream,
                detail="already on main when the runner took it",
            )  # fmt: skip
    ready = pending(paths)
    if not ready:
        return
    batch = next_batch(paths, ready)
    sandbox = _sandbox_git(paths, git, upstream)

    def run_git(arguments):
        proc = sandbox._run(list(arguments), check=False)
        return GitResult(proc.returncode, proc.stdout, proc.stderr)

    _step(f"landing batch of {len(batch)} on {git.short(upstream)}")
    candidate = build_candidate(run_git, upstream, batch)
    for request, reason in candidate.conflicted:
        record_result(paths, request, status="conflict", detail=reason)
        _warn(f"{request.id} {reason}")
    if not candidate.included:
        retire_batch_plan(paths, batch)
        return
    _provision_landing_sandbox(sandbox.repo)
    integration = drv.build_parser().parse_args(
        [
            "integrate",
            "--repo",
            str(sandbox.repo),
            "--remote",
            args.remote,
            "--branch",
            args.branch,
            "--wait-for-lock",
            *(
                ["--pinned-controller", args.pinned_controller]
                if args.pinned_controller
                else ["--candidate-controller"]
            ),
        ]
    )
    try:
        drv.cmd_integrate(integration)
    except (RebaseConflict, PublicationRace) as exc:
        # main moved under the batch; the next runner rebuilds it on the new tip.
        _warn(f"main moved; rebuilding the batch: {exc}")
        return
    except CandidateFailure as exc:
        if len(candidate.included) > 1:
            _warn(f"batch of {len(candidate.included)} failed; bisecting")
            bisect(paths, batch, candidate.included)
            return
        (request,) = candidate.included
        record_result(paths, request, status="failed", detail=str(exc))
        retire_batch_plan(paths, batch)
        return
    landed = drv.require_git_object_id(sandbox.head_sha(), label="landed batch")
    for request in candidate.included:
        record_result(
            paths, request, status="landed", landed=landed,
            detail=f"landed with {len(candidate.included)} lane(s)",
        )  # fmt: skip
    retire_batch_plan(paths, batch)
    _after_landing(sandbox.repo, candidate.included, args)


def _provision_landing_sandbox(sandbox: Path) -> None:
    """The sandbox's own environment and trees, built from the candidate's locks.

    Repository-scoped inputs (the shared hooks) wait for the landing: they may
    install only what main publishes.
    """
    from molt import checkout_provisioning as provisioning

    specs = [
        provisioning.checkout_input(name, sandbox)
        for name in provisioning.CHECKOUT_INPUT_NAMES
    ]
    names = [
        spec.name
        for spec in specs
        if spec.scope == "checkout" and provisioning.declares(spec)
    ]
    try:
        provisioning.ensure_all(sandbox, names=names)
    except provisioning.ProvisioningError as exc:
        raise DriverError(f"landing sandbox provisioning failed: {exc}") from exc


def _after_landing(sandbox: Path, landed, args: argparse.Namespace) -> None:
    """Refresh the shared hooks from the published main; retire landed lanes.

    The landing is already published: a failure here warns and keeps going.
    """
    drv = _driver()
    from molt import checkout_provisioning as provisioning

    try:
        hooks = provisioning.checkout_input("git-hooks", sandbox)
        if provisioning.declares(hooks):
            provisioning.ensure_input(hooks)
    except provisioning.ProvisioningError as exc:
        _warn(f"managed hooks not refreshed after the landing: {exc}")
    for request in landed:
        if not request.retire or not (request.branch or "").startswith("lane/"):
            continue
        retire = drv.build_parser().parse_args(
            [
                "lane",
                "retire",
                request.branch.removeprefix("lane/"),
                "--repo",
                str(sandbox),
                "--remote",
                args.remote,
                "--branch",
                args.branch,
            ]
        )
        try:
            retire.func(retire)
        except DriverError as exc:
            _warn(f"{request.id} landed; its lane was not retired: {exc}")


def cmd_queue_run(args: argparse.Namespace) -> int:

    drv = _driver()
    repo = Path(args.repo).resolve()
    git = drv.Git(repo)
    paths = _queue_paths(repo)
    paths.root.mkdir(parents=True, exist_ok=True)
    controller = integration_controller(args)
    # The runner serves every lane: the sandbox's gates use the sandbox's own
    # environment, never the venv selection of whoever started the runner.
    for key in ("MOLT_VENV", "UV_PROJECT_ENVIRONMENT", "VIRTUAL_ENV"):
        os.environ.pop(key, None)

    retire_old_records(paths, now=time.time())
    while True:
        handle = drv._try_acquire_file_lock(paths.runner_lock)
        if handle is None:
            _ok("another landing runner is active")
            return EXIT_OK
        try:
            _step(f"landing runner ({controller})")
            _land_one_batch(args, git, paths)
        finally:
            drv._release_file_lock(handle)
        # Checked after the release: a request that arrives later starts its
        # own runner, so none waits on a runner that is about to exit.
        if not pending(paths):
            _ok("landing queue is empty")
            return EXIT_OK
        if args.pinned_controller is not None:
            # Each batch runs under the driver of the main it lands on.
            return run_from_pinned_controller(
                args,
                ["queue", "run", "--remote", args.remote, "--branch", args.branch],
            )


def cmd_queue_status(args: argparse.Namespace) -> int:

    paths = _queue_paths(Path(args.repo).resolve())
    requests = pending(paths)
    plan = (
        json.loads(paths.plan.read_text(encoding="utf-8"))["groups"]
        if paths.plan.is_file()
        else []
    )
    results = (
        sorted(paths.results.glob("*.json"))[-args.recent :]
        if paths.results.is_dir()
        else []
    )
    report = {
        "pending": [request.id for request in requests],
        "plan": plan,
        "recent": [json.loads(path.read_text(encoding="utf-8")) for path in results],
    }
    print(json.dumps(report, indent=2, sort_keys=True))
    return EXIT_OK


def add_queue_arguments(
    sub: Any,
    *,
    add_repo_arg: Callable[[argparse.ArgumentParser], None],
    add_remote_branch: Callable[[argparse.ArgumentParser], None],
) -> None:
    """The ``land`` and ``queue run|status`` subcommands."""
    # land / queue (docs/design/landing_queue.md)
    p_land = sub.add_parser(
        "land",
        help="queue this checkout's commit for the landing runner and return",
    )
    add_repo_arg(p_land)
    add_remote_branch(p_land)
    p_land.add_argument(
        "--wait",
        action="store_true",
        help="wait for the result; exit 0 only when the commit landed",
    )
    p_land.add_argument(
        "--retire",
        action="store_true",
        help="retire this lane (worktree, branches, record, state) once it lands",
    )
    p_land.add_argument(
        "--candidate-controller",
        action="store_true",
        help="run this checkout's driver and runner instead of main's pinned "
        "controller (repairs a broken controller)",
    )
    p_land.add_argument(
        "--no-precheck",
        action="store_true",
        help="queue without first running the lane's static gates in the lane "
        "(the batch still runs every gate)",
    )
    p_land.add_argument("--pinned-controller", help=argparse.SUPPRESS)
    p_land.set_defaults(func=cmd_land)

    p_queue = sub.add_parser("queue", help="the landing queue's runner and state")
    queue_sub = p_queue.add_subparsers(dest="queue_command", required=True)
    p_run = queue_sub.add_parser(
        "run", help="land queued commits in batches; `land` starts it"
    )
    add_repo_arg(p_run)
    add_remote_branch(p_run)
    p_run.add_argument(
        "--candidate-controller",
        action="store_true",
        help="run this checkout's driver instead of main's pinned controller",
    )
    p_run.add_argument("--pinned-controller", help=argparse.SUPPRESS)
    p_run.set_defaults(func=cmd_queue_run)
    p_status = queue_sub.add_parser("status", help="pending requests and results")
    add_repo_arg(p_status)
    p_status.add_argument("--recent", type=int, default=10, help="results to show")
    p_status.set_defaults(func=cmd_queue_status)
