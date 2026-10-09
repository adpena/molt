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
from typing import Mapping, Sequence, TypedDict, cast

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
            authority, max_bytes=4096, label="proof supervisor protocol authority"
        )
    except (OSError, UnicodeDecodeError, ExactJsonError, json.JSONDecodeError) as exc:
        raise RuntimeError(
            "proof supervisor protocol authority is unavailable"
        ) from exc
    expected = {
        "policy_schema",
        "capability_schema",
        "receipt_schema",
        "event_log_schema",
    }
    if (
        not isinstance(payload, dict)
        or set(payload) != expected | {"export", "policy_max_bytes"}
        or type(payload.get("policy_max_bytes")) is not int
        or payload["policy_max_bytes"] <= 0
        or not all(
            isinstance(payload[name], str)
            and re.fullmatch(r"molt\.[a-z0-9.-]+\.v[0-9]+", payload[name])
            for name in expected
        )
    ):
        raise RuntimeError("proof supervisor protocol authority is malformed")
    export = payload["export"]
    if (
        not isinstance(export, dict)
        or set(export)
        != {"footer_magic", "length_hex_digits", "event_max_bytes", "receipt_max_bytes"}
        or not isinstance(export["footer_magic"], str)
        or not export["footer_magic"].isascii()
        or not export["footer_magic"].startswith("\n")
        or not export["footer_magic"].endswith("\n")
        or any(
            type(export[key]) is not int or export[key] <= 0
            for key in ("length_hex_digits", "event_max_bytes", "receipt_max_bytes")
        )
    ):
        raise RuntimeError("proof supervisor export authority is malformed")
    return payload


