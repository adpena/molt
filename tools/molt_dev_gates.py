"""Landing-gate selection and execution for ``tools/molt_dev.py integrate``.

Explicit pytest nodes are collected first under their selected environment,
without executing tests or replacing any gate. Then gates run in declared
order and the first failure in that order halts the
landing. A gate's nonzero exit raises ``GateFailure``, a ``CandidateFailure``
the landing queue bisects on; a stop of the runner itself stays a plain
``DriverError``. The leading run of always gates (read-only whole-repository checks
without Cargo outputs) executes through the one proof executor
(``tools/proof_executor.py``): its admission bounds the block by the plan's
executor ceiling and the gates' resource class, both capped by
``molt.host_capacity``, and it leaves a receipt with each gate's duration and
peak RSS. Every later gate runs alone (docs/design/host_capacity.md, "One
scheduler"). Every gate writes its streams to files while it runs
(`prepare_gate_streams`); the terminal shows a failure's bounded tail.
"""

from __future__ import annotations

from collections.abc import Callable, Mapping
from dataclasses import dataclass, field, replace
import datetime as dt
import hashlib
import json
import os
from pathlib import Path
import shlex
import shutil
import sys
import threading
import time
import tomllib
from typing import TYPE_CHECKING, Any, Protocol

from tools import proof_executor, proof_receipts
from tools.dirty_tree_policy import is_ignored as _is_ignored
from tools.molt_dev_common import EXIT_USAGE, DriverError, GateFailure, _say, _warn
from tools.memory_guard_core.reporting import NAMED_RECORD_MIN_AGE_S

if TYPE_CHECKING:
    from molt.disk_capacity import DiskCapacityProbe
    from molt.process_environment import ProcessEnvironment
    from tools.proof_plan import PytestMemoryUnits

# Local always gates share the repository-policy resource class with their CI
# commands; a policy without that class admits up to the executor ceiling.
GATE_RESOURCE_CLASS = "repository-policy"
LOCAL_GATE_FAMILY = "local_integration"
# The largest always gate's process tree peaks below 1 GiB (structural_audit
# 0.69 GB, generators 0.50 GB on the 2026-10 tree): the host-capacity unit.
CONCURRENT_GATE_BYTES = 1 << 30


@dataclass
class GateRule:
    name: str
    globs: list[str]
    gates: list[str]
    description: str = ""
    pytest: list[str] = field(default_factory=list)
    # (selection argv, targets): each runs as its own pytest invocation.
    pytest_select: list[tuple[tuple[str, ...], tuple[str, ...]]] = field(
        default_factory=list
    )


# The toolchains of a gate that only reads the candidate's files: linters,
# formatters, type checkers, generators and policy audits. `land` runs those
# gates in the lane before it queues the lane (molt_dev_precheck); a gate that
# builds with Cargo, runs compiled programs or is a pytest session waits for the
# queue's batch, which runs every selected gate once.
PRECHECK_TOOLCHAINS = frozenset({"generators", "git", "python", "rustfmt", "uv"})


@dataclass(frozen=True)
class SelectedGate:
    command: str | tuple[str, ...]
    toolchains: tuple[str, ...]
    # An always gate is a read-only whole-repository check; the leading run of
    # them executes as one concurrent block (run_selected_gates).
    concurrent: bool = False
    # The merged pytest session or one pytest_select cohort.
    pytest: bool = False
    # Selected node admission; unfiltered cohorts collect only explicit nodes.
    # Successful collection never replaces this gate's mandatory execution.
    collection_command: tuple[str, ...] | None = None

    @property
    def precheck(self) -> bool:
        """Whether `land` runs this gate in the lane before queueing it."""
        return not self.pytest and frozenset(self.toolchains) <= PRECHECK_TOOLCHAINS


