"""Repair a landing, then try it in its lane before the queue takes it.

The queue's runner replays queued lanes onto main and runs each batch's gates
once; one failing lane holds the runner for its whole gate run. ``land``
therefore does two things in the lane first.

1. **Repair.** The lane's own proof plan declares the mechanical repairs its
   read-only gates ask for (``repair``: encoding pins, formatting, generators
   the change reaches). ``repair_landing`` applies them against the merge base
   and commits what they change as one commit, so a stale projection never
   costs a precheck or a batch. Repairs have no deadline: they sit outside
   the try-run budget (a full generator write took 16-25 s, 2026-11-02).
2. **Precheck.** ``precheck_landing`` runs the static gates (toolchains within
   ``PRECHECK_TOOLCHAINS``, not a pytest session) and the try run: the
   dependency graph's test cohorts (changed tests and their direct readers) and
   the gates of ``precheck = true`` rules (changed-crate Clippy), within
   ``PRECHECK_BUDGET``. A failure stops ``land`` before it queues anything.
   The static gates run to their verdict without a deadline; the budget
   clock starts with the try run and refuses whatever has not finished when
   it runs out, naming each refused gate, as a disk-capacity refusal does;
   the batch still runs every selected gate.

Chromium's commit queue and Google's TAP run a fast presubmit of the affected
targets before the merge, as Zuul runs its check pipeline before its gate
pipeline. Other Cargo builds, compiled programs and the rules' curated pytest
cohorts stay with the batch.
"""

from __future__ import annotations

import json
import os
import time
from collections.abc import Callable
from dataclasses import dataclass
from enum import StrEnum
from pathlib import Path
from types import ModuleType

from tools.driver_errors import CandidateFailure, DriverError
from tools.git_identity import merge_base
from tools.molt_dev_common import _ok, _say, _step, _warn
from tools import molt_dev_policy
from tools.molt_dev_policy import (
    GATES_CONFIG_NAME,
    GateConfig,
    PrecheckBudget,
    SelectedGate,
    select_landing_gates,
)
from tools.molt_dev_gates import (
    CancelReason,
    GateCancellation,
    admit_selected_output_capacity,
    concurrent_gate_limit,
    display,
    gate_time_summary,
    run_selected_gates,
)

REPAIR_SUBJECT = "Apply the landing repairs the read-only gates ask for"


def _driver() -> ModuleType:
    """The canonical driver; imported late because it imports the queue."""
    from tools import molt_dev

    return molt_dev


def repair_steps(config: GateConfig, *, base: str) -> list[SelectedGate]:
    """The plan's repair commands, bound to ``base``, with their toolchains."""
    from tools.proof_plan import bind_local_gate_command

    try:
        return [
            SelectedGate(
                bind_local_gate_command(command, upstream_commit=base),
                config.prerequisites[f"local.repair.{index}"].toolchains,
            )
            for index, command in enumerate(config.repair)
        ]
    except (KeyError, ValueError) as exc:
        raise CandidateFailure(f"invalid repair declaration: {exc}") from exc


def run_repairs(repo: Path, config: GateConfig, *, base: str, session_id: str) -> None:
    """Run the plan's repairs in ``repo`` in order, admitted as gates are.

    Repairs rewrite the checkout, so they never defer: a missing toolchain
    stops here with the gate's own diagnostic.
    """
    drv = _driver()
    steps = repair_steps(config, base=base)
    prepared = config.prepare(
        steps,
        baseline=drv.gate_environment(repo, session_id=session_id),
        prepare_python=lambda environment, needed: drv.prepare_gate_environment(
            repo, environment, requires_python=needed
        ),
    )
    if prepared.deferred:
        raise DriverError(
            "repair cannot defer; "
            + "; ".join(item.diagnostic() for item in prepared.deferred)
        )
    for step in steps:
        if drv.run_gate(step.command, repo, prepared.environments[step.toolchains]):
            raise CandidateFailure(
                f"repair failed: {display(step.command)}; the checkout keeps "
                "what earlier steps changed: commit or discard it, then land again"
            )


class RepairRefusal(DriverError):
    """``land`` refused to commit a repair; nothing was committed or queued."""


class EnrolledLaneRefusal(RepairRefusal):
    """A pending landing request pins this branch; a repair would move it."""

    def __init__(self, request_id: str, head: str) -> None:
        super().__init__(
            f"landing request {request_id} pins this checkout at {head}; a repair "
            "commit would move an enrolled branch: wait for its result or withdraw it"
        )
        self.request_id = request_id