_PROTOCOL_SCHEMAS = _protocol_authority()
SUPERVISOR_POLICY_SCHEMA = _PROTOCOL_SCHEMAS["policy_schema"]
SUPERVISOR_CAPABILITY_SCHEMA = _PROTOCOL_SCHEMAS["capability_schema"]
SUPERVISOR_RECEIPT_SCHEMA = _PROTOCOL_SCHEMAS["receipt_schema"]
SUPERVISOR_EVENT_LOG_SCHEMA = _PROTOCOL_SCHEMAS["event_log_schema"]
SUPERVISOR_POLICY_MAX_BYTES = _PROTOCOL_SCHEMAS["policy_max_bytes"]
SUPERVISOR_RECEIPT_MAX_BYTES = _PROTOCOL_SCHEMAS["export"]["receipt_max_bytes"]
_MAX_SUPERVISOR_EVENT_LOG_BYTES = 1024 * 1024 * 1024
_MAX_SUPERVISOR_EVENT_RECORD_BYTES = 1024 * 1024
_MAX_SUPERVISOR_EVENT_RECORDS = 10_000_000
_MAX_INVENTORY_IMAGE_IDENTITIES = 16_384


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
    export = _PROTOCOL_SCHEMAS["export"]
    magic = export["footer_magic"].encode("ascii")
    digits = export["length_hex_digits"]
    footer_size = len(magic) + 2 * digits + 1
    with open_stable_regular_file(path, label="supervisor export") as opened:
        stream = opened.stream
        size = opened.stat.st_size
        if (
            size < footer_size
            or size > 16 * 1024 * 1024 + export["receipt_max_bytes"] + footer_size
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
            not 0 < receipt_size <= export["receipt_max_bytes"]
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


class SupervisorCapabilityUnavailable(ValueError):
    def __init__(self, capability: dict[str, object]) -> None:
        self.capability = dict(capability)
        super().__init__(
            "native supervisor launch capability unavailable: "
            + str(capability["reason"])
        )


def decode_supervisor_capability(capability: object, *, mode: str) -> dict[str, str]:
    """Validate one native capability and return its launch requirements."""
    if (
        not isinstance(capability, dict)
        or set(capability)
        != {
            "schema",
            "platform",
            "mode",
            "backend",
            "available",
            "pre_entry_exec_authority",
            "pre_entry_process_create_authority",
            "recursive_descendant_authority",
            "reason",
            "required_environment",
        }
        or capability.get("schema") != SUPERVISOR_CAPABILITY_SCHEMA
        or capability.get("mode") != mode
        or capability.get("platform")
        != {
            "win32": "windows",
            "darwin": "macos",
        }.get(sys.platform, sys.platform)
        or not isinstance(capability.get("backend"), str)
        or not capability["backend"]
        or not isinstance(capability.get("available"), bool)
        or not isinstance(capability.get("pre_entry_exec_authority"), bool)
        or not isinstance(capability.get("pre_entry_process_create_authority"), bool)
        or not isinstance(capability.get("recursive_descendant_authority"), bool)
        or not (
            capability.get("reason") is None or isinstance(capability["reason"], str)
        )
    ):
        raise ValueError("native supervisor launch capability schema or mode mismatch")
    if capability.get("available") is not True:
        raise SupervisorCapabilityUnavailable(capability)
    if (
        capability["pre_entry_exec_authority"] is not True
        or mode == "leaf"
        and capability["pre_entry_process_create_authority"] is not True
        or mode != "leaf"
        and capability["recursive_descendant_authority"] is not True
    ):
        raise ValueError("native supervisor launch capability lacks process custody")
    required = capability.get("required_environment")
    if not isinstance(required, dict):
        raise ValueError("native supervisor required environment is malformed")
    selected: dict[str, str] = {}
    for name, value in required.items():
        if (
            re.fullmatch(r"[A-Z_][A-Z0-9_]*", name) is None
            or not isinstance(value, str)
            or not value
            or any(character in value for character in ("\x00", "\r", "\n"))
        ):
            raise ValueError("native supervisor required environment is malformed")
        selected[name] = value
    return dict(sorted(selected.items()))


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
    return decode_supervisor_capability(capability, mode=mode)


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


def _validated_supervisor_receipt(
    *,
    binary: Path,
    policy_path: Path,
    receipt_path: Path,
    cwd: Path,
    env: Mapping[str, str],
    rootfs: Path | None = None,
) -> dict[str, object]:
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
    )
    if verified.returncode != 0:
        raise ValueError(
            "native proof supervisor receipt verification failed: "
            + (verified.stderr.strip() or verified.stdout.strip())
        )
    try:
        identity, receipt = capture_exact(
            receipt_path,
            max_bytes=SUPERVISOR_RECEIPT_MAX_BYTES,
            label="native proof supervisor receipt",
        )
    except (OSError, UnicodeDecodeError, ExactJsonError, json.JSONDecodeError) as exc:
        raise ValueError(
            "native proof supervisor returned no readable receipt"
        ) from exc
    if (
        not isinstance(receipt, dict)
        or receipt.get("schema") != SUPERVISOR_RECEIPT_SCHEMA
    ):
        raise ValueError("native proof supervisor receipt schema is unsupported")
    try:
        result = loads_exact(verified.stdout)
        policy_identity, _ = capture_exact(
            policy_path,
            max_bytes=SUPERVISOR_POLICY_MAX_BYTES,
            label="native proof supervisor policy",
        )
    except (OSError, UnicodeDecodeError, ValueError) as exc:
        raise ValueError(
            "native proof supervisor returned no readable verification binding"
        ) from exc
    require_verified_input_bindings(result, receipt=identity, policy=policy_identity)
    return receipt


def require_verified_input_bindings(
    result: object,
    *,
    receipt: StableRegularFileIdentity,
    policy: StableRegularFileIdentity,
) -> None:
    """Bind either verifier consumer to the exact native-decoded input buffers."""
    if (
        not isinstance(result, dict)
        or type(result.get("receipt_bytes")) is not int
        or type(result.get("policy_input_bytes")) is not int
        or (result.get("receipt_sha256"), result["receipt_bytes"])
        != (receipt.sha256, receipt.size)
        or (result.get("policy_input_sha256"), result["policy_input_bytes"])
        != (policy.sha256, policy.size)
    ):
        raise ValueError(
            "native proof supervisor verified different receipt or policy bytes"
        )


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
        _atomic_json(policy_path, policy)
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
                        max_bytes=SUPERVISOR_RECEIPT_MAX_BYTES,
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
            receipt.get("complete") is not True
            or receipt.get("state") != "COMPLETE"
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