@dataclass
class GateConfig:
    pytest_memory_units: "PytestMemoryUnits"
    rules: list[GateRule] = field(default_factory=list)
    always: list[str] = field(default_factory=list)
    prerequisites: dict[str, tuple[str, tuple[str, ...]]] = field(default_factory=dict)
    prerequisite_policy: dict = field(default_factory=dict)
    root: Path = field(default_factory=lambda: Path(__file__).resolve().parents[1])
    # The proof executor's declared admission: its ceiling and resource classes.
    executor_max_workers: int | None = None
    resource_limits: dict[str, int] = field(default_factory=dict)

    @staticmethod
    def load(path: Path) -> "GateConfig":
        if not path.exists():
            raise DriverError(
                f"gate manifest missing: {path}. The change-class->gates manifest "
                "is required for `integrate`.",
                code=EXIT_USAGE,
            )
        return GateConfig.parse(
            path.read_text(encoding="utf-8"), str(path), root=path.parent.parent
        )

    @staticmethod
    def parse(source: str, label: str, *, root: Path | None = None) -> "GateConfig":
        from tools.proof_plan import (
            PytestMemoryUnits,
            local_gate_commands,
            local_pytest_select_errors,
            local_pytest_selections,
            local_pytest_target_errors,
            local_rule_field_error,
        )

        root = root or Path(__file__).resolve().parents[1]
        try:
            data = tomllib.loads(source)
        except tomllib.TOMLDecodeError as exc:
            raise DriverError(f"gate manifest is not valid TOML ({label}): {exc}")
        rules: list[GateRule] = []
        for entry in data.get("rule", []):
            if field_error := local_rule_field_error(entry):
                raise DriverError(field_error)
            name = entry.get("name")
            globs = entry.get("globs")
            gates = entry.get("gates")
            if not isinstance(name, str) or not name:
                raise DriverError(f"gate rule missing string `name`: {entry!r}")
            if not isinstance(globs, list) or not all(
                isinstance(g, str) for g in globs
            ):
                raise DriverError(f"gate rule {name!r} `globs` must be a list of str")
            if not isinstance(gates, list) or not all(
                isinstance(g, str) for g in gates
            ):
                raise DriverError(f"gate rule {name!r} `gates` must be a list of str")
            pytest_targets = entry.get("pytest", [])
            errors = local_pytest_target_errors(name, pytest_targets, root=root)
            errors += local_pytest_select_errors(
                name, entry.get("pytest_select", []), root=root
            )
            if errors:
                raise DriverError("\n".join(errors))
            rules.append(
                GateRule(
                    name=name,
                    globs=globs,
                    gates=gates,
                    description=str(entry.get("description", "")),
                    pytest=pytest_targets,
                    pytest_select=local_pytest_selections(entry),
                )
            )
        always = data.get("always", [])
        if not isinstance(always, list) or not all(isinstance(g, str) for g in always):
            raise DriverError("gate manifest `always` must be a list of str")
        try:
            memory_units = PytestMemoryUnits(data.get("command", ()))
            commands = local_gate_commands(data, memory_units=memory_units)
        except ValueError as exc:
            raise DriverError(f"invalid local gate policy: {exc}") from exc
        ceiling = data.get("executor_max_workers")
        if ceiling is not None and (type(ceiling) is not int or ceiling < 1):
            raise DriverError("gate manifest `executor_max_workers` must be positive")
        resource_limits = {
            str(entry["name"]): int(entry["max_parallel"])
            for entry in data.get("resource_policy", [])
        }
        return GateConfig(
            pytest_memory_units=memory_units,
            rules=rules,
            always=list(always),
            prerequisites=commands,
            prerequisite_policy={"toolchain_policy": data.get("toolchain_policy", [])},
            root=Path(__file__).resolve().parents[1] if root is None else root,
            executor_max_workers=ceiling,
            resource_limits=resource_limits,
        )

    def admit(
        self,
        touched: list[str],
        matched: list[GateRule],
        extra: list[str],
        *,
        environ: Mapping[str, str],
    ) -> dict[str, dict]:
        """Admit the union once under ``environ``, the gates' baseline;
        execution projects each gate from that same baseline.

        Returns the admitted toolchain fingerprints the gate receipt records.
        """
        selected_owners = {f"local.{rule.name}" for rule in matched}
        if touched:
            selected_owners.add("local.always")
        unknown = sorted(
            set(extra) - {command for command, _ in self.prerequisites.values()}
        )
        if unknown:
            raise DriverError(
                "extra gates lack declared prerequisite custody; declare them in "
                "proof_plan.toml before selection: " + ", ".join(unknown)
            )
        return self._admit_owned(
            {
                identifier: tools
                for identifier, (command, tools) in self.prerequisites.items()
                if identifier.rsplit(".", 1)[0] in selected_owners or command in extra
            },
            environ=environ,
        )

    def admit_gates(
        self, gates: list[SelectedGate], *, environ: Mapping[str, str]
    ) -> dict[str, dict]:
        """Admit exactly the toolchains these gates declare (a landing precheck)."""
        owned: dict[str, tuple[str, ...]] = {}
        for gate in gates:
            owned[
                shlex.join(gate.command)
                if isinstance(gate.command, tuple)
                else gate.command
            ] = gate.toolchains
        return self._admit_owned(owned, environ=environ)

    def _admit_owned(
        self, selected: Mapping[str, tuple[str, ...]], *, environ: Mapping[str, str]
    ) -> dict[str, dict]:
        """Admit the union of each owner's toolchain closure once."""
        from tools.proof_plan import (
            admit_toolchains,
            load_toolchain_policies,
            toolchain_closure,
        )

        try:
            names = tuple(
                dict.fromkeys(name for tools in selected.values() for name in tools)
            )
            policies = load_toolchain_policies(
                self.prerequisite_policy, root=self.root, names=names
            )
            owners: dict[str, list[str]] = {}
            for identifier, tools in selected.items():
                for name in toolchain_closure(policies, tools):
                    owners.setdefault(name, []).append(identifier)
            return admit_toolchains(
                policies,
                tuple(owners),
                environ=environ,
                owners=owners,
                root=self.root,
            ).fingerprints
        except (ValueError, OSError, KeyError) as exc:
            raise DriverError(
                f"selected gate prerequisites failed; executed=0: {exc}"
            ) from exc

    def select(
        self, touched: list[str], *, extra: list[str] | None = None
    ) -> tuple[list[SelectedGate], list[GateRule]]:
        """Select commands with their declared prerequisites intact.

        Deduplicated commands union their owners' tools. Only rules contributing
        to the merged pytest invocation supply its tools; each selector cohort
        keeps its own. Command binding happens after this policy selection.
        """
        from tools.proof_plan import local_pytest_argv, local_pytest_collection_argv

        selected: list[SelectedGate] = []
        indices: dict[str, int] = {}
        seen_pytest: set[str] = set()
        matched_rules: list[GateRule] = []
        pytest_targets: list[str] = []
        pytest_tools: dict[str, None] = {}
        pytest_index: int | None = None
        pytest_commands: set[str] = set()

        def command_key(command: str | tuple[str, ...]) -> str:
            return command if isinstance(command, str) else shlex.join(command)

        def add(
            command: str | tuple[str, ...],
            tools: tuple[str, ...],
            *,
            concurrent: bool = False,
            pytest: bool = False,
            collection_command: tuple[str, ...] | None = None,
        ) -> None:
            key = command_key(command)
            if key in indices:
                index = indices[key]
                prior = selected[index]
                selected[index] = replace(
                    prior,
                    toolchains=tuple(dict.fromkeys((*prior.toolchains, *tools))),
                    pytest=prior.pytest or pytest,
                    collection_command=prior.collection_command or collection_command,
                )
            else:
                indices[key] = len(selected)
                selected.append(
                    SelectedGate(command, tools, concurrent, pytest, collection_command)
                )

        def tools(identifier: str) -> tuple[str, ...]:
            return self.prerequisites[identifier][1]

        if touched:
            for index, command in enumerate(self.always):
                add(command, tools(f"local.always.{index}"), concurrent=True)
        for rule in self.rules:
            if not any(_is_ignored(path, tuple(rule.globs)) for path in touched):
                continue
            matched_rules.append(rule)
            owner = f"local.{rule.name}"
            for index, command in enumerate(rule.gates):
                add(command, tools(f"{owner}.{index}"))
            if rule.pytest:
                if pytest_index is None:
                    pytest_index = len(selected)
                pytest_tools.update(dict.fromkeys(tools(f"{owner}.pytest")))
            for index, (select, targets) in enumerate(rule.pytest_select):
                command = local_pytest_argv(
                    targets, select, memory_units=self.pytest_memory_units
                )
                pytest_commands.add(command_key(command))
                add(
                    command,
                    tools(f"{owner}.pytest-select-{index}"),
                    pytest=True,
                    collection_command=(
                        local_pytest_collection_argv(command)
                        if any("::" in target for target in targets)
                        else None
                    ),
                )
            for target in rule.pytest:
                if target not in seen_pytest:
                    seen_pytest.add(target)
                    pytest_targets.append(target)
        # Extras are raw policy commands, not bound argv or unowned shell text.
        for command in extra or ():
            owners = [
                required
                for raw, required in self.prerequisites.values()
                if raw == command
            ]
            if not owners:
                raise DriverError(
                    "extra gates lack declared prerequisite custody; declare them in "
                    "proof_plan.toml before selection: " + command
                )
            add(
                command,
                tuple(dict.fromkeys(name for owner in owners for name in owner)),
            )
        if pytest_index is not None:
            command = local_pytest_argv(
                pytest_targets, memory_units=self.pytest_memory_units
            )
            pytest_commands.add(command_key(command))
            # File-only targets already have a validated path. Admit named nodes
            # without importing every file in the merged mandatory cohort again.
            named_targets = tuple(target for target in pytest_targets if "::" in target)
            size = len(selected)
            add(
                command,
                tuple(pytest_tools),
                pytest=True,
                collection_command=(
                    local_pytest_collection_argv(
                        local_pytest_argv(
                            named_targets, memory_units=self.pytest_memory_units
                        )
                    )
                    if named_targets
                    else None
                ),
            )
            if len(selected) != size:
                selected.insert(pytest_index, selected.pop())
        # Run declared source/metadata and compilation controls before Python
        # consumers. This also moves already-required Cargo gates earlier; it
        # adds no commands or builds and preserves each phase's relative order.
        explicit: list[SelectedGate] = []
        consumers: list[SelectedGate] = []
        for gate in selected:
            phase = (
                consumers if command_key(gate.command) in pytest_commands else explicit
            )
            phase.append(gate)
        selected = explicit + consumers
        return selected, matched_rules

    def environments(
        self, gates: list[SelectedGate], *, baseline: Mapping[str, str]
    ) -> dict[tuple[str, ...], ProcessEnvironment]:
        """Project once per declared tool selection, never from aggregate admission."""
        from molt.process_environment import ProcessEnvironment
        from tools.proof_plan import (
            load_toolchain_policies,
            project_toolchains,
            toolchain_closure,
        )

        try:
            names = tuple(
                dict.fromkeys(name for gate in gates for name in gate.toolchains)
            )
            policies = load_toolchain_policies(
                self.prerequisite_policy, root=self.root, names=names
            )
            environments = {}
            for gate in gates:
                if gate.toolchains in environments:
                    continue
                projection = project_toolchains(
                    policies,
                    toolchain_closure(policies, gate.toolchains),
                    environ=baseline,
                    root=self.root,
                )
                environments[gate.toolchains] = ProcessEnvironment(baseline).updated(
                    projection.delta
                )
            return environments
        except (ValueError, OSError, KeyError) as exc:
            raise DriverError(
                f"selected gate environment failed; executed=0: {exc}"
            ) from exc

    def cargo_outputs(
        self,
        environments: dict[tuple[str, ...], ProcessEnvironment],
    ) -> dict[tuple[str, ...], tuple[Path, ...]]:
        """Outputs of gate owners explicitly declaring Cargo capability.

        Declared capability is conservative: it does not prove that a Cargo
        child launches. Actual argv/output admission remains with each child.
        Transitive tool availability (for example generator metadata queries)
        does not impose a compilation-space requirement.
        """
        from molt.cargo_execution_policy import cargo_output_paths

        outputs = {}
        for tools, environment in environments.items():
            if "cargo" in tools:
                outputs[tools] = cargo_output_paths((), cwd=self.root, env=environment)
        return outputs


