from __future__ import annotations

from concurrent.futures import FIRST_COMPLETED, Future, ThreadPoolExecutor, wait
import datetime as dt
import heapq
from pathlib import Path
import sys
import threading
import time
from collections.abc import Callable, Iterable
from typing import Any, Protocol

from tools.artifact_publish import atomic_write_json


class ProofCommand(Protocol):
    id: str
    family: str
    data: dict[str, Any]
    dependencies: tuple[str, ...]


class ProofPlan(Protocol):
    receipt_schema: str
    executor_max_workers: int
    resource_policies: tuple[Any, ...]


def execute_commands(
    plan: ProofPlan,
    commands: Iterable[ProofCommand],
    receipt_path: Path,
    *,
    _source_tree_state: Callable[[], str],
    toolchain_fingerprints: Callable[
        [ProofPlan, tuple[str, ...]], dict[str, dict[str, Any]]
    ],
    _authority_sha256: Callable[[ProofPlan], str],
    _source_commit: Callable[[], str],
    _normalized_os: Callable[[], str],
    _normalized_arch: Callable[[], str],
    _required_toolchains: Callable[[ProofCommand], tuple[str, ...]],
    _run_command: Callable[
        [ProofPlan, ProofCommand, Path, threading.Event | None], dict[str, Any]
    ],
    _cache_disposition: Callable[[ProofCommand], str],
    _base_command_record: Callable[[ProofCommand], dict[str, Any]],
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
                        _run_command, plan, command, metrics_path, cancel_event
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
