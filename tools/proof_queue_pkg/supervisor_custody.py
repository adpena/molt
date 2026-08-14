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
import time
from typing import Mapping, Sequence

from tools.proof_queue_pkg import command_admission as admission
from tools.proof_queue_pkg import command_identity
from tools.proof_queue_pkg import custody_cas
from tools.proof_queue_pkg import process_image_capture


def _atomic_json(path: Path, payload: Mapping[str, object]) -> None:
    custody_cas.atomic_write_bytes(
        path,
        (json.dumps(payload, indent=2, sort_keys=True) + "\n").encode(),
    )


def _provision_proof_supervisor(
    *, cwd: Path, env: Mapping[str, str]
) -> tuple[Path, dict[str, object]]:
    started = time.perf_counter()
    build = admission._REPO_ROOT / "tools" / "proof_supervisor" / "build.py"
    completed = command_identity._run_captured(
        (sys.executable, str(build), "--release"),
        cwd=cwd,
        env=env,
        timeout=600.0,
    )
    if completed.returncode != 0:
        raise ValueError(
            "native proof supervisor provisioning failed: "
            + (completed.stderr.strip() or completed.stdout.strip())
        )
    lines = [line.strip() for line in completed.stdout.splitlines() if line.strip()]
    if not lines:
        raise ValueError("native proof supervisor build returned no binary")
    binary = Path(lines[-1]).resolve(strict=True)
    if not binary.is_file():
        raise ValueError("native proof supervisor binary is unavailable")
    binary_identity = command_identity._file_identity(binary)
    return binary, {
        "schema": "molt.proof-supervisor-provision-telemetry.v1",
        "build_s": time.perf_counter() - started,
        "build_target_dir": str(Path(env["CARGO_TARGET_DIR"]).resolve(strict=True)),
        "build_output_sha256": binary_identity["sha256"],
        "build_output_size_bytes": binary_identity["size_bytes"],
    }


def _supervisor_fixed_images(
    toolchains: Mapping[str, object],
    environment_executables: Mapping[str, object],
    execution_command: Sequence[str],
    platform_process_images: Sequence[Mapping[str, object]] = (),
) -> tuple[str, list[dict[str, str]]]:
    root = os.path.normcase(os.path.abspath(execution_command[0]))
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
        key = os.path.normcase(os.path.abspath(path))
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

    root_path = Path(os.path.abspath(execution_command[0]))
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
    for name, raw in environment_executables.items():
        if not isinstance(raw, Mapping):
            continue
        executable = raw.get("executable")
        if isinstance(executable, Mapping):
            add(f"env:{name}", executable.get("path"), executable.get("sha256"))
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


def _supervisor_derived_roots(
    *, descendants: object, env: Mapping[str, str]
) -> list[dict[str, str]]:
    if descendants == "forbidden":
        return []
    roots: list[dict[str, str]] = []
    for role, name in (("build-output", "CARGO_TARGET_DIR"),):
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
        "schema": "molt.proof-process-closure.v2",
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
            descendants=descendants, env=execution_env
        ),
    }