class Gate(Protocol):
    command: str | tuple[str, ...]
    toolchains: tuple[str, ...]
    concurrent: bool
    collection_command: tuple[str, ...] | None


class Progress(Protocol):
    def emit(self, event: str, **details: object) -> None: ...


@dataclass(frozen=True)
class GateOutcome:
    returncode: int
    stdout: str
    stderr: str
    peak_rss_bytes: int | None


#: A failed gate's terminal excerpt: the last lines of each stream, each cut
#: to a width, so one oversized line cannot flood the caller's context.
FAILURE_TAIL_LINES = 20
FAILURE_TAIL_WIDTH = 400


def gate_output_dir(receipt_path: Path) -> Path:
    """This run's gate logs, beside the receipt, named by UTC time and pid so
    a run never reuses another's."""
    stamp = time.strftime("%Y%m%dT%H%M%SZ", time.gmtime())
    return receipt_path.with_suffix(".d") / f"{stamp}-{os.getpid()}"


def retire_gate_logs(receipts: Path, *, now: float) -> None:
    """Delete gate run directories idle past the named-record window, then
    the receipt directories they leave empty; a landing calls this once and
    warns when one cannot go. Retiring whole receipts belongs to HF-435."""
    failures: list[str] = []
    for run in receipts.glob("*.d/*"):
        try:
            if run.is_dir() and now - run.stat().st_mtime > NAMED_RECORD_MIN_AGE_S:
                shutil.rmtree(run)
                if not any(run.parent.iterdir()):
                    run.parent.rmdir()
        except OSError as exc:
            failures.append(f"{run}: {type(exc).__name__}: {exc}")
    if failures:
        _warn(
            f"{len(failures)} idle gate log run(s) could not be retired; "
            f"first: {failures[0]}"
        )


