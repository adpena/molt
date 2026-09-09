"""Exclusive cold Cargo target custody and sealed candidate preservation.

Process supervision and Git source capture do not enforce every build-script
input. No candidate may be reused without a complete enforced input closure.
"""

from __future__ import annotations

from dataclasses import dataclass
import json
from pathlib import Path
import platform
import secrets
import sys
import time
from typing import Mapping, Sequence

from molt import file_publication
from molt.file_locks import _acquire_file_lock, _release_file_lock, _FileLockHandle
from molt.exact_json import canonical_json_sha256, loads_exact
from tools.proof_queue_pkg import (
    command_identity,
    custody_cas,
    execution_receipt_details,
    toolchain_capture,
)

SCHEMA = "molt.proof-cargo-cache.v1"
_INPUT_KIND = "molt.proof-cargo-cache-inputs.v1"
_SEAL_KIND = "molt.proof-cargo-cache-seal.v1"


class CargoInputClosureUnproven(ValueError):
    """A preserved candidate lacks enforced complete-input admission."""

    def __init__(self, *, candidate: Path, seal: object) -> None:
        self.diagnostic = {
            "state": "rejected",
            "code": "cargo-input-closure-unproven",
            "candidate_path": str(candidate),
            "candidate_seal": seal,
            "reason": "Git source and process custody do not enforce complete Cargo build-script inputs; candidate preserved without reuse",
        }
        super().__init__(json.dumps(self.diagnostic, sort_keys=True))


def _artifact(root: Path, kind: str, **payload: object) -> dict[str, object]:
    return custody_cas.put_json(
        root, {"schema": custody_cas.ARTIFACT_SCHEMA, "kind": kind, **payload}
    ).as_dict()


def _read(reference: object, root: Path, kind: str) -> dict[str, object]:
    if not isinstance(reference, Mapping):
        raise ValueError("Cargo cache custody artifact reference is missing")
    payload = custody_cas.read_ref(reference, expected_root=root)
    if payload.get("kind") != kind:
        raise ValueError("Cargo cache custody artifact kind mismatch")
    return payload


def input_identity(
    *,
    source: Mapping[str, object],
    toolchains: Mapping[str, object],
    command: Sequence[str],
    env: Mapping[str, str],
) -> dict[str, object]:
    # These names are execution transport, not Cargo compilation inputs. All
    # other selected variables (including target triple/profile flags) remain.
    ignored = command_identity._QUEUE_CUSTODY_ENV_NAMES | {"CARGO_TARGET_DIR"}
    semantic_env = {
        name: value for name, value in env.items() if name.upper() not in ignored
    }
    return {
        "schema": _INPUT_KIND,
        "host": {"os": sys.platform, "arch": platform.machine()},
        "source": dict(source),
        "toolchain_files": [
            row.as_dict() for row in toolchain_capture.frozen_files(toolchains)
        ],
        "command": list(command),
        "environment_sha256": canonical_json_sha256(semantic_env),
    }


def _complete_custody(result: Mapping[str, object], *, cas_root: Path) -> bool:
    context = result.get("receipt_context")
    if result.get("phase") != "complete" or not isinstance(context, Mapping):
        return False
    context = execution_receipt_details.expand_context(context, cas_root=cas_root)
    source = context.get("source_custody")
    process = context.get("process_supervisor")
    session = context.get("execution_custody_session")
    if not all(isinstance(value, Mapping) for value in (source, process, session)):
        return False
    reasons = source.get("ineligible_reasons")
    content = source.get("content")
    receipt = process.get("receipt")
    live = context.get("live_input_custody")
    child = context.get("child_process_custody")
    child_receipt = child.get("receipt") if isinstance(child, Mapping) else None
    stable_authorities = (
        "source_custody",
        "toolchain_custody",
        "execution_environment",
        "command_executable",
        "custody_authorities",
        "platform_process_custody",
    )
    return (
        isinstance(reasons, list)
        and all(isinstance(reason, str) for reason in reasons)
        and set(reasons) <= {"source-dirty-prelaunch", "source-dirty-postcompletion"}
        and isinstance(content, Mapping)
        and content.get("identical") is True
        and isinstance(content.get("prelaunch"), Mapping)
        and content.get("postcompletion") == content.get("prelaunch")
        and all(
            isinstance(context.get(name), Mapping)
            and context[name].get("identical") is True
            for name in stable_authorities
        )
        and isinstance(live, Mapping)
        and live.get("stable") is True
        and live.get("event_count") == 0
        and live.get("error_count") == 0
        and isinstance(child_receipt, Mapping)
        and child_receipt.get("broker_complete") is True
        and isinstance(receipt, Mapping)
        and receipt.get("state") == "COMPLETE"
        and receipt.get("complete") is True
        and receipt.get("violation_count") == 0
        and receipt.get("error_count") == 0
        and process.get("supervisor_returncode") == 0
        and session.get("state") == "DRAINED"
    )


