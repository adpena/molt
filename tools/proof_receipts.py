"""Canonical proof receipt serialization, evidence custody and verification.

The plan owns selection and identity; the executor owns scheduling. This owner
receives those facts and preserves the public receipt wire protocol. Repository
roots are explicit for evidence creation; downloaded evidence has its own root.
"""

from __future__ import annotations

import hashlib
import json
from pathlib import Path
import re
import shutil
from collections.abc import Callable, Iterable, Mapping
from typing import TYPE_CHECKING, Any, Protocol

from molt.file_publication import is_link_like, resolve_owned_path
from molt.exact_json import encode_exact, read_exact, write_exact
from molt.portable_paths import portable_relative_path
from tools.git_identity import require_git_object_id

if TYPE_CHECKING:
    from tools.proof_plan import ProofCommand, ProofPlan, ToolchainPolicy

RECEIPT_SCHEMA = "molt.proof-receipt.v4"
SELECTION_SCHEMA = "molt.proof-selection.v1"
SELECTION_MAX_BYTES = 16 * 1024 * 1024


def _selection_context(
    github_context: Mapping[str, Any], *, tier: str, event_tiers: Mapping[str, str]
) -> dict[str, Any]:
    if set(github_context) != {"run_id", "run_attempt", "event_name"}:
        raise ValueError("selection requires exact GitHub run context")
    for key in ("run_id", "run_attempt"):
        if type(github_context[key]) is not int or github_context[key] <= 0:
            raise ValueError(f"selection {key} must be a positive integer")
    event = github_context["event_name"]
    if not isinstance(event, str) or event not in event_tiers:
        raise ValueError("selection requires a supported GitHub event")
    if not isinstance(tier, str) or not tier or event_tiers[event] != tier:
        raise ValueError("selection tier does not match its GitHub event")
    return dict(github_context)


def _selection_material(
    plan: ProofPlan,
    inputs: object,
    *,
    tier: str,
    github_context: Mapping[str, Any],
    full_selection_events: Iterable[str],
    family_outputs: Callable[..., Mapping[str, str]],
) -> tuple[dict[str, Any], list[str]]:
    if not isinstance(inputs, dict) or set(inputs) != {
        "mode",
        "changed_paths",
        "reason",
        "reasons",
    }:
        raise ValueError("malformed selection inputs")
    mode, paths, reason = inputs["mode"], inputs["changed_paths"], inputs["reason"]
    if not isinstance(mode, str):
        raise ValueError("selection mode must be a string")
    if (
        not isinstance(paths, list)
        or not all(isinstance(path, str) and path.strip() for path in paths)
        or len(set(paths)) != len(paths)
    ):
        raise ValueError("selection changed paths must be distinct nonempty strings")
    if mode == "changed-paths":
        if github_context["event_name"] in full_selection_events:
            raise ValueError("full-plan event cannot use a changed-path selection")
        if reason is not None:
            raise ValueError("changed-path selection cannot declare a full-plan reason")
        selection = plan.select(paths)
        if list(selection.changed_paths) != paths:
            raise ValueError("selection changed paths are not canonical")
    elif mode in {"full", "fail-closed"}:
        if paths or not isinstance(reason, str) or not reason.strip():
            raise ValueError("full selection requires a reason and no changed paths")
        if mode == "full" and (
            github_context["event_name"] not in full_selection_events
            or reason != f"{github_context['event_name']}: full proof plan"
        ):
            raise ValueError("full selection does not match its canonical event")
        selection = plan.all_selected(reason=reason, fail_closed=mode == "fail-closed")
    else:
        raise ValueError("unknown selection mode")
    material = {
        "mode": mode,
        "changed_paths": list(selection.changed_paths),
        "reason": reason,
        "reasons": {name: list(values) for name, values in selection.reasons.items()},
    }
    if inputs != material:
        raise ValueError("selection reasons differ from canonical selection")
    selected = json.loads(family_outputs(plan, selection, tier=tier)["selected"])
    return material, selected