def prepare_gate_streams(
    directory: Path, ordinal: int
) -> tuple[tuple[Path, Path] | None, str | None]:
    """The files a gate writes its stdout and stderr to while it runs, or
    ``(None, error)`` when they cannot be prepared; the gate then runs with
    its streams in memory, and a failure shows them whole."""
    streams = (
        directory / f"gate-{ordinal:03d}.stdout.log",
        directory / f"gate-{ordinal:03d}.stderr.log",
    )
    try:
        directory.mkdir(parents=True, exist_ok=True)
    except OSError as exc:
        return None, f"{type(exc).__name__}: {exc}"
    return streams, None


def stream_record(
    streams: tuple[Path, Path] | None, error: str | None
) -> dict[str, str]:
    """Where a gate's streams are, for its receipt record and progress event."""
    if streams is None:
        return {"output_error": str(error)}
    return {"stdout_path": str(streams[0]), "stderr_path": str(streams[1])}


def gate_failure_report(outcome: GateOutcome, record: Mapping[str, str]) -> str:
    """What the terminal shows of a failed gate: where its full streams are
    and the bounded end of each; when they could not go to files, both
    streams whole, so the reason is never lost."""
    if "stdout_path" not in record:
        return (
            f"    gate output not kept ({record.get('output_error')}); "
            "its streams follow whole\n"
            f"--- stdout ---\n{outcome.stdout}\n--- stderr ---\n{outcome.stderr}\n"
        )

    def tail(text: str) -> list[str]:
        lines = text.rstrip("\n").splitlines()[-FAILURE_TAIL_LINES:]
        return [
            line
            if len(line) <= FAILURE_TAIL_WIDTH
            else f"{line[:FAILURE_TAIL_WIDTH]}... ({len(line)} chars)"
            for line in lines
        ]

    parts = [
        f"    full stdout: {record['stdout_path']}\n",
        f"    full stderr: {record['stderr_path']}\n",
    ]
    for name, text in (("stdout", outcome.stdout), ("stderr", outcome.stderr)):
        lines = tail(text)
        if lines:
            parts.append(f"    --- {name}, last {len(lines)} lines ---\n")
            parts.extend(f"{line}\n" for line in lines)
    return "".join(parts)


