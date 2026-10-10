"""Native process-supervisor policy, receipt, and custody authority."""

from __future__ import annotations

import hashlib
import json
import os
from pathlib import Path
import re
import secrets
import sys
import tempfile
from typing import Literal, Mapping, Sequence, TypedDict, cast

from molt import cargo_workspace
from molt.dx import PROOF_SCRATCH_ROOT_ENV
from molt.exact_json import (
    ExactJsonError,
    capture_exact,
    encode_exact,
    loads_exact,
    read_exact,
)
from molt.toolchain_identity import (
    StableRegularFileIdentity,
    open_stable_regular_file,
    stable_regular_file_handle_identity,
)
from tools.proof_queue_pkg import command_identity
from tools.proof_queue_pkg import custody_cas
from tools.proof_queue_pkg import execution_custody
from tools.proof_queue_pkg import process_image_capture


def _protocol_authority() -> dict[str, object]:
    authority = (
        Path(__file__).resolve().parents[1] / "proof_supervisor" / "protocol.json"
    )
    try:
        payload = read_exact(
            authority, max_bytes=8192, label="proof supervisor protocol authority"
        )
    except (OSError, UnicodeDecodeError, ExactJsonError, json.JSONDecodeError) as exc:
        raise RuntimeError(
            "proof supervisor protocol authority is unavailable"
        ) from exc
    schemas = {
        "policy_schema",
        "capability_schema",
        "receipt_schema",
        "event_log_schema",
    }
    budget_keys = {
        "event_record_bytes",
        "image_cache_entries",
        "fixed_image_rows",
        "file_id_utf8_bytes",
        "live_processes",
        "derived_roots",
        "environment_entries",
        "stable_process_id_utf8_bytes",
        "inventory_unique_images",
        "combined_diagnostics_json_bytes",
        "live_trace_tasks",
        "event_records",
        "role_utf8_bytes",
        "distinct_fixed_paths",
        "command_elements",
        "event_log_bytes",
        "path_utf8_bytes",
        "diagnostics_per_class",
        "lifetime_processes",
        "retained_derived_identity_bytes",
        "receipt_bytes",
        "policy_input_bytes",
        "canonical_policy_bytes",
        "one_image_roles_json_bytes",
        "nonce_utf8_bytes",
        "retained_observation_payload_bytes",
    }
    if type(payload) is not dict or set(payload) != schemas | {"budgets", "export"}:
        raise RuntimeError("proof supervisor protocol authority is malformed")
    if not all(
        type(payload[k]) is str
        and re.fullmatch(r"molt\.[a-z0-9.-]+\.v[0-9]+", payload[k])
        for k in schemas
    ):
        raise RuntimeError("proof supervisor schema authority is malformed")
    budgets = payload["budgets"]
    if (
        type(budgets) is not dict
        or set(budgets) != budget_keys
        or any(
            type(v) is not int or v <= 0 or v > sys.maxsize for v in budgets.values()
        )
    ):
        raise RuntimeError("proof supervisor budget authority is malformed")
    if (
        budgets["path_utf8_bytes"] * 6 + budgets["one_image_roles_json_bytes"] + 4096
        >= budgets["event_record_bytes"]
        or budgets["combined_diagnostics_json_bytes"] + 8192 >= budgets["receipt_bytes"]
        or budgets["retained_derived_identity_bytes"]
        > budgets["retained_observation_payload_bytes"]
        or budgets["live_processes"]
        > min(budgets["lifetime_processes"], budgets["live_trace_tasks"])
    ):
        raise RuntimeError("proof supervisor budget relationships are malformed")
    export = payload["export"]
    if (
        type(export) is not dict
        or set(export) != {"footer_magic", "length_hex_digits", "event_max_bytes"}
        or not isinstance(export["footer_magic"], str)
        or not export["footer_magic"].isascii()
        or not export["footer_magic"].startswith("\n")
        or not export["footer_magic"].endswith("\n")
        or type(export["length_hex_digits"]) is not int
        or export["length_hex_digits"] != 16
        or type(export["event_max_bytes"]) is not int
        or not 0 < export["event_max_bytes"] <= budgets["event_log_bytes"]
    ):
        raise RuntimeError("proof supervisor export authority is malformed")
    return payload


_PROTOCOL = _protocol_authority()
SUPERVISOR_BUDGETS = _PROTOCOL["budgets"]
SUPERVISOR_POLICY_SCHEMA = _PROTOCOL["policy_schema"]
SUPERVISOR_CAPABILITY_SCHEMA = _PROTOCOL["capability_schema"]
SUPERVISOR_RECEIPT_SCHEMA = _PROTOCOL["receipt_schema"]
SUPERVISOR_EVENT_LOG_SCHEMA = _PROTOCOL["event_log_schema"]
_MAX_SUPERVISOR_EVENT_LOG_BYTES = SUPERVISOR_BUDGETS["event_log_bytes"]
_MAX_SUPERVISOR_EVENT_RECORD_BYTES = SUPERVISOR_BUDGETS["event_record_bytes"]
_MAX_SUPERVISOR_EVENT_RECORDS = SUPERVISOR_BUDGETS["event_records"]
_MAX_INVENTORY_IMAGE_IDENTITIES = SUPERVISOR_BUDGETS["inventory_unique_images"]


