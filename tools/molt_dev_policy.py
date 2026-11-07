"""The local landing gate policy: rules, their selection and binding.

``tools/proof_plan.toml`` declares the always gates and the local rules. A rule
matches a landing through the paths it globs or, for the plan file itself,
through the plan entries it claims with ``plan_entities``: an edit to the plan
selects only the rules that read what it changed (HF-457). Tests are selected
by what they read: ``tools/test_dependency_graph.py`` derives direct readers
for presubmit and the transitive closure for postsubmit. The test's proof cell
supplies its targets and toolchains. Rules keep the gates, the curated cohorts
of files whose cells need builds, and the edges the graph cannot derive.
``integrate``, the land precheck and the queue select through
``select_landing_gates``, the one selection authority.
"""

from __future__ import annotations

import shlex
import tomllib
from collections.abc import Callable, Collection, Mapping, Sequence
from dataclasses import dataclass, field, replace
from enum import StrEnum
from pathlib import Path
from typing import TYPE_CHECKING, Final, assert_never

from tools.proof_plan import CellId, prerequisite_cell

from molt.process_environment import ProcessEnvironment
from tools.dirty_tree_policy import is_ignored as _is_ignored
from tools.git_identity import changed_paths
from tools.proof_plan import CargoTargetAccess
from tools.driver_errors import EXIT_USAGE, CandidateFailure, DriverError  # noqa: E402

if TYPE_CHECKING:
    from tools.molt_dev import Git
    from tools.test_dependency_graph import (
        DependencyGraph,
        GraphSelection,
        LandingEnvelope,
    )
    from tools.proof_plan import (
        LocalGateDeclaration,
        LocalPytestSelection,
        PlanEntity,
        PytestMemoryUnits,
    )

_REPO_ROOT = Path(__file__).resolve().parents[1]
GATES_CONFIG_NAME = "proof_plan.toml"


class LandingMode(StrEnum):
    PRESUBMIT = "presubmit"
    POSTSUBMIT = "postsubmit"


class LandingClass(StrEnum):
    """What a lane changes, which orders it in the landing queue (HF-711).

    LEDGER: every changed path is in ``LEDGER_PATHS``, and there is at least
    one. CODE: anything else, including a mixed ledger-plus-code diff.
    """

    LEDGER = "ledger"
    CODE = "code"


# POSIX repository paths, compared with git_identity.changed_paths() output.
# tests/test_molt_dev.py holds them equal to finding_status.LEDGER_PATH and
# gen_finding_ids.OUTPUT; importing those here would bind repository imports.
LEDGER_PATHS: Final[frozenset[str]] = frozenset(
    {
        "docs/agent/V1_HANDOFF_FINDINGS.md",
        "tests/findings/finding_ids.py",
    }
)


def landing_class(changed: Collection[str]) -> LandingClass:
    """LEDGER when the change is nonempty and touches only the ledger paths."""
    if changed and LEDGER_PATHS.issuperset(changed):
        return LandingClass.LEDGER
    return LandingClass.CODE


class _EveryPlanEntity:
    """A plan edit whose changed entries cannot be read: every rule matches."""


EVERY_PLAN_ENTITY = _EveryPlanEntity()


@dataclass
class GateRule:
    name: str
    globs: list[str]
    gates: list[str]
    description: str = ""
    pytest: list[str] = field(default_factory=list)
    # Each selection runs as its own invocation with resolved prerequisites.
    pytest_select: list[LocalPytestSelection] = field(default_factory=list)
    # The plan entries this rule reads (proof_plan.local_rule_plan_claims).
    plan_entities: frozenset[PlanEntity] = frozenset()
    # `land` runs this rule's gates in the lane before it queues the lane.
    precheck: bool = False


# The toolchains of a gate that only reads the candidate's files: linters,
# formatters, type checkers, generators and policy audits. `land` runs those
# gates in the lane before it queues the lane (molt_dev_precheck), with the
# lane's try run: the dependency graph's test cohorts (changed tests and their
# direct readers) and the gates of rules that declare `precheck = true`
# (changed-crate Clippy). Other Cargo builds, compiled programs and the rules'
# curated pytest cohorts wait for the queue's batch, which runs every selected
# gate once.
PRECHECK_TOOLCHAINS = frozenset({"generators", "git", "python", "rustfmt", "uv"})


@dataclass(frozen=True)
class PrecheckBudget:
    """The wall-clock limit of one lane try run, including any allowed retry.

    The static gates and the repairs before them run outside it. The try run
    stops at the limit: its running gates are cancelled through
    their guards and later gates never start. `land` names every gate the
    limit refused; the queue's batch still runs each of them.
    """

    seconds: float

    def __post_init__(self) -> None:
        if not self.seconds > 0:
            raise ValueError("a precheck budget must be positive")


# Owner correction, 2026-11-05 19:50Z: presubmit targets 600 s p90 by
# selecting changed tests and their actual readers, never by a larger budget.
# Static/repair wall time remains visible outside this try-run guard; the
# whole-presubmit p90 must be qualified from the retained run measurements.
PRECHECK_BUDGET = PrecheckBudget(seconds=10 * 60)
# The ceiling of one `land --fast` attempt's try run (docs/design/
# landing_queue.md, section 9). A fast landing pushes only a complete run, so
# a refusal at this ceiling fails the landing instead of queueing its gates.
FAST_LANDING_BUDGET = PrecheckBudget(seconds=90 * 60)