@dataclass(frozen=True)
class GateRuntime:
    """What one landing's gates run against; integrate builds it once."""

    repo: Path
    environments: Mapping[tuple[str, ...], ProcessEnvironment]
    cargo_outputs: Mapping[tuple[str, ...], tuple[Path, ...]]
    capacity_requirements: Mapping[tuple[str, ...], int]
    progress: Progress
    check_source: Callable[[], None]
    source_changes: Callable[[], tuple[str, ...]]
    source_identity: Callable[[], dict[str, str]]
    policy_text: str
    executor_max_workers: int | None
    resource_limits: Mapping[str, int]
    toolchains: Mapping[str, dict[str, Any]]
    receipt_path: Path
    # This run's gate logs (`gate_output_dir`); the receipt names each file.
    output_dir: Path
    run_captured_gate: Callable[..., GateOutcome]
    observe_capacity: Callable[..., Any]
    say: Callable[[str], None]
    ok: Callable[[str], None]


def display(command: str | tuple[str, ...]) -> str:
    import shlex

    return command if isinstance(command, str) else shlex.join(command)


def gate_time_summary(gate_seconds: list[tuple[float, str]], *, top: int = 8) -> str:
    """The gates' total time and the slowest ones, so the costliest gates
    are visible on every landing."""
    total = sum(seconds for seconds, _ in gate_seconds)
    lines = [f"    gates: {len(gate_seconds)} in {total:.1f} s; slowest:"]
    ranked = sorted(gate_seconds, key=lambda item: item[0], reverse=True)[:top]
    for seconds, cmd in ranked:
        lines.append(f"      {seconds:7.1f} s  {cmd}")
    return "\n".join(lines)


def gate_details(ordinal: int, gate: Gate) -> dict[str, object]:
    cmd = gate.command
    return {
        "gate_ordinal": ordinal,
        "command": (
            {"kind": "shell", "source": cmd}
            if isinstance(cmd, str)
            else {"kind": "argv", "argv": list(cmd)}
        ),
        "toolchains": list(gate.toolchains),
    }


def gate_failure(
    rc: int | None,
    elapsed: float,
    shown: str,
    gate_seconds: list[tuple[float, str]],
    record: Mapping[str, str] | None = None,
) -> GateFailure:
    """A selected gate exited nonzero: the candidate failed (the landing queue
    bisects on this type). A fault of the runner itself stays a DriverError."""
    kept = (
        f"    full stderr: {record['stderr_path']}\n"
        if record and "stderr_path" in record
        else ""
    )
    return GateFailure(
        f"gate failed (exit {rc}) after {elapsed:.1f} s: {shown}\n"
        + kept
        + gate_time_summary(gate_seconds)
        + "\n    Integration halts before push; fix the gate and re-run."
    )


def concurrent_gate_limit() -> int:
    """Gates this host runs at once, from the host capacity authority."""
    from molt import host_capacity

    return host_capacity.worker_count(
        bytes_per_worker=CONCURRENT_GATE_BYTES,
        headroom_bytes=host_capacity.headroom_bytes(host_capacity.memory_capacity()),
    )