def read_supervisor_export(
    path: Path,
    *,
    receipt_name: str = "receipt.json",
    expected: Mapping[str, object] | None = None,
) -> tuple[bytes, bytes, bytes]:
    """Retain the final exact frame; stderr before it is guest output, not proof.

    Parsing does not admit success. The caller must run the native verifier and
    require the successful terminal/coordinate/output contracts separately.
    """
    export = _PROTOCOL["export"]
    magic = export["footer_magic"].encode("ascii")
    digits = export["length_hex_digits"]
    footer_size = len(magic) + 2 * digits + 1
    with open_stable_regular_file(path, label="supervisor export") as opened:
        stream = opened.stream
        size = opened.stat.st_size
        if (
            size < footer_size
            or size
            > 16 * 1024 * 1024 + SUPERVISOR_BUDGETS["receipt_bytes"] + footer_size
        ):
            raise ValueError("supervisor export transport extent is invalid")
        if expected is not None:
            identity = stable_regular_file_handle_identity(
                opened, label="supervisor export"
            )
            if (identity.size, identity.sha256) != (
                expected.get("size"),
                expected.get("sha256"),
            ):
                raise ValueError("supervisor export differs from retained inventory")
        stream.seek(-footer_size, os.SEEK_END)
        footer = stream.read(footer_size)
        if not footer.startswith(magic) or footer[-1:] != b"\n":
            raise ValueError("supervisor export footer is missing at exact EOF")
        lengths = footer[len(magic) : -1]
        if re.fullmatch(rb"[0-9a-f]+", lengths) is None:
            raise ValueError("supervisor export lengths are not canonical")
        receipt_size, events_size = int(lengths[:digits], 16), int(lengths[digits:], 16)
        if (
            not 0 < receipt_size <= SUPERVISOR_BUDGETS["receipt_bytes"]
            or not 0 <= events_size <= export["event_max_bytes"]
        ):
            raise ValueError("supervisor export payload exceeds its protocol bound")
        prefix_size = size - footer_size - receipt_size - events_size
        if prefix_size < 0:
            raise ValueError("supervisor export payload is truncated")
        stream.seek(0)
        prefix = stream.read(prefix_size)
        receipt_bytes = stream.read(receipt_size)
        events = stream.read(events_size)
    receipt = loads_exact(receipt_bytes)
    descriptor = receipt.get("event_log") if isinstance(receipt, dict) else None
    if not isinstance(descriptor, dict):
        raise ValueError("supervisor export has no event descriptor")
    digest = hashlib.sha256(events).hexdigest()
    expected_name = f"{receipt_name}.events.{digest}.jsonl"
    if (
        descriptor.get("file") != expected_name
        or descriptor.get("sha256") != digest
        or descriptor.get("bytes") != len(events)
    ):
        raise ValueError("supervisor export event identity differs from receipt")
    return prefix, receipt_bytes, events


def decode_supervisor_export(path: Path, *, receipt_path: Path) -> bytes:
    prefix, receipt_bytes, events = read_supervisor_export(
        path, receipt_name=receipt_path.name
    )
    descriptor = loads_exact(receipt_bytes)["event_log"]
    receipt_path.parent.mkdir(parents=True, exist_ok=True)
    event_path = receipt_path.with_name(descriptor["file"])
    with event_path.open("xb") as stream:
        stream.write(events)
    with receipt_path.open("xb") as stream:
        stream.write(receipt_bytes)
    _verified_supervisor_event_artifact(
        receipt_path=receipt_path, descriptor=descriptor, collect_images=False
    )
    return prefix


def _atomic_json(path: Path, payload: Mapping[str, object]) -> None:
    custody_cas.atomic_write_bytes(
        path,
        encode_exact(payload),
    )


def encode_supervisor_policy(payload: Mapping[str, object]) -> bytes:
    """Freeze built-in policy containers, count exact compact UTF-8/LF bytes,
    then encode through the existing exact-JSON authority.
    No custom container callbacks or whole encoded string precede admission.
    """
    limit = SUPERVISOR_BUDGETS["policy_input_bytes"]
    count = 1  # final LF

    def charge(size: int) -> None:
        nonlocal count
        count += size
        if count > limit:
            raise ValueError("native policy exceeds protocol byte budget")

    def text_size(value: str) -> int:
        total = 2
        for ch in value:
            n = ord(ch)
            if 0xD800 <= n <= 0xDFFF:
                raise ValueError("native policy contains an unpaired Unicode surrogate")
            total += (
                2
                if ch in '"\\\b\f\n\r\t'
                else 6
                if n < 32
                else 1
                if n < 128
                else 2
                if n < 2048
                else 3
                if n < 65536
                else 4
            )
            if total > limit:
                raise ValueError("native policy string exceeds protocol byte budget")
        return total

    def freeze(value: object, depth: int = 0) -> object:
        if depth > 8:
            raise ValueError("native policy structure is too deep")
        if type(value) is str:
            charge(text_size(value))
            return value
        if value is None:
            charge(4)
            return None
        if type(value) is dict:
            if len(value) > SUPERVISOR_BUDGETS["environment_entries"]:
                raise ValueError("native policy object exceeds entry budget")
            charge(2 + max(0, len(value) - 1) + len(value))
            frozen: dict[str, object] = {}
            for key, item in value.items():
                if type(key) is not str:
                    raise ValueError("native policy object key is not a string")
                charge(text_size(key))
                frozen[key] = freeze(item, depth + 1)
            if len(frozen) != len(value):
                raise ValueError("native policy changed while freezing")
            return frozen
        if type(value) is list:
            if len(value) > max(
                SUPERVISOR_BUDGETS["fixed_image_rows"],
                SUPERVISOR_BUDGETS["command_elements"],
            ):
                raise ValueError("native policy sequence exceeds entry budget")
            charge(2 + max(0, len(value) - 1))
            length = len(value)
            frozen_list = [freeze(item, depth + 1) for item in value]
            if len(value) != length or len(frozen_list) != length:
                raise ValueError("native policy changed while freezing")
            return frozen_list
        raise ValueError(
            "native policy requires built-in primitive containers and strings"
        )

    frozen = freeze(payload)
    if type(frozen) is not dict or frozen.get("schema") != SUPERVISOR_POLICY_SCHEMA:
        raise ValueError("native policy schema is unsupported")

    def size_bound(value: object, budget: str) -> None:
        if (
            type(value) is not str
            or sum(
                1
                if ord(ch) < 128
                else 2
                if ord(ch) < 2048
                else 3
                if ord(ch) < 65536
                else 4
                for ch in value
            )
            > SUPERVISOR_BUDGETS[budget]
        ):
            raise ValueError(f"native policy exceeds {budget}")

    size_bound(frozen.get("nonce"), "nonce_utf8_bytes")
    size_bound(frozen.get("cwd"), "path_utf8_bytes")
    size_bound(frozen.get("root_role"), "role_utf8_bytes")
    for field, budget in (
        ("command", "command_elements"),
        ("environment", "environment_entries"),
        ("fixed_images", "fixed_image_rows"),
        ("derived_roots", "derived_roots"),
    ):
        value = frozen.get(field, {} if field == "environment" else [])
        if (
            type(value) is not (dict if field == "environment" else list)
            or len(value) > SUPERVISOR_BUDGETS[budget]
        ):
            raise ValueError(f"native policy exceeds {budget}")
    roles_by_path: dict[str, set[str]] = {}
    for field in ("fixed_images", "derived_roots"):
        for row in frozen.get(field, []):
            if type(row) is not dict:
                raise ValueError("native policy image row is malformed")
            size_bound(row.get("path"), "path_utf8_bytes")
            size_bound(row.get("role"), "role_utf8_bytes")
            if field == "fixed_images":
                roles_by_path.setdefault(row["path"], set()).add(row["role"])
    if len(roles_by_path) > SUPERVISOR_BUDGETS["distinct_fixed_paths"]:
        raise ValueError("native policy exceeds distinct fixed path budget")
    for roles in roles_by_path.values():
        if (
            2 + max(0, len(roles) - 1) + sum(text_size(role) for role in roles)
            > SUPERVISOR_BUDGETS["one_image_roles_json_bytes"]
        ):
            raise ValueError("native policy exceeds one-image role budget")
    encoded = encode_exact(frozen, indent=None)
    if len(encoded) != count:
        raise ValueError("native policy encoded size disagrees with preflight")
    return encoded