class RepairScopeRefusal(RepairRefusal):
    """The checkout changed paths no declared repair may write."""

    def __init__(self, paths: tuple[str, ...]) -> None:
        super().__init__(
            "the checkout changed paths outside the lane's diff since its merge "
            "base and the generator manifest's declared outputs (another writer, "
            "or a generator output family the manifest does not list); nothing "
            "was committed:\n" + "\n".join(f"      {path}" for path in paths)
        )
        self.paths = paths


def _repair_scope(git, head: str, base: str) -> frozenset[str]:
    """Paths a repair may write: the lane's own diff and declared outputs."""
    import tomllib

    lane = git._run(["diff", "--name-only", "-z", "--no-renames", base, head]).stdout
    manifest = git._run(["show", f"{head}:tools/generator_manifest.toml"], check=False)
    outputs: set[str] = set()
    if manifest.returncode == 0:
        for row in tomllib.loads(manifest.stdout).get("generator", ()):
            outputs.update(str(path) for path in row.get("outputs", ()))
    return frozenset(path for path in lane.split("\0") if path) | outputs


def _pending_request(git, repo: Path):
    """The pending landing request that pins this checkout, if any."""
    from tools.molt_dev_queue import pending, queue_paths

    branch = git.symbolic_head()
    for request in pending(queue_paths(repo)):
        if (branch is not None and request.branch == branch) or Path(
            request.worktree
        ).resolve() == repo.resolve():
            return request
    return None


def _changed_paths(git) -> list[str]:
    """Tracked edits, deletions and new files in the checkout, sorted."""
    paths = {
        path
        for arguments in (
            ["ls-files", "-z", "--modified", "--deleted"],
            ["ls-files", "-z", "--others", "--exclude-standard"],
        )
        for path in git._run(arguments).stdout.split("\0")
        if path
    }
    return sorted(paths)


def repair_landing(
    git, repo: Path, *, head: str, upstream: str, session_id: str
) -> str:
    """Apply ``head``'s declared repairs and commit any change; return the tip.

    The checkout is clean before (``_assert_integration_source``), so every
    path the repairs leave changed is theirs; the commit names exactly those
    paths. The installed Git hooks run on it like on any commit; a hook that
    refuses it stops the landing.
    """
    drv = _driver()
    drv._assert_integration_source(git, head)
    policy = git._run(["show", f"{head}:tools/{GATES_CONFIG_NAME}"]).stdout
    try:
        config = GateConfig.parse(
            policy, f"{head}:tools/{GATES_CONFIG_NAME}", root=repo
        )
    except DriverError as exc:
        raise CandidateFailure(str(exc), code=exc.code) from exc
    if not config.repair:
        return head
    if (request := _pending_request(git, repo)) is not None:
        raise EnrolledLaneRefusal(request.id, request.head)
    base = merge_base(head, upstream, run_git=lambda args: git._run(args).stdout)
    _step(f"repair in the lane ({len(config.repair)} declared step(s))")
    started = time.monotonic()
    run_repairs(repo, config, base=base, session_id=session_id)
    paths = _changed_paths(git)
    if not paths:
        _ok(f"repairs changed nothing ({time.monotonic() - started:.1f} s)")
        drv._assert_integration_source(git, head)
        return head
    if git.head_sha() != head:
        raise DriverError("landing source HEAD moved during its repairs")
    scope = _repair_scope(git, head, base)
    if outside := tuple(path for path in paths if path not in scope):
        raise RepairScopeRefusal(outside)
    # Literal, NUL-framed pathspecs keep unusual names exact and a large
    # generated family below Windows' command-line limit.
    framed = "".join(f"{path}\0" for path in paths)
    spec = ["--pathspec-from-file=-", "--pathspec-file-nul"]
    git._run(["--literal-pathspecs", "add", "--all", *spec], input_text=framed)
    committed = git._run(
        ["--literal-pathspecs", "commit", "-q", "-m", REPAIR_SUBJECT, *spec],
        check=False,
        input_text=framed,
    )
    if committed.returncode != 0:
        raise CandidateFailure(
            f"the repair commit failed (exit {committed.returncode}); a Git hook "
            "or Git refused it, and the checkout keeps the staged repairs:\n"
            + "\n".join(
                stream.strip()
                for stream in (committed.stdout, committed.stderr)
                if stream.strip()
            )
        )
    repaired = drv.require_git_object_id(git.head_sha(), label="repaired landing")
    drv._assert_integration_source(git, repaired)
    _ok(
        f"committed {len(paths)} repaired path(s) as {git.short(repaired)} "
        f"({time.monotonic() - started:.1f} s): "
        + ", ".join(paths[:8])
        + (" ..." if len(paths) > 8 else "")
    )
    return repaired


