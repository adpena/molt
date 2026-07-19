#!/usr/bin/env python3
"""Select and validate Molt proofs from the canonical proof-plan manifest.

This is policy-neutral execution code.  Path ownership, proof metadata, and
local gate commands live only in ``tools/proof_plan.toml``.
"""

from __future__ import annotations

import argparse
from concurrent.futures import FIRST_COMPLETED, Future, ThreadPoolExecutor, wait
import datetime as dt
import fnmatch
import hashlib
import heapq
import json
import os
import platform
import re
import shutil
from dataclasses import dataclass
from pathlib import Path
import subprocess
import sys
import threading
import time
import tomllib
from typing import Any, Iterable, Mapping

try:
    from tools.artifact_publish import atomic_write_json
except ModuleNotFoundError:  # pragma: no cover - direct script execution
    from artifact_publish import atomic_write_json

ROOT = Path(__file__).resolve().parents[1]
DEFAULT_MANIFEST = ROOT / "tools" / "proof_plan.toml"
NULL_SHA = "0" * 40
REQUIRED_FAMILY_FIELDS = (
    "description",
    "inputs",
    "executor",
    "workflow",
    "job",
    "tiers",
    "required",
    "timeout_minutes",
    "dependencies",
    "resource_class",
)
REQUIRED_COMMAND_FIELDS = (
    "family",
    "cell",
    "tiers",
    "resource_class",
    "timeout_seconds",
    "cache_domain",
    "dependencies",
    "argv",
    "toolchains",
)
REQUIRED_TOOLCHAIN_FIELDS = (
    "executable",
    "version_args",
    "version_pattern",
    "setup_value",
    "setup_evidence",
)
RECEIPT_SCHEMA = "molt.proof-receipt.v2"


@dataclass(frozen=True, slots=True)
class ProofFamily:
    name: str
    data: dict[str, Any]

    @property
    def inputs(self) -> tuple[str, ...]:
        return tuple(self.data["inputs"])


@dataclass(frozen=True, slots=True)
class MatrixCell:
    id: str
    data: dict[str, str]


@dataclass(frozen=True, slots=True)
class ProofCommand:
    id: str
    data: dict[str, Any]

    @property
    def family(self) -> str:
        return str(self.data["family"])

    @property
    def dependencies(self) -> tuple[str, ...]:
        return tuple(self.data["dependencies"])

    @property
    def argv(self) -> tuple[str, ...]:
        return tuple(self.data["argv"])

    @property
    def toolchains(self) -> tuple[str, ...]:
        return tuple(self.data["toolchains"])


@dataclass(frozen=True, slots=True)
class ToolchainPolicy:
    name: str
    data: dict[str, Any]


@dataclass(frozen=True, slots=True)
class ResourcePolicy:
    name: str
    max_parallel: int


@dataclass(frozen=True, slots=True)
class TimeoutEnvelope:
    projected_makespan_seconds: int
    critical_path_seconds: int
    resource_capacity_floor_seconds: dict[str, int]


@dataclass(frozen=True, slots=True)
class Selection:
    changed_paths: tuple[str, ...]
    selected: tuple[ProofFamily, ...]
    reasons: dict[str, tuple[str, ...]]
    fail_closed_reason: str | None = None