def admit_selected_output_capacity(
    outputs: dict[tuple[str, ...], tuple[Path, ...]],
    environments: dict[tuple[str, ...], ProcessEnvironment],
) -> dict[tuple[str, ...], int]:
    """Check current volumes before any gate, without forecasting growth."""
    from molt.disk_capacity import minimum_headroom_bytes, require_build_capacity

    requirements = {}
    observed: set[tuple[tuple[Path, ...], int]] = set()
    for tools, paths in outputs.items():
        environment = environments[tools]
        required = minimum_headroom_bytes(environment)
        requirements[tools] = required
        key = (paths, required)
        if key in observed:
            continue
        receipt = require_build_capacity(paths, env=environment)
        observed.add(key)
        _say(
            "    selected output capacity: "
            + json.dumps(
                {
                    **receipt.as_dict(),
                    "scope": "current Cargo-capable selected outputs",
                    "remaining_campaign_growth_bytes": None,
                    "reserved_bytes": 0,
                },
                sort_keys=True,
            )
        )
    if outputs:
        _say("    remaining campaign growth unknown; capacity is not reserved")
    return requirements


def observe_gate_capacity(
    paths: tuple[Path, ...],
    *,
    required_bytes: int,
    phase: str,
    gate: str,
    before: tuple[DiskCapacityProbe, ...] = (),
) -> tuple[DiskCapacityProbe, ...]:
    """Record interval observations; telemetry cannot change a gate verdict."""
    from molt.disk_capacity import probe_capacity

    try:
        probes = tuple(
            probe_capacity(path, required_bytes=required_bytes) for path in paths
        )
        previous = {probe.requested_path: probe.free_bytes for probe in before}
        changes = {
            str(probe.requested_path): (
                probe.free_bytes - prior
                if isinstance(prior := previous.get(probe.requested_path), int)
                and isinstance(probe.free_bytes, int)
                else None
            )
            for probe in probes
        }
        _say(
            "    gate capacity: "
            + json.dumps(
                {
                    "scope": "Cargo-capable gate interval; not exclusive allocation",
                    "phase": phase,
                    "gate": gate,
                    "probes": [probe.as_dict() for probe in probes],
                    "free_bytes_delta": changes if phase == "after" else {},
                },
                sort_keys=True,
            )
        )
        return probes
    except Exception as exc:
        _warn(
            f"gate capacity observation unavailable ({phase}): {type(exc).__name__}: {exc}"
        )
        return ()


def run_selected_gates(
    gates: list[Gate],
    runtime: GateRuntime,
    gate_seconds: list[tuple[float, str]],
    *,
    host_limit: Callable[[], int] = concurrent_gate_limit,
) -> None:
    """Admit named nodes before builds, then execute every selected gate."""
    for ordinal, gate in enumerate(gates, start=1):
        if gate.collection_command is not None:
            _run_serial_gate(ordinal, gate, runtime, gate_seconds, collection=True)
    block = 0
    while (
        block < len(gates)
        and gates[block].concurrent
        and not runtime.cargo_outputs.get(gates[block].toolchains)
    ):
        block += 1
    if block >= 2:
        _run_executor_block(gates[:block], runtime, gate_seconds, host_limit())
    else:
        block = 0
    for ordinal, gate in enumerate(gates[block:], start=block + 1):
        _run_serial_gate(ordinal, gate, runtime, gate_seconds)


def _run_serial_gate(
    ordinal: int,
    gate: Gate,
    runtime: GateRuntime,
    gate_seconds: list[tuple[float, str]],
    *,
    collection: bool = False,
) -> None:
    cmd = gate.collection_command if collection else gate.command
    assert cmd is not None
    phase = "collection" if collection else "gate"
    shown = display(cmd)
    runtime.check_source()
    paths = () if collection else runtime.cargo_outputs.get(gate.toolchains, ())
    required = runtime.capacity_requirements.get(gate.toolchains, 0)
    before = (
        runtime.observe_capacity(
            paths, required_bytes=required, phase="before", gate=shown
        )
        if paths
        else ()
    )
    started = time.monotonic()
    details = gate_details(ordinal, gate)
    if collection:
        details["command"] = {"kind": "argv", "argv": list(cmd)}
    runtime.progress.emit(f"{phase}_started", **details)
    rc = None
    outcome: GateOutcome | None = None
    streams, stream_error = prepare_gate_streams(
        runtime.output_dir / "collection" if collection else runtime.output_dir,
        ordinal,
    )
    record = stream_record(streams, stream_error)
    try:
        outcome = runtime.run_captured_gate(
            cmd,
            runtime.repo,
            runtime.environments[gate.toolchains],
            lambda: False,
            streams,
        )
        rc = outcome.returncode
    finally:
        elapsed = time.monotonic() - started
        runtime.progress.emit(
            f"{phase}_finished",
            **details,
            **record,
            returncode=rc,
            duration_seconds=elapsed,
        )
        if paths:
            runtime.observe_capacity(
                paths,
                required_bytes=required,
                phase="after",
                gate=shown,
                before=before,
            )
    if not collection:
        gate_seconds.append((elapsed, shown))
    runtime.check_source()
    if rc != 0:
        runtime.say(f"    gate: {shown}")
        sys.stderr.write(gate_failure_report(outcome, record))
        sys.stderr.flush()
        timings = [*gate_seconds, (elapsed, shown)] if collection else gate_seconds
        raise gate_failure(rc, elapsed, shown, timings, record)
    runtime.ok(
        f"pytest collection passed in {elapsed:.1f} s (execution still required): {shown}"
        if collection
        else f"gate passed in {elapsed:.1f} s: {shown}"
    )


