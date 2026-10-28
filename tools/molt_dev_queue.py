#!/usr/bin/env python3
"""The landing queue: one runner owns main; agents enqueue and continue.

bors-ng tests a batch of approved changes merged onto main and bisects a
failed batch; Zuul and the GitHub merge queue test each change on top of the
changes ahead of it. ``molt_dev.py land`` writes one request and returns. The
runner, main's pinned driver or an explicitly adopted committed controller,
replays every ready lane onto main's tip
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
from contextlib import ExitStack
from types import ModuleType
from typing import Any
from dataclasses import asdict, dataclass
from datetime import UTC, datetime
from enum import StrEnum
from heapq import nlargest
from pathlib import Path

from tools.controller_source import (
    controller_for_source,
    controller_source,
    controller_source_use,
)  # noqa: E402
from tools.molt_dev_controller import (
    ENVIRONMENT_NAME,
    add_pinned_controller_arguments,
    integration_controller,
    run_from_pinned_controller,
)  # noqa: E402
from tools.detached_execution import launch_detached, python_command
from tools.venv_exec import venv_python
from molt.path_custody import host_path_is_within
from molt.custody_layout import landing_sandbox_path
from molt import process_model
from molt.file_locks import FileLockTimeout
from tools.driver_errors import (
    EXIT_FAIL,
    EXIT_OK,
    EXIT_USAGE,
    CandidateFailure,
    DriverError,
    PublicationRace,
    RebaseConflict,
)  # noqa: E402
from tools.molt_dev_common import _COMMANDS, _fail, _ok, _step, _warn  # noqa: E402


def _driver() -> ModuleType:
    """The canonical driver; imported late because it imports this module."""
    from tools import molt_dev

    return molt_dev


REQUEST_SCHEMA = "molt.landing-request.v2"
RESULT_SCHEMA = "molt.landing-result.v2"
PLAN_SCHEMA = "molt.landing-plan.v1"
# A batch larger than this costs more in bisection than one more gate run.
MAX_BATCH = 8
# Retention: finished runner logs beyond the newest RUNNER_LOGS_KEPT, and
# results older than RESULT_DAYS, retire when a runner starts.
RUNNER_LOGS_KEPT = 20
RESULT_DAYS = 30
# `land --wait` watches for the queue's result record. Each look costs two
# stats and a lock probe, so the wait starts short and doubles to one second:
# a landing that takes two seconds is seen within a second of landing, and a
# long one costs one look per second. A fixed ten-second look made every
# `land --wait` end up to ten seconds after its landing (measured 2026-10-21:
# a 1.8 s fixture landing returned after 24.7 s).
_AWAIT_POLL_MIN_S = 0.05
_AWAIT_POLL_MAX_S = 1.0


class LandingStatus(StrEnum):
    """How a landing request closed; each result records exactly one."""

    LANDED = "landed"
    LANDED_RETIREMENT_FAILED = "landed_retirement_failed"
    FAILED = "failed"
    CONFLICT = "conflict"
    # The integrator withdrew it before any batch tested it (`queue withdraw`).
    WITHDRAWN = "withdrawn"


@dataclass(frozen=True)
class LandingRequest:
    id: str
    head: str
    worktree: str
    branch: str | None
    enqueued_at: str

    @classmethod
    def load(cls, path: Path) -> "LandingRequest":
        data = json.loads(path.read_text(encoding="utf-8"))
        if data.get("schema") != REQUEST_SCHEMA:
            raise ValueError(
                f"{path} is not a {REQUEST_SCHEMA} record; preserve it and reconcile "
                "the old request under the queue runner lock before retrying"
            )
        return cls(
            id=str(data["id"]),
            head=str(data["head"]),
            worktree=str(data["worktree"]),
            branch=data["branch"],
            enqueued_at=str(data["enqueued_at"]),
        )


@dataclass(frozen=True)
class QueuePaths:
    root: Path
    sandbox: Path

    @classmethod
    def of(cls, common_dir: Path, *, primary_checkout: Path) -> "QueuePaths":
        return cls(
            common_dir / "molt" / "landing-queue",
            landing_sandbox_path(primary_checkout, common_dir),
        )

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
) -> LandingRequest:
    stamp = datetime.now(UTC).strftime("%Y%m%dT%H%M%S%fZ")
    request = LandingRequest(
        id=f"{stamp}-{head[:12]}",
        head=head,
        worktree=str(worktree),
        branch=branch,
        enqueued_at=_now(),
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
    status: LandingStatus,
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


def recorded_results(paths: QueuePaths, *, limit: int | None = None) -> list[dict]:
    """Closed requests in the order their results were recorded (oldest
    first), the newest ``limit``. A request's id orders it by enqueue time,
    which is not when it closed: a bisected batch closes out of queue order.
    The id breaks a tie between two results recorded in one clock tick."""

    if limit is not None and limit < 0:
        raise DriverError("--recent must be non-negative", code=EXIT_USAGE)
    if limit == 0 or not paths.results.is_dir():
        return []
    records = (
        json.loads(path.read_text(encoding="utf-8"))
        for path in paths.results.glob("*.json")
        if not path.name.startswith(".")
    )

    def completion_key(result):
        return (
            datetime.fromisoformat(result["recorded_at"]),
            result["request"]["id"],
        )

    # Completion order is stored in each receipt, not in its enqueue-time name.
    # Read every receipt once; bounded requests retain only the newest K bodies.
    selected = (
        records if limit is None else nlargest(limit, records, key=completion_key)
    )
    return sorted(selected, key=completion_key)


def withdraw(paths: QueuePaths, request_id: str, *, reason: str) -> LandingRequest:
    """Close a pending request without testing it.

    The caller holds the runner lock, which a runner holds for each batch, so
    no batch is testing the request; ``next_batch`` drops a withdrawn id from a
    bisection plan.
    """

    request = next((item for item in pending(paths) if item.id == request_id), None)
    if request is None:
        raise DriverError(f"{request_id} is not a pending landing request")
    record_result(paths, request, status=LandingStatus.WITHDRAWN, detail=reason)
    return request


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
    """Merge each immutable lane tip onto upstream in queue order.

    A lane that does not apply on top of the lanes ahead of it is reported and
    left out; the rest keep their order. Materialize an up-to-date first lane
    directly, without rewriting unchanged inputs through an older upstream.
    """

    def git(*args: str) -> GitResult:
        result = run_git(list(args))
        if result.returncode != 0:
            raise DriverError(
                f"git {' '.join(args)} failed in the landing sandbox: "
                f"{result.stderr.strip() or result.stdout.strip()}"
            )
        return result

    first = batch[0] if batch else None
    direct = False
    if first is not None and first.head != upstream:
        ancestry = run_git(["merge-base", "--is-ancestor", upstream, first.head])
        if ancestry.returncode not in (0, 1):
            raise DriverError(
                "cannot establish enrolled lane's upstream ancestry: "
                + (ancestry.stderr.strip() or ancestry.stdout.strip())
            )
        direct = ancestry.returncode == 0
    target = first.head if first is not None and direct else upstream
    git("checkout", "--quiet", "--detach", "--force", target)
    # Ignored files (.venv, target/, node_modules) stay warm between batches.
    git("clean", "-ffd", "--quiet")
    included: list[LandingRequest] = []
    conflicted: list[tuple[LandingRequest, str]] = []
    if direct:
        assert first is not None
        # Citation resolution amends this dedicated candidate, never the
        # immutable enrolled source. Ordinary commit hooks remain active.
        git("commit", "--quiet", "--allow-empty", "-m", f"Integrate {first.head}")
        observed = git(
            "rev-parse", f"{first.head}^{{tree}}", "HEAD^{tree}", "HEAD^@"
        ).stdout.splitlines()
        if (
            len(observed) != 3
            or observed[0] != observed[1]
            or observed[2] != first.head
        ):
            raise DriverError(
                "direct landing candidate changed the enrolled source tree or ancestry"
            )
        included.append(first)
    for request in batch[1:] if direct else batch:
        before = git("rev-parse", "HEAD").stdout.strip()
        # Preserve the source DAG and manual merge resolutions. A forced merge
        # also keeps request.head ancestral when integrate amends the batch tip
        # to resolve its change citations.
        replay = run_git(["merge", "--no-ff", "--no-edit", request.head])
        if replay.returncode != 0:
            merging = run_git(["rev-parse", "--quiet", "--verify", "MERGE_HEAD"])
            if merging.returncode == 0:
                git("merge", "--abort")
            elif merging.returncode != 1:
                raise DriverError("cannot inspect failed merge in the landing sandbox")
            if (
                git("rev-parse", "HEAD").stdout.strip() != before
                or git("status", "--porcelain").stdout.strip()
            ):
                raise DriverError(
                    "failed merge did not restore the preceding candidate"
                )
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
                record = json.loads(result.read_text(encoding="utf-8"))
                if record.get("status") != LandingStatus.LANDED_RETIREMENT_FAILED:
                    result.unlink()


# --------------------------------------------------------------------------
# Commands: land, queue run, queue status
# --------------------------------------------------------------------------


def queue_paths(repo: Path) -> QueuePaths:
    """The landing queue of ``repo``'s Git common directory, which every
    worktree of the checkout shares."""

    drv = _driver()
    common_dir = drv._git_common_directory(repo)
    if common_dir is None:
        raise DriverError("cannot resolve the Git common directory for the queue")
    primary_checkout = drv._main_worktree(drv.Git(repo))
    if primary_checkout is None:
        raise DriverError("cannot resolve the primary checkout for the queue sandbox")
    return QueuePaths.of(common_dir, primary_checkout=primary_checkout)


def cmd_land(args: argparse.Namespace) -> int:

    drv = _driver()
    repo = Path(args.repo).resolve()
    git = drv.Git(repo)
    head = drv.require_git_object_id(git.head_sha(), label="landing source")
    drv._assert_integration_source(git, head)
    git.fetch(args.remote, args.branch)
    upstream = git.rev_parse(f"{args.remote}/{args.branch}")
    paths = queue_paths(repo)
    branch = git.symbolic_head()
    managed = (branch or "").startswith("lane/")
    already_landed = git.is_ancestor(head, upstream)
    if managed:
        from tools.molt_dev_lanes import validate_lane_handoff

        # A merged dependency refresh leaves the lane's inputs stale.
        for item in validate_lane_handoff(git, branch, repo, head, provision=True):
            if item.provisioned:
                _ok(f"{item.name}: provisioned ({item.seconds:.2f} s)")
    elif already_landed:
        _ok(
            f"{git.short(head)} is already on {args.remote}/{args.branch}; checkout retained"
        )
        return EXIT_OK
    if not already_landed and not args.no_precheck:
        from tools.molt_dev_precheck import precheck_landing

        precheck_landing(
            git,
            repo,
            head=head,
            upstream=upstream,
            session_id=os.environ.get("MOLT_SESSION_ID", "devtool"),
        )
    request = enqueue(paths, head=head, worktree=repo, branch=branch)
    position = [item.id for item in pending(paths)].index(request.id) + 1
    _ok(f"queued {request.id} (position {position}); the runner lands it")
    if managed:
        _step("lane custody handed to the runner; stop editing until its result closes")
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


def _runner_lock_free(paths: QueuePaths) -> bool:
    """No runner holds the runner lock. The probe takes it shared, so probes
    from ``land``, ``land --wait`` and ``withdraw`` never read each other as a
    runner; only a runner's or a withdrawal's exclusive hold does."""
    drv = _driver()
    probe = drv._try_acquire_file_lock(paths.runner_lock, shared=True)
    if probe is None:
        return False
    drv._release_file_lock(probe)
    return True


#: A liveness probe holds the runner lock shared for an instant; a runner
#: holds it for a whole batch. Waiting this long tells the two apart.
RUNNER_LOCK_WAIT_S = 2.0


def _take_runner_lock(paths: QueuePaths) -> Any:
    """The runner lock held exclusively, or None while a runner holds it.

    A bare try would read a probe's instant as a runner: a runner starting
    between batches would exit with requests still queued, and a withdrawal
    would refuse with no batch running.
    """
    try:
        return _driver()._acquire_file_lock(
            paths.runner_lock,
            timeout_s=RUNNER_LOCK_WAIT_S,
            timeout_message="a landing runner holds the runner lock",
        )
    except FileLockTimeout:
        return None


def _start_queue_runner(paths, repo: Path, args: argparse.Namespace) -> None:
    """Start a runner unless one holds the runner lock; the lock decides."""
    drv = _driver()
    if not _runner_lock_free(paths):
        _ok("a landing runner is active; it takes this request")
        return
    # A unique name: a runner that is exiting never blocks the next one.
    name = "runner-" + datetime.now(UTC).strftime("%Y%m%dT%H%M%S%fZ")
    if args.candidate_controller:
        driver_file = drv.__file__
        if driver_file is None:
            raise DriverError("candidate controller has no source file")
        source = Path(driver_file).resolve().parents[1]
        common_dir = drv._git_common_directory(repo)
        if common_dir is None:
            raise DriverError("cannot resolve candidate runner's Git common directory")
        with ExitStack() as custody:
            controller = controller_for_source(source)
            if controller is None:
                source_git = drv.Git(source)
                observed = source_git.checkout_observation()
                if observed.dirty_records:
                    raise DriverError(
                        "candidate runner requires clean committed controller source: "
                        f"{source}; commit the reviewed changes before adoption"
                    )
                controller = custody.enter_context(
                    controller_source(source, common_dir, observed.head)
                )
                if source_git.checkout_observation() != observed:
                    raise DriverError(
                        "candidate controller source changed during capture"
                    )
            else:
                custody.enter_context(controller_source_use(controller))
            from tools import molt_dev_controller

            # The caller may be an editable image whose site startup imports
            # its checkout even under -I. Use the controller's existing locked,
            # non-editable dependency environment while its source is retained.
            molt_dev_controller._sync_environment(controller.root)
            runner_prefix = python_command(str(controller.driver))
            runner_prefix[0] = str(venv_python(controller.root / ENVIRONMENT_NAME))
            if "-I" not in runner_prefix:
                runner_prefix.insert(1, "-I")
            if "-B" not in runner_prefix:
                runner_prefix.insert(1, "-B")
            pid = launch_detached(
                paths.runners / name,
                [
                    *runner_prefix,
                    "queue",
                    "run",
                    "--repo",
                    str(repo),
                    "--remote",
                    args.remote,
                    "--branch",
                    args.branch,
                    "--candidate-controller",
                ],
                cwd=repo,
                environment=os.environ,
                controller=controller,
            )
        _ok(f"candidate runner {name!r} spawned from {controller.root}: pid {pid}")
        _ok(f"runner evidence: {paths.runners / name} (run.log/rc)")
        return
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
        ]
    )
    drv.cmd_detached_run(detached)


def _runner_exited_without(paths: QueuePaths, request_id: str) -> Path | None:
    """The newest runner's state dir when no runner can still land the request.

    A runner holds its lock while it lands a batch and waits for it briefly
    between batches (`_take_runner_lock`), and its supervisor writes the exit
    status last (the detached-run protocol). So a free lock, no runner that may
    still land (`_runner_may_land`) and a still-queued request mean nothing will
    land it. Every runner counts, not only the newest: an older one can be
    waiting for the lock while a newer one has exited.
    """
    if not (paths.requests / f"{request_id}.json").is_file():
        return None
    if not _runner_lock_free(paths):
        return None
    runners = sorted(paths.runners.iterdir()) if paths.runners.is_dir() else []
    if not runners:
        return None
    newest = runners[-1]
    if any(_runner_may_land(state, newest=state == newest) for state in runners):
        return None
    if read_result(paths, request_id) is not None:
        return None
    return newest


def _runner_may_land(state: Path, *, newest: bool) -> bool:
    """Whether the runner recorded in ``state`` has not finished.

    It has not written ``rc``, and the process its ``pid`` file names is
    alive and is that runner's supervisor: its argv names the runner's own
    ``worker.json``, so a reused pid never counts. The launcher publishes
    ``pid`` once the supervisor runs; before that only the newest runner can
    still be starting. The supervisor is the launching ``land`` process's
    child, so after it dies it stays an unreaped zombie of the process that
    waits here: the probe counts a zombie dead and never reaps. A ``pid`` file
    that names no process (0, negative, beyond the OS width) proves no exit,
    so it reads as one not yet written.
    """
    if (state / "rc").is_file():
        return False
    try:
        pid = int((state / "pid").read_text(encoding="utf-8"))
    except (OSError, ValueError):
        return newest
    if not process_model.valid_pid(pid):
        return newest
    try:
        if not process_model.pid_alive(pid):
            return False
    except OSError:
        return True  # Unknown does not establish supervisor death.
    argv = process_model.process_command_argv(pid)
    if argv is None:
        return True  # This platform does not show argv; liveness decides.
    return any(
        Path(part).name == "worker.json" and Path(part).parent.name == state.name
        for part in argv
    )


def _await_landing(paths, request_id: str) -> int:
    delay = _AWAIT_POLL_MIN_S
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
        time.sleep(delay)
        delay = min(_AWAIT_POLL_MAX_S, delay * 2)
    status = LandingStatus(result["status"])
    if status is LandingStatus.LANDED:
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
    if sandbox.resolve() not in git.worktree_paths():
        raise DriverError(f"{sandbox} is not a worktree of this repository")
    return drv.Git(sandbox.resolve())


def _land_one_batch(args: argparse.Namespace, git, paths) -> bool:
    """Land one batch; True transfers unfinished retirement to published main."""

    drv = _driver()
    git.fetch(args.remote, args.branch)
    upstream = drv.require_git_object_id(
        git.rev_parse(f"{args.remote}/{args.branch}"), label="queue upstream"
    )
    ready = pending(paths)
    from tools.molt_dev_change_ids import landing_representation

    already_landed = [
        request
        for request in ready
        if landing_representation(git, request.head, upstream).landed
    ]
    if already_landed:
        if not _after_landing(git.repo, already_landed, args, paths, upstream):
            return True
    ready = pending(paths)
    if not ready:
        # The same plan owner drops closed requests after a retirement repin.
        next_batch(paths, ready)
        return False
    batch = next_batch(paths, ready)
    sandbox = _sandbox_git(paths, git, upstream)

    def run_git(arguments):
        proc = sandbox._run(list(arguments), check=False)
        return GitResult(proc.returncode, proc.stdout, proc.stderr)

    _step(f"landing batch of {len(batch)} on {git.short(upstream)}")
    candidate = build_candidate(run_git, upstream, batch)
    for request, reason in candidate.conflicted:
        record_result(paths, request, status=LandingStatus.CONFLICT, detail=reason)
        _warn(f"{request.id} {reason}")
    if not candidate.included:
        retire_batch_plan(paths, batch)
        return False
    integration = drv.build_parser().parse_args(
        [
            "integrate",
            "--repo",
            str(sandbox.repo),
            "--remote",
            args.remote,
            "--branch",
            args.branch,
            "--expected-upstream",
            upstream,
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
        return False
    except CandidateFailure as exc:
        if len(candidate.included) > 1:
            _warn(f"batch of {len(candidate.included)} failed; bisecting")
            bisect(paths, batch, candidate.included)
            return False
        (request,) = candidate.included
        record_result(paths, request, status=LandingStatus.FAILED, detail=str(exc))
        # The runner's log is where an operator looks first: say why.
        _fail(f"{request.id} failed: {exc}")
        retire_batch_plan(paths, batch)
        return False
    landed = drv.require_git_object_id(sandbox.head_sha(), label="landed batch")
    if not _after_landing(sandbox.repo, candidate.included, args, paths, landed):
        return True
    retire_batch_plan(paths, batch)
    return False


def _after_landing(
    sandbox: Path, requests, args: argparse.Namespace, paths, landed: str
) -> bool:
    """Refresh the shared hooks from the published main; retire landed lanes.

    Publish terminal success only after the managed lane retirement finishes.
    """
    drv = _driver()
    # A candidate facade from an older launcher may still execute in a lane
    # this batch just published. Leave the exact enrolled requests pending;
    # published main can finish retirement after the runner lock is released.
    source_paths = [_COMMANDS.repo_root.resolve(), Path.cwd().resolve()]
    if drv.__file__ is not None:
        source_paths.append(Path(drv.__file__).resolve())
    if any(
        (request.branch or "").startswith("lane/")
        and any(
            host_path_is_within(source, Path(request.worktree).resolve())
            for source in source_paths
        )
        for request in requests
    ):
        _warn("published batch needs a surviving controller for managed retirement")
        return False
    from molt import checkout_provisioning as provisioning

    try:
        hooks = provisioning.checkout_input(
            "git-hooks", sandbox, guard_source_root=_COMMANDS.repo_root
        )
        if provisioning.declares(hooks):
            provisioning.ensure_input(hooks)
    except provisioning.ProvisioningError as exc:
        _warn(f"managed hooks not refreshed after the landing: {exc}")
    from tools import molt_dev_lanes

    for request in requests:
        status = LandingStatus.LANDED
        detail = "published; unmanaged checkout retained"
        if (request.branch or "").startswith("lane/"):
            slug = request.branch.removeprefix("lane/")
            retire = drv.build_parser().parse_args(
                [
                    "lane",
                    "retire",
                    slug,
                    "--repo",
                    str(sandbox),
                    "--remote",
                    args.remote,
                    "--branch",
                    args.branch,
                ]
            )
            try:
                rc = molt_dev_lanes.cmd_lane_retire(
                    retire,
                    expected_tip=request.head,
                    expected_worktree=Path(request.worktree),
                    lock_wait_s=molt_dev_lanes.LANDED_RETIREMENT_LOCK_WAIT_S,
                )
                if rc != EXIT_OK:
                    raise DriverError(
                        f"lane retire exited {rc}; inspect its preserved evidence"
                    )
                detail = "published; managed lane retirement complete"
            except (DriverError, OSError) as exc:
                status = LandingStatus.LANDED_RETIREMENT_FAILED
                detail = (
                    f"source landed in {landed}; retirement failed: {exc}. "
                    f"Integration owner: reconcile custody, then run `molt_dev.py lane "
                    f"retire {slug}` from a surviving checkout; do not rerun landing gates."
                )
                _warn(detail)
        record_result(paths, request, status=status, landed=landed, detail=detail)
    return True


def cmd_queue_run(args: argparse.Namespace) -> int:

    drv = _driver()
    repo = Path(args.repo).resolve()
    git = drv.Git(repo)
    paths = queue_paths(repo)
    paths.root.mkdir(parents=True, exist_ok=True)
    controller = integration_controller(args)
    # The runner serves every lane: the sandbox's gates use the sandbox's own
    # environment, never the venv selection of whoever started the runner.
    for key in ("MOLT_VENV", "UV_PROJECT_ENVIRONMENT", "VIRTUAL_ENV"):
        os.environ.pop(key, None)

    retire_old_records(paths, now=time.time())
    while True:
        handle = _take_runner_lock(paths)
        if handle is None:
            _ok("another landing runner is active")
            return EXIT_OK
        needs_repin = False
        try:
            if pending(paths):
                _step(f"landing runner ({controller})")
                needs_repin = _land_one_batch(args, git, paths)
        finally:
            drv._release_file_lock(handle)
        if needs_repin:
            main = drv._main_worktree(git)
            if main is None:
                raise DriverError(
                    "cannot find a surviving main checkout for retirement"
                )
            # A forwarded POSIX process inherits cwd. Move both it and the
            # queue's repo coordinate outside the lane before publication can
            # release that lane; no candidate-adoption flag crosses this hop.
            os.chdir(main)
            repin_args = argparse.Namespace(**{**vars(args), "repo": str(main)})
            return run_from_pinned_controller(
                repin_args,
                ["queue", "run", "--remote", args.remote, "--branch", args.branch],
            )
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

    if args.recent < 0:
        raise DriverError("--recent must be non-negative", code=EXIT_USAGE)
    paths = queue_paths(Path(args.repo).resolve())
    requests = pending(paths)
    plan = (
        json.loads(paths.plan.read_text(encoding="utf-8"))["groups"]
        if paths.plan.is_file()
        else []
    )
    report = {
        "pending": [request.id for request in requests],
        "plan": plan,
        "recent": recorded_results(paths, limit=args.recent),
    }
    print(json.dumps(report, indent=2, sort_keys=True))
    return EXIT_OK


def cmd_queue_withdraw(args: argparse.Namespace) -> int:
    """Close pending requests without testing them, between batches."""

    drv = _driver()
    paths = queue_paths(Path(args.repo).resolve())
    paths.root.mkdir(parents=True, exist_ok=True)
    handle = _take_runner_lock(paths)
    if handle is None:
        _fail(
            "a landing batch is running; withdraw after it ends "
            "(`molt_dev.py queue status` shows the batch plan)"
        )
        return EXIT_FAIL
    try:
        request_ids = list(dict.fromkeys(args.request_ids))
        queued = {request.id for request in pending(paths)}
        unknown = [item for item in request_ids if item not in queued]
        if unknown:
            raise DriverError("not pending landing requests: " + ", ".join(unknown))
        for request_id in request_ids:
            request = withdraw(paths, request_id, reason=args.reason)
            _ok(f"withdrew {request.id} ({request.branch or request.head[:12]})")
    finally:
        drv._release_file_lock(handle)
    # A `land` that probed the lock while this command held it started no
    # runner; start one for any request still queued, as `land` would.
    if pending(paths):
        main_checkout = drv._main_worktree(drv.Git(Path(args.repo).resolve()))
        if main_checkout is None:
            raise DriverError(
                "cannot find the main worktree to run the landing runner from"
            )
        _start_queue_runner(paths, main_checkout, args)
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
        help="wait for landing and managed lane retirement; stop editing the enrolled lane",
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
    add_pinned_controller_arguments(p_land)
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
    add_pinned_controller_arguments(p_run)
    p_run.set_defaults(func=cmd_queue_run)
    p_status = queue_sub.add_parser("status", help="pending requests and results")
    add_repo_arg(p_status)
    p_status.add_argument(
        "--recent",
        type=int,
        default=10,
        help="newest results to show (0 omits history)",
    )
    p_status.set_defaults(func=cmd_queue_status)
    p_withdraw = queue_sub.add_parser(
        "withdraw", help="close pending requests without testing them"
    )
    add_repo_arg(p_withdraw)
    add_remote_branch(p_withdraw)
    p_withdraw.add_argument(
        "--candidate-controller",
        action="store_true",
        help="run this checkout's driver instead of main's pinned controller",
    )
    add_pinned_controller_arguments(p_withdraw)
    p_withdraw.add_argument(
        "request_ids", nargs="+", metavar="REQUEST_ID", help="ids that `land` printed"
    )
    p_withdraw.add_argument(
        "--reason", required=True, help="why; the result records it as its detail"
    )
    p_withdraw.set_defaults(func=cmd_queue_withdraw)
