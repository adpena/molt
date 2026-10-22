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
from collections.abc import Collection, Mapping
from dataclasses import dataclass, field, replace
from pathlib import Path
from typing import TYPE_CHECKING

from molt.process_environment import ProcessEnvironment
from tools.dirty_tree_policy import is_ignored as _is_ignored
from tools.git_identity import changed_paths
from tools.molt_dev_common import EXIT_USAGE, CandidateFailure, DriverError

if TYPE_CHECKING:
    from tools.molt_dev import Git
    from tools.proof_plan import PlanEntity, PytestMemoryUnits

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
    # (selection argv, targets): each runs as its own pytest invocation.
    pytest_select: list[tuple[tuple[str, ...], tuple[str, ...]]] = field(
        default_factory=list
    )
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
            root=_REPO_ROOT if root is None else root,
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
            # File-only targets run once; only explicit named nodes collect early.
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