def publish_supervisor_policy(path: Path, payload: Mapping[str, object]) -> None:
    custody_cas.atomic_write_bytes(path, encode_supervisor_policy(payload))


def source_authority_paths(repo_root: Path) -> tuple[Path, ...]:
    """Bind native supervisor sources and their declared local dependencies.

    Workspace inheritance is a manifest input, not permission to include the
    whole runtime workspace. Cargo still owns actual dependency resolution.
    """
    source = repo_root / "tools" / "proof_supervisor"
    facts = cargo_workspace.workspace_manifest_facts(source)
    crate_roots = {source.resolve()}
    crate_roots.update(edge.dependency_manifest.parent for edge in facts.dependencies)
    paths = {
        *facts.input_manifests,
        source / "build.py",
        source / "Cargo.lock",
        source / "protocol.json",
        Path(cargo_workspace.__file__),
        Path(__file__),
        Path(sys.modules[encode_exact.__module__].__file__),
        Path(__file__).with_name("supervisor_generation.py"),
        Path(__file__).with_name("cargo_output_layout.py"),
        Path(__file__).with_name("guarded_execution.py"),
        Path(__file__).with_name("command_identity.py"),
        Path(__file__).with_name("toolchain_capture.py"),
    }
    for crate_root in crate_roots:
        paths.update((crate_root / "src").rglob("*"))
        build_script = crate_root / "build.rs"
        if build_script.is_file():
            paths.add(build_script)
    return tuple(
        sorted({path.resolve(strict=True) for path in paths if not path.is_dir()})
    )


def _verified_supervisor_event_artifact(
    *,
    receipt_path: Path,
    descriptor: Mapping[str, object],
    collect_images: bool,
) -> tuple[Path, list[tuple[str, str, int]]]:
    if descriptor.get("schema") != SUPERVISOR_EVENT_LOG_SCHEMA:
        raise ValueError(
            "native proof supervisor event artifact descriptor has unsupported schema"
        )
    file_name = descriptor.get("file")
    expected_sha256 = descriptor.get("sha256")
    expected_bytes = descriptor.get("bytes")
    expected_count = descriptor.get("count")
    if (
        not isinstance(file_name, str)
        or Path(file_name).name != file_name
        or not isinstance(expected_sha256, str)
        or re.fullmatch(r"[0-9a-f]{64}", expected_sha256) is None
        or not isinstance(expected_bytes, int)
        or isinstance(expected_bytes, bool)
        or not 0 <= expected_bytes <= _MAX_SUPERVISOR_EVENT_LOG_BYTES
        or not isinstance(expected_count, int)
        or isinstance(expected_count, bool)
        or not 0 <= expected_count <= _MAX_SUPERVISOR_EVENT_RECORDS
        or file_name != f"{receipt_path.name}.events.{expected_sha256}.jsonl"
    ):
        raise ValueError(
            "native proof supervisor event artifact descriptor is malformed"
        )
    event_path = receipt_path.with_name(file_name).resolve(strict=True)
    if event_path.parent != receipt_path.parent.resolve(strict=True):
        raise ValueError(
            "native proof supervisor event artifact escaped its receipt directory"
        )

    digest = hashlib.sha256()
    count = 0
    actual_bytes = 0
    unique_images: dict[tuple[str, str, int], None] = {}
    with event_path.open("rb") as stream:
        if os.fstat(stream.fileno()).st_size != expected_bytes:
            raise ValueError("native proof supervisor event artifact identity changed")
        while raw_line := stream.readline(_MAX_SUPERVISOR_EVENT_RECORD_BYTES + 1):
            if len(
                raw_line
            ) > _MAX_SUPERVISOR_EVENT_RECORD_BYTES or not raw_line.endswith(b"\n"):
                raise ValueError(
                    "native proof supervisor event artifact has an invalid record bound"
                )
            actual_bytes += len(raw_line)
            if actual_bytes > expected_bytes:
                raise ValueError(
                    "native proof supervisor event artifact identity changed"
                )
            digest.update(raw_line)
            count += 1
            if count > _MAX_SUPERVISOR_EVENT_RECORDS:
                raise ValueError(
                    "native proof supervisor event artifact exceeds its record bound"
                )
            if not collect_images:
                continue
            try:
                event = loads_exact(raw_line.decode("utf-8"))
            except (UnicodeDecodeError, ExactJsonError, json.JSONDecodeError) as exc:
                raise ValueError(
                    "native proof supervisor event artifact is malformed"
                ) from exc
            payload = event.get("event") if isinstance(event, Mapping) else None
            image = payload.get("image") if isinstance(payload, Mapping) else None
            if image is None:
                continue
            if not isinstance(image, Mapping):
                raise ValueError("native proof supervisor event image is malformed")
            raw_path = image.get("path")
            image_sha256 = image.get("sha256")
            image_size = image.get("size_bytes")
            if (
                not isinstance(raw_path, str)
                or not Path(raw_path).is_absolute()
                or not isinstance(image_size, int)
                or isinstance(image_size, bool)
                or image_size < 0
                or not isinstance(image_sha256, str)
                or re.fullmatch(r"[0-9a-f]{64}", image_sha256) is None
            ):
                raise ValueError(
                    "native proof supervisor event image identity is malformed"
                )
            unique_images[(raw_path, image_sha256, image_size)] = None
            if len(unique_images) > _MAX_INVENTORY_IMAGE_IDENTITIES:
                raise ValueError(
                    "native proof supervisor inventory exceeds its unique-image bound"
                )
    if (
        actual_bytes != expected_bytes
        or count != expected_count
        or digest.hexdigest() != expected_sha256
    ):
        raise ValueError("native proof supervisor event artifact identity changed")
    return event_path, list(unique_images)