def validate_prelaunch(
    row: Mapping[str, object],
    *,
    cas_root: Path,
    command: Sequence[str],
    env: Mapping[str, str],
    toolchains: Mapping[str, object],
    source_root: str,
    source_snapshot: Mapping[str, object],
    source_content: Mapping[str, object],
) -> None:
    """Validate cold custody; reject reuse without an enforced input closure."""
    if row.get("schema") != SCHEMA or row.get("run_owned") is not True:
        raise ValueError("Cargo cache has no exclusive prelaunch custody")
    inputs = _read(row.get("inputs"), cas_root, _INPUT_KIND)
    identity = inputs.get("identity")
    source = _source_descriptor(source_root, source_content, source_snapshot, cas_root)
    expected = input_identity(
        source=source, toolchains=toolchains, command=command, env=env
    )
    if identity != expected or row.get("input_sha256") != canonical_json_sha256(
        expected
    ):
        raise ValueError(
            "Cargo cache source/toolchain/command/environment identity mismatch"
        )
    target = Path(str(row.get("path")))
    owned = cas_root.parent / "cargo-cache" / str(row["input_sha256"])
    if target.parent.parent != owned or target.name != "target":
        raise ValueError("Cargo cache target is outside its identity-owned generation")
    state = row.get("state")
    if state == "cold":
        if row.get("seed") is not None or row.get("initial_entry_count") != 0:
            raise ValueError("cold Cargo cache is not fresh")
        if row.get("initial_manifest_sha256") != canonical_json_sha256([]):
            raise ValueError("cold Cargo cache manifest is not empty")
        return
    if state != "reused":
        raise ValueError("Cargo cache prelaunch state is invalid")
    raise CargoInputClosureUnproven(candidate=target, seal=row.get("seed"))


def _source_descriptor(
    root: str,
    content: Mapping[str, object],
    snapshot: Mapping[str, object],
    cas_root: Path,
) -> dict[str, object]:
    from tools.proof_queue_pkg.execution_environment import SOURCE_CONTENT_KIND

    if not isinstance(content, Mapping) or not isinstance(snapshot, Mapping):
        raise ValueError(
            "Cargo cache requires independent current source content custody"
        )
    payload = _read(content, cas_root, SOURCE_CONTENT_KIND)
    if payload.get("root") != root or snapshot.get("root") != root:
        raise ValueError("Cargo cache input source root differs from proof custody")
    return {"root": root, "content": dict(content), "git_snapshot": dict(snapshot)}


@dataclass
class CargoCacheLease:
    target: Path
    pointer: Path
    cas_root: Path
    lock: _FileLockHandle
    provenance: dict[str, object]
    closed: bool = False

    def publish(self, result: Mapping[str, object]) -> dict[str, object]:
        if self.closed:
            raise ValueError("Cargo cache cannot publish without its exclusive lease")
        try:
            complete = _complete_custody(result, cas_root=self.cas_root)
        except (OSError, ValueError) as exc:
            return {
                "state": "unsealed",
                "reason": "invalid-execution-details",
                "error": f"{type(exc).__name__}: {exc}",
            }
        if not complete:
            return {"state": "unsealed", "reason": "incomplete-or-unstable-custody"}
        started = time.perf_counter()
        manifest = command_identity._directory_manifest_identity(
            self.target, label="Cargo cache output", strict_owned=True
        )
        producer = _artifact(
            self.cas_root, "molt.proof-cargo-cache-producer.v1", result=dict(result)
        )
        seal = _artifact(
            self.cas_root,
            _SEAL_KIND,
            input_sha256=self.provenance["input_sha256"],
            target=str(self.target),
            manifest=manifest,
            producer=producer,
            reuse_admission={
                "state": "unproven",
                "code": "cargo-input-closure-unproven",
            },
        )
        custody_cas.atomic_write_bytes(
            self.pointer,
            (
                json.dumps(
                    {
                        "schema": SCHEMA,
                        "state": "sealed",
                        "seal": seal,
                        "target": str(self.target),
                    },
                    sort_keys=True,
                )
                + "\n"
            ).encode(),
        )
        return {
            "state": "sealed",
            "purpose": "preserved-candidate",
            "reusable": False,
            "reuse_rejection_code": "cargo-input-closure-unproven",
            "seal": seal,
            "publish_s": time.perf_counter() - started,
        }

    def close(self) -> None:
        if not self.closed:
            self.closed = True
            _release_file_lock(self.lock)