@dataclass(frozen=True)
class _ResourcePolicy:
    name: str
    max_parallel: int


@dataclass(frozen=True)
class _LocalGatePlan:
    receipt_schema: str
    executor_max_workers: int
    resource_policies: tuple[_ResourcePolicy, ...]
    toolchain_policies: tuple[Any, ...] = ()


@dataclass(frozen=True)
class _LocalGateCommand:
    id: str
    family: str
    data: dict[str, Any]
    argv: tuple[str, ...]
    gate: Any
    ordinal: int
    dependencies: tuple[str, ...] = ()
    evidence_outputs: tuple[str, ...] = ()


@dataclass
class _BlockState:
    first_failure: int
    outcomes: dict[str, GateOutcome | BaseException] = field(default_factory=dict)
    lock: threading.Lock = field(default_factory=threading.Lock)

    def moot(self, ordinal: int) -> bool:
        return ordinal > self.first_failure

    def fail(self, ordinal: int) -> None:
        with self.lock:
            self.first_failure = min(self.first_failure, ordinal)


def _local_plan(runtime: GateRuntime, block: int, host: int) -> _LocalGatePlan:
    ceiling = min(host, block, runtime.executor_max_workers or host)
    max_parallel = min(
        ceiling, runtime.resource_limits.get(GATE_RESOURCE_CLASS, ceiling)
    )
    return _LocalGatePlan(
        receipt_schema=proof_receipts.RECEIPT_SCHEMA,
        executor_max_workers=ceiling,
        resource_policies=(_ResourcePolicy(GATE_RESOURCE_CLASS, max_parallel),),
    )


def _local_command(ordinal: int, gate: Gate, cell: str) -> _LocalGateCommand:
    cmd = gate.command
    argv = ("/bin/sh", "-c", cmd) if isinstance(cmd, str) else tuple(cmd)
    return _LocalGateCommand(
        id=f"local.always.{ordinal}",
        family=LOCAL_GATE_FAMILY,
        data={
            "cell": cell,
            "tiers": ["local-integration"],
            "resource_class": GATE_RESOURCE_CLASS,
            # Local always gates keep their unbounded deadline.
            "timeout_budget": "none",
            "timeout_seconds": 0,
        },
        argv=argv,
        gate=gate,
        ordinal=ordinal,
    )