class SupervisorPrelaunchRefused(ValueError):
    def __init__(self, capability: dict[str, object]) -> None:
        self.capability = dict(capability)
        admission = capability["admission"]
        assert isinstance(admission, dict)
        super().__init__(
            "native supervisor prelaunch refused: " + str(admission["reason"])
        )


def decode_supervisor_capability(
    capability: object,
    *,
    mode: str,
    context: Literal["prelaunch", "verified_terminal"],
    expected_platform: str,
) -> dict[str, str]:
    """Decode one strict state contract; terminal decoding is not success admission.

    Native replay owns the witness. Prelaunch can only refuse or permit an
    attempt; a verified terminal may honestly remain eligible after launch
    failure. Callers separately require a complete admitted receipt for success.
    """
    if context not in {"prelaunch", "verified_terminal"}:
        raise ValueError("native supervisor capability context is unsupported")
    if (
        not isinstance(capability, dict)
        or set(capability)
        != {
            "schema",
            "platform",
            "mode",
            "backend",
            "admission",
            "pre_entry_exec_authority",
            "pre_entry_process_create_authority",
            "recursive_descendant_authority",
            "required_environment",
        }
        or capability.get("schema") != SUPERVISOR_CAPABILITY_SCHEMA
        or mode not in {"leaf", "declared-tree", "inventory-tree"}
        or capability.get("mode") != mode
        or expected_platform not in {"linux", "macos", "windows"}
        or capability.get("platform") != expected_platform
        or not isinstance(capability.get("backend"), str)
        or not capability["backend"]
        or any(
            type(capability.get(name)) is not bool
            for name in (
                "pre_entry_exec_authority",
                "pre_entry_process_create_authority",
                "recursive_descendant_authority",
            )
        )
    ):
        raise ValueError("native supervisor capability schema or mode mismatch")
    admission = capability["admission"]
    if not isinstance(admission, dict):
        raise ValueError("native supervisor admission state is malformed")
    state = admission.get("state")
    if state == "ineligible":
        reason = admission.get("reason")
        valid = (
            set(admission) == {"state", "reason"}
            and isinstance(reason, str)
            and bool(reason.strip())
            and len(reason.encode("utf-8"))
            <= SUPERVISOR_BUDGETS["combined_diagnostics_json_bytes"]
            // (2 * SUPERVISOR_BUDGETS["diagnostics_per_class"])
        )
    elif state == "eligible":
        valid = set(admission) == {"state"}
    elif state == "admitted":
        root = admission.get("root_stable_process_id")
        created = admission.get("root_create_sequence")
        image = admission.get("initial_image_sequence")
        valid = (
            set(admission)
            == {
                "state",
                "root_stable_process_id",
                "root_create_sequence",
                "initial_image_sequence",
            }
            and isinstance(root, str)
            and 0
            < len(root.encode("utf-8"))
            <= SUPERVISOR_BUDGETS["stable_process_id_utf8_bytes"]
            and type(created) is int
            and type(image) is int
            and 0 < created < image <= _MAX_SUPERVISOR_EVENT_RECORDS
        )
    else:
        valid = False
    if not valid or context == "prelaunch" and state == "admitted":
        raise ValueError("native supervisor admission state or context is malformed")
    if state != "ineligible" and (
        capability["pre_entry_exec_authority"] is not True
        or mode == "leaf"
        and capability["pre_entry_process_create_authority"] is not True
        or mode != "leaf"
        and capability["recursive_descendant_authority"] is not True
    ):
        raise ValueError("native supervisor planned backend lacks process custody")
    required = capability.get("required_environment")
    if not isinstance(required, dict):
        raise ValueError("native supervisor required environment is malformed")
    selected: dict[str, str] = {}
    for name, value in required.items():
        if (
            not isinstance(name, str)
            or re.fullmatch(r"[A-Z_][A-Z0-9_]*", name) is None
            or not isinstance(value, str)
            or not value
            or any(character in value for character in ("\x00", "\r", "\n"))
        ):
            raise ValueError("native supervisor required environment is malformed")
        selected[name] = value
    if context == "prelaunch" and state == "ineligible":
        raise SupervisorPrelaunchRefused(capability)
    return dict(sorted(selected.items()))


