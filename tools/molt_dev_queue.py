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

import json
import os
import shutil
import tempfile
from collections.abc import Callable, Sequence
from dataclasses import asdict, dataclass
from datetime import UTC, datetime
from pathlib import Path

from tools.molt_dev_common import DriverError

REQUEST_SCHEMA = "molt.landing-request.v1"
RESULT_SCHEMA = "molt.landing-result.v1"
PLAN_SCHEMA = "molt.landing-plan.v1"
# A batch larger than this costs more in bisection than one more gate run.
MAX_BATCH = 8
# Retention: finished runner logs beyond the newest RUNNER_LOGS_KEPT, and
# results older than RESULT_DAYS, retire when a runner starts.
RUNNER_LOGS_KEPT = 20
RESULT_DAYS = 30


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
            raise ValueError(f"{path} is not a {REQUEST_SCHEMA} record")
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
    paths: QueuePaths, *, head: str, worktree: Path, branch: str | None
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
