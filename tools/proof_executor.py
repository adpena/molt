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

from molt.artifact_publication import atomic_write_json


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
    _source_identity: Callable[[], dict[str, str]],
    _normalized_os: Callable[[], str],
    _normalized_arch: Callable[[], str],
    _required_toolchains: Callable[[ProofCommand], tuple[str, ...]],
    _run_command: Callable[
        [ProofPlan, ProofCommand, threading.Event | None], dict[str, Any]
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
    source_identity = _source_identity()
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
        "schema": "molt.proof-plan-dag-executor.v2",
        "max_workers": plan.executor_max_workers,
        "resource_limits": resource_limits,
        "declared_timeout_seconds": sum(
            int(command.data["timeout_seconds"]) for command in command_list
        ),
        "scheduled_commands": 0,
        "peak_active_commands": 0,
        "peak_active_by_resource": {name: 0 for name in sorted(resource_limits)},
        "global_stop_triggered": False,
        "global_stop_reasons": [],
        "source_observation_boundaries": "before-scheduling-and-after-partition",
    }
    receipt_errors: list[str] = []
    receipt: dict[str, Any] = {
        "schema": plan.receipt_schema,
        "authority_sha256": _authority_sha256(plan),
        "source_commit": source_identity["commit"],
        "source_tree": source_identity["tree"],
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
        print(
            f"proof-plan: family={receipt['family']} stage=toolchain-preflight "
            f"executed=0 error={toolchain_error}; receipt={receipt_path}",
            file=sys.stderr,
        )
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
    custody_errors: list[Exception] = []
    failed = False
    global_stop = False

    def record_error(message: str) -> None:
        receipt_errors.append(message)
        receipt["errors"] = receipt_errors

    def stop_all(message: str) -> None:
        nonlocal failed, global_stop
        failed = global_stop = True
        cancel_event.set()
        receipt["status"] = "failure"
        execution["global_stop_triggered"] = True
        execution["global_stop_reasons"].append(message)
        record_error(message)

    def source_change() -> str | None:
        try:
            if _source_tree_state() != "clean":
                return "source tree changed or is dirty"
            if _source_identity() != source_identity:
                return "candidate HEAD or tree identity changed"
        except Exception as exc:
            return f"candidate source identity unavailable: {exc}"
        return None

    def skipped_record(command: ProofCommand, reason: str) -> dict[str, Any]:
        return {
            **_base_command_record(command),
            "started_at": None,
            "duration_seconds": 0.0,
            "peak_rss_bytes": None,
            "cache_disposition": _cache_disposition(command),
            "status": "skipped",
            "returncode": None,
            "guard_metrics_schema": None,
            "skip_reason": reason,
        }

    def block_dependents(failed_id: str) -> None:
        blocked = list(dependents[failed_id])
        visited: set[str] = set()
        while blocked:
            dependent = blocked.pop()
            if dependent in visited:
                continue
            visited.add(dependent)
            blocked.extend(dependents[dependent])
            if dependent in pending_ids:
                pending_ids.remove(dependent)
                records_by_id[dependent] = skipped_record(
                    command_by_id[dependent], "required dependency failed"
                )
            record = records_by_id.get(dependent)
            if record is not None and record.get("status") == "skipped":
                causes = set(record.get("blocked_by", ())) | {failed_id}
                record["blocked_by"] = sorted(causes, key=command_index.__getitem__)

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
        try:
            atomic_write_json(receipt_path, receipt, indent=2, sort_keys=True)
        except BaseException as publication_error:
            if custody_errors:
                publication_error.guard_errors = tuple(custody_errors)
                publication_error.add_note(
                    "receipt publication failed with retained guard custody"
                )
            raise

    with ThreadPoolExecutor(
        max_workers=plan.executor_max_workers,
        thread_name_prefix="proof-plan",
    ) as executor:
        try:
            while pending_ids or active:
                if not global_stop:
                    changed = source_change()
                    if changed is not None:
                        stop_all(f"{changed} before executable scheduling wave")
                    while not global_stop and len(active) < plan.executor_max_workers:
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
                        if command_id not in pending_ids:
                            continue
                        command = command_by_id[command_id]
                        pending_ids.remove(command.id)
                        future = executor.submit(
                            _run_command, plan, command, cancel_event
                        )
                        active[future] = command
                        active_by_resource[resource] += 1
                        execution["scheduled_commands"] = (
                            int(execution["scheduled_commands"]) + 1
                        )
                        execution["peak_active_commands"] = max(
                            int(execution["peak_active_commands"]), len(active)
                        )
                        peaks: dict[str, int] = execution["peak_active_by_resource"]
                        peaks[resource] = max(
                            peaks[resource], active_by_resource[resource]
                        )

                if not active:
                    if pending_ids and not global_stop:
                        blocked = ", ".join(
                            command.id
                            for command in command_list
                            if command.id in pending_ids
                        )
                        stop_all(f"executor dependency deadlock: {blocked}")
                    break

                completed, _ = wait(tuple(active), return_when=FIRST_COMPLETED)
                for future in sorted(
                    completed, key=lambda item: command_index[active[item].id]
                ):
                    command = active[future]
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
                            "failure_scope": "global",
                            "failure_reason": "executor lost a classified command outcome",
                        }
                        if getattr(exc, "guard_command", None) is not None:
                            custody_errors.append(exc)
                            record = getattr(exc, "proof_record", record)
                    records_by_id[command.id] = record
                    active.pop(future)
                    resource = str(command.data["resource_class"])
                    active_by_resource[resource] -= 1
                    changed = source_change()
                    if changed is not None:
                        record["status"] = "failure"
                        record["returncode"] = 2
                        record["source_tree_state_after"] = "changed"
                        record["failure_scope"] = "global"
                        record["failure_reason"] = changed
                    if record["status"] == "success":
                        for dependent in dependents[command.id]:
                            if dependent not in pending_ids:
                                continue
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
                    else:
                        failed = True
                        block_dependents(command.id)
                        if (
                            record.get("failure_scope") != "partition"
                            and not global_stop
                        ):
                            stop_all(
                                f"{command.id}: {record.get('failure_reason') or 'unclassified command failure'}"
                            )
                    receipt["status"] = "failure" if failed else "running"
                    refresh_receipt()
        except BaseException as interruption:
            # Set the guard-owned cancellation signal before ThreadPoolExecutor
            # joins active workers. An operator interrupt must not wait on an
            # unrelated command's full deadline or become an ordinary failure.
            stop_all("executor interrupted by operator or control-plane exception")
            executor.shutdown(wait=True, cancel_futures=True)
            for future, command in sorted(
                active.items(), key=lambda item: command_index[item[1].id]
            ):
                try:
                    record = future.result()
                except BaseException as exc:
                    record = {
                        **_base_command_record(command),
                        "started_at": None,
                        "duration_seconds": None,
                        "peak_rss_bytes": None,
                        "cache_disposition": _cache_disposition(command),
                        "status": "cancelled" if future.cancelled() else "failure",
                        "returncode": 130,
                        "guard_metrics_schema": None,
                        "executor_error": type(exc).__name__,
                        "failure_scope": "global",
                        "failure_reason": "executor interrupted before classified outcome",
                    }
                    if getattr(exc, "guard_command", None) is not None:
                        custody_errors.append(exc)
                        record = getattr(exc, "proof_record", record)
                records_by_id[command.id] = record
            for command in command_list:
                if command.id in pending_ids:
                    records_by_id[command.id] = skipped_record(
                        command, "executor global stop"
                    )
            execution["completed_commands"] = len(records_by_id)
            execution["cancelled_commands"] = sum(
                record["status"] == "cancelled" for record in records_by_id.values()
            )
            execution["skipped_commands"] = sum(
                record["status"] == "skipped" for record in records_by_id.values()
            )
            refresh_receipt()
            if custody_errors:
                interruption.guard_errors = tuple(custody_errors)
                interruption.add_note(
                    f"unresolved guard custody; inspect {receipt_path}"
                )
            raise

    for command in command_list:
        if command.id in pending_ids:
            records_by_id[command.id] = skipped_record(command, "executor global stop")

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
    if custody_errors:
        raise ExceptionGroup(
            f"proof executor retained unresolved guard custody; inspect {receipt_path}",
            custody_errors,
        )
    return returncode