def supervisor_receipt_is_complete(receipt: Mapping[str, object]) -> bool:
    """Success predicate after native integrity/replay validation, not a verifier."""
    capability = receipt.get("capability")
    admission = capability.get("admission") if isinstance(capability, dict) else None
    return (
        receipt.get("complete") is True
        and receipt.get("state") == "COMPLETE"
        and isinstance(admission, dict)
        and admission.get("state") == "admitted"
    )


def required_execution_environment(
    *, binary: Path, mode: str, cwd: Path, env: Mapping[str, str]
) -> dict[str, str]:
    """Read launch requirements from the exact native supervisor authority."""
    completed = command_identity._run_captured(
        (str(binary), "capability", mode), cwd=cwd, env=env, timeout=30.0
    )
    if completed.returncode != 0:
        raise ValueError(
            "native supervisor launch capability failed: "
            + (completed.stderr.strip() or completed.stdout.strip())
        )
    try:
        capability = loads_exact(completed.stdout)
    except (ExactJsonError, json.JSONDecodeError) as exc:
        raise ValueError("native supervisor launch capability is not JSON") from exc
    return decode_supervisor_capability(
        capability,
        mode=mode,
        context="prelaunch",
        expected_platform={"win32": "windows", "darwin": "macos"}.get(
            sys.platform, sys.platform
        ),
    )


def bind_required_environment(
    env: Mapping[str, str], required: Mapping[str, str]
) -> dict[str, str]:
    """Bind native-owned settings before capture, rejecting conflicting inputs."""
    required_names = {name.casefold(): name for name in required}
    selected: dict[str, str] = {}
    seen: set[str] = set()
    for name, value in env.items():
        folded = name.casefold()
        if folded in seen:
            raise ValueError(f"execution environment has case-ambiguous name {name!r}")
        seen.add(folded)
        canonical = required_names.get(folded)
        if canonical is None:
            selected[name] = value
        elif value != required[canonical]:
            raise ValueError(
                f"native supervisor requires canonical environment {canonical!r}; "
                "the supplied value conflicts with its launch contract"
            )
    selected.update(required)
    return selected


def validate_required_environment_binding(
    *,
    required_environment: Mapping[str, str],
    supervisor_metadata: object,
    policy_environment: Mapping[str, object],
    supervisor_owned_names: object,
) -> None:
    """Cross-bind native launch requirements to receipt and policy metadata."""
    required = dict(required_environment)
    if not isinstance(supervisor_metadata, dict) or supervisor_metadata != required:
        raise ValueError(
            "native process supervisor required-environment metadata mismatch"
        )
    required_names = {name.casefold() for name in required}
    policy_required = {
        str(name): value
        for name, value in policy_environment.items()
        if str(name).casefold() in required_names
    }
    if policy_required != required:
        raise ValueError(
            "native process supervisor required environment differs from policy"
        )
    if supervisor_owned_names != sorted(required, key=str.casefold):
        raise ValueError(
            "native process supervisor environment ownership metadata mismatch"
        )


def _supervisor_fixed_images(
    toolchains: Mapping[str, object],
    environment_executables: Mapping[str, object],
    execution_command: Sequence[str],
    platform_process_images: Sequence[Mapping[str, object]] = (),
) -> tuple[str, list[dict[str, str]]]:
    root = process_image_capture._image_path_key(Path(execution_command[0]))
    identities: dict[str, tuple[str, str]] = {}
    images: dict[tuple[str, str], dict[str, str]] = {}

    def add(
        role: str,
        raw_path: object,
        raw_digest: object,
        raw_root_exit_disposition: object = None,
    ) -> None:
        if not isinstance(raw_path, str) or not isinstance(raw_digest, str):
            return
        if re.fullmatch(r"[0-9a-f]{64}", raw_digest) is None:
            return
        path = Path(raw_path)
        if not path.is_absolute() or not path.is_file():
            return
        key = process_image_capture._image_path_key(path)
        disposition = (
            str(raw_root_exit_disposition)
            if raw_root_exit_disposition is not None
            else "require-exit"
        )
        if disposition not in {"require-exit", "terminate"}:
            raise ValueError(
                f"supervisor image has invalid root-exit disposition: {path}"
            )
        row = {"role": role, "path": str(path), "sha256": raw_digest}
        if disposition != "require-exit":
            row["root_exit_disposition"] = disposition
        identity = (raw_digest, disposition)
        prior_identity = identities.get(key)
        if prior_identity is not None and prior_identity[0] != raw_digest:
            raise ValueError(f"supervisor image has conflicting identities: {path}")
        if prior_identity is not None and prior_identity[1] != disposition:
            raise ValueError(
                f"supervisor image has conflicting root-exit dispositions: {path}"
            )
        identities[key] = identity
        images[(key, role)] = row

    root_path = process_image_capture.custody_path(Path(execution_command[0]))
    if not root_path.is_file():
        raise ValueError("supervisor root executable is unavailable")
    add("root-command", str(root_path), command_identity._hash_file(root_path))
    for name, raw in toolchains.items():
        if not isinstance(raw, Mapping):
            continue
        for image in process_image_capture.toolchain_images(str(name), raw):
            add(
                str(image["role"]),
                image["path"],
                image["sha256"],
                image.get("root_exit_disposition"),
            )
    for image in process_image_capture.environment_images(environment_executables):
        add(str(image["role"]), image["path"], image["sha256"])
    for image in platform_process_images:
        add(
            str(image.get("role") or "platform-process"),
            image.get("path"),
            image.get("sha256"),
            image.get("root_exit_disposition"),
        )
    if root not in identities:
        raise ValueError("supervisor policy has no captured root executable image")
    return "root-command", [images[key] for key in sorted(images)]


BUILD_OUTPUT_ROLE = "build-output"
SCRATCH_OUTPUT_ROLE = "scratch-output"