def selection_record(
    plan: ProofPlan,
    selection: Any,
    *,
    mode: str,
    tier: str,
    source_identity: Mapping[str, str],
    authority_sha256: str,
    github_context: Mapping[str, Any],
    event_tiers: Mapping[str, str],
    full_selection_events: Iterable[str],
    family_outputs: Callable[..., Mapping[str, str]],
) -> dict[str, Any]:
    """Project a source-bound classifier result, never executable receipts."""
    context = _selection_context(github_context, tier=tier, event_tiers=event_tiers)
    if set(source_identity) != {"commit", "tree"}:
        raise ValueError("selection source identity must contain commit and tree")
    for key in ("commit", "tree"):
        require_git_object_id(source_identity[key], label=f"selection source {key}")
    if (
        not isinstance(authority_sha256, str)
        or re.fullmatch(r"[0-9a-f]{64}", authority_sha256) is None
    ):
        raise ValueError("selection authority digest is invalid")
    reason = selection.fail_closed_reason
    if mode == "full":
        reason = f"{context['event_name']}: full proof plan"
    inputs, selected = _selection_material(
        plan,
        {
            "mode": mode,
            "changed_paths": list(selection.changed_paths),
            "reason": reason,
            "reasons": {
                name: list(values) for name, values in selection.reasons.items()
            },
        },
        tier=tier,
        github_context=context,
        full_selection_events=full_selection_events,
        family_outputs=family_outputs,
    )
    actual = json.loads(family_outputs(plan, selection, tier=tier)["selected"])
    if actual != selected:
        raise ValueError("classifier selection differs from its declared inputs")
    return {
        "schema": SELECTION_SCHEMA,
        "source_commit": source_identity["commit"],
        "source_tree": source_identity["tree"],
        "source_tree_state": "clean",
        "authority_sha256": authority_sha256,
        "tier": tier,
        "github_context": context,
        "selection": inputs,
        "selected": selected,
    }


def validate_selection(
    payload: object,
    plan: ProofPlan,
    *,
    tier: str,
    source_identity: Mapping[str, str],
    authority_sha256: str,
    github_context: Mapping[str, Any],
    event_tiers: Mapping[str, str],
    full_selection_events: Iterable[str],
    family_outputs: Callable[..., Mapping[str, str]],
) -> tuple[str, ...]:
    """Reconstruct the complete expected selection before examining receipts."""
    context = _selection_context(github_context, tier=tier, event_tiers=event_tiers)
    if set(source_identity) != {"commit", "tree"}:
        raise ValueError("selection source identity must contain commit and tree")
    for key in ("commit", "tree"):
        require_git_object_id(source_identity[key], label=f"selection source {key}")
    if (
        not isinstance(authority_sha256, str)
        or re.fullmatch(r"[0-9a-f]{64}", authority_sha256) is None
    ):
        raise ValueError("selection authority digest is invalid")
    if not isinstance(payload, dict) or set(payload) != {
        "schema",
        "source_commit",
        "source_tree",
        "source_tree_state",
        "authority_sha256",
        "tier",
        "github_context",
        "selection",
        "selected",
    }:
        raise ValueError("malformed proof selection artifact")
    expected = {
        "schema": SELECTION_SCHEMA,
        "source_commit": source_identity["commit"],
        "source_tree": source_identity["tree"],
        "source_tree_state": "clean",
        "authority_sha256": authority_sha256,
        "tier": tier,
        "github_context": context,
    }
    observed_context = payload["github_context"]
    if not isinstance(observed_context, dict):
        raise ValueError("malformed artifact GitHub context")
    _selection_context(observed_context, tier=tier, event_tiers=event_tiers)
    for name, value in expected.items():
        if payload[name] != value:
            if name == "source_commit":
                raise ValueError(
                    f"selection source_commit requires checkout {payload[name]}; "
                    f"this checkout is {value}. No checkout was changed"
                )
            raise ValueError(f"selection {name} does not match requested context")
    names = payload["selected"]
    known = {family.name for family in plan.families}
    if (
        not isinstance(names, list)
        or not all(isinstance(name, str) and name in known for name in names)
        or len(set(names)) != len(names)
    ):
        raise ValueError("selection names must be distinct known proof families")
    _, selected = _selection_material(
        plan,
        payload["selection"],
        tier=tier,
        github_context=context,
        full_selection_events=full_selection_events,
        family_outputs=family_outputs,
    )
    if names != selected:
        raise ValueError(
            "selection is incomplete or differs from canonical family outputs"
        )
    return tuple(selected)