@dataclass(frozen=True)
class SelectedGate:
    command: str | tuple[str, ...]
    toolchains: tuple[str, ...]
    # An always gate is a read-only whole-repository check; the leading run of
    # them executes as one concurrent block (run_selected_gates).
    concurrent: bool = False
    # The merged pytest session or one pytest_select cohort.
    pytest: bool = False
    # Named-node admission never replaces mandatory execution.
    collection_command: tuple[str, ...] | None = None
    cargo_target_access: CargoTargetAccess | None = None
    pinned_assets: tuple[str, ...] = ()
    cells: tuple[str, ...] = ()
    # Its selector runs it in the lane's try run (a graph test cohort or a
    # `precheck = true` rule); static gates qualify by their toolchains alone.
    try_run: bool = False

    def __post_init__(self) -> None:
        object.__setattr__(
            self,
            "cargo_target_access",
            CargoTargetAccess.resolve(self.toolchains, self.cargo_target_access),
        )

    @property
    def static(self) -> bool:
        """A gate that only reads the candidate's files."""
        return not self.pytest and frozenset(self.toolchains) <= PRECHECK_TOOLCHAINS

    @property
    def precheck(self) -> bool:
        """Whether `land` runs this gate in the lane before queueing it."""
        return self.static or self.try_run


@dataclass(frozen=True)
class GateEnvironment:
    """One operation's selection, not permission to reuse physical admission."""

    environment: ProcessEnvironment
    python_root: Path | None
    managed: bool

    def __post_init__(self) -> None:
        if self.python_root is None and self.managed:
            raise ValueError("an absent gate image cannot be managed")
        if self.python_root is not None and not self.python_root.is_absolute():
            raise ValueError("a prepared gate image must be absolute")

    def overlay(self, values: Mapping[str, str]) -> "GateEnvironment":
        return replace(self, environment=self.environment.updated(values))


class PrerequisiteKind(StrEnum):
    TOOLCHAIN = "toolchain"
    PINNED_ASSET = "pinned asset"
    EXECUTION_CELL = "execution cell"


@dataclass(frozen=True)
class Prerequisite:
    kind: PrerequisiteKind
    name: str
    setup_value: str = ""
    setup_evidence: tuple[str, ...] = ()


@dataclass(frozen=True)
class Runnable:
    command: SelectedGate


@dataclass(frozen=True)
class Deferred:
    command: SelectedGate
    missing: tuple[Prerequisite, ...]
    owner: CellId

    def record(self) -> dict:
        return {
            "command": self.command.command,
            "missing": [
                {"kind": item.kind.value, "name": item.name} for item in self.missing
            ],
            "owner": self.owner,
        }

    def diagnostic(self) -> str:
        command = self.command.command
        display = shlex.join(command) if isinstance(command, tuple) else command
        missing = ", ".join(f"{item.kind.value} {item.name}" for item in self.missing)
        hints = "; ".join(
            f"provide {item.name}: {item.setup_value}; setup: {', '.join(item.setup_evidence)}"
            for item in self.missing
            if item.kind is PrerequisiteKind.TOOLCHAIN
        )
        return (
            f"deferred (unproven here): {display}; missing: {missing}; "
            f"runs in: proof cell {self.owner}" + (f"; {hints}" if hints else "")
        )


GateResolution = Runnable | Deferred


@dataclass(frozen=True)
class PreparedGates:
    environments: dict[tuple[str, ...], GateEnvironment]
    fingerprints: dict[str, dict]
    resolutions: tuple[GateResolution, ...]

    def runnable(self) -> list[SelectedGate]:
        gates = []
        for resolution in self.resolutions:
            match resolution:
                case Runnable(command):
                    gates.append(command)
                case Deferred():
                    pass
                case _:
                    assert_never(resolution)
        return gates

    @property
    def deferred(self) -> tuple[Deferred, ...]:
        return tuple(item for item in self.resolutions if isinstance(item, Deferred))