def _supervisor_derived_roots(
    *, descendants: object, env: Mapping[str, str]
) -> list[dict[str, str]]:
    if descendants == "forbidden":
        return []
    roots: list[dict[str, str]] = []
    for role, name in (
        (BUILD_OUTPUT_ROLE, "CARGO_TARGET_DIR"),
        (SCRATCH_OUTPUT_ROLE, PROOF_SCRATCH_ROOT_ENV),
    ):
        raw = env.get(name)
        if not raw:
            continue
        path = Path(raw)
        if not path.is_absolute() or not path.is_dir():
            raise ValueError(
                f"declared-tree supervisor requires existing absolute {name}"
            )
        roots.append({"role": role, "path": str(path.resolve(strict=True))})
    return roots


def _derived_root_provenance(
    *,
    descendants: object,
    env: Mapping[str, str],
    source_root: Path,
    result_path: Path,
    cargo_cache: Mapping[str, object] | None = None,
) -> list[dict[str, object]]:
    roots = _supervisor_derived_roots(descendants=descendants, env=env)
    source = source_root.resolve(strict=True)
    cas_root = Path(os.path.abspath(result_path.parent / "custody-cas"))
    admitted: list[dict[str, object]] = []
    for row in roots:
        path = Path(row["path"])
        lexical = Path(os.path.abspath(path))
        resolved = lexical.resolve(strict=True)
        if os.path.normcase(str(lexical)) != os.path.normcase(str(resolved)):
            raise ValueError("derived executable root may not traverse a symlink")
        if (
            resolved == source
            or resolved.is_relative_to(source)
            or source.is_relative_to(resolved)
        ):
            raise ValueError("derived executable root overlaps admitted source")
        if (
            resolved == cas_root
            or resolved.is_relative_to(cas_root)
            or cas_root.is_relative_to(resolved)
        ):
            raise ValueError("derived executable root overlaps proof custody CAS")
        if resolved == Path(os.path.abspath(result_path)):
            raise ValueError("derived executable root overlaps terminal result")
        if cargo_cache is not None and row["role"] == "build-output":
            if (
                cargo_cache.get("path") != str(resolved)
                or cargo_cache.get("run_owned") is not True
            ):
                raise ValueError(
                    "derived Cargo root differs from exclusive cache custody"
                )
            admitted.append(dict(cargo_cache))
            continue
        entries = list(resolved.iterdir())
        if entries:
            raise ValueError(
                f"derived executable root is not fresh and empty: {resolved}"
            )
        admitted.append(
            {
                **row,
                "initial_entry_count": 0,
                "initial_manifest_sha256": _canonical_payload_sha256([]),
                "run_owned": True,
            }
        )
    return admitted


def _supervisor_policy(
    *,
    envelope: Mapping[str, object],
    execution_command: Sequence[str],
    execution_env: Mapping[str, str],
    cwd: Path,
    nonce: str,
    toolchains: Mapping[str, object],
    environment_executables: Mapping[str, object],
    platform_process_images: Sequence[Mapping[str, object]],
) -> dict[str, object]:
    closure = envelope.get("process_closure")
    if not isinstance(closure, Mapping):
        raise ValueError("proof envelope has no supervisor closure authority")
    descendants = closure.get("descendants")
    mode = "leaf" if descendants == "forbidden" else "declared-tree"
    root_role, fixed_images = _supervisor_fixed_images(
        toolchains,
        environment_executables,
        execution_command,
        platform_process_images,
    )
    return {
        "schema": SUPERVISOR_POLICY_SCHEMA,
        "nonce": nonce,
        "mode": mode,
        "cwd": str(cwd.resolve(strict=True)),
        "command": [str(value) for value in execution_command],
        "environment": dict(
            sorted(execution_env.items(), key=lambda item: item[0].casefold())
        ),
        "root_role": root_role,
        "fixed_images": fixed_images,
        "derived_roots": _supervisor_derived_roots(
            descendants=descendants,
            env=execution_env,
        ),
    }


def validate_supervisor_receipt_verification(
    response: object,
    *,
    receipt_identity: StableRegularFileIdentity,
    policy_identity: StableRegularFileIdentity,
) -> dict[str, object]:
    """Bind native replay to the exact JSON generation retained by its consumer."""
    if not isinstance(response, str):
        raise ValueError(
            "native process supervisor verification response is not UTF-8 text"
        )
    try:
        payload = loads_exact(response)
    except (ExactJsonError, json.JSONDecodeError) as exc:
        raise ValueError(
            "native process supervisor verification response is not exact JSON"
        ) from exc
    if not isinstance(payload, dict):
        raise ValueError(
            "native process supervisor verification response is not an object"
        )
    digest = payload.get("receipt_sha256")
    size = payload.get("receipt_bytes")
    if (
        not isinstance(digest, str)
        or re.fullmatch(r"[0-9a-f]{64}", digest) is None
        or isinstance(size, bool)
        or not isinstance(size, int)
        or size < 0
    ):
        raise ValueError(
            "native process supervisor verification has no receipt byte identity"
        )
    if digest != receipt_identity.sha256 or size != receipt_identity.size:
        raise ValueError("native process supervisor verified different receipt bytes")
    if (
        payload.get("policy_input_sha256") != policy_identity.sha256
        or type(payload.get("policy_input_bytes")) is not int
        or payload["policy_input_bytes"] != policy_identity.size
    ):
        raise ValueError("native process supervisor verified different policy bytes")
    if (
        payload.get("native_custody_valid") is not True
        or payload.get("journal_coverage_valid") is not True
    ):
        raise ValueError(
            "native process supervisor custody or coverage evidence is invalid"
        )
    return payload