@dataclass(frozen=True, slots=True)
class ProofPlan:
    path: Path
    authority_inputs: tuple[str, ...]
    receipt_schema: str
    matrix_cells: tuple[MatrixCell, ...]
    families: tuple[ProofFamily, ...]
    commands: tuple[ProofCommand, ...]
    toolchain_policies: tuple[ToolchainPolicy, ...]
    executor_max_workers: int
    resource_policies: tuple[ResourcePolicy, ...]
    local_rules: tuple[dict[str, Any], ...]
    always: tuple[str, ...]

    def timeout_envelope(self, family_name: str) -> TimeoutEnvelope:
        """Project the bounded DAG schedule when every partition hits its timeout."""
        commands = tuple(
            command for command in self.commands if command.family == family_name
        )
        if not commands:
            raise ValueError(f"{family_name}: selected family has no commands")
        command_by_id = {command.id: command for command in commands}
        command_index = {command.id: index for index, command in enumerate(commands)}
        resource_limits = {
            policy.name: policy.max_parallel for policy in self.resource_policies
        }
        remaining_dependencies: dict[str, int] = {}
        dependents: dict[str, list[str]] = {command.id: [] for command in commands}
        critical_paths: dict[str, int] = {}
        for command in commands:
            dependencies = tuple(
                dependency
                for dependency in command.dependencies
                if dependency in command_by_id
            )
            remaining_dependencies[command.id] = len(dependencies)
            for dependency in dependencies:
                dependents[dependency].append(command.id)

        ready_by_resource: dict[str, list[tuple[int, str]]] = {
            name: [] for name in resource_limits
        }
        for command in commands:
            if remaining_dependencies[command.id] == 0:
                resource = str(command.data["resource_class"])
                heapq.heappush(
                    ready_by_resource[resource],
                    (command_index[command.id], command.id),
                )
        active_by_resource = {name: 0 for name in resource_limits}
        active: list[tuple[int, int, str, str]] = []
        completed: set[str] = set()
        now = 0
        while len(completed) < len(commands):
            while len(active) < self.executor_max_workers:
                available_resources = tuple(
                    resource
                    for resource, ready in ready_by_resource.items()
                    if ready
                    and active_by_resource[resource] < resource_limits[resource]
                )
                if not available_resources:
                    break
                resource = min(
                    available_resources,
                    key=lambda name: ready_by_resource[name][0][0],
                )
                _, command_id = heapq.heappop(ready_by_resource[resource])
                command = command_by_id[command_id]
                finish = now + int(command.data["timeout_seconds"])
                heapq.heappush(
                    active,
                    (finish, command_index[command_id], command_id, resource),
                )
                active_by_resource[resource] += 1
            if not active:
                blocked = sorted(set(command_by_id) - completed)
                raise ValueError(
                    f"{family_name}: timeout projection dependency deadlock: {blocked!r}"
                )
            now = active[0][0]
            finishing: list[tuple[int, int, str, str]] = []
            while active and active[0][0] == now:
                finishing.append(heapq.heappop(active))
            for _, _, command_id, resource in finishing:
                command = command_by_id[command_id]
                active_by_resource[resource] -= 1
                completed.add(command_id)
                parent_paths = [
                    critical_paths[dependency]
                    for dependency in command.dependencies
                    if dependency in critical_paths
                ]
                critical_paths[command_id] = int(command.data["timeout_seconds"]) + max(
                    parent_paths, default=0
                )
                for dependent in dependents[command_id]:
                    remaining_dependencies[dependent] -= 1
                    if remaining_dependencies[dependent] == 0:
                        dependent_command = command_by_id[dependent]
                        dependent_resource = str(
                            dependent_command.data["resource_class"]
                        )
                        heapq.heappush(
                            ready_by_resource[dependent_resource],
                            (command_index[dependent], dependent),
                        )
        resource_capacity_floor_seconds = {
            resource: (
                sum(
                    int(command.data["timeout_seconds"])
                    for command in commands
                    if command.data["resource_class"] == resource
                )
                + limit
                - 1
            )
            // limit
            for resource, limit in resource_limits.items()
            if any(command.data["resource_class"] == resource for command in commands)
        }
        return TimeoutEnvelope(
            projected_makespan_seconds=now,
            critical_path_seconds=max(critical_paths.values(), default=0),
            resource_capacity_floor_seconds=resource_capacity_floor_seconds,
        )

    @classmethod
    def load(cls, path: Path = DEFAULT_MANIFEST) -> "ProofPlan":
        data = tomllib.loads(path.read_text(encoding="utf-8"))
        if data.get("schema") != "molt.proof-plan.v3":
            raise ValueError(f"{path}: expected schema molt.proof-plan.v3")
        families = tuple(
            ProofFamily(str(entry.get("name", "")), dict(entry))
            for entry in data.get("ci_family", [])
        )
        plan = cls(
            path=path,
            authority_inputs=tuple(data.get("authority_inputs", [])),
            receipt_schema=str(data.get("receipt_schema", "")),
            matrix_cells=tuple(
                MatrixCell(str(entry.get("id", "")), dict(entry))
                for entry in data.get("matrix_cell", [])
            ),
            families=families,
            commands=tuple(
                ProofCommand(str(entry.get("id", "")), dict(entry))
                for entry in data.get("command", [])
            ),
            toolchain_policies=tuple(
                ToolchainPolicy(str(entry.get("name", "")), dict(entry))
                for entry in data.get("toolchain_policy", [])
            ),
            executor_max_workers=int(data.get("executor_max_workers", 0)),
            resource_policies=tuple(
                ResourcePolicy(
                    str(entry.get("name", "")), int(entry.get("max_parallel", 0))
                )
                for entry in data.get("resource_policy", [])
            ),
            local_rules=tuple(dict(entry) for entry in data.get("rule", [])),
            always=tuple(data.get("always", [])),
        )
        errors = plan.validate()
        if errors:
            raise ValueError("invalid proof plan:\n- " + "\n- ".join(errors))
        return plan

    def validate(self) -> list[str]:
        errors: list[str] = []
        if self.receipt_schema != RECEIPT_SCHEMA:
            errors.append(f"receipt_schema must be {RECEIPT_SCHEMA!r}")
        if self.executor_max_workers <= 0:
            errors.append("executor_max_workers must be positive")
        resource_names = [policy.name for policy in self.resource_policies]
        if not resource_names or any(not name for name in resource_names):
            errors.append("resource policies must have non-empty names")
        if len(resource_names) != len(set(resource_names)):
            errors.append("resource policy names must be unique")
        for policy in self.resource_policies:
            if policy.max_parallel <= 0:
                errors.append(f"{policy.name}: max_parallel must be positive")
            elif policy.max_parallel > self.executor_max_workers:
                errors.append(
                    f"{policy.name}: max_parallel exceeds executor_max_workers"
                )
        authority_inputs = list(self.authority_inputs)
        if not authority_inputs:
            errors.append("authority_inputs must declare the executable closure")
        if len(authority_inputs) != len(set(authority_inputs)):
            errors.append("authority_inputs must be unique")
        for relative in authority_inputs:
            normalized = _normalize_path(relative)
            if (
                normalized != relative
                or Path(relative).is_absolute()
                or ".." in Path(relative).parts
            ):
                errors.append(f"authority input is not canonical: {relative!r}")
                continue
            if not (ROOT / relative).is_file():
                errors.append(f"authority input does not exist: {relative!r}")
        try:
            manifest_relative = self.path.resolve().relative_to(ROOT).as_posix()
        except ValueError:
            manifest_relative = ""
        if manifest_relative and manifest_relative not in authority_inputs:
            errors.append("authority_inputs must include the proof-plan manifest")

        policy_names = [policy.name for policy in self.toolchain_policies]
        if not policy_names or any(not name for name in policy_names):
            errors.append("toolchain policies must have non-empty names")
        if len(policy_names) != len(set(policy_names)):
            errors.append("toolchain policy names must be unique")
        for policy in self.toolchain_policies:
            for field in REQUIRED_TOOLCHAIN_FIELDS:
                if field not in policy.data:
                    errors.append(f"{policy.name}: missing toolchain field {field}")
            probe_cwd = policy.data.get("probe_cwd", ".")
            if not isinstance(probe_cwd, str) or not probe_cwd:
                errors.append(f"{policy.name}: probe_cwd must be a non-empty string")
            else:
                normalized_probe_cwd = _normalize_path(probe_cwd)
                probe_path = Path(normalized_probe_cwd)
                if (
                    normalized_probe_cwd != probe_cwd
                    or probe_path.is_absolute()
                    or ".." in probe_path.parts
                ):
                    errors.append(
                        f"{policy.name}: probe_cwd must be a canonical "
                        f"repository-relative directory: {probe_cwd!r}"
                    )
                else:
                    try:
                        resolved_probe_cwd = (ROOT / probe_path).resolve(strict=True)
                        resolved_probe_cwd.relative_to(ROOT.resolve())
                    except (OSError, ValueError):
                        errors.append(
                            f"{policy.name}: probe_cwd must resolve inside the "
                            f"repository: {probe_cwd!r}"
                        )
                    else:
                        if not resolved_probe_cwd.is_dir():
                            errors.append(
                                f"{policy.name}: probe_cwd is not a directory: "
                                f"{probe_cwd!r}"
                            )
            args = policy.data.get("version_args")
            if not isinstance(args, list) or not all(
                isinstance(item, str) and item for item in args
            ):
                errors.append(f"{policy.name}: version_args must be a string list")
            content_path_command = policy.data.get("content_path_command")
            if content_path_command is not None and (
                not isinstance(content_path_command, list)
                or not content_path_command
                or not all(
                    isinstance(item, str) and item for item in content_path_command
                )
            ):
                errors.append(
                    f"{policy.name}: content_path_command must be a non-empty string list"
                )
            fingerprint_domain = policy.data.get("fingerprint_domain")
            if fingerprint_domain is not None and (
                not isinstance(fingerprint_domain, str) or not fingerprint_domain
            ):
                errors.append(
                    f"{policy.name}: fingerprint_domain must be a non-empty string"
                )
            pattern = policy.data.get("version_pattern")
            if not isinstance(pattern, str) or not pattern:
                errors.append(f"{policy.name}: version_pattern must be non-empty")
            else:
                try:
                    re.compile(pattern)
                except re.error as exc:
                    errors.append(f"{policy.name}: invalid version pattern: {exc}")
            evidence = policy.data.get("setup_evidence")
            if not isinstance(evidence, list) or not evidence:
                errors.append(f"{policy.name}: setup_evidence must be non-empty")
            elif all(isinstance(item, str) for item in evidence):
                for item in evidence:
                    if "::" not in item:
                        errors.append(
                            f"{policy.name}: setup evidence must be PATH::TOKEN"
                        )
                        continue
                    relative, token = item.split("::", 1)
                    evidence_path = ROOT / relative
                    if not evidence_path.is_file():
                        errors.append(
                            f"{policy.name}: setup evidence file missing: {relative}"
                        )
                    elif token not in evidence_path.read_text(encoding="utf-8"):
                        errors.append(
                            f"{policy.name}: setup evidence token missing from {relative}"
                        )
        cell_ids = [cell.id for cell in self.matrix_cells]
        if not cell_ids or any(not cell_id for cell_id in cell_ids):
            errors.append("matrix_cell IDs must be non-empty")
        if len(cell_ids) != len(set(cell_ids)):
            errors.append("matrix_cell IDs must be unique")
        for cell in self.matrix_cells:
            for field in (
                "runner",
                "os",
                "arch",
                "python",
                "backend",
                "target",
                "profile",
            ):
                if not isinstance(cell.data.get(field), str) or not cell.data[field]:
                    errors.append(f"{cell.id}: matrix cell missing non-empty {field}")
        names = [family.name for family in self.families]
        if not names:
            errors.append("at least one [[ci_family]] is required")
        if len(names) != len(set(names)):
            errors.append("ci_family names must be unique")
        for family in self.families:
            if not family.name:
                errors.append("ci_family.name must be non-empty")
            for field in REQUIRED_FAMILY_FIELDS:
                if field not in family.data:
                    errors.append(f"{family.name}: missing {field}")
            executor = family.data.get("executor")
            if executor not in {"github-job", "github-matrix", "github-workflow"}:
                errors.append(f"{family.name}: unknown executor {executor!r}")
            if family.data.get("resource_class") not in set(resource_names):
                errors.append(
                    f"{family.name}: unknown resource class "
                    f"{family.data.get('resource_class')!r}"
                )
            if not family.data.get("required"):
                errors.append(
                    f"{family.name}: selected proof families must be required"
                )
            workflow = ROOT / str(family.data.get("workflow", ""))
            if not workflow.is_file():
                errors.append(f"{family.name}: workflow does not exist: {workflow}")
            elif family.data.get("executor") in {"github-job", "github-matrix"}:
                job = str(family.data.get("job", ""))
                workflow_text = workflow.read_text(encoding="utf-8")
                block = _workflow_job_block(workflow_text, job)
                if block is None:
                    errors.append(f"{family.name}: workflow job {job!r} is missing")
                else:
                    invocation = f"--run-family {family.name} --receipt"
                    if invocation not in block:
                        errors.append(
                            f"{family.name}: workflow job does not execute {invocation!r}"
                        )
                    for receipt_token in (
                        "actions/upload-artifact@",
                        "if-no-files-found: error",
                    ):
                        if receipt_token not in block:
                            errors.append(
                                f"{family.name}: workflow job does not enforce "
                                f"receipt upload token {receipt_token!r}"
                            )
                    if "continue-on-error" in block:
                        errors.append(
                            f"{family.name}: claimed correctness job may not continue-on-error"
                        )
                    timeout = f"timeout-minutes: {family.data['timeout_minutes']}"
                    if timeout not in block:
                        errors.append(
                            f"{family.name}: workflow job does not enforce {timeout!r}"
                        )
        known = set(names)
        dependency_graph: dict[str, tuple[str, ...]] = {}
        for family in self.families:
            dependencies = family.data.get("dependencies", [])
            if not isinstance(dependencies, list) or not all(
                isinstance(dependency, str) for dependency in dependencies
            ):
                errors.append(f"{family.name}: dependencies must be a list of names")
                continue
            dependency_graph[family.name] = tuple(dependencies)
            unknown = set(dependencies) - known
            if unknown:
                errors.append(
                    f"{family.name}: unknown dependencies {sorted(unknown)!r}"
                )
        visiting: list[str] = []
        visited: set[str] = set()

        def visit(name: str) -> None:
            if name in visited:
                return
            if name in visiting:
                start = visiting.index(name)
                cycle = (*visiting[start:], name)
                errors.append(f"dependency cycle: {' -> '.join(cycle)}")
                return
            visiting.append(name)
            for dependency in dependency_graph.get(name, ()):
                if dependency in dependency_graph:
                    visit(dependency)
            visiting.pop()
            visited.add(name)

        for name in names:
            visit(name)

        command_ids = [command.id for command in self.commands]
        if not command_ids or any(not command_id for command_id in command_ids):
            errors.append("command IDs must be non-empty")
        if len(command_ids) != len(set(command_ids)):
            errors.append("command IDs must be unique")
        known_commands = set(command_ids)
        known_cells = set(cell_ids)
        known_toolchains = set(policy_names)
        known_resources = set(resource_names)
        commands_by_family: dict[str, int] = {name: 0 for name in names}
        referenced_cells: set[str] = set()
        command_shapes: set[tuple[str, tuple[str, ...], str]] = set()
        command_graph: dict[str, tuple[str, ...]] = {}
        for command in self.commands:
            if re.fullmatch(r"[a-z0-9]+(?:[.-][a-z0-9]+)*", command.id) is None:
                errors.append(f"{command.id}: command ID is not canonical")
            for field in REQUIRED_COMMAND_FIELDS:
                if field not in command.data:
                    errors.append(f"{command.id}: missing {field}")
            if command.family not in known:
                errors.append(f"{command.id}: unknown family {command.family!r}")
            else:
                commands_by_family[command.family] += 1
            resource_class = command.data.get("resource_class")
            if resource_class not in known_resources:
                errors.append(
                    f"{command.id}: unknown resource class {resource_class!r}"
                )
            if command.data.get("cell") not in known_cells:
                errors.append(
                    f"{command.id}: unknown matrix cell {command.data.get('cell')!r}"
                )
            else:
                referenced_cells.add(str(command.data["cell"]))
            argv = command.data.get("argv")
            if (
                not isinstance(argv, list)
                or not argv
                or not all(isinstance(part, str) and part for part in argv)
            ):
                errors.append(f"{command.id}: argv must be a non-empty string list")
            toolchains = command.data.get("toolchains")
            if (
                not isinstance(toolchains, list)
                or not toolchains
                or not all(isinstance(name, str) and name for name in toolchains)
            ):
                errors.append(f"{command.id}: toolchains must be a non-empty list")
            elif len(toolchains) != len(set(toolchains)):
                errors.append(f"{command.id}: toolchains must be unique")
            elif unknown_toolchains := set(toolchains) - known_toolchains:
                errors.append(
                    f"{command.id}: unknown toolchains {sorted(unknown_toolchains)!r}"
                )
            timeout_env = command.data.get("timeout_env", [])
            if not isinstance(timeout_env, list) or not all(
                isinstance(name, str) and name for name in timeout_env
            ):
                errors.append(f"{command.id}: timeout_env must be a string list")
            elif len(timeout_env) != len(set(timeout_env)):
                errors.append(f"{command.id}: timeout_env must be unique")
            environment = command.data.get("env", {})
            if not isinstance(environment, dict) or not all(
                isinstance(name, str) and name and isinstance(value, str)
                for name, value in environment.items()
            ):
                errors.append(f"{command.id}: env must be a string map")
            timeout = command.data.get("timeout_seconds")
            if not isinstance(timeout, int) or timeout <= 0:
                errors.append(f"{command.id}: timeout_seconds must be positive")
            elif command.family in known:
                family = next(
                    item for item in self.families if item.name == command.family
                )
                if timeout > int(family.data["timeout_minutes"]) * 60:
                    errors.append(
                        f"{command.id}: command timeout exceeds family job timeout"
                    )
                tiers = command.data.get("tiers")
                if not isinstance(tiers, list) or not tiers:
                    errors.append(f"{command.id}: tiers must be a non-empty list")
                elif not set(tiers).issubset(set(family.data["tiers"])):
                    errors.append(f"{command.id}: command tiers escape family tiers")
            if isinstance(argv, list) and all(isinstance(part, str) for part in argv):
                for part in argv:
                    candidate = part.split("::", 1)[0]
                    if (
                        candidate.startswith(
                            ("examples/", "formal/", "src/", "tests/", "tools/")
                        )
                        and not (ROOT / candidate).exists()
                    ):
                        errors.append(
                            f"{command.id}: repository input does not exist: {candidate!r}"
                        )
                shape = (
                    command.family,
                    tuple(argv),
                    str(command.data.get("cwd", ".")),
                    str(command.data.get("cell", "")),
                )
                if shape in command_shapes:
                    errors.append(f"{command.id}: duplicate executable command shape")
                command_shapes.add(shape)
            dependencies = command.data.get("dependencies")
            if not isinstance(dependencies, list) or not all(
                isinstance(dependency, str) for dependency in dependencies
            ):
                errors.append(f"{command.id}: dependencies must be a list of IDs")
                continue
            command_graph[command.id] = tuple(dependencies)
            unknown = set(dependencies) - known_commands
            if unknown:
                errors.append(
                    f"{command.id}: unknown command dependencies {sorted(unknown)!r}"
                )
        for family, count in commands_by_family.items():
            if count == 0:
                errors.append(f"{family}: selected family has no executable commands")
        used_resources = {
            str(command.data.get("resource_class", "")) for command in self.commands
        }
        for unused in sorted(known_resources - used_resources):
            errors.append(f"{unused}: resource policy has no executable command")
        for cell_id in sorted(known_cells - referenced_cells):
            errors.append(f"{cell_id}: matrix cell has no executable command")

        visiting_commands: list[str] = []
        visited_commands: set[str] = set()

        def visit_command(command_id: str) -> None:
            if command_id in visited_commands:
                return
            if command_id in visiting_commands:
                start = visiting_commands.index(command_id)
                cycle = (*visiting_commands[start:], command_id)
                errors.append(f"command dependency cycle: {' -> '.join(cycle)}")
                return
            visiting_commands.append(command_id)
            command = next(
                (item for item in self.commands if item.id == command_id), None
            )
            for dependency in command_graph.get(command_id, ()):
                dependency_command = next(
                    (item for item in self.commands if item.id == dependency), None
                )
                if (
                    command is not None
                    and dependency_command is not None
                    and command.family != dependency_command.family
                ):
                    errors.append(
                        f"{command_id}: command dependency {dependency!r} crosses families"
                    )
                if (
                    command is not None
                    and dependency_command is not None
                    and command.family == dependency_command.family
                ):
                    family = next(
                        item for item in self.families if item.name == command.family
                    )
                    if family.data.get(
                        "executor"
                    ) == "github-matrix" and command.data.get(
                        "cell"
                    ) != dependency_command.data.get("cell"):
                        errors.append(
                            f"{command_id}: matrix command dependency {dependency!r} "
                            "crosses runner cells"
                        )
                visit_command(dependency)
            visiting_commands.pop()
            visited_commands.add(command_id)

        for command_id in command_ids:
            visit_command(command_id)

        if not any(error.startswith("command dependency cycle:") for error in errors):
            for family in self.families:
                if family.data.get("executor") != "github-job":
                    continue
                try:
                    envelope = self.timeout_envelope(family.name)
                except (KeyError, TypeError, ValueError) as exc:
                    errors.append(str(exc))
                    continue
                job_budget = int(family.data["timeout_minutes"]) * 60
                if envelope.projected_makespan_seconds > job_budget:
                    errors.append(
                        f"{family.name}: projected resource-aware timeout envelope "
                        f"{envelope.projected_makespan_seconds}s exceeds GitHub job "
                        f"budget {job_budget}s"
                    )
        for family in self.families:
            if family.data.get("executor") != "github-workflow":
                continue
            workflow = ROOT / str(family.data["workflow"])
            if not workflow.is_file():
                continue
            text = workflow.read_text(encoding="utf-8")
            for command in self.commands:
                if command.family != family.name:
                    continue
                invocation = f"--run-command {command.id} --receipt"
                if invocation not in text:
                    errors.append(
                        f"{family.name}: workflow does not execute {invocation!r}"
                    )
            expected_receipts = sum(
                1 for command in self.commands if command.family == family.name
            )
            for token in ("actions/upload-artifact@", "if-no-files-found: error"):
                if text.count(token) < expected_receipts:
                    errors.append(
                        f"{family.name}: workflow has fewer {token!r} receipt "
                        "uploads than executable commands"
                    )
            if "continue-on-error" in text:
                errors.append(
                    f"{family.name}: claimed correctness workflow may not continue-on-error"
                )
        verdict_workflow = ROOT / ".github" / "workflows" / "ci.yml"
        if verdict_workflow.is_file():
            verdict_text = verdict_workflow.read_text(encoding="utf-8")
            verdict_block = _workflow_job_block(verdict_text, "proof-plan-verdict")
            for token in (
                "actions/download-artifact@",
                "--verify-selected",
                "--receipt-dir proof-receipts",
            ):
                if verdict_block is None or token not in verdict_block:
                    errors.append(
                        f"proof-plan-verdict: missing executable input {token!r}"
                    )
            if verdict_block is not None and "--result" in verdict_block:
                errors.append(
                    "proof-plan-verdict: legacy synthetic job results are forbidden"
                )
        local_names: set[str] = set()
        for rule in self.local_rules:
            name = rule.get("name")
            if not isinstance(name, str) or not name:
                errors.append("local [[rule]] missing non-empty name")
                continue
            if name in local_names:
                errors.append(f"duplicate local rule {name!r}")
            local_names.add(name)
            for field in ("globs", "gates"):
                value = rule.get(field)
                if not isinstance(value, list) or not all(
                    isinstance(item, str) for item in value
                ):
                    errors.append(f"{name}: {field} must be a list of strings")
        return errors

    def all_selected(self, *, reason: str, fail_closed: bool = True) -> Selection:
        return Selection(
            changed_paths=(),
            selected=self.families,
            reasons={family.name: (reason,) for family in self.families},
            fail_closed_reason=reason if fail_closed else None,
        )

    def select(self, paths: list[str] | tuple[str, ...]) -> Selection:
        normalized = tuple(_normalize_path(path) for path in paths if path.strip())
        authority_matches = tuple(
            path
            for path in normalized
            if any(_matches(path, pattern) for pattern in self.authority_inputs)
        )
        direct_reasons: dict[str, tuple[str, ...]] = {}
        for family in self.families:
            matched = tuple(
                path
                for path in normalized
                if any(_matches(path, pattern) for pattern in family.inputs)
            )
            family_reasons = tuple(dict.fromkeys((*authority_matches, *matched)))
            if family_reasons:
                direct_reasons[family.name] = family_reasons

        selected_names = set(direct_reasons)
        dependency_reasons: dict[str, list[str]] = {}
        pending = [
            family.name for family in self.families if family.name in selected_names
        ]
        family_by_name = {family.name: family for family in self.families}
        for name in pending:
            for dependency in family_by_name[name].data["dependencies"]:
                reason = f"dependency:{name}"
                if dependency not in selected_names:
                    dependency_reasons.setdefault(dependency, []).append(reason)
                    selected_names.add(dependency)
                    pending.append(dependency)
                elif dependency not in direct_reasons:
                    dependency_reasons.setdefault(dependency, [])
                    if reason not in dependency_reasons[dependency]:
                        dependency_reasons[dependency].append(reason)

        selected = tuple(
            family for family in self.families if family.name in selected_names
        )
        reasons = {
            family.name: tuple(
                (
                    *direct_reasons.get(family.name, ()),
                    *dependency_reasons.get(family.name, ()),
                )
            )
            for family in selected
        }
        return Selection(normalized, selected, reasons)


