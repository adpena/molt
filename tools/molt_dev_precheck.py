"""Repair a landing, then try it in its lane before the queue takes it.

The queue's runner replays queued lanes onto main and runs each batch's gates
once; one failing lane holds the runner for its whole gate run. ``land``
therefore does two things in the lane first.

1. **Repair.** The lane's own proof plan declares the mechanical repairs its
   read-only gates ask for (``repair``: encoding pins, formatting, generators
   the change reaches). ``repair_landing`` applies them against the merge base
   and commits what they change as one commit, so a stale projection never
   costs a precheck or a batch.
2. **Precheck.** ``precheck_landing`` runs the static gates (toolchains within
   ``PRECHECK_TOOLCHAINS``, not a pytest session) and the try run: the
   dependency graph's test cohorts (changed tests and their direct readers) and
   the gates of ``precheck = true`` rules (changed-crate Clippy), within
   ``PRECHECK_BUDGET``. A failure stops ``land`` before it queues anything.
   The budget refuses whatever has not finished when it runs out and names
   each refused gate; the batch still runs every selected gate.

Chromium's commit queue and Google's TAP run a fast presubmit of the affected
targets before the merge, as Zuul runs its check pipeline before its gate
pipeline. Other Cargo builds, compiled programs and the rules' curated pytest
cohorts stay with the batch.
"""

from __future__ import annotations

import time
from dataclasses import dataclass
from pathlib import Path
from types import ModuleType

from tools.driver_errors import CandidateFailure, DriverError
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


def _merge_base(git, head: str, upstream: str) -> str:
    return _driver().require_git_object_id(
        git._run(["merge-base", head, upstream]).stdout.strip(),
        label="landing merge base",
    )


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
    base = _merge_base(git, head, upstream)
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


@dataclass(frozen=True)
class PrecheckResult:
    """What one lane precheck ran; ``refused`` names what the budget stopped."""

    passed: tuple[str, ...]
    refused: tuple[str, ...]
    seconds: float
    budget: PrecheckBudget

    @property
    def complete(self) -> bool:
        return not self.refused

    def refusal(self) -> str:
        return (
            f"precheck budget of {self.budget.seconds:.0f} s ran out after "
            f"{self.seconds:.0f} s: {len(self.passed)} gate(s) passed; it refused "
            f"{len(self.refused)} gate(s) that were still running or not yet "
            "started, and the queue's batch runs them:\n"
            + "\n".join(f"      {command}" for command in self.refused)
        )


def precheck_landing(
    git,
    repo: Path,
    *,
    head: str,
    upstream: str,
    session_id: str,
    budget: PrecheckBudget | None = None,
) -> PrecheckResult:
    """Run ``head``'s precheck gates against its merge base within ``budget``.

    The gates, their toolchains and their environment come from the same
    authorities ``integrate`` uses (``select_landing_gates``, ``prepare``,
    ``gate_environment``); only the selection differs. A failing gate raises
    ``GateFailure`` and nothing is queued. ``budget`` defaults to the policy's
    ``PRECHECK_BUDGET``.
    """
    drv = _driver()
    from tools.molt_dev_queue import queue_paths, recent_postsubmit_failures

    budget = budget or molt_dev_policy.PRECHECK_BUDGET
    started = time.monotonic()
    base = _merge_base(git, head, upstream)
    policy, config, _touched, gates, _matched = select_landing_gates(
        git,
        repo,
        head,
        base,
        extra=[],
        recent_failures=recent_postsubmit_failures(queue_paths(repo)),
    )
    selected = [gate for gate in gates if gate.precheck]
    tried = sum(1 for gate in selected if not gate.static)
    _step(
        f"precheck in the lane ({len(selected)} of {len(gates)} selected gate(s), "
        f"{tried} of them the try run; budget {budget.seconds:.0f} s; "
        "the queue's batch runs every gate)"
    )
    if not selected:
        return PrecheckResult((), (), time.monotonic() - started, budget)
    drv._assert_integration_source(git, head)
    baseline = drv.gate_environment(repo, session_id=session_id)
    prepared = config.prepare(
        selected,
        baseline=baseline,
        prepare_python=lambda environment, needed: drv.prepare_gate_environment(
            repo, environment, requires_python=needed
        ),
    )
    selected = prepared.runnable()
    try:
        cargo_outputs = config.cargo_outputs(prepared.environments, selected)
        capacity = admit_selected_output_capacity(cargo_outputs, prepared.environments)
    except (ValueError, OSError, KeyError) as exc:
        raise DriverError(
            f"selected output capacity failed; executed=0: {exc}"
        ) from exc
    from molt.dx import RunContext

    for context in prepared.environments.values():
        RunContext(repo).materialize_environment(
            context.environment, protected_python_root=context.python_root
        )
    drv._assert_integration_source(git, head)
    deadline = started + budget.seconds
    runtime = drv._gate_runtime(
        git,
        head,
        repo=repo,
        policy=policy,
        config=config,
        environments=prepared.environments,
        cargo_outputs=cargo_outputs,
        capacity_requirements=capacity,
        progress=drv._IntegrationProgress(),
        toolchains=prepared.fingerprints,
        receipt_kind="precheck",
        cancelled=lambda: time.monotonic() >= deadline,
        deferred=prepared.deferred,
    )
    # Gates that reached an outcome; a failure raises GateFailure, so every
    # one of them passed. Only the deadline cancels a precheck.
    gate_seconds: list[tuple[float, str]] = []
    try:
        run_selected_gates(
            selected, runtime, gate_seconds, host_limit=lambda: concurrent_gate_limit()
        )
    except GateCancellation:
        pass
    _say(gate_time_summary(gate_seconds))
    finished = {command for _seconds, command in gate_seconds}
    shown = [display(gate.command) for gate in selected]
    result = PrecheckResult(
        passed=tuple(command for command in shown if command in finished),
        refused=tuple(command for command in shown if command not in finished),
        seconds=time.monotonic() - started,
        budget=budget,
    )
    if not result.complete:
        _warn(result.refusal())
    return result