def selection_path_outside_receipts(path: Path, receipt_root: Path) -> Path:
    path = resolve_owned_path(path)
    root = resolve_owned_path(receipt_root)
    if path == root or path.is_relative_to(root):
        raise ValueError(
            "selection artifact must be outside executable receipt directory"
        )
    return path


def read_selection(path: Path, *, receipt_root: Path) -> object:
    return read_exact(
        selection_path_outside_receipts(path, receipt_root),
        max_bytes=SELECTION_MAX_BYTES,
        label="proof selection artifact",
    )


def write_selection(
    path: Path, payload: Mapping[str, Any], *, receipt_root: Path
) -> None:
    if len(encode_exact(payload)) > SELECTION_MAX_BYTES:
        raise ValueError("proof selection artifact exceeds size limit")
    write_exact(
        selection_path_outside_receipts(path, receipt_root), payload, exclusive=True
    )


class ReceiptCommand(Protocol):
    """Read-only projection of an immutable declared proof command."""

    @property
    def id(self) -> str: ...

    @property
    def family(self) -> str: ...

    @property
    def data(self) -> dict[str, Any]: ...

    @property
    def argv(self) -> tuple[str, ...]: ...

    @property
    def dependencies(self) -> tuple[str, ...]: ...

    @property
    def evidence_outputs(self) -> tuple[str, ...]: ...


def receipt_context(
    *,
    schema: str,
    authority_sha256: str,
    source_commit: object,
    source_tree: object,
    source_tree_state: str,
    environment: Mapping[str, Any],
    toolchains: Mapping[str, Any],
) -> dict[str, Any]:
    """Serialize admitted context; callers retain custody/eligibility decisions."""
    return {
        "schema": schema,
        "authority_sha256": authority_sha256,
        "source_commit": source_commit,
        "source_tree": source_tree,
        "source_tree_state": source_tree_state,
        "environment": environment,
        "toolchains": toolchains,
    }


def queued_receipt(
    context: Mapping[str, Any],
    *,
    command_id: str,
    argv: list[str],
    cwd: object,
    resource_class: object,
    started_at: object,
    duration_seconds: object,
    peak_rss_bytes: int,
    status: str,
    returncode: object,
) -> dict[str, object]:
    """Project an already validated dynamic queue terminal into its wire variant.

    Queue admission and terminal evidence remain the queue's authority. Its
    dynamic command has no static plan timeout, cell or declared evidence list.
    """
    succeeded = status == "passed" and returncode == 0
    environment = context["environment"]
    return {
        **context,
        "authority_kind": "proof-queue-dynamic-command",
        "family": "heavy_queue",
        "commands": [
            {
                "id": command_id,
                "family": "heavy_queue",
                "cell": f"{environment.get('os')}-{environment.get('arch')}-dynamic",
                "argv": argv,
                "cwd": cwd,
                "dependencies": [],
                "tiers": ["queued-heavy"],
                "resource_class": resource_class,
                "timeout_seconds": None,
                "started_at": started_at,
                "duration_seconds": duration_seconds,
                "peak_rss_bytes": peak_rss_bytes,
                "cache_disposition": "unknown",
                "status": "success" if succeeded else status,
                "returncode": returncode,
            }
        ],
        "executed_partitions": [command_id] if succeeded else [],
        "status": "success" if succeeded else status,
    }