def _normalize_path(path: str) -> str:
    return path.replace("\\", "/").removeprefix("./")


def _toolchain_probe_cwd(policy: ToolchainPolicy) -> tuple[str, Path]:
    relative = str(policy.data.get("probe_cwd", "."))
    return relative, (ROOT / relative).resolve(strict=True)


def _workflow_job_block(text: str, job: str) -> str | None:
    match = re.search(
        rf"(?ms)^  {re.escape(job)}:\n.*?(?=^  [A-Za-z0-9_-]+:\n|\Z)", text
    )
    return match.group(0) if match is not None else None


def _matches(path: str, pattern: str) -> bool:
    normalized = _normalize_path(pattern)
    if normalized.endswith("/**"):
        prefix = normalized[:-3].rstrip("/")
        return path == prefix or path.startswith(prefix + "/")
    return fnmatch.fnmatchcase(path, normalized)


def _run_git(args: list[str]) -> str:
    return subprocess.check_output(
        ["git", *args], cwd=ROOT, text=True, stderr=subprocess.STDOUT
    )


def _source_commit() -> str:
    commit = (
        os.environ.get("GITHUB_SHA", "").strip()
        or _run_git(["rev-parse", "HEAD"]).strip()
    )
    if re.fullmatch(r"[0-9a-fA-F]{40}|[0-9a-fA-F]{64}", commit) is None:
        raise ValueError(f"invalid source commit identity {commit!r}")
    return commit.lower()