def _validated_supervisor_receipt(
    *,
    binary: Path,
    policy_path: Path,
    receipt_path: Path,
    cwd: Path,
    env: Mapping[str, str],
    rootfs: Path | None = None,
) -> dict[str, object]:
    try:
        receipt_identity, receipt = capture_exact(
            receipt_path,
            max_bytes=SUPERVISOR_BUDGETS["receipt_bytes"],
            label="native proof supervisor receipt",
        )
    except (OSError, UnicodeDecodeError, ExactJsonError, json.JSONDecodeError) as exc:
        raise ValueError(
            "native proof supervisor returned no readable receipt"
        ) from exc
    policy_identity, _policy = capture_exact(
        policy_path,
        max_bytes=SUPERVISOR_BUDGETS["policy_input_bytes"],
        label="native proof supervisor policy",
    )
    verified = command_identity._run_captured(
        (
            str(binary),
            "verify-rooted" if rootfs is not None else "verify",
            *(("--rootfs", str(rootfs)) if rootfs is not None else ()),
            "--policy",
            str(policy_path),
            "--receipt",
            str(receipt_path),
        ),
        cwd=cwd,
        env=env,
        text=False,
    )
    if verified.returncode != 0:
        raise ValueError(
            "native proof supervisor receipt verification failed: "
            + (verified.stderr.strip() or verified.stdout.strip()).decode(
                "utf-8", errors="replace"
            )
        )
    validate_supervisor_receipt_verification(
        verified.stdout.decode("utf-8"),
        receipt_identity=receipt_identity,
        policy_identity=policy_identity,
    )
    if (
        not isinstance(receipt, dict)
        or receipt.get("schema") != SUPERVISOR_RECEIPT_SCHEMA
    ):
        raise ValueError("native proof supervisor receipt schema is unsupported")
    capability = receipt.get("capability")
    mode = capability.get("mode") if isinstance(capability, dict) else None
    if not isinstance(mode, str):
        raise ValueError("native proof supervisor receipt has no closure mode")
    decode_supervisor_capability(
        capability,
        mode=mode,
        context="verified_terminal",
        expected_platform="linux"
        if rootfs is not None
        else {"win32": "windows", "darwin": "macos"}.get(sys.platform, sys.platform),
    )
    return receipt


def capture_process_image_inventory(
    *,
    binary: Path,
    role: str,
    executable: Path,
    probe_args: Sequence[str],
    cwd: Path,
    env: Mapping[str, str],
) -> tuple[list[dict[str, object]], dict[str, object]]:
    """Observe one bounded toolchain probe without granting proof authority."""

    if not probe_args or not all(
        isinstance(value, str) and value for value in probe_args
    ):
        raise ValueError("process-image probe arguments must be non-empty strings")
    launcher = process_image_capture.capture_image(
        f"{role}-launcher", executable, preserve_path=True
    )
    required_environment = required_execution_environment(
        binary=binary, mode="inventory-tree", cwd=cwd, env=env
    )
    env = bind_required_environment(env, required_environment)
    command = [str(launcher["path"]), *probe_args]
    with tempfile.TemporaryDirectory(prefix="molt-process-image-inventory-") as raw:
        root = Path(raw).resolve()
        policy_path = root / "policy.json"
        receipt_path = root / "receipt.json"
        policy = {
            "schema": SUPERVISOR_POLICY_SCHEMA,
            "nonce": secrets.token_hex(32),
            "mode": "inventory-tree",
            "cwd": str(cwd.resolve(strict=True)),
            "command": command,
            "environment": dict(
                sorted(env.items(), key=lambda item: item[0].casefold())
            ),
            "root_role": launcher["role"],
            "fixed_images": [
                {
                    "role": launcher["role"],
                    "path": launcher["path"],
                    "sha256": launcher["sha256"],
                }
            ],
            "derived_roots": [],
        }
        publish_supervisor_policy(policy_path, policy)
        completed = command_identity._run_captured(
            (
                str(binary),
                "inventory",
                "--policy",
                str(policy_path),
                "--receipt",
                str(receipt_path),
            ),
            cwd=cwd,
            env=env,
            timeout=30.0,
        )
        if completed.returncode != 0:
            detail = completed.stderr.strip() or completed.stdout.strip()
            if receipt_path.is_file():
                try:
                    failed_receipt = read_exact(
                        receipt_path,
                        max_bytes=SUPERVISOR_BUDGETS["receipt_bytes"],
                        label="failed native proof supervisor receipt",
                    )
                except (
                    OSError,
                    UnicodeDecodeError,
                    ExactJsonError,
                    json.JSONDecodeError,
                ):
                    failed_receipt = None
                if isinstance(failed_receipt, Mapping):
                    diagnostics = [
                        str(value)
                        for field in ("errors", "violations")
                        for value in failed_receipt.get(field, [])
                    ]
                    if diagnostics:
                        detail = "; ".join(diagnostics)
            raise ValueError(
                f"{role} process-image inventory failed: {detail or completed.returncode}"
            )
        receipt = _validated_supervisor_receipt(
            binary=binary,
            policy_path=policy_path,
            receipt_path=receipt_path,
            cwd=cwd,
            env=env,
        )
        if (
            not supervisor_receipt_is_complete(receipt)
            or receipt.get("root_exit_code") != 0
            or receipt.get("errors") != []
            or receipt.get("violations") != []
        ):
            raise ValueError(f"{role} process-image inventory is incomplete")
        descriptor = receipt.get("event_log")
        if not isinstance(descriptor, Mapping):
            raise ValueError(f"{role} process-image inventory has no event log")
        _, observed_images = _verified_supervisor_event_artifact(
            receipt_path=receipt_path, descriptor=descriptor, collect_images=True
        )
        rows: list[dict[str, object]] = []
        launcher_path = Path(str(launcher["path"]))
        for raw_path, digest, size in observed_images:
            observed = Path(raw_path)
            try:
                is_launcher = observed.samefile(launcher_path)
            except OSError as exc:
                raise ValueError(
                    f"{role} process-image inventory image is unavailable: {observed}"
                ) from exc
            captured = process_image_capture.capture_image(
                f"{role}-launcher" if is_launcher else f"{role}-runtime",
                observed,
            )
            if captured["sha256"] != digest or captured["size_bytes"] != size:
                raise ValueError(
                    f"{role} process-image inventory changed before capture: {observed}"
                )
            rows.append(captured)
        images = process_image_capture.canonical_images(rows)
        if not any(
            Path(str(image["path"])).samefile(launcher_path) for image in images
        ):
            raise ValueError(f"{role} process-image inventory omitted its launcher")
        telemetry = {
            "required_environment": required_environment,
            "schema": "molt.proof-process-image-inventory.v1",
            "probe_argv_sha256": _canonical_payload_sha256(command),
            "stdout_sha256": hashlib.sha256(completed.stdout.encode()).hexdigest(),
            "stderr_sha256": hashlib.sha256(completed.stderr.encode()).hexdigest(),
            "observed_image_count": len(images),
            "receipt_identity_sha256": receipt.get("identity_sha256"),
        }
        return images, telemetry