def new_receipt(
    *,
    schema: str,
    authority_sha256: str,
    source_identity: Mapping[str, str],
    family: str,
    environment: dict[str, str],
    toolchains: dict[str, dict[str, Any]],
    execution: dict[str, Any],
    status: str,
) -> dict[str, Any]:
    return {
        **receipt_context(
            schema=schema,
            authority_sha256=authority_sha256,
            source_commit=source_identity["commit"],
            source_tree=source_identity["tree"],
            source_tree_state="clean",
            environment=environment,
            toolchains=toolchains,
        ),
        "family": family,
        "commands": [],
        "executed_partitions": [],
        "status": status,
        "execution": execution,
    }


def command_record(command: ReceiptCommand) -> dict[str, Any]:
    from tools.proof_plan import CargoTargetAccess

    effects = {}
    if "cargo_target_access" in command.data:
        # Only an explicit declaration owns this facet. In particular, CI tool
        # prerequisites do not declare the local scheduler's target effects.
        effects["cargo_target_access"] = CargoTargetAccess.resolve(
            command.data.get("toolchains", ()),
            CargoTargetAccess(command.data["cargo_target_access"]),
        ).value
    return {
        "id": command.id,
        "family": command.family,
        "cell": command.data["cell"],
        "argv": list(command.argv),
        "cwd": str(command.data.get("cwd", ".")),
        "dependencies": list(command.dependencies),
        "tiers": list(command.data["tiers"]),
        "resource_class": command.data["resource_class"],
        **effects,
        "timeout_budget": command.data.get("timeout_budget"),
        "timeout_seconds": int(command.data["timeout_seconds"]),
        "timeout_env": list(command.data.get("timeout_env", [])),
        "environment_overrides": dict(command.data.get("env", {})),
        "declared_evidence_outputs": list(command.evidence_outputs),
    }


def _evidence_path(root: Path, relative: str) -> Path:
    parsed = portable_relative_path(relative)
    if not parsed.parts:
        raise ValueError("evidence output must name a descendant of its root")
    return resolve_owned_path(root / parsed)


def _evidence_tree_paths(root: Path) -> list[Path]:
    """Inventory direct entries without traversing links or Windows junctions."""
    root = resolve_owned_path(root)
    pending = [root]
    paths: list[Path] = []
    while pending:
        directory = pending.pop()
        for child in directory.iterdir():
            if is_link_like(child):
                raise ValueError(f"evidence tree contains a link or junction: {child}")
            paths.append(child)
            if child.is_dir():
                pending.append(child)
    return sorted(paths)


def clear_evidence_outputs(root: Path, command: ReceiptCommand) -> None:
    # Validate every spelling before filesystem lookup, then admit the entire
    # cleanup set before deleting any existing evidence.
    for relative in command.evidence_outputs:
        if not portable_relative_path(relative).parts:
            raise ValueError("evidence output must name a descendant of its root")
    paths = [_evidence_path(root, relative) for relative in command.evidence_outputs]
    for path in paths:
        if path.is_dir():
            _evidence_tree_paths(path)
    for path in paths:
        if path.is_file():
            path.unlink()
        elif path.is_dir():
            shutil.rmtree(path)
        path.parent.mkdir(parents=True, exist_ok=True)


def snapshot_evidence_output(root: Path, relative: str) -> dict[str, Any]:
    path = _evidence_path(root, relative)
    return _snapshot_evidence_at(relative, path)


def _snapshot_evidence_at(relative: str, path: Path) -> dict[str, Any]:
    portable_relative_path(relative)
    path = resolve_owned_path(path)
    if path.is_file():
        size = path.stat().st_size
        if size <= 0:
            raise ValueError(f"evidence output is empty: {relative}")
        with path.open("rb") as handle:
            digest = hashlib.file_digest(handle, "sha256").hexdigest()
        return {
            "path": relative,
            "kind": "file",
            "sha256": digest,
            "total_size_bytes": size,
            "files": [{"path": Path(relative).name, "sha256": digest, "size": size}],
        }
    if not path.is_dir():
        raise ValueError(f"evidence output is missing: {relative}")
    files: list[dict[str, Any]] = []
    for candidate in _evidence_tree_paths(path):
        if not candidate.is_file():
            continue
        size = candidate.stat().st_size
        with candidate.open("rb") as handle:
            digest = hashlib.file_digest(handle, "sha256").hexdigest()
        files.append(
            {
                "path": candidate.relative_to(path).as_posix(),
                "sha256": digest,
                "size": size,
            }
        )
    if not files or sum(int(item["size"]) for item in files) <= 0:
        raise ValueError(f"evidence directory is empty: {relative}")
    manifest = json.dumps(files, separators=(",", ":"), sort_keys=True).encode()
    return {
        "path": relative,
        "kind": "directory",
        "sha256": hashlib.sha256(manifest).hexdigest(),
        "total_size_bytes": sum(int(item["size"]) for item in files),
        "files": files,
    }