def _source_tree_state() -> str:
    """Return whether executable proof inputs are exactly commit-backed.

    A commit SHA alone does not attest staged, unstaged, or untracked source.
    Receipts therefore fail closed unless Git reports an entirely clean tree at
    the instant execution begins. Keep the probe byte-oriented so unusual path
    encodings cannot weaken the cleanliness decision.
    """
    status = subprocess.check_output(
        [
            "git",
            "status",
            "--porcelain=v1",
            "-z",
            "--untracked-files=all",
            "--ignore-submodules=none",
        ],
        cwd=ROOT,
    )
    return "clean" if not status else "dirty"


def _diff_paths(base: str, head: str, *, three_dot: bool = False) -> list[str]:
    if not base or not head or base == NULL_SHA or head == NULL_SHA:
        raise RuntimeError("event does not provide two non-null commit identities")
    separator = "..." if three_dot else ".."
    output = _run_git(
        [
            "diff",
            "--name-status",
            "-z",
            "--diff-filter=ACDMRTUXB",
            f"{base}{separator}{head}",
        ]
    )
    tokens = output.split("\0")
    paths: list[str] = []
    index = 0
    while index < len(tokens):
        status = tokens[index]
        index += 1
        if not status:
            continue
        path_count = 2 if status[0] in {"R", "C"} else 1
        if index + path_count > len(tokens):
            raise RuntimeError(f"malformed git diff record for status {status!r}")
        for path in tokens[index : index + path_count]:
            if path and path not in paths:
                paths.append(path)
        index += path_count
    return paths


def _pull_request_paths(base_ref: str) -> list[str]:
    if not base_ref:
        raise RuntimeError("GITHUB_BASE_REF is not set")
    remote_ref = f"origin/{base_ref}"
    try:
        _run_git(["rev-parse", "--verify", remote_ref])
    except subprocess.CalledProcessError:
        _run_git(
            [
                "fetch",
                "--no-tags",
                "--prune",
                "origin",
                f"+{base_ref}:refs/remotes/{remote_ref}",
            ]
        )
    return _diff_paths(remote_ref, "HEAD", three_dot=True)


def _event_payload(path: str) -> dict[str, Any]:
    if not path:
        return {}
    value = json.loads(Path(path).read_text(encoding="utf-8"))
    if not isinstance(value, dict):
        raise RuntimeError("GitHub event payload is not an object")
    return value


def selection_for_event(
    plan: ProofPlan,
    *,
    event_name: str,
    base_ref: str,
    event_path: str,
    before: str,
    after: str,
) -> Selection:
    try:
        payload = _event_payload(event_path)
        if event_name in {"pull_request", "pull_request_target"}:
            return plan.select(_pull_request_paths(base_ref))
        if event_name == "push":
            if bool(payload.get("forced")):
                raise RuntimeError("forced push has no trustworthy incremental base")
            base = before or str(payload.get("before", ""))
            head = after or str(payload.get("after", "")) or "HEAD"
            return plan.select(_diff_paths(base, head))
        if event_name in {
            "merge_group",
            "schedule",
            "workflow_dispatch",
            "workflow_call",
        }:
            return plan.all_selected(
                reason=f"{event_name}: full proof plan", fail_closed=False
            )
        raise RuntimeError(f"unsupported or missing event {event_name!r}")
    except Exception as exc:
        return plan.all_selected(reason=f"fail-closed event selection: {exc}")


def family_outputs(plan: ProofPlan, selection: Selection) -> dict[str, str]:
    selected = {family.name for family in selection.selected}
    outputs = {
        family.name: "true" if family.name in selected else "false"
        for family in plan.families
    }
    topology = [
        {
            "name": family.name,
            **{
                key: family.data[key]
                for key in (
                    "executor",
                    "workflow",
                    "job",
                    "required",
                    "timeout_minutes",
                    "resource_class",
                )
            },
            "command_ids": [
                command.id for command in plan.commands if command.family == family.name
            ],
            "matrix_cells": list(
                dict.fromkeys(
                    str(command.data["cell"])
                    for command in plan.commands
                    if command.family == family.name
                )
            ),
            "selected_by": list(selection.reasons.get(family.name, ())),
        }
        for family in selection.selected
    ]
    matrix_family_names = {
        family.name
        for family in selection.selected
        if family.data["executor"] == "github-matrix"
    }
    matrix = []
    for cell in plan.matrix_cells:
        command_ids = [
            command.id
            for command in plan.commands
            if command.family in matrix_family_names and command.data["cell"] == cell.id
        ]
        if not command_ids:
            continue
        families = {
            plan_command.family
            for plan_command in plan.commands
            if plan_command.id in command_ids
        }
        if len(families) != 1:
            raise ValueError(f"{cell.id}: executable matrix cell spans families")
        family_name = families.pop()
        matrix.append(
            {
                "family": family_name,
                "cell": cell.id,
                **{
                    key: cell.data[key]
                    for key in (
                        "runner",
                        "os",
                        "arch",
                        "python",
                        "backend",
                        "target",
                        "profile",
                    )
                },
                "command_ids": command_ids,
                "selected_by": list(selection.reasons.get(family_name, ())),
            }
        )
    outputs["topology"] = json.dumps({"include": topology}, separators=(",", ":"))
    outputs["matrix"] = json.dumps({"include": matrix}, separators=(",", ":"))
    outputs["selected"] = json.dumps(sorted(selected), separators=(",", ":"))
    outputs["changed_paths"] = json.dumps(
        selection.changed_paths, separators=(",", ":")
    )
    return outputs