def _publish_supervisor_event_artifact(
    *, receipt_path: Path, receipt: Mapping[str, object], cas_root: Path
) -> dict[str, object]:
    event_log = receipt.get("event_log")
    if not isinstance(event_log, Mapping):
        raise ValueError("native proof supervisor receipt has no event artifact")
    event_path, _ = _verified_supervisor_event_artifact(
        receipt_path=receipt_path, descriptor=event_log, collect_images=False
    )
    file_name = str(event_log["file"])
    expected_sha256 = event_log["sha256"]
    expected_bytes = event_log["bytes"]
    expected_count = event_log["count"]
    artifact = custody_cas.put_file(
        cas_root, event_path, logical_name=file_name, executable=False
    ).as_dict()
    if (
        artifact.get("sha256") != expected_sha256
        or artifact.get("size_bytes") != expected_bytes
    ):
        raise ValueError("durable supervisor event artifact identity mismatch")
    return {
        "schema": "molt.proof-process-event-artifact.v1",
        "artifact": artifact,
        "count": expected_count,
        "bytes": expected_bytes,
        "sha256": expected_sha256,
    }


def _canonical_payload_sha256(payload: object) -> str:
    return hashlib.sha256(
        json.dumps(payload, sort_keys=True, separators=(",", ":")).encode()
    ).hexdigest()


def _publish_live_custody_receipt(
    receipt: Mapping[str, object], *, cas_root: Path
) -> dict[str, object]:
    events = receipt.get("events")
    apparatus_events = receipt.get("apparatus_events")
    errors = receipt.get("errors")
    lifecycle = receipt.get("lifecycle")
    state = receipt.get("state")
    if (
        not isinstance(events, list)
        or not isinstance(apparatus_events, list)
        or not isinstance(errors, list)
        or not isinstance(lifecycle, list)
    ):
        raise ValueError("live custody receipt event authority is malformed")
    artifact_payload = {
        "schema": custody_cas.ARTIFACT_SCHEMA,
        "kind": "live-input-custody-events",
        "events": events,
        "apparatus_events": apparatus_events,
        "errors": errors,
    }
    artifact = custody_cas.put_json(cas_root, artifact_payload).as_dict()
    if receipt.get("identity_sha256") != execution_custody.live_custody_identity_sha256(
        events=events,
        apparatus_events=apparatus_events,
        errors=errors,
        state=state,
        lifecycle=lifecycle,
    ):
        raise ValueError("live custody receipt identity is inconsistent")
    return {
        **{
            key: value
            for key, value in receipt.items()
            if key not in {"events", "apparatus_events", "errors"}
        },
        "event_artifact": artifact,
        "event_count": len(events),
        "error_count": len(errors),
    }


def execution_custody_sha256(
    context: Mapping[str, object], *, run_id: str, returncode: int
) -> str:
    custody_context = dict(context)
    for field in (
        "execution_custody_sha256",
        "guard_receipt",
        "cargo_generation_lifecycle",
        "terminal_evidence_sha256",
        "queue_terminal",
    ):
        custody_context.pop(field, None)
    material = {
        "run_id": run_id,
        "execution_nonce_sha256": custody_context.get("execution_nonce_sha256"),
        "command_returncode": returncode,
        "receipt_context": custody_context,
    }
    return hashlib.sha256(
        json.dumps(material, sort_keys=True, separators=(",", ":")).encode()
    ).hexdigest()


QUEUE_TERMINAL_SCHEMA = "molt.proof-queue-terminal.v1"


class QueueTerminalOutcome(TypedDict):
    schema: str
    status: str
    returncode: int | None
    command_returncode: int | None
    execution_error: str | None


def validate_queue_terminal(value: object) -> QueueTerminalOutcome:
    """Require the explicit parent outcome; never infer one for old receipts."""
    if not isinstance(value, dict) or value.get("schema") != QUEUE_TERMINAL_SCHEMA:
        raise ValueError("terminal proof has no supported final queue outcome")
    if (
        set(value) != set(QueueTerminalOutcome.__annotations__)
        or not isinstance(value.get("status"), str)
        or value.get("status") not in {"passed", "failed", "non-evidence", "stale"}
        or type(value.get("returncode")) is not int
        or (
            value.get("command_returncode") is not None
            and type(value.get("command_returncode")) is not int
        )
        or (
            value.get("execution_error") is not None
            and not isinstance(value.get("execution_error"), str)
        )
        or (
            value.get("status") == "passed"
            and (value.get("returncode") != 0 or value.get("command_returncode") != 0)
        )
        or (
            value.get("status") == "non-evidence"
            and (value.get("returncode") != 2 or value.get("command_returncode") != 0)
        )
        or (value.get("status") in {"failed", "stale"} and value.get("returncode") == 0)
    ):
        raise ValueError("terminal proof has a malformed final queue outcome")
    return cast(QueueTerminalOutcome, value)


def terminal_evidence_sha256(
    context: Mapping[str, object], *, run_id: str, returncode: int
) -> str:
    terminal_context = dict(context)
    terminal_context.pop("terminal_evidence_sha256", None)
    material = {
        "run_id": run_id,
        "execution_nonce_sha256": terminal_context.get("execution_nonce_sha256"),
        "queue_returncode": returncode,
        "receipt_context": terminal_context,
    }
    return hashlib.sha256(
        json.dumps(material, sort_keys=True, separators=(",", ":")).encode()
    ).hexdigest()