def _run_executor_block(
    gates: list[Gate],
    runtime: GateRuntime,
    gate_seconds: list[tuple[float, str]],
    host: int,
) -> None:
    """Run the block through the proof executor; report it in declared order."""
    from tools.proof_plan import _normalized_arch, _normalized_os

    plan = _local_plan(runtime, len(gates), host)
    cell = f"{_normalized_os()}-{_normalized_arch()}-local"
    commands = [
        _local_command(ordinal, gate, cell)
        for ordinal, gate in enumerate(gates, start=1)
    ]
    state = _BlockState(first_failure=len(gates) + 1)

    def run_command(
        _plan: Any, command: _LocalGateCommand, cancel: threading.Event
    ) -> dict[str, Any]:
        record: dict[str, Any] = {
            **proof_receipts.command_record(command),
            "started_at": dt.datetime.now(dt.UTC).isoformat(),
            "duration_seconds": 0.0,
            "peak_rss_bytes": None,
            "cache_disposition": "none",
            "guard_metrics_schema": None,
            "returncode": None,
        }
        # A moot gate's record is partition-scoped: it must not stop the
        # executor, which would cancel the earlier gates still running.
        moot = {
            "failure_scope": "partition",
            "failure_reason": "an earlier gate failed",
        }
        if state.moot(command.ordinal):
            return {
                **record,
                **moot,
                "status": "skipped",
                "skip_reason": "earlier gate failed",
            }
        runtime.progress.emit(
            "gate_started", **gate_details(command.ordinal, command.gate)
        )
        started = time.monotonic()
        streams, stream_error = prepare_gate_streams(
            runtime.output_dir, command.ordinal
        )
        record.update(stream_record(streams, stream_error))
        try:
            outcome = runtime.run_captured_gate(
                command.gate.command,
                runtime.repo,
                runtime.environments[command.gate.toolchains],
                lambda: cancel.is_set() or state.moot(command.ordinal),
                streams,
            )
        except BaseException as exc:  # reported in declared order below
            state.outcomes[command.id] = exc
            state.fail(command.ordinal)
            raise
        record["duration_seconds"] = round(time.monotonic() - started, 6)
        record["peak_rss_bytes"] = outcome.peak_rss_bytes
        record["returncode"] = outcome.returncode
        state.outcomes[command.id] = outcome
        if outcome.returncode == 0:
            return {**record, "status": "success"}
        if state.moot(command.ordinal):
            return {**record, **moot, "status": "cancelled"}
        if cancel.is_set():
            return {**record, "status": "cancelled"}
        state.fail(command.ordinal)
        # A gate failure does not stop gates declared before it: the landing
        # reports the first failure in declared order.
        return {
            **record,
            "status": "failure",
            "failure_scope": "partition",
            "failure_reason": f"exit {outcome.returncode}",
        }

    runtime.check_source()
    authority = hashlib.sha256(b"molt.local-integration-gates.v1\0")
    authority.update(runtime.policy_text.encode("utf-8"))
    block_started = time.monotonic()
    proof_executor.execute_commands(
        plan,
        commands,
        runtime.receipt_path,
        _source_tree_changes=runtime.source_changes,
        toolchain_fingerprints=lambda _policies, _names: dict(runtime.toolchains),
        _authority_sha256=lambda _plan: authority.hexdigest(),
        _source_identity=runtime.source_identity,
        _normalized_os=_normalized_os,
        _normalized_arch=_normalized_arch,
        _required_toolchains=lambda command: command.gate.toolchains,
        _run_command=run_command,
        _cache_disposition=lambda _command: "none",
    )
    wall = time.monotonic() - block_started
    receipt = json.loads(runtime.receipt_path.read_text(encoding="utf-8"))
    records = {record["id"]: record for record in receipt.get("commands", ())}

    def stopped(where: str) -> DriverError:
        # The runner, not the candidate, stopped the block.
        errors = "; ".join(receipt.get("errors", ()) or ("no reason recorded",))
        return DriverError(
            f"gate block stopped {where}: {errors}; receipt {runtime.receipt_path}"
        )

    for command in commands:
        outcome = state.outcomes.get(command.id)
        record = records.get(command.id, {})
        status = record.get("status")
        shown = display(command.gate.command)
        if status in {"cancelled", "skipped"} and not isinstance(
            outcome, BaseException
        ):
            # Only a stop caused elsewhere cancels a gate; it never passed.
            runtime.say(f"    gate {status}: {shown}")
            continue
        if outcome is None:
            # Every earlier gate passed or was stopped, and this one never ran.
            raise stopped(f"before {shown}")
        elapsed = float(record.get("duration_seconds") or 0.0)
        runtime.progress.emit(
            "gate_finished",
            **gate_details(command.ordinal, command.gate),
            **{
                key: record[key]
                for key in ("stdout_path", "stderr_path", "output_error")
                if key in record
            },
            returncode=record.get("returncode"),
            duration_seconds=elapsed,
        )
        if isinstance(outcome, BaseException) or outcome.returncode != 0:
            runtime.say(f"    gate: {shown}")
        if isinstance(outcome, GateOutcome) and outcome.returncode != 0:
            sys.stderr.write(gate_failure_report(outcome, record))
            sys.stderr.flush()
        gate_seconds.append((elapsed, shown))
        if isinstance(outcome, BaseException):
            raise outcome
        # The executor observed the source after every completion; a gate
        # that changed it reports the canonical clean-source refusal.
        if record.get("source_tree_state_after") == "changed":
            runtime.check_source()
        if status == "failure" and outcome.returncode != 0:
            raise gate_failure(outcome.returncode, elapsed, shown, gate_seconds, record)
        if status != "success":
            raise stopped(f"at {shown} ({status})")
        runtime.ok(f"gate passed in {elapsed:.1f} s: {shown}")
    if receipt.get("status") != "success":
        raise stopped("without a classified gate failure")
    runtime.say(
        f"    gate block: {len(commands)} gates in {wall:.1f} s wall through the "
        f"proof executor, at most {plan.executor_max_workers} at once; "
        f"receipt {runtime.receipt_path}"
    )