def _authority_sha256(
    plan: ProofPlan,
    overrides: Mapping[str, bytes] | None = None,
) -> str:
    """Hash canonical path + content for the complete declared authority closure."""
    override_map = {} if overrides is None else dict(overrides)
    unknown = set(override_map) - set(plan.authority_inputs)
    if unknown:
        raise ValueError(f"unknown authority input overrides: {sorted(unknown)!r}")
    digest = hashlib.sha256(b"molt.proof-authority.v2\0")
    for relative in sorted(plan.authority_inputs):
        content = override_map.get(relative)
        if content is None:
            content = (ROOT / relative).read_bytes()
        try:
            canonical_content = (
                content.decode("utf-8")
                .replace("\r\n", "\n")
                .replace("\r", "\n")
                .encode("utf-8")
            )
        except UnicodeDecodeError as exc:
            raise ValueError(
                f"authority input must be canonical UTF-8 text: {relative}"
            ) from exc
        encoded_path = relative.encode("utf-8")
        digest.update(len(encoded_path).to_bytes(8, "big"))
        digest.update(encoded_path)
        digest.update(len(canonical_content).to_bytes(8, "big"))
        digest.update(canonical_content)
    return digest.hexdigest()


def _normalized_os() -> str:
    value = platform.system().lower()
    return {"darwin": "macos", "windows": "windows"}.get(value, value)


def _normalized_arch() -> str:
    value = platform.machine().lower()
    return {"amd64": "x86_64", "x64": "x86_64", "arm64": "aarch64"}.get(value, value)


def _version_fingerprint(policy: ToolchainPolicy) -> dict[str, str] | None:
    executable = str(policy.data["executable"])
    requested = sys.executable if executable == "{python}" else executable
    path = shutil.which(requested)
    if path is None:
        return None
    probe_cwd, probe_directory = _toolchain_probe_cwd(policy)
    command_path = Path(path).absolute()
    launcher_path = command_path.resolve()

    def content_hash(executable_path: Path) -> str:
        try:
            with executable_path.open("rb") as executable_file:
                return hashlib.file_digest(executable_file, "sha256").hexdigest()
        except OSError as exc:
            return f"unavailable:{type(exc).__name__}"

    launcher_sha256 = content_hash(launcher_path)
    content_path = launcher_path
    content_path_command = policy.data.get("content_path_command")
    if isinstance(content_path_command, list):
        try:
            resolved = subprocess.run(
                content_path_command,
                cwd=probe_directory,
                check=False,
                stdout=subprocess.PIPE,
                stderr=subprocess.PIPE,
                text=True,
                timeout=5,
            )
            candidates = tuple(
                line.strip() for line in resolved.stdout.splitlines() if line.strip()
            )
            if resolved.returncode != 0 or len(candidates) != 1:
                raise OSError("toolchain content resolver failed")
            candidate = candidates[0]
            candidate_path = Path(candidate)
            if not candidate_path.is_absolute():
                candidate_path = probe_directory / candidate_path
            content_path = candidate_path.resolve(strict=True)
        except (IndexError, OSError, subprocess.TimeoutExpired):
            content_path = Path("unavailable")
    executable_sha256 = content_hash(content_path)
    try:
        completed = subprocess.run(
            [path, *policy.data["version_args"]],
            cwd=probe_directory,
            check=False,
            stdout=subprocess.PIPE,
            stderr=subprocess.STDOUT,
            text=True,
            timeout=5,
        )
        version = completed.stdout.strip()
    except (OSError, subprocess.TimeoutExpired) as exc:
        version = f"unavailable:{type(exc).__name__}"
    material = (
        f"{command_path}\0{launcher_path}\0{launcher_sha256}\0{content_path}\0"
        f"{executable_sha256}\0{version}\0{probe_cwd}"
    ).encode()
    return {
        "path": str(command_path),
        "launcher_path": str(launcher_path),
        "launcher_sha256": launcher_sha256,
        "content_path": str(content_path),
        "version": version,
        "version_pattern": str(policy.data["version_pattern"]),
        "probe_cwd": probe_cwd,
        "executable_sha256": executable_sha256,
        "identity_sha256": hashlib.sha256(material).hexdigest(),
    }


def toolchain_fingerprints(
    plan: ProofPlan,
    names: tuple[str, ...],
) -> dict[str, dict[str, str]]:
    policies = {policy.name: policy for policy in plan.toolchain_policies}
    fingerprints: dict[str, dict[str, str]] = {}
    errors: list[str] = []
    domains: dict[str, list[str]] = {}
    for name in names:
        policy = policies[name]
        domain = str(policy.data.get("fingerprint_domain", f"toolchain:{name}"))
        domains.setdefault(domain, []).append(name)
    worker_count = min(4, len(domains))
    if worker_count == 0:
        return fingerprints

    def fingerprint_domain(
        domain_names: list[str],
    ) -> dict[str, dict[str, str] | None]:
        # Launchers in one provisioning domain can share mutable install state
        # (notably rustup). Probe that domain serially while independent
        # toolchains retain bounded parallelism.
        return {name: _version_fingerprint(policies[name]) for name in domain_names}

    with ThreadPoolExecutor(max_workers=worker_count) as executor:
        futures = {
            domain: executor.submit(fingerprint_domain, domain_names)
            for domain, domain_names in domains.items()
        }
    domain_results = {domain: future.result() for domain, future in futures.items()}
    for name in names:
        policy = policies[name]
        domain = str(policy.data.get("fingerprint_domain", f"toolchain:{name}"))
        fingerprint = domain_results[domain][name]
        if fingerprint is None:
            errors.append(f"{name}: executable {policy.data['executable']!r} not found")
            continue
        if (
            re.search(str(policy.data["version_pattern"]), fingerprint["version"])
            is None
        ):
            errors.append(
                f"{name}: version {fingerprint['version']!r} violates "
                f"{policy.data['version_pattern']!r}"
            )
            continue
        for hash_field in ("launcher_sha256", "executable_sha256"):
            digest = fingerprint[hash_field]
            if len(digest) != 64 or any(
                character not in "0123456789abcdef" for character in digest.lower()
            ):
                errors.append(f"{name}: {hash_field} is unavailable")
                break
        else:
            fingerprints[name] = fingerprint
            continue
    if errors:
        raise ValueError("toolchain contract violation: " + "; ".join(errors))
    return fingerprints


def _cache_disposition(command: ProofCommand) -> str:
    domain = str(command.data["cache_domain"])
    if domain in {"none", "network"}:
        return "not-applicable"
    # Directory existence is not cache-hit evidence: a failed or partial build
    # also leaves directories behind. Until a command supplies counted restore
    # or rebuilt-byte telemetry, receipts must remain explicitly unobserved.
    return "unknown"


def _required_toolchains(command: ProofCommand) -> tuple[str, ...]:
    return command.toolchains


def _topological_commands(
    plan: ProofPlan,
    *,
    family: str | None = None,
    command_id: str | None = None,
    matrix_cell: str | None = None,
) -> tuple[ProofCommand, ...]:
    by_id = {command.id: command for command in plan.commands}
    if family is not None:
        selected = {
            command.id
            for command in plan.commands
            if command.family == family
            and (matrix_cell is None or command.data["cell"] == matrix_cell)
        }
        if not selected:
            suffix = "" if matrix_cell is None else f" in matrix cell {matrix_cell!r}"
            raise ValueError(f"unknown or empty proof family {family!r}{suffix}")
    elif command_id is not None:
        if command_id not in by_id:
            raise ValueError(f"unknown proof command {command_id!r}")
        selected = {command_id}
    else:
        raise ValueError("one of family or command_id is required")
    ordered: list[ProofCommand] = []
    emitted: set[str] = set()

    def emit(current: str) -> None:
        if current in emitted or current not in selected:
            return
        for dependency in by_id[current].dependencies:
            emit(dependency)
        ordered.append(by_id[current])
        emitted.add(current)

    for current in (command.id for command in plan.commands if command.id in selected):
        emit(current)
    return tuple(ordered)