class RefusalReason(StrEnum):
    """Why the try run left gates to the queue's batch."""

    BUDGET = "budget"
    DISK_CAPACITY = "disk capacity"


@dataclass(frozen=True)
class TryRunRefusal:
    """The try-run gates the precheck did not finish, and why."""

    reason: RefusalReason
    gates: tuple[SelectedGate, ...]
    detail: str
    # The refused gates as a landing executor record (tools/trunk_landing.py
    # --record): a rerun prepares each gate's interpreter and toolchains
    # exactly as the landing does, so no caller names an interpreter.
    record: Path | None = None


@dataclass(frozen=True)
class GateRun:
    """One executed gate block: ``gates[i]`` wrote ``gate-{i+1:03d}.stdout.log``
    under ``output_dir`` (run_selected_gates numbers gates from 1)."""

    phase: str  # "static" or "try-run"
    output_dir: Path
    gates: tuple[SelectedGate, ...]

    def log(self, gate: SelectedGate) -> Path:
        return self.output_dir / f"gate-{self.gates.index(gate) + 1:03d}.stdout.log"


@dataclass(frozen=True)
class PrecheckResult:
    """What one lane precheck ran; ``refusal`` names what it left undone."""

    passed: tuple[SelectedGate, ...]
    refusal: TryRunRefusal | None
    budget: PrecheckBudget
    # The executed blocks, for a landing receipt's per-gate logs and counts.
    runs: tuple[GateRun, ...] = ()
    # Seconds per gate, keyed by its displayed command.
    seconds: tuple[tuple[str, float], ...] = ()
    # Selected gates the precheck does not run: the queue batch's.
    waived: tuple[SelectedGate, ...] = ()
    # Try-run gates an earlier run of this lane passed on an older base and
    # that main's move since then cannot affect (``rerun``); the caller's
    # receipt carries their earlier records.
    carried: tuple[SelectedGate, ...] = ()
    # Data readers of the change that presubmit leaves to postsubmit.
    postsubmit_only: tuple[str, ...] = ()

    @property
    def complete(self) -> bool:
        return self.refusal is None

    def report(self) -> str:
        assert self.refusal is not None
        return (
            f"precheck try run refused by {self.refusal.reason.value}: "
            f"{self.refusal.detail}; {len(self.passed)} gate(s) passed; it refused "
            f"{len(self.refusal.gates)} gate(s) that were still running or not yet "
            "started, and the queue's batch runs them:\n"
            + "\n".join(f"      {display(gate.command)}" for gate in self.refusal.gates)
            + (
                ""
                if self.refusal.record is None
                else "\n    rerun them on this commit with the landing executor:\n"
                f"      uv run python tools/trunk_landing.py --record "
                f"{self.refusal.record} --output {self.refusal.record.parent / 'rerun'}"
            )
        )


def _write_refused_record(
    git, repo: Path, head: str, refused: tuple[SelectedGate, ...], output: Path
) -> Path:
    """Write ``refused`` as a landing executor record beside the try run's logs."""
    from tools.trunk_landing import landing_request

    record = output / "refused-gates.json"
    request = landing_request(head, git.rev_parse(f"{head}^{{tree}}"), refused)
    payload = {"landing": request, "checkout": str(repo)}
    record.parent.mkdir(parents=True, exist_ok=True)
    record.write_text(json.dumps(payload, indent=2) + "\n", encoding="utf-8")
    return record


