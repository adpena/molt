"""The local landing gate policy: rules, their selection and binding.

``tools/proof_plan.toml`` declares the always gates and the local rules. A rule
matches a landing through the paths it globs or, for the plan file itself,
through the plan entries it claims with ``plan_entities``: an edit to the plan
selects only the rules that read what it changed (HF-457). ``integrate``, the
land precheck and the queue select through ``select_landing_gates``, the one
selection authority.
"""

from __future__ import annotations

import shlex
import tomllib
from collections.abc import Callable, Collection, Mapping
from dataclasses import dataclass, field, replace
from pathlib import Path
from typing import TYPE_CHECKING

from molt.process_environment import ProcessEnvironment
from tools.dirty_tree_policy import is_ignored as _is_ignored
from tools.git_identity import changed_paths
from tools.molt_dev_common import EXIT_USAGE, CandidateFailure, DriverError

if TYPE_CHECKING:
    from tools.molt_dev import Git
    from tools.proof_plan import LocalPytestSelection, PlanEntity, PytestMemoryUnits

_REPO_ROOT = Path(__file__).resolve().parents[1]
GATES_CONFIG_NAME = "proof_plan.toml"


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
    # Named-node admission never replaces mandatory execution.
    collection_command: tuple[str, ...] | None = None

    @property
    def precheck(self) -> bool:
        """Whether `land` runs this gate in the lane before queueing it."""
        return not self.pytest and frozenset(self.toolchains) <= PRECHECK_TOOLCHAINS


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


@dataclass(frozen=True)
class PreparedGates:
    environments: dict[tuple[str, ...], GateEnvironment]
    fingerprints: dict[str, dict]


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
                    plan_entities=claims,
                )
            )
        always = data.get("always", [])
        if not isinstance(always, list) or not all(isinstance(g, str) for g in always):
            raise DriverError("gate manifest `always` must be a list of str")
        try:
            memory_units = PytestMemoryUnits(data.get("command", ()), root=root)
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
            root=_REPO_ROOT if root is None else root,
            executor_max_workers=ceiling,
            resource_limits=resource_limits,
        )

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
        )
        from tools.venv_exec import venv_python

        try:
            names = tuple(
                dict.fromkeys(name for gate in gates for name in gate.toolchains)
            )
            policies = load_toolchain_policies(
                self.prerequisite_policy, root=self.root, names=names
            )
            by_name = {policy.name: policy for policy in policies}
            closures = {
                tools: toolchain_closure(policies, tools)
                for tools in dict.fromkeys(gate.toolchains for gate in gates)
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
            for gate in gates:
                identifiers = [
                    identifier
                    for identifier, (command, _tools) in self.prerequisites.items()
                    if command == gate.command
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
            )
            environments = {}
            for tools, closure in closures.items():
                context = (
                    selected
                    if needs_python[tools] or explicit
                    else GateEnvironment(ProcessEnvironment(baseline), None, False)
                )
                projection = project_toolchains(
                    policies, closure, environ=context.environment, root=self.root
                )
                environments[tools] = context.overlay(projection.delta)
            return PreparedGates(environments, admitted.fingerprints)
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
        indices: dict[str, int] = {}
        seen_pytest: set[str] = set()
        matched_rules: list[GateRule] = []
        pytest_targets: list[str] = []
        pytest_tools: dict[str, dict[str, None]] = {}
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
            if not (by_path or by_plan):
                continue
            matched_rules.append(rule)
            owner = f"local.{rule.name}"
            for index, command in enumerate(rule.gates):
                add(command, tools(f"{owner}.{index}"))
            if rule.pytest:
                if pytest_index is None:
                    pytest_index = len(selected)
                for target in rule.pytest:
                    pytest_tools.setdefault(target, {}).update(
                        dict.fromkeys(tools(f"{owner}.pytest"))
                    )
            for index, selection in enumerate(rule.pytest_select):
                command = local_pytest_argv(
                    selection.targets,
                    selection.select,
                    memory_units=self.pytest_memory_units,
                )
                pytest_commands.add(command_key(command))
                add(
                    command,
                    tools(f"{owner}.pytest-select-{index}"),
                    pytest=True,
                    collection_command=(
                        local_pytest_collection_argv(command)
                        if any("::" in target for target in selection.targets)
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
            assignments: dict[str, tuple[str, ...]] = {}
            for group in self.pytest_memory_units.overlapping_requests(pytest_targets):
                group_tools = tuple(
                    dict.fromkeys(
                        tool for target in group for tool in pytest_tools[target]
                    )
                )
                assignments.update(dict.fromkeys(group, group_tools))
            cohorts: dict[tuple[str, ...], list[str]] = {}
            for target in pytest_targets:
                cohorts.setdefault(assignments[target], []).append(target)
            # Keep add()'s command indices valid until every deduplication and
            # prerequisite union is complete. Only newly appended cohorts move.
            before_cohorts = len(selected)
            for cohort_tools, targets in cohorts.items():
                command = local_pytest_argv(
                    targets, memory_units=self.pytest_memory_units
                )
                pytest_commands.add(command_key(command))
                # Preserve each literal request; only explicit nodes collect early.
                named_targets = tuple(target for target in targets if "::" in target)
                add(
                    command,
                    cohort_tools,
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
            appended = selected[before_cohorts:]
            del selected[before_cohorts:]
            selected[pytest_index:pytest_index] = appended
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

    def cargo_outputs(
        self,
        environments: dict[tuple[str, ...], GateEnvironment],
    ) -> dict[tuple[str, ...], tuple[Path, ...]]:
        """Outputs of gate owners explicitly declaring Cargo capability.

        Declared capability is conservative: it does not prove that a Cargo
        child launches. Actual argv/output admission remains with each child.
        Transitive tool availability (for example generator metadata queries)
        does not impose a compilation-space requirement.
        """
        from molt.cargo_execution_policy import cargo_output_paths

        outputs = {}
        for tools, context in environments.items():
            if "cargo" in tools:
                outputs[tools] = cargo_output_paths(
                    (), cwd=self.root, env=context.environment
                )
        return outputs


# --------------------------------------------------------------------------
# Content marker byte membership
# --------------------------------------------------------------------------


def select_landing_gates(
    git: Git,
    repo: Path,
    commit: str,
    upstream: str,
    *,
    extra: list[str],
) -> tuple[str, GateConfig, list[str], list[SelectedGate], list[GateRule]]:
    """The policy, touched paths and bound gates of ``commit`` over ``upstream``.

    Policy comes from the same immutable commit whose clean source executes it.
    """
    policy = git._run(["show", f"{commit}:tools/{GATES_CONFIG_NAME}"]).stdout
    try:
        config = GateConfig.parse(
            policy, f"{commit}:tools/{GATES_CONFIG_NAME}", root=repo
        )
    except DriverError as exc:
        # The selected commit's own gate policy is invalid.
        raise CandidateFailure(str(exc), code=exc.code) from exc
    touched = changed_paths(
        upstream, commit, run_git=lambda arguments: git._run(arguments).stdout
    )
    from tools.proof_plan import PLAN_PATH, bind_local_gate_command

    plan_changes = None
    if PLAN_PATH in touched:
        plan_changes = _plan_changes(git, upstream, policy)
    gates, matched_rules = config.select(
        touched, extra=extra, plan_changes=plan_changes
    )

    try:
        gates = [
            replace(
                gate,
                command=bind_local_gate_command(gate.command, upstream_commit=upstream),
                collection_command=(
                    bind_local_gate_command(
                        gate.collection_command, upstream_commit=upstream
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