def acquire(
    *,
    result_root: Path,
    source_root: Path,
    toolchains: Mapping[str, object],
    command: Sequence[str],
    env: Mapping[str, str],
    requested_target: str | None,
    run_id: str,
    timeout_s: float,
    source_snapshot: Mapping[str, object],
    source_content: Mapping[str, object],
) -> CargoCacheLease:
    started = time.perf_counter()
    if any(
        value == "--target-dir" or value.startswith("--target-dir=")
        for value in command
    ):
        raise ValueError(
            "Cargo --target-dir bypasses proof target custody; supply CARGO_TARGET_DIR as the requested target instead"
        )
    cas_root = result_root / "custody-cas"
    source_manifest = _source_descriptor(
        str(source_root.resolve(strict=True)), source_content, source_snapshot, cas_root
    )
    identity = input_identity(
        source=source_manifest, toolchains=toolchains, command=command, env=env
    )
    digest = canonical_json_sha256(identity)
    root = file_publication.resolve_owned_path(result_root / "cargo-cache" / digest)
    source_path = source_root.resolve(strict=True)
    if (
        root == source_path
        or root.is_relative_to(source_path)
        or source_path.is_relative_to(root)
    ):
        raise ValueError("Cargo cache output overlaps admitted source")
    custody_cas._durable_makedirs(root)
    file_publication.resolve_owned_path(root / "target.lock")
    lock = _acquire_file_lock(
        root / "target.lock",
        timeout_s=max(0.0, min(timeout_s, 30.0)),
        timeout_message=f"Cargo cache target is busy: {root}",
    )
    try:
        pointer = root / "state.json"
        file_publication.resolve_owned_path(pointer)
        cas_root = result_root / "custody-cas"
        cold_reason = "no-sealed-generation"
        if pointer.exists():
            file_publication.resolve_owned_path(pointer)
            previous = loads_exact(pointer.read_text(encoding="utf-8"))
            if not isinstance(previous, dict) or previous.get("schema") != SCHEMA:
                raise ValueError("Cargo cache generation state is malformed")
            if previous.get("state") == "sealed":
                seed = previous.get("seal")
                target = Path(str(previous.get("target")))
                if target.parent.parent != root or target.name != "target":
                    raise ValueError("Cargo cache seal target escaped its generation")
                raise CargoInputClosureUnproven(candidate=target, seal=seed)
            elif previous.get("state") == "pending":
                cold_reason = "prior-generation-unsealed"
            else:
                raise ValueError("Cargo cache generation state is invalid")
        generation = root / secrets.token_hex(8)
        generation.mkdir(exist_ok=False)
        file_publication.fsync_directory(root)
        target = generation / "target"
        custody_cas._durable_makedirs(target)
        if any(target.iterdir()):
            raise ValueError("new Cargo cache generation is not empty")
        inputs = _artifact(cas_root, _INPUT_KIND, identity=identity)
        provenance: dict[str, object] = {
            "schema": SCHEMA,
            "role": "build-output",
            "path": str(target),
            "run_owned": True,
            "state": "cold",
            "input_sha256": digest,
            "inputs": inputs,
            "seed": None,
            "initial_entry_count": 0,
            "initial_manifest_sha256": canonical_json_sha256([]),
            "requested_target": requested_target,
            "effective_target": str(target),
            "selection_s": time.perf_counter() - started,
            "cold_reason": cold_reason,
        }
        validate_prelaunch(
            provenance,
            cas_root=cas_root,
            command=command,
            env=env,
            toolchains=toolchains,
            source_root=str(source_root.resolve(strict=True)),
            source_snapshot=source_snapshot,
            source_content=source_content,
        )
        custody_cas.atomic_write_bytes(
            pointer,
            (
                json.dumps(
                    {
                        "schema": SCHEMA,
                        "state": "pending",
                        "run_id": run_id,
                        "target": str(target),
                    },
                    sort_keys=True,
                )
                + "\n"
            ).encode(),
        )
        return CargoCacheLease(target, pointer, cas_root, lock, provenance)
    except BaseException:
        _release_file_lock(lock)
        raise