def _base_command_record(command: ProofCommand) -> dict[str, Any]:
    return {
        "id": command.id,
        "family": command.family,
        "cell": command.data["cell"],
        "argv": list(command.argv),
        "cwd": str(command.data.get("cwd", ".")),
        "dependencies": list(command.dependencies),
        "tiers": list(command.data["tiers"]),
        "resource_class": command.data["resource_class"],
        "timeout_seconds": int(command.data["timeout_seconds"]),
        "timeout_env": list(command.data.get("timeout_env", [])),
        "environment_overrides": dict(command.data.get("env", {})),
    }


def _terminate_guarded_executor(process: subprocess.Popen[Any]) -> bool:
    """Terminate one guarded-exec owner; its existing custody reaps descendants.

    On POSIX, ``terminate`` delivers SIGTERM to guarded_exec, whose memory guard
    records the interruption and terminates its tracked process tree. On Windows,
    terminating guarded_exec closes its sole KILL_ON_JOB_CLOSE handle, so the OS
    reaps the guarded subtree. Escalation remains scoped to that exact owner PID.
    """

    if process.poll() is not None:
        return False
    process.terminate()
    try:
        process.wait(timeout=5.0)
        return False
    except subprocess.TimeoutExpired:
        process.kill()
        process.wait(timeout=5.0)
        return True


def _run_command(
    command: ProofCommand,
    metrics_path: Path,
    cancel_event: threading.Event | None = None,
) -> dict[str, Any]:
    relative_cwd = str(command.data.get("cwd", "."))
    cache = _cache_disposition(command)
    started_at = dt.datetime.now(dt.UTC).isoformat()
    timeout = int(command.data["timeout_seconds"])
    if cancel_event is not None and cancel_event.is_set():
        return {
            **_base_command_record(command),
            "started_at": started_at,
            "duration_seconds": 0.0,
            "peak_rss_bytes": None,
            "cache_disposition": cache,
            "status": "cancelled",
            "returncode": 130,
            "guard_metrics_schema": None,
            "cancelled_by_fail_fast": True,
            "termination_escalated": False,
        }
    wrapped = [
        sys.executable,
        str(ROOT / "tools" / "guarded_exec.py"),
        "--prefix",
        "MOLT_PROOF",
        "--timeout",
        str(timeout),
        "--metrics-json",
        str(metrics_path),
    ]
    if relative_cwd != ".":
        wrapped.extend(("--cwd", relative_cwd))
    wrapped.extend(("--", *command.argv))
    child_env = dict(os.environ)
    existing_pythonpath = child_env.get("PYTHONPATH", "")
    child_env["PYTHONPATH"] = os.pathsep.join(
        part for part in (str(ROOT / "src"), existing_pythonpath) if part
    )
    for name in command.data.get("timeout_env", []):
        child_env[str(name)] = str(timeout)
    child_env.update(
        {str(name): str(value) for name, value in command.data.get("env", {}).items()}
    )
    process = subprocess.Popen(
        wrapped,
        cwd=ROOT,
        env=child_env,
    )
    cancelled = False
    termination_escalated = False
    while process.poll() is None:
        if cancel_event is not None and cancel_event.wait(0.05):
            if process.poll() is None:
                cancelled = True
                termination_escalated = _terminate_guarded_executor(process)
            break
    if process.poll() is None:
        process.wait()
    completed_returncode = int(process.returncode or 0)
    try:
        metrics = json.loads(metrics_path.read_text(encoding="utf-8"))
    except (OSError, json.JSONDecodeError):
        metrics = {}
    finally:
        metrics_path.unlink(missing_ok=True)
    metrics_valid = (
        metrics.get("schema") == "molt.guarded-command-metrics.v1"
        and metrics.get("returncode") == completed_returncode
        and isinstance(metrics.get("duration_seconds"), (int, float))
        and isinstance(metrics.get("peak_tree_rss_bytes"), int)
    )
    returncode = (
        130
        if cancelled
        else completed_returncode
        if metrics_valid
        else completed_returncode or 2
    )
    status = (
        "cancelled"
        if cancelled
        else "timeout"
        if returncode == 124
        else "success"
        if returncode == 0 and metrics_valid
        else "failure"
    )
    return {
        **_base_command_record(command),
        "started_at": started_at,
        "duration_seconds": (
            round(float(metrics["duration_seconds"]), 6) if metrics_valid else None
        ),
        "peak_rss_bytes": metrics.get("peak_tree_rss_bytes") if metrics_valid else None,
        "cache_disposition": cache,
        "status": status,
        "returncode": returncode,
        "guard_metrics_schema": metrics.get("schema"),
        "cancelled_by_fail_fast": cancelled,
        "termination_escalated": termination_escalated,
    }