def precheck_landing(
    git,
    repo: Path,
    *,
    head: str,
    upstream: str,
    session_id: str,
    budget: PrecheckBudget | None = None,
    static_only: bool = False,
    rerun: Callable[[SelectedGate], bool] | None = None,
) -> PrecheckResult:
    """Run ``head``'s static gates, then its try run within ``budget``.

    ``static_only`` stops after the static gates. ``land --fast`` passes
    ``FAST_LANDING_BUDGET`` and pushes only a complete result. ``rerun``, when
    given, keeps only the try-run gates it accepts; the others are ``carried``
    (an earlier run passed them and the caller proved the change since cannot
    affect them). Static gates always run.

    The gates, their toolchains and their environment come from the same
    authorities ``integrate`` uses (``select_landing_gates``, ``prepare``,
    ``gate_environment``); only the selection differs. The static gates run
    without a deadline: a lint, format, generator or policy failure always
    stops ``land``. The budget clock starts with the try run. A failing gate
    raises ``GateFailure`` and nothing is queued; a budget or disk-capacity
    refusal names the gates it left to the batch. ``budget`` defaults to the
    policy's ``PRECHECK_BUDGET``.
    """
    drv = _driver()
    from tools.molt_dev_queue import queue_paths, recent_postsubmit_failures

    budget = budget or molt_dev_policy.PRECHECK_BUDGET
    base = merge_base(head, upstream, run_git=lambda args: git._run(args).stdout)
    recent = recent_postsubmit_failures(queue_paths(repo))
    policy, config, touched, gates, _matched = select_landing_gates(
        git,
        repo,
        head,
        base,
        extra=[],
        recent_failures=recent,
    )
    # The same selection object that chose the gates (GateConfig memoizes it).
    postsubmit_only = molt_dev_policy.left_to_postsubmit(
        config.graph_selection(touched, recent_failures=recent), gates
    )
    selected = [
        gate for gate in gates if gate.precheck and (gate.static or not static_only)
    ]
    carried: tuple[SelectedGate, ...] = ()
    if rerun is not None:
        carried = tuple(
            gate for gate in selected if not gate.static and not rerun(gate)
        )
        selected = [gate for gate in selected if gate not in carried]
    waived = tuple(
        gate for gate in gates if gate not in selected and gate not in carried
    )
    tried = sum(1 for gate in selected if not gate.static)
    _step(
        f"static gates in the lane ({len(selected)} of {len(gates)} selected gate(s); "
        "no try run)"
        if static_only
        else f"precheck in the lane ({len(selected)} of {len(gates)} selected gate(s), "
        f"{tried} of them the try run within {budget.seconds:.0f} s; "
        "the queue's batch runs every gate)"
    )
    if not selected:
        return PrecheckResult(
            (),
            None,
            budget,
            waived=waived,
            carried=carried,
            postsubmit_only=postsubmit_only,
        )
    drv._assert_integration_source(git, head)
    prepared = config.prepare(
        selected,
        baseline=drv.gate_environment(repo, session_id=session_id),
        prepare_python=lambda environment, needed: drv.prepare_gate_environment(
            repo, environment, requires_python=needed
        ),
    )
    runnable = prepared.runnable()
    static = [gate for gate in runnable if gate.static]
    trial = [gate for gate in runnable if not gate.static]
    from molt.dx import RunContext

    for context in prepared.environments.values():
        RunContext(repo).materialize_environment(
            context.environment, protected_python_root=context.python_root
        )

    def runtime(kind: str, **options):
        drv._assert_integration_source(git, head)
        return drv._gate_runtime(
            git,
            head,
            repo=repo,
            policy=policy,
            config=config,
            environments=prepared.environments,
            progress=drv._IntegrationProgress(),
            toolchains=prepared.fingerprints,
            receipt_kind=kind,
            deferred=prepared.deferred,
            **options,
        )

    gate_seconds: list[tuple[float, str]] = []
    checked = runtime("precheck", cargo_outputs={}, capacity_requirements={})
    if static:
        run_selected_gates(
            static, checked, gate_seconds, host_limit=lambda: concurrent_gate_limit()
        )
    passed = tuple(checked.passed)
    runs = [GateRun("static", checked.output_dir, tuple(static))] if static else []

    def seconds() -> tuple[tuple[str, float], ...]:
        return tuple((shown, elapsed) for elapsed, shown in gate_seconds)

    if not trial:
        _say(gate_time_summary(gate_seconds))
        return PrecheckResult(
            passed,
            None,
            budget,
            tuple(runs),
            seconds(),
            waived=waived,
            carried=carried,
            postsubmit_only=postsubmit_only,
        )
    from tools.trunk_landing import HOST_SETTING

    farm_host = os.environ.get(HOST_SETTING, "").strip()
    if farm_host:
        # The farm host runs every try-run gate its OS can admit on the exact
        # commit and returns its gate logs in the same layout. A gate bound only
        # to this host's portability cells (a Mac cell) runs here after it, as
        # an execution platform routes each action (Bazel). Merge, receipt and
        # push stay here (docs/design/trunk_loop.md, "Opt-in remote landing
        # gates").
        from tools.trunk_landing import host_os, run_remote

        farm_foreign = config.foreign_cells(host_os(repo, farm_host))
        remote = [gate for gate in trial if config.runs_on(gate, farm_foreign)]
        trial = [gate for gate in trial if gate not in remote]
        if remote:
            drv._assert_integration_source(git, head)
            remote_seconds, remote_logs = run_remote(repo, head, remote, host=farm_host)
            gate_seconds.extend(remote_seconds)
            runs.append(GateRun("try-run", remote_logs, tuple(remote)))
            passed = (*passed, *remote)
        if trial:
            _say(
                f"    {len(trial)} gate(s) bound to this host's cells run here: "
                + ", ".join(display(gate.command) for gate in trial)
            )
        else:
            _say(gate_time_summary(gate_seconds))
            return PrecheckResult(
                passed,
                None,
                budget,
                tuple(runs),
                seconds(),
                waived=waived,
                carried=carried,
                postsubmit_only=postsubmit_only,
            )
    from molt.disk_capacity import DiskCapacityError

    # Only the capacity verdict is a refusal; any other fault stays a fault.
    cargo_outputs = config.cargo_outputs(prepared.environments, trial)
    try:
        capacity = admit_selected_output_capacity(cargo_outputs, prepared.environments)
    except DiskCapacityError as exc:
        result = PrecheckResult(
            passed,
            TryRunRefusal(RefusalReason.DISK_CAPACITY, tuple(trial), str(exc)),
            budget,
            tuple(runs),
            seconds(),
            waived=waived,
            carried=carried,
            postsubmit_only=postsubmit_only,
        )
        _warn(result.report())
        return result
    from molt import host_capacity

    # The try run is suite-sized: it takes the host's development slot (it
    # tests the lane, not main, so it never waits out a batch), in arrival
    # order, and its budget starts only once it holds the slot, so waiting
    # never spends the budget (HF-623).
    with host_capacity.host_work_slot(
        host_capacity.host_work_dir(repo),
        label=f"landing try run ({git.short(head)})",
        work=host_capacity.HostWorkClass.DEVELOPMENT,
        say=_say,
    ) as admission:
        if admission.waited_seconds >= 1:
            _say(
                f"    host-work: try run admitted after {admission.waited_seconds:.0f} s"
            )
        started = time.monotonic()
        deadline = started + budget.seconds
        tried_runtime = runtime(
            "try-run",
            cargo_outputs=cargo_outputs,
            capacity_requirements=capacity,
            cancelled=lambda: time.monotonic() >= deadline,
            cancel_reason=CancelReason.PRECHECK_BUDGET,
        )
        refusal = None
        try:
            run_selected_gates(
                trial,
                tried_runtime,
                gate_seconds,
                host_limit=lambda: concurrent_gate_limit(),
            )
        except GateCancellation as cancellation:
            if cancellation.reason is not CancelReason.PRECHECK_BUDGET:
                raise
            refused = tuple(gate for gate in trial if gate not in tried_runtime.passed)
            # The deadline can pass after the last gate finished: nothing refused.
            if refused:
                refusal = TryRunRefusal(
                    RefusalReason.BUDGET,
                    refused,
                    f"{budget.seconds:.0f} s ran out after "
                    f"{time.monotonic() - started:.0f} s of try run"
                    + "".join(
                        f"; guard closure unresolved for {item}"
                        for item in cancellation.unresolved
                    ),
                    record=_write_refused_record(
                        git, repo, head, refused, tried_runtime.output_dir
                    ),
                )
    _say(gate_time_summary(gate_seconds))
    runs.append(GateRun("try-run", tried_runtime.output_dir, tuple(trial)))
    result = PrecheckResult(
        (*passed, *tried_runtime.passed),
        refusal,
        budget,
        tuple(runs),
        seconds(),
        waived=waived,
        carried=carried,
        postsubmit_only=postsubmit_only,
    )
    if refusal is not None:
        _warn(result.report())
    return result