@dataclass
class GateConfig:
    pytest_memory_units: "PytestMemoryUnits"
    rules: list[GateRule] = field(default_factory=list)
    always: list[str] = field(default_factory=list)
    prerequisites: dict[str, LocalGateDeclaration] = field(default_factory=dict)
    prerequisite_policy: dict = field(default_factory=dict)
    root: Path = field(default_factory=lambda: Path(__file__).resolve().parents[1])
    # The proof executor's declared admission: its ceiling and resource classes.
    executor_max_workers: int | None = None
    resource_limits: dict[str, int] = field(default_factory=dict)
    # The plan's proof commands: each test file's cell and envelope.
    commands: tuple[Mapping, ...] = ()
    # The plan's matrix cells; a portability cell verifies one OS.
    matrix_cells: tuple[Mapping, ...] = ()
    pinned_assets: Mapping[str, Mapping] = field(default_factory=dict)
    # The graph that selects tests by what they read. Without one (a driver
    # test's synthetic policy), rules alone select.
    dependency_graph: Callable[[], DependencyGraph] | None = None
    # Mechanical repairs, in order; `land` applies them before its precheck.
    repair: list[str] = field(default_factory=list)
    # One graph selection per request: the gates it selects and the receipt
    # that names what it left to postsubmit read the same object.
    _graph_selections: dict = field(default_factory=dict, repr=False, compare=False)

    @staticmethod
    def load(path: Path) -> "GateConfig":
        """A policy file; a checkout's own plan selects over its working tree.

        The dependency graph belongs to the checkout that owns the plan, at
        ``tools/proof_plan.toml``. Another manifest file (a driver test's
        synthetic policy) names no checkout, so its rules alone select.
        """
        if not path.exists():
            raise DriverError(
                f"gate manifest missing: {path}. The change-class->gates manifest "
                "is required for `integrate`.",
                code=EXIT_USAGE,
            )
        config = GateConfig.parse(
            path.read_text(encoding="utf-8"), str(path), root=path.parent.parent
        )
        root = config.root
        if path.resolve() == (root / "tools" / GATES_CONFIG_NAME).resolve():
            from tools.test_dependency_graph import cached_graph

            config.dependency_graph = lambda: cached_graph(root)
        return config

    @staticmethod
    def parse(source: str, label: str, *, root: Path | None = None) -> "GateConfig":
        from tools.proof_plan import (
            PytestMemoryUnits,
            local_gate_commands,
            PlanClaims,
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
        plan_claims = PlanClaims(data, root=root)
        toolchain_names = {
            row["name"]
            for row in data.get("toolchain_policy", [])
            if isinstance(row, dict) and isinstance(row.get("name"), str)
        }
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
                name,
                entry.get("pytest_select", []),
                toolchain_names=toolchain_names,
                root=root,
            )
            try:
                claim_errors, claims = plan_claims.claims(entry)
            except ValueError as exc:
                raise DriverError(f"invalid proof plan ({label}): {exc}") from exc
            errors += claim_errors
            try:
                resolved_selections = local_pytest_selections(entry)
            except (ValueError, TypeError, KeyError) as exc:
                errors.append(f"{name}: {exc}")
                resolved_selections = []
            if errors:
                raise DriverError("\n".join(errors))
            rules.append(
                GateRule(
                    name=name,
                    globs=globs,
                    gates=gates,
                    description=str(entry.get("description", "")),
                    pytest=pytest_targets,
                    pytest_select=resolved_selections,
                    plan_entities=claims,
                    precheck=entry.get("precheck", False),
                )
            )
        always = data.get("always", [])
        if not isinstance(always, list) or not all(isinstance(g, str) for g in always):
            raise DriverError("gate manifest `always` must be a list of str")
        repair = data.get("repair", [])
        if not isinstance(repair, list) or not all(isinstance(g, str) for g in repair):
            raise DriverError("gate manifest `repair` must be a list of str")
        # Repairs rewrite the checkout before any gate runs, so their
        # declaration is proven here, by the plan's own section contract.
        from tools.proof_plan import (
            load_toolchain_policies,
            local_command_section_errors,
        )

        repair_toolchains = data.get("repair_toolchains", [])
        try:
            repair_errors = local_command_section_errors(
                load_toolchain_policies(data, root=root, names=repair_toolchains),
                {"repair": (repair, repair_toolchains)},
            )
        except (ValueError, KeyError, OSError, TypeError) as exc:
            repair_errors = [f"local repair: {exc}"]
        if repair_errors:
            raise DriverError(
                f"invalid repair declaration ({label}): " + "; ".join(repair_errors)
            )
        try:
            memory_units = PytestMemoryUnits(data.get("command", ()), root=root)
            commands = local_gate_commands(data, memory_units=memory_units)
            from tools.proof_plan import load_pinned_assets

            assets = load_pinned_assets(data)
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
            root=_REPO_ROOT if root is None else root,
            executor_max_workers=ceiling,
            resource_limits=resource_limits,
            commands=tuple(data.get("command", ())),
            matrix_cells=tuple(data.get("matrix_cell", ())),
            pinned_assets=assets,
            repair=list(repair),
        )

    def graph_selection(
        self,
        touched: list[str],
        *,
        mode: LandingMode = LandingMode.PRESUBMIT,
        recent_failures: Collection[str] = (),
    ) -> GraphSelection | None:
        """The tests that read a touched path, and where each one runs."""
        if self.dependency_graph is None or not (touched or recent_failures):
            return None
        # The provider is part of the request: a reassigned graph is another one.
        key = (self.dependency_graph, tuple(touched), mode, tuple(recent_failures))
        if key not in self._graph_selections:
            self._graph_selections[key] = self._compute_graph_selection(
                touched, mode=mode, recent_failures=recent_failures
            )
        return self._graph_selections[key]

    def _compute_graph_selection(
        self,
        touched: list[str],
        *,
        mode: LandingMode,
        recent_failures: Collection[str],
    ) -> GraphSelection:
        from tools.test_dependency_graph import (
            ObservedTestInputs,
            graph_selection,
            observation_context,
            observation_directory,
            observed_test_targets,
        )

        graph = self.dependency_graph()
        qualified = observed_test_targets(self.commands)

        return graph_selection(
            graph,
            touched,
            self.commands,
            mode=mode,
            recent_failures=recent_failures,
            foreign_cells=self.foreign_cells(),
            observations=(
                ObservedTestInputs(
                    observation_directory(
                        observation_context(graph.tree, self.commands)
                    ),
                    targets=qualified,
                    guest_only=True,
                    current_inputs=graph.tree.blobs,
                )
                if mode is LandingMode.PRESUBMIT and qualified
                else None
            ),
        )

    def foreign_cells(self, host_os: str | None = None) -> frozenset[str]:
        """Portability cells of another OS: they verify that OS's hosted shells
        and compilers, which the executing host does not provide. ``host_os``
        names the executing host; it defaults to this one."""
        from molt.native_target import resolve_native_target_spec

        if host_os is None:
            host_os = resolve_native_target_spec(None).os
        return frozenset(
            str(cell["id"])
            for cell in self.matrix_cells
            if cell.get("profile") == "portability" and cell.get("os") != host_os
        )

    @staticmethod
    def runs_on(gate: SelectedGate, foreign: frozenset[str]) -> bool:
        """Whether a host whose foreign cells are ``foreign`` can run ``gate``:
        a gate bound only to foreign cells runs on another OS's host."""
        return not gate.cells or not set(gate.cells) <= foreign

    def prepare(
        self,
        gates: list[SelectedGate],
        *,
        baseline: Mapping[str, str],
        prepare_python: Callable[[ProcessEnvironment, bool], GateEnvironment],
    ) -> PreparedGates:
        """Prepare selected closures once, then identify the tools they execute.

        No command spelling grants a source environment. Explicit selectors
        remain obligations even for an empty closure. Aggregate admission never
        supplies a child's compiler projection.
        """
        from tools.proof_plan import (
            admit_toolchains,
            load_toolchain_policies,
            project_toolchains,
            toolchain_closure,
            provision_pinned_assets,
        )
        from tools.venv_exec import venv_python

        try:
            foreign = self.foreign_cells()
            local = [gate for gate in gates if self.runs_on(gate, foreign)]
            absent_assets = provision_pinned_assets(
                self.pinned_assets,
                tuple(
                    dict.fromkeys(name for gate in local for name in gate.pinned_assets)
                ),
                root=self.root,
                environment=baseline,
            )
            names = tuple(
                dict.fromkeys(name for gate in local for name in gate.toolchains)
            )
            policies = load_toolchain_policies(
                self.prerequisite_policy, root=self.root, names=names
            )
            by_name = {policy.name: policy for policy in policies}
            closures = {
                tools: toolchain_closure(policies, tools)
                for tools in dict.fromkeys(gate.toolchains for gate in local)
            }
            needs_python = {
                tools: any(
                    by_name[name].python_identity_role == "environment"
                    for name in closure
                )
                for tools, closure in closures.items()
            }
            selected = prepare_python(
                ProcessEnvironment(baseline), any(needs_python.values())
            )
            explicit = bool(
                baseline.get("MOLT_VENV")
                or baseline.get("UV_PROJECT_ENVIRONMENT", "").strip()
            )
            owners: dict[str, list[str]] = {}
            for gate in local:
                identifiers = [
                    identifier
                    for identifier, declaration in self.prerequisites.items()
                    if declaration.command == gate.command
                ]
                if not identifiers:
                    identifiers = [
                        shlex.join(gate.command)
                        if isinstance(gate.command, tuple)
                        else gate.command
                    ]
                for name in closures[gate.toolchains]:
                    owners.setdefault(name, []).extend(identifiers)
            admitted = admit_toolchains(
                policies,
                tuple(owners),
                environ=selected.environment,
                owners=owners,
                python_executable=(
                    venv_python(selected.python_root)
                    if selected.python_root is not None
                    else None
                ),
                root=self.root,
                defer_absent=True,
            )
            resolutions: list[GateResolution] = []
            tool_owners: dict[frozenset[str], CellId] = {}
            for gate in gates:
                missing = (
                    tuple(
                        Prerequisite(PrerequisiteKind.EXECUTION_CELL, cell)
                        for cell in gate.cells
                    )
                    if not self.runs_on(gate, foreign)
                    else (
                        *(
                            Prerequisite(
                                PrerequisiteKind.TOOLCHAIN,
                                name,
                                str(by_name[name].data.get("setup_value", "")),
                                tuple(by_name[name].data.get("setup_evidence", ())),
                            )
                            for name in closures[gate.toolchains]
                            if name in admitted.absent
                        ),
                        *(
                            Prerequisite(PrerequisiteKind.PINNED_ASSET, name)
                            for name in gate.pinned_assets
                            if name in absent_assets
                        ),
                    )
                )
                if missing:
                    plan = {"matrix_cell": self.matrix_cells, "command": self.commands}
                    candidates = list(gate.cells) or [
                        self.pinned_assets[item.name]["owner"]
                        for item in missing
                        if item.kind is PrerequisiteKind.PINNED_ASSET
                    ]
                    if not candidates:
                        required = frozenset(
                            item.name
                            for item in missing
                            if item.kind is PrerequisiteKind.TOOLCHAIN
                        )
                        if required in tool_owners:
                            candidates = [tool_owners[required]]
                        else:
                            candidates = [
                                row["cell"]
                                for row in self.commands
                                if required
                                <= set(
                                    toolchain_closure(
                                        policies,
                                        (
                                            name
                                            for name in row.get("toolchains", ())
                                            if name in by_name
                                        ),
                                    )
                                )
                            ]
                            if candidates:
                                tool_owners[required] = prerequisite_cell(
                                    plan, candidates[0]
                                )
                    if not candidates:
                        raise ValueError(
                            "deferred command has no declared execution owner"
                        )
                    owner = prerequisite_cell(plan, candidates[0])
                    resolutions.append(Deferred(gate, missing, owner))
                else:
                    resolutions.append(Runnable(gate))
            environments = {}
            runnable_tools = {
                item.command.toolchains
                for item in resolutions
                if isinstance(item, Runnable)
            }
            for tools, closure in closures.items():
                if tools not in runnable_tools:
                    continue
                context = (
                    selected
                    if needs_python[tools] or explicit
                    else GateEnvironment(ProcessEnvironment(baseline), None, False)
                )
                projection = project_toolchains(
                    policies, closure, environ=context.environment, root=self.root
                )
                environments[tools] = context.overlay(projection.delta)
            return PreparedGates(
                environments, admitted.fingerprints, tuple(resolutions)
            )
        except (ValueError, OSError, KeyError) as exc:
            raise DriverError(
                f"selected gate prerequisites failed; executed=0: {exc}"
            ) from exc

    def select(
        self,
        touched: list[str],
        *,
        extra: list[str] | None = None,
        plan_changes: Collection[PlanEntity] | _EveryPlanEntity | None = None,
        mode: LandingMode = LandingMode.PRESUBMIT,
        recent_failures: Collection[str] = (),
    ) -> tuple[list[SelectedGate], list[GateRule]]:
        """Select commands with their declared prerequisites intact.

        A rule matches the paths it globs. An edit to the plan file matches the
        rules that claim an entry it changed and every rule it changed, so a
        caller that touches the plan must say what changed (``plan_changes``).
        Deduplicated commands union their owners' tools. Only rules contributing
        to the merged pytest invocation supply its tools; each selector cohort
        keeps its own. Command binding happens after this policy selection.
        """
        from tools.proof_plan import (
            PLAN_PATH,
            local_pytest_argv,
            local_pytest_collection_argv,
            plan_claim_matches,
        )

        plan_touched = PLAN_PATH in touched
        if plan_touched and plan_changes is None:
            raise DriverError(
                f"{PLAN_PATH} changed but its changed entries are unknown; "
                "select through select_landing_gates"
            )

        selected: list[SelectedGate] = []
        indices: dict[tuple[str, tuple[str, ...]], int] = {}
        foreign = self.foreign_cells() if self.matrix_cells else frozenset()
        seen_pytest: set[str] = set()
        matched_rules: list[GateRule] = []
        pytest_targets: list[str] = []
        pytest_tools: dict[str, dict[str, None]] = {}
        pytest_access: dict[str, list[CargoTargetAccess]] = {}
        pytest_assets: dict[str, dict[str, None]] = {}
        pytest_index: int | None = None
        pytest_commands: set[str] = set()
        graph_commands: set[str] = set()

        def command_key(command: str | tuple[str, ...]) -> str:
            return command if isinstance(command, str) else shlex.join(command)

        def add(
            command: str | tuple[str, ...],
            tools: tuple[str, ...],
            *,
            concurrent: bool = False,
            pytest: bool = False,
            collection_command: tuple[str, ...] | None = None,
            cargo_target_access: CargoTargetAccess,
            pinned_assets: tuple[str, ...] = (),
            cells: tuple[str, ...] = (),
            try_run: bool = False,
        ) -> None:
            from tools.proof_command_argv import pytest_arguments

            argv = tuple(shlex.split(command)) if isinstance(command, str) else command
            pytest = pytest or pytest_arguments(argv, root=self.root) is not None
            key = (
                command_key(command),
                cells if cells and set(cells) <= foreign else (),
            )
            if key in indices:
                index = indices[key]
                prior = selected[index]
                selected[index] = replace(
                    prior,
                    toolchains=tuple(dict.fromkeys((*prior.toolchains, *tools))),
                    pytest=prior.pytest or pytest,
                    collection_command=prior.collection_command or collection_command,
                    cargo_target_access=CargoTargetAccess.union(
                        (prior.cargo_target_access, cargo_target_access)
                    ),
                    pinned_assets=tuple(
                        dict.fromkeys((*prior.pinned_assets, *pinned_assets))
                    ),
                    cells=tuple(dict.fromkeys((*prior.cells, *cells))),
                    try_run=prior.try_run or try_run,
                )
            else:
                indices[key] = len(selected)
                selected.append(
                    SelectedGate(
                        command,
                        tools,
                        concurrent,
                        pytest,
                        collection_command,
                        cargo_target_access,
                        pinned_assets,
                        cells,
                        try_run,
                    )
                )

        def tools(identifier: str) -> tuple[str, ...]:
            return self.prerequisites[identifier].toolchains

        def access(identifier: str) -> CargoTargetAccess:
            return self.prerequisites[identifier].cargo_target_access

        def matches(rule: GateRule) -> bool:
            by_path = any(_is_ignored(path, tuple(rule.globs)) for path in touched)
            by_plan = plan_touched and (
                isinstance(plan_changes, _EveryPlanEntity)
                or (
                    plan_changes is not None
                    and (
                        ("rule", rule.name) in plan_changes
                        or plan_claim_matches(rule.plan_entities, plan_changes)
                    )
                )
            )
            return by_path or by_plan

        rules = [rule for rule in self.rules if matches(rule)]
        # What matched rules already run of each file: () is the whole file.
        rule_scopes: dict[str, set[tuple[str, ...]]] = {}
        for rule in rules:
            for target in rule.pytest:
                if "::" not in target:
                    rule_scopes.setdefault(target, set()).add(())
            for cohort in rule.pytest_select:
                for target in cohort.targets:
                    if "::" not in target:
                        rule_scopes.setdefault(target, set()).add(cohort.select)
        # The graph adds each selected file in its cell's envelope unless a
        # rule already runs the same or a wider scope of it.
        graph_runs: list[tuple[str, LandingEnvelope]] = []
        selection = self.graph_selection(
            touched, mode=mode, recent_failures=recent_failures
        )
        for envelope in selection.run.values() if selection else ():
            for target in envelope.targets:
                scopes = rule_scopes.get(target.partition("::")[0], set())
                if () in scopes or envelope.select in scopes:
                    continue
                graph_runs.append((target, envelope))
        # A direct reader the graph selects that a matched rule already runs
        # is still a direct reader: in presubmit its rule cohort splits, and
        # the readers' part runs in the lane's try run while the rest stays
        # with the batch, which runs both (HF-701: a lane's own tests were
        # left to the batch when a rule curated them).
        readers = (
            {
                target.partition("::")[0]
                for envelope in selection.run.values()
                for target in envelope.targets
            }
            if selection is not None and mode is LandingMode.PRESUBMIT
            else set()
        )

        def split(targets) -> list[tuple[list[str], bool]]:
            direct = [t for t in targets if t.partition("::")[0] in readers]
            rest = [t for t in targets if t.partition("::")[0] not in readers]
            return [(part, part is direct) for part in (direct, rest) if part]

        if touched:
            for index, command in enumerate(self.always):
                add(
                    command,
                    tools(f"local.always.{index}"),
                    concurrent=True,
                    cargo_target_access=access(f"local.always.{index}"),
                )
        for rule in rules:
            matched_rules.append(rule)
            owner = f"local.{rule.name}"
            for index, command in enumerate(rule.gates):
                add(
                    command,
                    tools(f"{owner}.{index}"),
                    cargo_target_access=access(f"{owner}.{index}"),
                    pinned_assets=self.prerequisites[f"{owner}.{index}"].pinned_assets,
                    try_run=rule.precheck,
                )
            if rule.pytest:
                if pytest_index is None:
                    pytest_index = len(selected)
                for target in rule.pytest:
                    pytest_tools.setdefault(target, {}).update(
                        dict.fromkeys(tools(f"{owner}.pytest"))
                    )
                    pytest_access.setdefault(target, []).append(
                        access(f"{owner}.pytest")
                    )
                    pytest_assets.setdefault(target, {}).update(
                        dict.fromkeys(
                            self.prerequisites[f"{owner}.pytest"].pinned_assets
                        )
                    )
            for index, cohort in enumerate(rule.pytest_select):
                for part, reads_change in split(cohort.targets):
                    command = local_pytest_argv(
                        part,
                        cohort.select,
                        memory_units=self.pytest_memory_units,
                    )
                    pytest_commands.add(command_key(command))
                    add(
                        command,
                        tools(f"{owner}.pytest-select-{index}"),
                        cargo_target_access=access(f"{owner}.pytest-select-{index}"),
                        pytest=True,
                        collection_command=(
                            local_pytest_collection_argv(command)
                            if any("::" in target for target in part)
                            else None
                        ),
                        pinned_assets=self.prerequisites[
                            f"{owner}.pytest-select-{index}"
                        ].pinned_assets,
                        try_run=reads_change,
                    )
            for target in rule.pytest:
                if target not in seen_pytest:
                    seen_pytest.add(target)
                    pytest_targets.append(target)
        # Graph commands keep their failure-first order and fail-fast policy.
        # Rule-owned pytest commands retain their exact declared invocation.
        filtered: dict[
            tuple[
                tuple[bool, bool],
                tuple[str, ...],
                bool,
                tuple[str, ...],
                CargoTargetAccess,
            ],
            list[str],
        ] = {}
        recent_paths = {target.partition("::")[0] for target in recent_failures}
        for target, envelope in graph_runs:
            access = CargoTargetAccess.resolve(envelope.toolchains)
            path = target.partition("::")[0]
            priority = (path not in recent_paths, path not in touched)
            key = (
                priority,
                envelope.select,
                envelope.skip_slow,
                envelope.toolchains,
                access,
            )
            filtered.setdefault(key, []).append(target)
        for (_, select, skip_slow, cohort_tools, cohort_access), targets in sorted(
            filtered.items(), key=lambda item: item[0][0]
        ):
            match mode:
                case LandingMode.PRESUBMIT:
                    # tests/conftest.py skips `slow` tests under --skip-slow; a
                    # skip, unlike -m "not slow", never empties the session.
                    options = (
                        *select,
                        "--maxfail=1",
                        *(("--skip-slow",) if skip_slow else ()),
                    )
                case LandingMode.POSTSUBMIT:
                    options = select
                case _:
                    assert_never(mode)
            for shard in pytest_shards(targets):
                command = local_pytest_argv(
                    shard, options, memory_units=self.pytest_memory_units
                )
                pytest_commands.add(command_key(command))
                graph_commands.add(command_key(command))
                add(
                    command,
                    cohort_tools,
                    cargo_target_access=cohort_access,
                    pytest=True,
                    # Changed tests and their direct readers: the lane's try run.
                    try_run=mode is LandingMode.PRESUBMIT,
                    cells=tuple(
                        dict.fromkeys(
                            cell
                            for target, envelope in graph_runs
                            if target in shard
                            for cell in envelope.cells
                        )
                    ),
                    collection_command=(
                        local_pytest_collection_argv(command)
                        if any("::" in target for target in shard)
                        else None
                    ),
                )
        for envelope in selection.foreign if selection else ():
            add(
                local_pytest_argv(
                    envelope.targets,
                    envelope.select,
                    memory_units=self.pytest_memory_units,
                ),
                envelope.toolchains,
                cargo_target_access=CargoTargetAccess.resolve(envelope.toolchains),
                pytest=True,
                cells=envelope.cells,
            )
        # Extras are raw policy commands, not bound argv or unowned shell text.
        for command in extra or ():
            owners = [
                declaration
                for declaration in self.prerequisites.values()
                if declaration.command == command
            ]
            if not owners:
                raise DriverError(
                    "extra gates lack declared prerequisite custody; declare them in "
                    "proof_plan.toml before selection: " + command
                )
            add(
                command,
                tuple(
                    dict.fromkeys(name for owner in owners for name in owner.toolchains)
                ),
                cargo_target_access=CargoTargetAccess.union(
                    tuple(owner.cargo_target_access for owner in owners)
                ),
                pinned_assets=tuple(
                    dict.fromkeys(
                        asset for owner in owners for asset in owner.pinned_assets
                    )
                ),
            )
        if pytest_index is not None:
            assignments: dict[
                str, tuple[tuple[str, ...], CargoTargetAccess, tuple[str, ...]]
            ] = {}
            for group in self.pytest_memory_units.overlapping_requests(pytest_targets):
                group_tools = tuple(
                    dict.fromkeys(
                        tool for target in group for tool in pytest_tools[target]
                    )
                )
                group_access = CargoTargetAccess.union(
                    tuple(
                        effect for target in group for effect in pytest_access[target]
                    )
                )
                group_assets = tuple(
                    dict.fromkeys(
                        asset for target in group for asset in pytest_assets[target]
                    )
                )
                assignments.update(
                    dict.fromkeys(group, (group_tools, group_access, group_assets))
                )
            cohorts: dict[
                tuple[tuple[str, ...], CargoTargetAccess, tuple[str, ...]], list[str]
            ] = {}
            for target in pytest_targets:
                cohorts.setdefault(assignments[target], []).append(target)
            parts = [
                (key, part, reads_change)
                for key, targets in cohorts.items()
                for part, reads_change in split(targets)
            ]
            # Keep add()'s command indices valid until every deduplication and
            # prerequisite union is complete. Only newly appended cohorts move.
            before_cohorts = len(selected)
            for (
                cohort_tools,
                cohort_access,
                cohort_assets,
            ), targets, reads_change in parts:
                command = local_pytest_argv(
                    targets, memory_units=self.pytest_memory_units
                )
                pytest_commands.add(command_key(command))
                # Preserve each literal request; only explicit nodes collect early.
                named_targets = tuple(target for target in targets if "::" in target)
                add(
                    command,
                    cohort_tools,
                    cargo_target_access=cohort_access,
                    pytest=True,
                    pinned_assets=cohort_assets,
                    try_run=reads_change,
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
            appended = selected[before_cohorts:]
            del selected[before_cohorts:]
            selected[pytest_index:pytest_index] = appended
        # Run declared source/metadata and compilation controls before Python
        # consumers. This also moves already-required Cargo gates earlier; it
        # adds no commands or builds and preserves each phase's relative order.
        explicit: list[SelectedGate] = []
        graph_consumers: list[SelectedGate] = []
        consumers: list[SelectedGate] = []
        for gate in selected:
            key = command_key(gate.command)
            phase = (
                graph_consumers
                if key in graph_commands
                else (consumers if key in pytest_commands else explicit)
            )
            phase.append(gate)
        selected = explicit + graph_consumers + consumers
        if mode is LandingMode.POSTSUBMIT and self.dependency_graph is not None:
            from tools.proof_command_argv import pytest_arguments, pytest_run_selections
            from tools.test_dependency_graph import (
                OBSERVED_CONTEXT_OPTION,
                OBSERVED_PLUGIN,
                observation_context,
                observed_test_targets,
            )

            qualified = observed_test_targets(self.commands)
            if not qualified:
                return selected, matched_rules
            context = observation_context(self.dependency_graph().tree, self.commands)
            observed_gates = []
            for gate in selected:
                argv = (
                    tuple(shlex.split(gate.command))
                    if isinstance(gate.command, str)
                    else gate.command
                )
                arguments = (
                    pytest_arguments(argv, root=self.root) if gate.pytest else None
                )
                targets = (
                    pytest_run_selections(arguments, root=self.root)
                    if arguments is not None
                    else ()
                )
                subjects = sorted(
                    path
                    for path in qualified
                    if any(
                        path == target.partition("::")[0]
                        or path.startswith(target.rstrip("/") + "/")
                        for target in targets
                    )
                )
                if subjects:
                    options = (
                        "-p",
                        OBSERVED_PLUGIN,
                        OBSERVED_CONTEXT_OPTION,
                        context,
                        *("--molt-observed-target=" + path for path in subjects),
                    )
                    gate = replace(
                        gate,
                        command=(*argv, *options),
                        collection_command=(*gate.collection_command, *options)
                        if gate.collection_command is not None
                        else None,
                    )
                observed_gates.append(gate)
            selected = observed_gates
        return selected, matched_rules

    def cargo_outputs(
        self,
        environments: dict[tuple[str, ...], GateEnvironment],
        gates: Collection[SelectedGate],
    ) -> dict[tuple[str, ...], tuple[Path, ...]]:
        """Shared outputs of the selected writers, not all admitted tools.

        Isolated fixtures admit their actual private output through child custody.
        A toolchain shared with a writer does not make a read-only gate a writer.
        """
        from molt.cargo_execution_policy import cargo_output_paths

        shared = {
            gate.toolchains
            for gate in gates
            if gate.cargo_target_access is CargoTargetAccess.SHARED
        }
        outputs = {}
        for tools, context in environments.items():
            if tools in shared:
                outputs[tools] = cargo_output_paths(
                    (), cwd=self.root, env=context.environment
                )
        return outputs


# --------------------------------------------------------------------------
# Content marker byte membership
# --------------------------------------------------------------------------


#: Test files per pytest gate. A cohort is sharded into gates of at most this
#: many files: each gate's workers import fewer test modules, its process tree
#: closes quickly, and a failure or a retry costs one shard. Landing attempts
#: 15-17 ran one 300-file cohort: 8.6 GiB, two memory-pressure stops and one
#: indeterminate descendant closure after its tests had passed to 99%.
PYTEST_SHARD_FILES: Final = 40


def pytest_shards(targets: Sequence[str]) -> list[list[str]]:
    """``targets`` in order, cut into shards of at most ``PYTEST_SHARD_FILES``
    files; the nodes of one file stay in one shard."""
    shards: list[list[str]] = []
    files: set[str] = set()
    for target in targets:
        path = target.partition("::")[0]
        if path not in files and len(files) == PYTEST_SHARD_FILES:
            shards.append([])
            files = set()
        if not shards:
            shards.append([])
        files.add(path)
        shards[-1].append(target)
    return shards


def left_to_postsubmit(
    selection: "GraphSelection | None", gates: Sequence[SelectedGate]
) -> tuple[str, ...]:
    """The data readers presubmit leaves to postsubmit (owner 2026-11-06,
    importers only), less any target a selected gate runs anyway: a curated
    rule cohort that runs a node or a whole file covers it here."""
    if selection is None:
        return ()
    ran = {
        argument
        for gate in gates
        if isinstance(gate.command, tuple)
        for argument in gate.command
        if isinstance(argument, str) and argument.startswith("tests/")
    }
    return tuple(
        target
        for target in selection.postsubmit_only
        if target not in ran and target.partition("::")[0] not in ran
    )


def select_landing_gates(
    git: Git,
    repo: Path,
    commit: str,
    upstream: str | None,
    *,
    extra: list[str],
    mode: LandingMode = LandingMode.PRESUBMIT,
    recent_failures: Collection[str] = (),
) -> tuple[str, GateConfig, list[str], list[SelectedGate], list[GateRule]]:
    """The policy, touched paths and bound gates of ``commit`` over ``upstream``.

    Policy comes from the same immutable commit whose clean source executes it.
    Without a known green base, postsubmit selects every tracked input.
    """
    if upstream is None and mode is not LandingMode.POSTSUBMIT:
        raise DriverError(
            "presubmit selection requires an upstream commit", code=EXIT_USAGE
        )
    policy = git._run(["show", f"{commit}:tools/{GATES_CONFIG_NAME}"]).stdout
    try:
        config = GateConfig.parse(
            policy, f"{commit}:tools/{GATES_CONFIG_NAME}", root=repo
        )
    except DriverError as exc:
        # The selected commit's own gate policy is invalid.
        raise CandidateFailure(str(exc), code=exc.code) from exc
    touched = (
        [
            path
            for path in git._run(
                ["ls-tree", "--full-tree", "--name-only", "-r", "-z", commit]
            ).stdout.split("\0")
            if path
        ]
        if upstream is None
        else changed_paths(
            upstream, commit, run_git=lambda arguments: git._run(arguments).stdout
        )
    )
    from tools.proof_plan import PLAN_PATH, bind_local_gate_command
    from tools.test_dependency_graph import cached_graph

    # The selected commit's tree, read through Git objects: what executes.
    config.dependency_graph = lambda: cached_graph(repo, commit=commit)

    plan_changes = None
    if upstream is None:
        plan_changes = EVERY_PLAN_ENTITY
    elif PLAN_PATH in touched:
        plan_changes = _plan_changes(git, upstream, policy)
    gates, matched_rules = config.select(
        touched,
        extra=extra,
        plan_changes=plan_changes,
        mode=mode,
        recent_failures=recent_failures,
    )

    try:
        gates = [
            replace(
                gate,
                command=bind_local_gate_command(
                    gate.command, upstream_commit=upstream or commit
                ),
                collection_command=(
                    bind_local_gate_command(
                        gate.collection_command, upstream_commit=upstream or commit
                    )
                    if gate.collection_command is not None
                    else None
                ),
            )
            for gate in gates
        ]
    except ValueError as exc:
        raise CandidateFailure(
            f"invalid local gate binding; executed=0: {exc}"
        ) from exc
    return policy, config, touched, gates, matched_rules


def _plan_changes(
    git: Git, upstream: str, policy: str
) -> frozenset[PlanEntity] | _EveryPlanEntity:
    """The plan entries ``policy`` changed over ``upstream``'s plan. Without a
    readable pair of plans the change is unknown, and every rule matches."""
    from tools.proof_plan import changed_plan_entities

    shown = git._run(["show", f"{upstream}:tools/{GATES_CONFIG_NAME}"], check=False)
    if shown.returncode != 0:
        return EVERY_PLAN_ENTITY
    try:
        return changed_plan_entities(tomllib.loads(shown.stdout), tomllib.loads(policy))
    except (tomllib.TOMLDecodeError, ValueError):
        return EVERY_PLAN_ENTITY