def _downloaded_evidence_path(
    receipt_root: Path, relative: str, kind: str
) -> Path | None:
    declared = portable_relative_path(relative)
    direct = _evidence_path(receipt_root, relative)
    if (kind == "file" and direct.is_file()) or (
        kind == "directory" and direct.is_dir()
    ):
        return direct
    matches = [
        candidate
        for candidate in _evidence_tree_paths(receipt_root)
        if candidate.name == declared.name
        and (
            (kind == "file" and candidate.is_file())
            or (kind == "directory" and candidate.is_dir())
        )
    ]
    exact_suffix = [
        candidate
        for candidate in matches
        if candidate.parts[-len(declared.parts) :] == declared.parts
    ]
    candidates = exact_suffix or matches
    return candidates[0] if len(candidates) == 1 else None


def _receipt_files(root: Path) -> list[Path]:
    root = resolve_owned_path(root)
    if root.is_file():
        return [root]
    return (
        [path for path in _evidence_tree_paths(root) if path.suffix == ".json"]
        if root.is_dir()
        else []
    )


def _verify_receipt_evidence(
    command_id: str,
    command: ReceiptCommand,
    value: object,
    receipt_root: Path,
) -> list[str]:
    if not isinstance(value, list):
        return [f"{command_id}: receipt evidence_outputs must be a list"]
    errors: list[str] = []
    observed_paths = [
        item.get("path") if isinstance(item, dict) else None for item in value
    ]
    if observed_paths != list(command.evidence_outputs):
        return [f"{command_id}: receipt evidence outputs do not match authority"]
    try:
        for relative in observed_paths:
            parsed = portable_relative_path(relative)
            if not parsed.parts:
                raise ValueError("evidence output must name a descendant of its root")
    except ValueError as exc:
        return [f"{command_id}: invalid evidence output path: {exc}"]
    for item in value:
        if (
            not isinstance(item, dict)
            or item.get("kind") not in {"file", "directory"}
            or not isinstance(item.get("sha256"), str)
            or re.fullmatch(r"[0-9a-f]{64}", item["sha256"]) is None
            or not isinstance(item.get("total_size_bytes"), int)
            or item["total_size_bytes"] <= 0
            or not isinstance(item.get("files"), list)
            or not item["files"]
        ):
            errors.append(f"{command_id}: malformed evidence output receipt")
            continue
        try:
            materialized = _downloaded_evidence_path(
                receipt_root, str(item["path"]), str(item["kind"])
            )
            if materialized is None:
                errors.append(
                    f"{command_id}: downloaded evidence output is missing or ambiguous: "
                    f"{item['path']}"
                )
                continue
            observed_evidence = _snapshot_evidence_at(str(item["path"]), materialized)
        except (OSError, ValueError) as exc:
            errors.append(f"{command_id}: downloaded evidence output is invalid: {exc}")
        else:
            if observed_evidence != item:
                errors.append(
                    f"{command_id}: downloaded evidence bytes do not match receipt: "
                    f"{item['path']}"
                )
    return errors