def execute_commands(
    plan: ProofPlan,
    commands: Iterable[ProofCommand],
    receipt_path: Path,
) -> int:
    command_list = tuple(commands)
    if not command_list:
        raise ValueError("receipt execution requires at least one command")
    source_tree_state = _source_tree_state()
    if source_tree_state != "clean":
        raise ValueError(
            "executable proof receipts require a clean source tree; commit or "
            "remove every staged, unstaged, and untracked input first"
        )
    receipt_path.parent.mkdir(parents=True, exist_ok=True)
    command_ids = [command.id for command in command_list]
    if len(command_ids) != len(set(command_ids)):
        raise ValueError("receipt execution command IDs must be unique")
    command_by_id = {command.id: command for command in command_list}
    command_index = {command.id: index for index, command in enumerate(command_list)}
    resource_limits = {
        policy.name: policy.max_parallel for policy in plan.resource_policies
    }
    unknown_resources = {
        str(command.data["resource_class"])
        for command in command_list
        if str(command.data["resource_class"]) not in resource_limits
    }
    if unknown_resources:
        raise ValueError(
            f"receipt execution has unknown resources {sorted(unknown_resources)!r}"
        )
    records_by_id: dict[str, dict[str, Any]] = {}
    requested_toolchains = tuple(
        dict.fromkeys(
            name for command in command_list for name in _required_toolchains(command)
        )
    )
    toolchain_error: str | None = None
    try:
        toolchains = toolchain_fingerprints(plan, requested_toolchains)
    except ValueError as exc:
        toolchains = {}
        toolchain_error = str(exc)
    execution: dict[str, Any] = {
        "schema": "molt.proof-plan-dag-executor.v1",
        "max_workers": plan.executor_max_workers,
        "resource_limits": resource_limits,
        "declared_timeout_seconds": sum(
            int(command.data["timeout_seconds"]) for command in command_list
        ),
        "scheduled_commands": 0,
        "peak_active_commands": 0,
        "peak_active_by_resource": {name: 0 for name in sorted(resource_limits)},
        "fail_fast_triggered": False,
    }
    receipt_errors: list[str] = []
    receipt: dict[str, Any] = {
        "schema": plan.receipt_schema,
        "authority_sha256": _authority_sha256(plan),
        "source_commit": _source_commit(),
        "source_tree_state": source_tree_state,
        "family": command_list[0].family,
        "environment": {
            "os": _normalized_os(),
            "arch": _normalized_arch(),
            "python": f"{sys.version_info.major}.{sys.version_info.minor}",
        },
        "toolchains": toolchains,
        "commands": [],
        "executed_partitions": [],
        "status": "failure" if toolchain_error else "running",
        "execution": execution,
    }
    if toolchain_error:
        receipt_errors.append(toolchain_error)
        receipt["errors"] = receipt_errors
    atomic_write_json(receipt_path, receipt, indent=2, sort_keys=True)
    if toolchain_error:
        return 2
    scheduler_started = time.monotonic()
    pending_ids = set(command_ids)
    dependents: dict[str, list[str]] = {command_id: [] for command_id in command_ids}
    remaining_dependencies: dict[str, int] = {}
    for command in command_list:
        included_dependencies = tuple(
            dependency
            for dependency in command.dependencies
            if dependency in command_by_id
        )
        remaining_dependencies[command.id] = len(included_dependencies)
        for dependency in included_dependencies:
            dependents[dependency].append(command.id)
    ready_by_resource: dict[str, list[tuple[int, str]]] = {
        name: [] for name in resource_limits
    }
    for command in command_list:
        if remaining_dependencies[command.id] == 0:
            resource = str(command.data["resource_class"])
            heapq.heappush(
                ready_by_resource[resource], (command_index[command.id], command.id)
            )
    active_by_resource = {name: 0 for name in resource_limits}
    active: dict[Future[dict[str, Any]], ProofCommand] = {}
    cancel_event = threading.Event()
    failed = False

    def record_error(message: str) -> None:
        receipt_errors.append(message)
        receipt["errors"] = receipt_errors

    def refresh_receipt() -> None:
        ordered_records = [
            records_by_id[command.id]
            for command in command_list
            if command.id in records_by_id
        ]
        receipt["commands"] = ordered_records
        receipt["executed_partitions"] = [
            command.id
            for command in command_list
            if records_by_id.get(command.id, {}).get("status") == "success"
        ]
        execution["duration_seconds"] = round(time.monotonic() - scheduler_started, 6)
        atomic_write_json(receipt_path, receipt, indent=2, sort_keys=True)

    with ThreadPoolExecutor(
        max_workers=plan.executor_max_workers,
        thread_name_prefix="proof-plan",
    ) as executor:
        while pending_ids or active:
            if not failed:
                if _source_tree_state() != "clean":
                    failed = True
                    cancel_event.set()
                    receipt["status"] = "failure"
                    execution["fail_fast_triggered"] = True
                    record_error(
                        "source tree changed before executable scheduling wave"
                    )
                while not failed and len(active) < plan.executor_max_workers:
                    available_resources = tuple(
                        resource
                        for resource, ready in ready_by_resource.items()
                        if ready
                        and active_by_resource[resource] < resource_limits[resource]
                    )
                    if not available_resources:
                        break
                    resource = min(
                        available_resources,
                        key=lambda name: ready_by_resource[name][0][0],
                    )
                    _, command_id = heapq.heappop(ready_by_resource[resource])
                    command = command_by_id[command_id]
                    pending_ids.remove(command.id)
                    metrics_path = receipt_path.with_name(
                        f".{receipt_path.name}.{command.id}.metrics.json"
                    )
                    future = executor.submit(
                        _run_command, command, metrics_path, cancel_event
                    )
                    active[future] = command
                    active_by_resource[resource] += 1
                    execution["scheduled_commands"] = (
                        int(execution["scheduled_commands"]) + 1
                    )
                    execution["peak_active_commands"] = max(
                        int(execution["peak_active_commands"]),
                        len(active),
                    )
                    peaks: dict[str, int] = execution["peak_active_by_resource"]
                    peaks[resource] = max(peaks[resource], active_by_resource[resource])

            if not active:
                if pending_ids and not failed:
                    blocked = ", ".join(
                        command.id
                        for command in command_list
                        if command.id in pending_ids
                    )
                    record_error(f"executor dependency deadlock: {blocked}")
                    receipt["status"] = "failure"
                    execution["fail_fast_triggered"] = True
                    failed = True
                break

            completed, _ = wait(tuple(active), return_when=FIRST_COMPLETED)
            for future in sorted(
                completed, key=lambda item: command_index[active[item].id]
            ):
                command = active.pop(future)
                resource = str(command.data["resource_class"])
                active_by_resource[resource] -= 1
                try:
                    record = future.result()
                except Exception as exc:
                    record = {
                        **_base_command_record(command),
                        "started_at": dt.datetime.now(dt.UTC).isoformat(),
                        "duration_seconds": None,
                        "peak_rss_bytes": None,
                        "cache_disposition": _cache_disposition(command),
                        "status": "failure",
                        "returncode": 2,
                        "guard_metrics_schema": None,
                        "executor_error": f"{type(exc).__name__}: {exc}",
                    }
                if _source_tree_state() != "clean":
                    record["status"] = "failure"
                    record["returncode"] = 2
                    record["source_tree_state_after"] = "dirty"
                    record_error(
                        f"{command.id}: executable partition mutated the source tree"
                    )
                records_by_id[command.id] = record
                if record["status"] == "success":
                    for dependent in dependents[command.id]:
                        remaining_dependencies[dependent] -= 1
                        if remaining_dependencies[dependent] == 0:
                            dependent_command = command_by_id[dependent]
                            dependent_resource = str(
                                dependent_command.data["resource_class"]
                            )
                            heapq.heappush(
                                ready_by_resource[dependent_resource],
                                (command_index[dependent], dependent),
                            )
                elif record["status"] != "cancelled":
                    failed = True
                    cancel_event.set()
                    execution["fail_fast_triggered"] = True
                receipt["status"] = "failure" if failed else "running"
                refresh_receipt()

    if pending_ids:
        for command in command_list:
            if command.id not in pending_ids:
                continue
            records_by_id[command.id] = {
                **_base_command_record(command),
                "started_at": None,
                "duration_seconds": 0.0,
                "peak_rss_bytes": None,
                "cache_disposition": _cache_disposition(command),
                "status": "skipped",
                "returncode": None,
                "guard_metrics_schema": None,
                "skip_reason": "fail-fast dependency cancellation",
            }
    failures = [
        records_by_id[command.id]
        for command in command_list
        if command.id in records_by_id
        and records_by_id[command.id]["status"]
        not in {"success", "cancelled", "skipped"}
    ]
    if failures or failed:
        receipt["status"] = "failure"
        returncode = int(failures[0].get("returncode") or 2) if failures else 2
    else:
        receipt["status"] = "success"
        returncode = 0
    execution["completed_commands"] = len(records_by_id)
    execution["cancelled_commands"] = sum(
        record["status"] == "cancelled" for record in records_by_id.values()
    )
    execution["skipped_commands"] = sum(
        record["status"] == "skipped" for record in records_by_id.values()
    )
    refresh_receipt()
    return returncode


def _receipt_files(root: Path) -> list[Path]:
    if root.is_file():
        return [root]
    return sorted(root.rglob("*.json")) if root.is_dir() else []


def verify_receipts(
    plan: ProofPlan,
    selected_names: list[str] | tuple[str, ...],
    receipt_root: Path,
) -> list[str]:
    known_families = {family.name for family in plan.families}
    selected = set(selected_names)
    errors = [
        f"unknown selected proof family {name!r}"
        for name in sorted(selected - known_families)
    ]
    expected_commands = {
        command.id: command for command in plan.commands if command.family in selected
    }
    expected_digest = _authority_sha256(plan)
    expected_commit = _source_commit()
    policies = {policy.name: policy for policy in plan.toolchain_policies}
    observed: dict[str, dict[str, Any]] = {}
    for path in _receipt_files(receipt_root):
        try:
            payload = json.loads(path.read_text(encoding="utf-8"))
        except (OSError, json.JSONDecodeError) as exc:
            errors.append(f"{path}: unreadable receipt: {exc}")
            continue
        if payload.get("schema") != plan.receipt_schema:
            continue
        if payload.get("authority_sha256") != expected_digest:
            errors.append(
                f"{path}: receipt authority digest does not match selected plan"
            )
            continue
        if payload.get("source_commit") != expected_commit:
            errors.append(f"{path}: receipt source commit does not match checkout")
            continue
        if payload.get("source_tree_state") != "clean":
            errors.append(f"{path}: receipt source tree is not clean and commit-backed")
            continue
        environment = payload.get("environment")
        toolchains = payload.get("toolchains")
        if not isinstance(environment, dict) or not all(
            environment.get(key) for key in ("os", "arch", "python")
        ):
            errors.append(f"{path}: receipt environment is incomplete")
        if not isinstance(toolchains, dict) or not toolchains:
            errors.append(f"{path}: receipt toolchain hashes are missing")
        records = payload.get("commands")
        if not isinstance(records, list):
            errors.append(f"{path}: receipt commands must be a list")
            continue
        partitions = payload.get("executed_partitions")
        if not isinstance(partitions, list):
            errors.append(f"{path}: executed_partitions must be a list")
            partitions = []
        elif len(partitions) != len(set(str(item) for item in partitions)):
            errors.append(f"{path}: executed_partitions contains duplicates")
        for record in records:
            if not isinstance(record, dict) or not isinstance(record.get("id"), str):
                errors.append(f"{path}: malformed command receipt")
                continue
            command_id = record["id"]
            if command_id not in expected_commands:
                continue
            if command_id in observed:
                errors.append(f"{command_id}: duplicate command receipts")
                continue
            command = expected_commands[command_id]
            cell = next(
                item for item in plan.matrix_cells if item.id == command.data["cell"]
            )
            if record.get("argv") != list(command.argv):
                errors.append(f"{command_id}: receipt command does not match authority")
            if (
                payload.get("family") != command.family
                or record.get("family") != command.family
            ):
                errors.append(f"{command_id}: receipt family does not match authority")
            if record.get("cell") != command.data["cell"]:
                errors.append(
                    f"{command_id}: receipt matrix cell does not match authority"
                )
            if record.get("status") != "success" or record.get("returncode") != 0:
                errors.append(f"{command_id}: executable partition did not succeed")
            if payload.get("status") != "success":
                errors.append(f"{command_id}: enclosing receipt did not succeed")
            exact_fields = {
                "cwd": str(command.data.get("cwd", ".")),
                "dependencies": list(command.dependencies),
                "tiers": list(command.data["tiers"]),
                "resource_class": command.data["resource_class"],
                "timeout_seconds": command.data["timeout_seconds"],
                "timeout_env": list(command.data.get("timeout_env", [])),
                "environment_overrides": dict(command.data.get("env", {})),
                "guard_metrics_schema": "molt.guarded-command-metrics.v1",
            }
            for field, expected in exact_fields.items():
                if record.get(field) != expected:
                    errors.append(
                        f"{command_id}: receipt {field} does not match authority"
                    )
            if isinstance(environment, dict):
                if environment.get("os") != cell.data["os"]:
                    errors.append(
                        f"{command_id}: receipt OS does not match matrix cell"
                    )
                if environment.get("arch") != cell.data["arch"]:
                    errors.append(
                        f"{command_id}: receipt architecture does not match matrix cell"
                    )
                if (
                    cell.data["python"] != "none"
                    and environment.get("python") != cell.data["python"]
                ):
                    errors.append(
                        f"{command_id}: receipt Python does not match matrix cell"
                    )
            if command_id not in partitions:
                errors.append(
                    f"{command_id}: selected command absent from executed_partitions"
                )
            duration = record.get("duration_seconds")
            if not isinstance(duration, (int, float)) or duration < 0:
                errors.append(f"{command_id}: receipt duration_seconds is invalid")
            peak_rss = record.get("peak_rss_bytes")
            if not isinstance(peak_rss, int) or peak_rss < 0:
                errors.append(f"{command_id}: receipt peak_rss_bytes is invalid")
            if record.get("cache_disposition") not in {
                "cold",
                "warm",
                "unknown",
                "not-applicable",
            }:
                errors.append(f"{command_id}: receipt cache_disposition is invalid")
            if isinstance(toolchains, dict):
                for name in _required_toolchains(command):
                    if name not in toolchains:
                        errors.append(
                            f"{command_id}: required {name} toolchain hash is missing"
                        )
                for name, identity in toolchains.items():
                    if not isinstance(identity, dict) or not isinstance(
                        identity.get("identity_sha256"), str
                    ):
                        errors.append(f"{command_id}: malformed {name} toolchain hash")
                    elif (
                        len(identity["identity_sha256"]) != 64
                        or any(
                            character not in "0123456789abcdef"
                            for character in identity["identity_sha256"].lower()
                        )
                        or not identity.get("path")
                        or not identity.get("launcher_path")
                        or not identity.get("content_path")
                        or not identity.get("version")
                        or not isinstance(identity.get("launcher_sha256"), str)
                        or len(identity["launcher_sha256"]) != 64
                        or any(
                            character not in "0123456789abcdef"
                            for character in identity["launcher_sha256"].lower()
                        )
                        or not isinstance(identity.get("executable_sha256"), str)
                        or len(identity["executable_sha256"]) != 64
                        or any(
                            character not in "0123456789abcdef"
                            for character in identity["executable_sha256"].lower()
                        )
                        or name not in policies
                        or identity.get("version_pattern")
                        != policies[name].data["version_pattern"]
                        or identity.get("probe_cwd")
                        != policies[name].data.get("probe_cwd", ".")
                    ):
                        errors.append(
                            f"{command_id}: invalid {name} toolchain identity"
                        )
                    elif (
                        re.search(
                            str(policies[name].data["version_pattern"]),
                            str(identity["version"]),
                        )
                        is None
                    ):
                        errors.append(
                            f"{command_id}: {name} version violates authority contract"
                        )
                    elif (
                        identity["identity_sha256"]
                        != hashlib.sha256(
                            (
                                f"{identity['path']}\0{identity['launcher_path']}\0"
                                f"{identity['launcher_sha256']}\0"
                                f"{identity['content_path']}\0"
                                f"{identity['executable_sha256']}\0"
                                f"{identity['version']}\0"
                                f"{identity['probe_cwd']}"
                            ).encode()
                        ).hexdigest()
                    ):
                        errors.append(
                            f"{command_id}: {name} toolchain identity hash is invalid"
                        )
            observed[command_id] = record
    for command_id in expected_commands:
        if command_id not in observed:
            errors.append(f"{command_id}: required executable receipt is missing")
    return errors