def _validated_supervisor_receipt(
    *,
    binary: Path,
    policy_path: Path,
    receipt_path: Path,
    cwd: Path,
    env: Mapping[str, str],
) -> dict[str, object]:
    verified = command_identity._run_captured(
        (
            str(binary),
            "verify",
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
        receipt = json.loads(receipt_path.read_text(encoding="utf-8"))
    except (OSError, json.JSONDecodeError) as exc:
        raise ValueError(
            "native proof supervisor returned no readable receipt"
        ) from exc
    if not isinstance(receipt, dict):
        raise ValueError("native proof supervisor receipt is not an object")
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
    command = [str(executable.resolve(strict=True)), *probe_args]
    with tempfile.TemporaryDirectory(prefix="molt-process-image-inventory-") as raw:
        root = Path(raw).resolve()
        policy_path = root / "policy.json"
        receipt_path = root / "receipt.json"
        policy = {
            "schema": "molt.proof-process-closure.v2",
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
                    failed_receipt = json.loads(
                        receipt_path.read_text(encoding="utf-8")
                    )
                except (OSError, json.JSONDecodeError):
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
        file_name = descriptor.get("file")
        if not isinstance(file_name, str) or Path(file_name).name != file_name:
            raise ValueError(f"{role} process-image inventory event path is invalid")
        event_path = receipt_path.with_name(file_name).resolve(strict=True)
        rows: list[dict[str, object]] = []
        launcher_path = Path(str(launcher["path"]))
        for line in event_path.read_text(encoding="utf-8").splitlines():
            try:
                event = json.loads(line)
            except json.JSONDecodeError as exc:
                raise ValueError(
                    f"{role} process-image inventory event is malformed"
                ) from exc
            image = event.get("image") if isinstance(event, Mapping) else None
            if not isinstance(image, Mapping):
                continue
            raw_path = image.get("path")
            digest = image.get("sha256")
            size = image.get("size_bytes")
            if (
                not isinstance(raw_path, str)
                or not isinstance(digest, str)
                or not isinstance(size, int)
            ):
                raise ValueError(
                    f"{role} process-image inventory identity is malformed"
                )
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
    file_name = event_log.get("file")
    expected_sha256 = event_log.get("sha256")
    expected_bytes = event_log.get("bytes")
    expected_count = event_log.get("count")
    if (
        not isinstance(file_name, str)
        or Path(file_name).name != file_name
        or not isinstance(expected_sha256, str)
        or re.fullmatch(r"[0-9a-f]{64}", expected_sha256) is None
        or not isinstance(expected_bytes, int)
        or isinstance(expected_bytes, bool)
        or expected_bytes < 0
        or not isinstance(expected_count, int)
        or isinstance(expected_count, bool)
        or expected_count < 0
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
    size = 0
    count = 0
    final_byte = None
    with event_path.open("rb") as stream:
        for chunk in iter(lambda: stream.read(1024 * 1024), b""):
            digest.update(chunk)
            size += len(chunk)
            count += chunk.count(b"\n")
            final_byte = chunk[-1]
    if (
        digest.hexdigest() != expected_sha256
        or size != expected_bytes
        or count != expected_count
        or (size > 0 and final_byte != ord("\n"))
    ):
        raise ValueError("native proof supervisor event artifact identity changed")
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
    errors = receipt.get("errors")
    lifecycle = receipt.get("lifecycle")
    state = receipt.get("state")
    if not isinstance(events, list) or not isinstance(errors, list):
        raise ValueError("live custody receipt event authority is malformed")
    artifact_payload = {
        "schema": custody_cas.ARTIFACT_SCHEMA,
        "kind": "live-input-custody-events",
        "events": events,
        "errors": errors,
    }
    artifact = custody_cas.put_json(cas_root, artifact_payload).as_dict()
    material = {
        "events": events,
        "errors": errors,
        "state": state,
        "lifecycle": lifecycle,
    }
    if receipt.get("identity_sha256") != _canonical_payload_sha256(material):
        raise ValueError("live custody receipt identity is inconsistent")
    return {
        **{
            key: value
            for key, value in receipt.items()
            if key not in {"events", "errors"}
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
        "terminal_evidence_sha256",
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


def terminal_evidence_sha256(
    context: Mapping[str, object], *, run_id: str, returncode: int
) -> str:
    terminal_context = dict(context)
    terminal_context.pop("terminal_evidence_sha256", None)
    material = {
        "run_id": run_id,
        "execution_nonce_sha256": terminal_context.get("execution_nonce_sha256"),
        "command_returncode": returncode,
        "receipt_context": terminal_context,
    }
    return hashlib.sha256(
        json.dumps(material, sort_keys=True, separators=(",", ":")).encode()
    ).hexdigest()