def verify_receipts(
    plan: ProofPlan,
    selected_names: list[str] | tuple[str, ...],
    receipt_root: Path,
    *,
    selected_commands: Callable[..., Iterable[ProofCommand]],
    authority_sha256: Callable[[ProofPlan], str],
    source_identity: Callable[[], Mapping[str, str]],
    toolchain_fingerprint_digest: Callable[[Mapping[str, Any]], str],
    validate_python_fingerprint: Callable[[ToolchainPolicy, Mapping[str, Any]], None],
    tier: str | None = None,
) -> list[str]:
    known_families = {
        *(family.name for family in plan.families),
        *(family.name for family in plan.scheduled_families),
    }
    selected = set(selected_names)
    errors = [
        f"unknown selected proof family {name!r}"
        for name in sorted(selected - known_families)
    ]
    # Exactly what `--run-family` executes for this tier: the family's tiered
    # roots plus their dependency closure.
    expected_commands = {
        command.id: command
        for name in sorted(selected & {command.family for command in plan.commands})
        for command in selected_commands(plan, family=name, tier=tier)
    }
    expected_digest = authority_sha256(plan)
    expected_source = source_identity()
    policies = {policy.name: policy for policy in plan.toolchain_policies}
    observed: dict[str, dict[str, Any]] = {}
    try:
        receipt_files = _receipt_files(receipt_root)
    except (OSError, ValueError) as exc:
        return [*errors, f"{receipt_root}: invalid receipt custody: {exc}"]
    for path in receipt_files:
        try:
            payload = json.loads(path.read_text(encoding="utf-8"))
        except (OSError, UnicodeDecodeError, json.JSONDecodeError) as exc:
            errors.append(f"{path}: unreadable receipt: {exc}")
            continue
        if (
            not isinstance(payload, dict)
            or payload.get("schema") != plan.receipt_schema
        ):
            continue
        if payload.get("authority_sha256") != expected_digest:
            errors.append(
                f"{path}: receipt authority digest does not match selected plan"
            )
            continue
        if payload.get("source_commit") != expected_source["commit"]:
            errors.append(f"{path}: receipt source commit does not match checkout")
            continue
        if payload.get("source_tree") != expected_source["tree"]:
            errors.append(
                f"{path}: receipt source tree identity does not match checkout"
            )
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
            exact_fields = command_record(command)
            if (
                "cargo_target_access" in record
                and "cargo_target_access" not in exact_fields
            ):
                errors.append(
                    f"{command_id}: receipt cargo_target_access does not match authority"
                )
            for field in ("id", "family", "cell", "argv"):
                del exact_fields[
                    field
                ]  # Checked above with their specific diagnostics.
            exact_fields["guard_metrics_schema"] = "molt.guarded-command-metrics.v1"
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
            errors.extend(
                _verify_receipt_evidence(
                    command_id,
                    command,
                    record.get("evidence_outputs"),
                    receipt_root,
                )
            )
            if isinstance(toolchains, dict):
                for name in plan.required_toolchains(command):
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
                        or not isinstance(identity.get("version"), str)
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
                        or (
                            re.fullmatch(
                                r"[0-9a-f]{64}",
                                str(identity.get("wasi_sdk_sha256", "")),
                            )
                            is None
                            if policies[name].data.get("wasi_sdk_tool") is not None
                            else "wasi_sdk_sha256" in identity
                        )
                        or (
                            re.fullmatch(
                                r"[0-9a-f]{64}",
                                str(identity.get("llvm_sdk_sha256", "")),
                            )
                            is None
                            if policies[name].data.get("compiler_tool_selector")
                            == "llvm-sdk"
                            else "llvm_sdk_sha256" in identity
                        )
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
                    elif identity["identity_sha256"] != toolchain_fingerprint_digest(
                        identity
                    ):
                        errors.append(
                            f"{command_id}: {name} toolchain identity hash is invalid"
                        )
                    if (
                        name in policies
                        and policies[name].python_identity_role == "runtime"
                    ):
                        try:
                            validate_python_fingerprint(policies[name], identity)
                        except (KeyError, TypeError, ValueError) as exc:
                            errors.append(
                                f"{command_id}: {name} runtime fingerprint is invalid: {exc}"
                            )
            observed[command_id] = record
    for command_id in expected_commands:
        if command_id not in observed:
            errors.append(f"{command_id}: required executable receipt is missing")
    return errors