def replay_recent_commits(plan: ProofPlan, count: int) -> dict[str, Any]:
    """Replay first-parent commit diffs and quantify avoided family launches."""
    if count <= 0:
        raise ValueError("replay commit count must be positive")
    started = time.perf_counter()
    commits = _run_git(
        ["rev-list", "--first-parent", f"--max-count={count}", "HEAD"]
    ).splitlines()
    launches = {family.name: 0 for family in plan.families}
    path_total = 0
    for commit in commits:
        paths = _diff_paths(f"{commit}^", commit)
        path_total += len(paths)
        for family in plan.select(paths).selected:
            launches[family.name] += 1
    total = len(commits)
    return {
        "schema": "molt.proof-plan-replay.v1",
        "commits": total,
        "changed_paths_examined": path_total,
        "wall_time_ms": round((time.perf_counter() - started) * 1000, 2),
        "families": {
            name: {
                "selected": selected,
                "avoided": total - selected,
                "avoidable_percent": round(
                    100.0 * (total - selected) / total if total else 0.0, 1
                ),
            }
            for name, selected in launches.items()
        },
    }


def write_github_outputs(path: Path, outputs: dict[str, str]) -> None:
    with path.open("a", encoding="utf-8") as handle:
        for name, value in outputs.items():
            print(f"{name}={value}", file=handle)


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--manifest", type=Path, default=DEFAULT_MANIFEST)
    parser.add_argument("--github-output", type=Path)
    parser.add_argument("--path", action="append", default=[])
    parser.add_argument("--check", action="store_true")
    parser.add_argument("--json", action="store_true")
    parser.add_argument("--replay-commits", type=int)
    parser.add_argument(
        "--verify-selected",
        help="JSON array of selected family names whose required results must pass.",
    )
    parser.add_argument("--receipt-dir", type=Path)
    parser.add_argument("--run-family")
    parser.add_argument("--run-command")
    parser.add_argument("--matrix-cell")
    parser.add_argument("--receipt", type=Path)
    parser.add_argument("--event-name", default=os.environ.get("GITHUB_EVENT_NAME", ""))
    parser.add_argument("--base-ref", default=os.environ.get("GITHUB_BASE_REF", ""))
    parser.add_argument("--event-path", default=os.environ.get("GITHUB_EVENT_PATH", ""))
    parser.add_argument("--before", default="")
    parser.add_argument("--after", default=os.environ.get("GITHUB_SHA", ""))
    args = parser.parse_args(argv)

    try:
        plan = ProofPlan.load(args.manifest)
    except (OSError, ValueError, tomllib.TOMLDecodeError) as exc:
        print(f"proof-plan: {exc}", file=sys.stderr)
        return 2
    if args.check:
        print(
            f"proof-plan: OK families={len(plan.families)} "
            f"local_rules={len(plan.local_rules)} authority={plan.path}"
        )
        return 0
    if args.run_family is not None or args.run_command is not None:
        if args.run_family is not None and args.run_command is not None:
            print(
                "proof-plan: choose only one of --run-family/--run-command",
                file=sys.stderr,
            )
            return 2
        if args.matrix_cell is not None and args.run_family is None:
            print(
                "proof-plan: --matrix-cell requires --run-family",
                file=sys.stderr,
            )
            return 2
        if args.receipt is None:
            print("proof-plan: executable proofs require --receipt", file=sys.stderr)
            return 2
        try:
            commands = _topological_commands(
                plan,
                family=args.run_family,
                command_id=args.run_command,
                matrix_cell=args.matrix_cell,
            )
            return execute_commands(plan, commands, args.receipt)
        except (OSError, ValueError) as exc:
            print(f"proof-plan execution: {exc}", file=sys.stderr)
            return 2
    if args.replay_commits is not None:
        try:
            replay = replay_recent_commits(plan, args.replay_commits)
        except (RuntimeError, subprocess.CalledProcessError, ValueError) as exc:
            print(f"proof-plan replay: {exc}", file=sys.stderr)
            return 2
        print(json.dumps(replay, indent=2, sort_keys=True))
        return 0
    if args.verify_selected is not None:
        try:
            selected = json.loads(args.verify_selected)
            if not isinstance(selected, list) or not all(
                isinstance(name, str) for name in selected
            ):
                raise ValueError("--verify-selected must be a JSON string array")
            if args.receipt_dir is None:
                raise ValueError("--verify-selected requires --receipt-dir")
            errors = verify_receipts(plan, selected, args.receipt_dir)
        except (ValueError, json.JSONDecodeError) as exc:
            print(f"proof-plan verdict: {exc}", file=sys.stderr)
            return 2
        if errors:
            for error in errors:
                print(f"proof-plan verdict: {error}", file=sys.stderr)
            return 1
        print(
            f"proof-plan verdict: OK selected={len(selected)} "
            f"required={sum(1 for family in plan.families if family.name in selected and family.data['required'])}"
        )
        return 0
    selection = (
        plan.select(args.path)
        if args.path
        else selection_for_event(
            plan,
            event_name=args.event_name,
            base_ref=args.base_ref,
            event_path=args.event_path,
            before=args.before,
            after=args.after,
        )
    )
    outputs = family_outputs(plan, selection)
    if args.json:
        print(json.dumps(outputs, indent=2, sort_keys=True))
    else:
        for family in plan.families:
            print(f"{family.name}={outputs[family.name]}")
        print(f"matrix={outputs['matrix']}")
        if selection.fail_closed_reason:
            print(f"proof-plan: {selection.fail_closed_reason}", file=sys.stderr)
    if args.github_output is not None:
        write_github_outputs(args.github_output, outputs)
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
