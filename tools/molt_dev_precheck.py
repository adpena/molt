"""Run a landing's static gates in its lane before the queue takes it.

The queue's runner replays queued lanes onto main and runs each batch's gates
once; a failed batch bisects, which costs every lane in it another gate run.
A gate that only reads the candidate's files (``SelectedGate.precheck``:
toolchains within ``PRECHECK_TOOLCHAINS``, not a pytest session) is cheap and
answers for this lane alone, so ``land`` runs those gates in the lane first. A
lint, format, generator or policy failure then stops this lane before it
queues, as GitHub's merge queue admits only changes whose required checks
passed and Zuul runs its check pipeline before its gate pipeline. Gates that
build with Cargo, run compiled programs or run pytest stay with the batch.
"""

from __future__ import annotations

from pathlib import Path
from types import ModuleType

from tools.molt_dev_common import _say, _step
from tools.molt_dev_gates import (
    concurrent_gate_limit,
    gate_time_summary,
    run_selected_gates,
)


def _driver() -> ModuleType:
    """The canonical driver; imported late because it imports the queue."""
    from tools import molt_dev

    return molt_dev


def precheck_landing(
    git, repo: Path, *, head: str, upstream: str, session_id: str
) -> int:
    """Run ``head``'s static gates against its merge base; return how many ran.

    The gates, their toolchains and their environment come from the same
    authorities ``integrate`` uses (``select_landing_gates``,
    ``gate_environment``); only the selection differs. A failing gate raises
    ``GateFailure`` and nothing is queued.
    """
    drv = _driver()
    base = drv.require_git_object_id(
        git._run(["merge-base", head, upstream]).stdout.strip(),
        label="landing merge base",
    )
    policy, config, _touched, gates, _matched = drv.select_landing_gates(
        git, repo, head, base, extra=[]
    )
    static = [gate for gate in gates if gate.precheck]
    _step(
        f"precheck in the lane ({len(static)} of {len(gates)} selected gate(s); "
        "the queue's batch runs the rest)"
    )
    if not static:
        return 0
    drv._assert_integration_source(git, head)
    environments = config.environments(
        static, baseline=drv.gate_environment(repo, session_id=session_id)
    )
    runtime = drv._gate_runtime(
        git,
        head,
        repo=repo,
        policy=policy,
        config=config,
        environments=environments,
        cargo_outputs={},
        capacity_requirements={},
        progress=drv._IntegrationProgress(),
        toolchains=config.admit_gates(static),
        receipt_kind="precheck",
    )
    gate_seconds: list[tuple[float, str]] = []
    run_selected_gates(
        static, runtime, gate_seconds, host_limit=lambda: concurrent_gate_limit()
    )
    _say(gate_time_summary(gate_seconds))
    return len(static)
