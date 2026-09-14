"""Exclusive cold Cargo target custody and sealed candidate preservation.

Process supervision and Git source capture do not enforce every build-script
input. No candidate may be reused without a complete enforced input closure.
"""

from __future__ import annotations

from dataclasses import dataclass
import datetime as dt
import json
from pathlib import Path
import platform
import re
import secrets
import sys
import time
from typing import Mapping, Sequence

from molt import file_publication
from molt import disk_capacity
from molt.file_deletion import delete_path
from molt.file_locks import _acquire_file_lock, _release_file_lock, _FileLockHandle
from molt.exact_json import canonical_json_sha256, loads_exact
from tools.proof_queue_pkg import (
    cargo_output_environment,
    command_identity,
    custody_cas,
    execution_receipt_details,
    supervisor_custody,
    toolchain_capture,
)

SCHEMA = "molt.proof-cargo-cache.v1"
GENERATION_SCHEMA = "molt.proof-cargo-cache-generation.v1"
TERMINAL_RECEIPT_SCHEMA = "molt.proof-cargo-cache-terminal-receipt.v2"
LIFECYCLE_PROJECTION_SCHEMA = "molt.proof-cargo-cache-generation-lifecycle.v1"
_INPUT_KIND = "molt.proof-cargo-cache-inputs.v2"
_SEAL_KIND = "molt.proof-cargo-cache-seal.v1"
_TERMINAL_RECEIPT_KIND = TERMINAL_RECEIPT_SCHEMA
_RECLAIM_EVIDENCE_KIND = "molt.proof-cargo-cache-reclaim-evidence.v1"
_TIMINGS_KIND = "molt.proof-cargo-cache-timings.v1"
_HEX_64 = re.compile(r"[0-9a-f]{64}")
_HEX_16 = re.compile(r"[0-9a-f]{16}")


def _utc_now() -> str:
    return dt.datetime.now(dt.UTC).replace(microsecond=0).isoformat()


def _atomic_json(path: Path, payload: Mapping[str, object]) -> None:
    custody_cas.atomic_write_bytes(
        path,
        (json.dumps(payload, sort_keys=True, separators=(",", ":")) + "\n").encode(),
    )


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
    outputs: cargo_output_environment.CargoOutputEnvironment,
    env: Mapping[str, str],
) -> dict[str, object]:
    # These names are execution transport, not Cargo compilation inputs. All
    # other selected variables (including target triple/profile flags) remain.
    ignored = command_identity._QUEUE_CUSTODY_ENV_NAMES
    semantic_env = {
        name: value
        for name, value in outputs.caller_environment(env).items()
        if name.upper() not in ignored
    }
    return {
        "schema": _INPUT_KIND,
        "host": {"os": sys.platform, "arch": platform.machine()},
        "source": dict(source),
        "toolchain_files": [
            row.as_dict() for row in toolchain_capture.frozen_files(toolchains)
        ],
        "command": list(command),
        "output_environment": outputs.identity(),
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
    outputs: cargo_output_environment.CargoOutputEnvironment,
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
        source=source, toolchains=toolchains, command=command, outputs=outputs, env=env
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
    generation_id = row.get("generation_id")
    generation_owner = row.get("generation_owner")
    generation_run_id = row.get("generation_run_id")
    execution_nonce_sha256 = row.get("execution_nonce_sha256")
    if (
        not isinstance(generation_id, str)
        or _HEX_16.fullmatch(generation_id) is None
        or generation_id != target.parent.name
        or generation_owner != str(target.parent / "owner.json")
        or not isinstance(generation_run_id, str)
        or not generation_run_id
        or not isinstance(execution_nonce_sha256, str)
        or _HEX_64.fullmatch(execution_nonce_sha256) is None
    ):
        raise ValueError("Cargo cache generation provenance is incomplete")
    outputs.validate(env, target=target)
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


def _generation_paths(
    result_root: Path, provenance: Mapping[str, object]
) -> tuple[Path, Path, Path, Path]:
    """Resolve one metadata-owned generation without accepting caller path drift."""
    if provenance.get("schema") != SCHEMA or provenance.get("run_owned") is not True:
        raise ValueError("Cargo cache generation has no exclusive provenance")
    digest = provenance.get("input_sha256")
    generation_id = provenance.get("generation_id")
    if not isinstance(digest, str) or _HEX_64.fullmatch(digest) is None:
        raise ValueError("Cargo cache generation has no canonical input identity")
    if not isinstance(generation_id, str) or _HEX_16.fullmatch(generation_id) is None:
        raise ValueError("Cargo cache generation has no canonical generation identity")
    cache_root = file_publication.resolve_owned_path(result_root / "cargo-cache")
    identity_root = file_publication.resolve_owned_path(cache_root / digest)
    generation = file_publication.resolve_owned_path(identity_root / generation_id)
    target = file_publication.resolve_owned_path(generation / "target")
    owner_path = file_publication.resolve_owned_path(generation / "owner.json")
    if provenance.get("path") != str(target):
        raise ValueError("Cargo cache generation target binding changed")
    if provenance.get("generation_owner") != str(owner_path):
        raise ValueError("Cargo cache generation owner binding changed")
    return identity_root, generation, target, owner_path


def _read_owner(path: Path) -> dict[str, object]:
    try:
        payload = loads_exact(path.read_text(encoding="utf-8"))
    except (OSError, ValueError) as exc:
        raise ValueError("Cargo cache generation owner is unreadable") from exc
    if not isinstance(payload, dict) or payload.get("schema") != GENERATION_SCHEMA:
        raise ValueError("Cargo cache generation owner is malformed")
    return payload


def _validate_owner(
    owner: Mapping[str, object], provenance: Mapping[str, object], target: Path
) -> None:
    expected = {
        "generation_id": provenance.get("generation_id"),
        "input_sha256": provenance.get("input_sha256"),
        "run_id": provenance.get("generation_run_id"),
        "execution_nonce_sha256": provenance.get("execution_nonce_sha256"),
        "target": str(target),
        "inputs": provenance.get("inputs"),
    }
    if any(owner.get(name) != value for name, value in expected.items()):
        raise ValueError("Cargo cache generation owner/provenance binding mismatch")


def _write_owner(path: Path, owner: Mapping[str, object]) -> None:
    _atomic_json(path, owner)


def _update_pointer_if_current(
    pointer: Path,
    *,
    target: Path,
    state: str,
    owner_path: Path,
    terminal_receipt: object | None = None,
) -> None:
    if not pointer.exists():
        return
    payload = loads_exact(pointer.read_text(encoding="utf-8"))
    if not isinstance(payload, dict) or payload.get("schema") != SCHEMA:
        raise ValueError("Cargo cache generation state is malformed")
    if payload.get("target") != str(target):
        return
    updated: dict[str, object] = {
        "schema": SCHEMA,
        "state": state,
        "run_id": payload.get("run_id"),
        "target": str(target),
        "generation_owner": str(owner_path),
    }
    if terminal_receipt is not None:
        updated["terminal_receipt"] = terminal_receipt
    if state == "sealed" and payload.get("seal") is not None:
        updated["seal"] = payload["seal"]
    _atomic_json(pointer, updated)


@dataclass
class CargoCacheLease:
    target: Path
    pointer: Path
    owner_path: Path
    cas_root: Path
    lock: _FileLockHandle
    provenance: dict[str, object]
    owner: dict[str, object]
    environment: dict[str, str]
    publication_outcome: dict[str, object] | None = None
    published: bool = False
    owner_persisted: bool = True
    closed: bool = False

    def _persist_publication(self, publication: Mapping[str, object]) -> None:
        self.publication_outcome = dict(publication)
        self.owner.update(
            lifecycle="awaiting-terminal-receipt",
            publication=self.publication_outcome,
            execution_finished_at=_utc_now(),
        )
        self.published = True
        self.owner_persisted = False
        _write_owner(self.owner_path, self.owner)
        self.owner_persisted = True

    def publish(self, result: Mapping[str, object]) -> dict[str, object]:
        if self.closed:
            raise ValueError("Cargo cache cannot publish without its exclusive lease")
        publication: dict[str, object]
        try:
            complete = _complete_custody(result, cas_root=self.cas_root)
        except (OSError, ValueError) as exc:
            publication = {
                "state": "unsealed",
                "reason": "invalid-execution-details",
                "error": f"{type(exc).__name__}: {exc}",
            }
        else:
            if not complete:
                publication = {
                    "state": "unsealed",
                    "reason": "incomplete-or-unstable-custody",
                }
            else:
                started = time.perf_counter()
                try:
                    manifest = command_identity._directory_manifest_identity(
                        self.target, label="Cargo cache output", strict_owned=True
                    )
                    producer = _artifact(
                        self.cas_root,
                        "molt.proof-cargo-cache-producer.v1",
                        result=dict(result),
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
                    publication = {
                        "state": "sealed",
                        "purpose": "preserved-candidate",
                        "reusable": False,
                        "reuse_rejection_code": "cargo-input-closure-unproven",
                        "seal": seal,
                        "publish_s": time.perf_counter() - started,
                    }
                except BaseException as exc:
                    failed = {
                        "state": "unsealed",
                        "reason": "publication-failed",
                        "error": f"{type(exc).__name__}: {exc}",
                    }
                    try:
                        self._persist_publication(failed)
                    except BaseException as owner_exc:
                        exc.add_note(
                            "Cargo generation owner publication also failed: "
                            f"{type(owner_exc).__name__}: {owner_exc}"
                        )
                    raise
        self._persist_publication(publication)
        if publication.get("state") == "sealed":
            _atomic_json(
                self.pointer,
                {
                    "schema": SCHEMA,
                    "state": "sealed",
                    "seal": publication["seal"],
                    "target": str(self.target),
                    "run_id": self.provenance["generation_run_id"],
                    "generation_owner": str(self.owner_path),
                },
            )
        return publication

    def close(self) -> None:
        if not self.closed:
            try:
                if not self.published:
                    self._persist_publication(
                        {
                            "state": "unsealed",
                            "reason": "lease-closed-without-publication",
                        }
                    )
                elif not self.owner_persisted:
                    _write_owner(self.owner_path, self.owner)
                    self.owner_persisted = True
            finally:
                self.closed = True
                _release_file_lock(self.lock)


def _terminal_state(
    publication: Mapping[str, object], *, process_cleanup_safe: bool
) -> str:
    state = publication.get("state")
    if state == "sealed":
        return "terminal-sealed-retained"
    if state != "unsealed":
        raise ValueError("Cargo cache terminal receipt has invalid publication state")
    return (
        "terminal-unsealed-reclaimable"
        if process_cleanup_safe
        else "terminal-indeterminate-retained"
    )


def _validate_terminal_payload(
    receipt: Mapping[str, object], provenance: Mapping[str, object]
) -> str:
    if receipt.get("schema") != TERMINAL_RECEIPT_SCHEMA:
        raise ValueError("Cargo cache terminal receipt schema mismatch")
    outcome = supervisor_custody.validate_queue_terminal(receipt.get("queue_terminal"))
    if outcome["command_returncode"] != receipt.get("command_returncode") or type(
        outcome["command_returncode"]
    ) is not type(receipt.get("command_returncode")):
        raise ValueError(
            "Cargo cache terminal command result differs from queue outcome"
        )
    expected = {
        "run_id": provenance.get("generation_run_id"),
        "execution_nonce_sha256": provenance.get("execution_nonce_sha256"),
        "input_sha256": provenance.get("input_sha256"),
        "generation_id": provenance.get("generation_id"),
        "target": provenance.get("path"),
    }
    if any(receipt.get(name) != value for name, value in expected.items()):
        raise ValueError("Cargo cache terminal receipt owner binding mismatch")
    nonce = receipt.get("execution_nonce_sha256")
    custody = receipt.get("execution_custody_sha256")
    publication = receipt.get("cargo_cache_publication")
    cleanup_safe = receipt.get("process_cleanup_safe")
    if (
        not isinstance(nonce, str)
        or _HEX_64.fullmatch(nonce) is None
        or not isinstance(publication, Mapping)
        or not isinstance(cleanup_safe, bool)
    ):
        raise ValueError("Cargo cache terminal receipt binding is incomplete")
    if cleanup_safe and (
        not isinstance(custody, str)
        or _HEX_64.fullmatch(custody) is None
        or not isinstance(receipt.get("guard_receipt"), Mapping)
        or not isinstance(receipt.get("process_supervisor"), Mapping)
    ):
        raise ValueError(
            "Cargo cache reclaimable receipt lacks guard/supervisor custody"
        )
    return _terminal_state(publication, process_cleanup_safe=cleanup_safe)


def _lifecycle_projection(
    *,
    state: str,
    provenance: Mapping[str, object],
    terminal_receipt: Mapping[str, object],
) -> dict[str, object]:
    return {
        "schema": LIFECYCLE_PROJECTION_SCHEMA,
        "state": state,
        "run_id": provenance["generation_run_id"],
        "execution_nonce_sha256": provenance["execution_nonce_sha256"],
        "input_sha256": provenance["input_sha256"],
        "generation_id": provenance["generation_id"],
        "target": provenance["path"],
        "terminal_receipt": dict(terminal_receipt),
    }


def validate_terminal_receipt(
    *,
    result_root: Path,
    projection: Mapping[str, object],
    provenance: Mapping[str, object],
    run_id: str,
    execution_nonce_sha256: str,
) -> dict[str, object]:
    """Validate immutable terminal custody without reading mutable generation state."""
    if projection.get("schema") != LIFECYCLE_PROJECTION_SCHEMA:
        raise ValueError("Cargo cache lifecycle projection schema mismatch")
    if run_id != provenance.get(
        "generation_run_id"
    ) or execution_nonce_sha256 != provenance.get("execution_nonce_sha256"):
        raise ValueError("Cargo cache lifecycle consumer binding mismatch")
    reference = projection.get("terminal_receipt")
    payload = _read(
        reference,
        result_root / "custody-cas",
        _TERMINAL_RECEIPT_KIND,
    )
    receipt = payload.get("receipt")
    if not isinstance(receipt, Mapping):
        raise ValueError("Cargo cache terminal receipt artifact is malformed")
    state = _validate_terminal_payload(receipt, provenance)
    expected = _lifecycle_projection(
        state=state,
        provenance=provenance,
        terminal_receipt=reference,
    )
    if dict(projection) != expected:
        raise ValueError("Cargo cache lifecycle projection differs from its receipt")
    return dict(receipt)


def record_terminal_receipt(
    *,
    result_root: Path,
    provenance: Mapping[str, object],
    terminal_receipt: Mapping[str, object],
    timeout_s: float = 30.0,
) -> dict[str, object]:
    """Bind one parent-validated terminal receipt to its exact generation owner."""
    state = _validate_terminal_payload(terminal_receipt, provenance)
    identity_root, _generation, target, owner_path = _generation_paths(
        result_root, provenance
    )
    lock = _acquire_file_lock(
        identity_root / "target.lock",
        timeout_s=max(0.0, min(timeout_s, 30.0)),
        timeout_message=f"Cargo cache target is busy: {identity_root}",
    )
    try:
        owner = _read_owner(owner_path)
        _validate_owner(owner, provenance, target)
        publication = owner.get("publication")
        if not isinstance(publication, Mapping):
            raise ValueError("Cargo cache generation has no publication outcome")
        if publication != terminal_receipt.get("cargo_cache_publication"):
            raise ValueError("Cargo cache terminal publication binding mismatch")
        reference = _artifact(
            result_root / "custody-cas",
            _TERMINAL_RECEIPT_KIND,
            receipt=dict(terminal_receipt),
        )
        prior_state = owner.get("lifecycle")
        if prior_state in {
            "terminal-sealed-retained",
            "terminal-unsealed-reclaimable",
            "terminal-indeterminate-retained",
        }:
            if prior_state != state or owner.get("terminal_receipt") != reference:
                raise ValueError("Cargo cache generation terminal receipt changed")
        elif prior_state != "awaiting-terminal-receipt":
            raise ValueError("Cargo cache generation is not awaiting terminal custody")
        else:
            owner.update(
                lifecycle=state,
                terminal_receipt=reference,
                terminal_at=_utc_now(),
            )
            _write_owner(owner_path, owner)
        _update_pointer_if_current(
            identity_root / "state.json",
            target=target,
            state=("sealed" if state == "terminal-sealed-retained" else state),
            owner_path=owner_path,
            terminal_receipt=reference,
        )
        projection = _lifecycle_projection(
            state=state,
            provenance=provenance,
            terminal_receipt=reference,
        )
        validate_terminal_receipt(
            result_root=result_root,
            projection=projection,
            provenance=provenance,
            run_id=str(provenance["generation_run_id"]),
            execution_nonce_sha256=str(provenance["execution_nonce_sha256"]),
        )
        return projection
    finally:
        _release_file_lock(lock)


def _validate_terminal_generation_authority(
    *,
    result_root: Path,
    provenance: Mapping[str, object],
    projection: Mapping[str, object],
    target: Path,
    owner_path: Path,
) -> tuple[dict[str, object], dict[str, object], dict[str, object]]:
    """Bind mutable owner lifecycle to the caller's immutable terminal proof."""
    owner = _read_owner(owner_path)
    _validate_owner(owner, provenance, target)
    immutable_receipt = validate_terminal_receipt(
        result_root=result_root,
        projection=projection,
        provenance=provenance,
        run_id=str(provenance.get("generation_run_id")),
        execution_nonce_sha256=str(provenance.get("execution_nonce_sha256")),
    )
    terminal_receipt = projection.get("terminal_receipt")
    if owner.get("terminal_receipt") != terminal_receipt:
        raise ValueError(
            "Cargo cache generation owner differs from persisted terminal projection"
        )
    publication = owner.get("publication")
    if (
        not isinstance(publication, Mapping)
        or immutable_receipt.get("cargo_cache_publication") != publication
    ):
        raise ValueError(
            "Cargo cache generation publication differs from terminal receipt"
        )
    return owner, immutable_receipt, dict(publication)


def inspect_terminal_generation(
    *,
    result_root: Path,
    provenance: Mapping[str, object],
    projection: Mapping[str, object],
) -> dict[str, object]:
    """Inspect terminal generation custody without locks, writes, or target hashing."""
    _identity_root, _generation, target, owner_path = _generation_paths(
        result_root, provenance
    )
    owner, immutable_receipt, publication = _validate_terminal_generation_authority(
        result_root=result_root,
        provenance=provenance,
        projection=projection,
        target=target,
        owner_path=owner_path,
    )
    lifecycle = owner.get("lifecycle")
    target_present = target.exists()
    eligible = (
        lifecycle == "terminal-unsealed-reclaimable"
        and projection.get("state") == "terminal-unsealed-reclaimable"
        and publication.get("state") == "unsealed"
        and immutable_receipt.get("process_cleanup_safe") is True
        and target_present
    )
    if eligible:
        reason = "terminal-unsealed-cleanup-safe"
    elif lifecycle != "terminal-unsealed-reclaimable":
        reason = "owner-lifecycle-not-reclaimable"
    elif projection.get("state") != "terminal-unsealed-reclaimable":
        reason = "terminal-projection-not-reclaimable"
    elif not target_present:
        reason = "target-absent"
    else:
        reason = "terminal-receipt-not-cleanup-safe"
    return {
        "state": lifecycle,
        "target": str(target),
        "owner": str(owner_path),
        "target_present": target_present,
        "terminal_state": projection.get("state"),
        "publication_state": publication.get("state"),
        "process_cleanup_safe": immutable_receipt.get("process_cleanup_safe"),
        "reclaim_eligible": eligible,
        "reclaim_reason": reason,
    }


def _preserve_timings(target: Path, *, cas_root: Path) -> dict[str, object]:
    timings = target / "cargo-timings"
    if not timings.exists():
        return _artifact(cas_root, _TIMINGS_KIND, present=False, files=[])
    if not timings.is_dir() or file_publication.is_link_like(timings):
        raise ValueError("Cargo timing evidence is not an owned directory")
    before = command_identity._directory_manifest_identity(
        timings, label="Cargo timing evidence", strict_owned=True
    )
    files = before.get("files")
    if not isinstance(files, list):
        raise ValueError("Cargo timing evidence manifest is malformed")
    preserved: list[dict[str, object]] = []
    for row in files:
        if not isinstance(row, Mapping) or not isinstance(
            row.get("relative_path"), str
        ):
            raise ValueError("Cargo timing evidence entry is malformed")
        source = timings / str(row["relative_path"])
        reference = custody_cas.put_file(
            cas_root, source, logical_name=source.name
        ).as_dict()
        if reference.get("sha256") != row.get("sha256") or reference.get(
            "size_bytes"
        ) != row.get("size"):
            raise ValueError("Cargo timing evidence changed while being copied")
        preserved.append({"relative_path": row["relative_path"], "file": reference})
    after = command_identity._directory_manifest_identity(
        timings, label="Cargo timing evidence", strict_owned=True
    )
    if after != before:
        raise ValueError("Cargo timing evidence changed while being preserved")
    return _artifact(
        cas_root,
        _TIMINGS_KIND,
        present=True,
        manifest=before,
        files=preserved,
    )


def reclaim_terminal_unsealed(
    *,
    result_root: Path,
    provenance: Mapping[str, object],
    projection: Mapping[str, object],
    timeout_s: float = 30.0,
) -> dict[str, object]:
    """Reclaim one terminal unsealed target; retain immutable evidence and owner."""
    identity_root, _generation, target, owner_path = _generation_paths(
        result_root, provenance
    )
    lock = _acquire_file_lock(
        identity_root / "target.lock",
        timeout_s=max(0.0, min(timeout_s, 30.0)),
        timeout_message=f"Cargo cache target is busy: {identity_root}",
    )
    try:
        owner, immutable_receipt, publication = _validate_terminal_generation_authority(
            result_root=result_root,
            provenance=provenance,
            projection=projection,
            target=target,
            owner_path=owner_path,
        )
        terminal_receipt = projection.get("terminal_receipt")
        assert isinstance(terminal_receipt, Mapping)
        lifecycle = owner.get("lifecycle")
        if lifecycle == "reclaimed":
            _update_pointer_if_current(
                identity_root / "state.json",
                target=target,
                state="reclaimed",
                owner_path=owner_path,
                terminal_receipt=terminal_receipt,
            )
            return {
                "state": "reclaimed",
                "target": str(target),
                "owner": str(owner_path),
                "idempotent": True,
            }
        if lifecycle == "reclaim-blocked":
            return {
                "state": "reclaim-blocked",
                "target": str(target),
                "owner": str(owner_path),
                "reason": "prior-reclaim-failure",
            }
        if lifecycle == "reclaiming":
            if not target.exists():
                owner.update(lifecycle="reclaimed", reclaimed_at=_utc_now())
                _write_owner(owner_path, owner)
                _update_pointer_if_current(
                    identity_root / "state.json",
                    target=target,
                    state="reclaimed",
                    owner_path=owner_path,
                    terminal_receipt=terminal_receipt,
                )
                return {
                    "state": "reclaimed",
                    "target": str(target),
                    "owner": str(owner_path),
                    "idempotent": True,
                }
            owner.update(
                lifecycle="reclaim-blocked",
                reclaim_blocked_at=_utc_now(),
                reclaim_error="prior reclaim was interrupted; automatic retry refused",
            )
            _write_owner(owner_path, owner)
            return {
                "state": "reclaim-blocked",
                "target": str(target),
                "owner": str(owner_path),
                "reason": "prior-reclaim-interrupted",
            }
        if lifecycle != "terminal-unsealed-reclaimable":
            return {
                "state": "not-reclaimable",
                "lifecycle": lifecycle,
                "target": str(target),
                "owner": str(owner_path),
            }
        if (
            projection.get("state") != "terminal-unsealed-reclaimable"
            or publication.get("state") != "unsealed"
            or immutable_receipt.get("process_cleanup_safe") is not True
        ):
            raise ValueError("Cargo cache reclaim state lacks terminal evidence")
        try:
            output_before = command_identity._directory_manifest_identity(
                target, label="terminal unsealed Cargo output", strict_owned=True
            )
            timings = _preserve_timings(target, cas_root=result_root / "custody-cas")
            output_after = command_identity._directory_manifest_identity(
                target, label="terminal unsealed Cargo output", strict_owned=True
            )
            if output_after != output_before:
                raise ValueError(
                    "terminal Cargo output changed during evidence capture"
                )
            evidence = _artifact(
                result_root / "custody-cas",
                _RECLAIM_EVIDENCE_KIND,
                target=str(target),
                publication=dict(publication),
                terminal_receipt=dict(terminal_receipt),
                output_manifest=output_before,
                timings=timings,
            )
        except (OSError, ValueError) as exc:
            owner.update(
                lifecycle="reclaim-blocked",
                reclaim_blocked_at=_utc_now(),
                reclaim_error=f"{type(exc).__name__}: {exc}",
            )
            _write_owner(owner_path, owner)
            return {
                "state": "reclaim-blocked",
                "target": str(target),
                "owner": str(owner_path),
                "reason": "evidence-preservation-failed",
                "error": owner["reclaim_error"],
            }
        files = output_before.get("files")
        file_rows = files if isinstance(files, list) else []
        owner.update(
            lifecycle="reclaiming",
            reclaim_started_at=_utc_now(),
            reclaim={
                "evidence": evidence,
                "file_count": output_before.get("file_count"),
                "size_bytes": sum(
                    int(row.get("size", 0))
                    for row in file_rows
                    if isinstance(row, Mapping) and isinstance(row.get("size"), int)
                ),
            },
        )
        _write_owner(owner_path, owner)
        deleted, error = delete_path(target)
        if not deleted:
            owner.update(
                lifecycle="reclaim-blocked",
                reclaim_blocked_at=_utc_now(),
                reclaim_error=error,
            )
            _write_owner(owner_path, owner)
            _update_pointer_if_current(
                identity_root / "state.json",
                target=target,
                state="reclaim-blocked",
                owner_path=owner_path,
                terminal_receipt=terminal_receipt,
            )
            return {
                "state": "reclaim-blocked",
                "target": str(target),
                "owner": str(owner_path),
                "reason": "delete-failed",
                "error": error,
            }
        owner.update(lifecycle="reclaimed", reclaimed_at=_utc_now())
        _write_owner(owner_path, owner)
        _update_pointer_if_current(
            identity_root / "state.json",
            target=target,
            state="reclaimed",
            owner_path=owner_path,
            terminal_receipt=terminal_receipt,
        )
        reclaim = owner["reclaim"]
        assert isinstance(reclaim, Mapping)
        return {
            "state": "reclaimed",
            "target": str(target),
            "owner": str(owner_path),
            "file_count": reclaim.get("file_count"),
            "size_bytes": reclaim.get("size_bytes"),
            "evidence": reclaim.get("evidence"),
        }
    finally:
        _release_file_lock(lock)


def inventory_legacy_generations(result_root: Path) -> list[dict[str, object]]:
    """Report metadata-less historical targets; never infer an owner or delete."""
    cache_root = result_root / "cargo-cache"
    if not cache_root.is_dir() or file_publication.is_link_like(cache_root):
        return []
    inventory: list[dict[str, object]] = []
    for identity_root in sorted(cache_root.iterdir(), key=lambda path: path.name):
        if (
            _HEX_64.fullmatch(identity_root.name) is None
            or not identity_root.is_dir()
            or file_publication.is_link_like(identity_root)
        ):
            continue
        for generation in sorted(identity_root.iterdir(), key=lambda path: path.name):
            if (
                _HEX_16.fullmatch(generation.name) is None
                or not generation.is_dir()
                or file_publication.is_link_like(generation)
            ):
                continue
            owner_path = generation / "owner.json"
            target = generation / "target"
            if (
                owner_path.exists()
                or not target.is_dir()
                or file_publication.is_link_like(target)
            ):
                continue
            inventory.append(
                {
                    "state": "legacy-owner-absent-retained",
                    "input_sha256": identity_root.name,
                    "generation_id": generation.name,
                    "target": str(target),
                    "reason": "persistent per-generation owner metadata is absent",
                }
            )
    return inventory


def acquire(
    *,
    result_root: Path,
    source_root: Path,
    toolchains: Mapping[str, object],
    command: Sequence[str],
    outputs: cargo_output_environment.CargoOutputEnvironment,
    env: Mapping[str, str],
    requested_target: str | None,
    run_id: str,
    execution_nonce_sha256: str,
    timeout_s: float,
    source_snapshot: Mapping[str, object],
    source_content: Mapping[str, object],
) -> CargoCacheLease:
    started = time.perf_counter()
    if _HEX_64.fullmatch(execution_nonce_sha256) is None:
        raise ValueError("Cargo cache generation requires an execution nonce digest")
    capacity_admission = disk_capacity.require_build_capacity(
        (result_root / "cargo-cache",), env=env
    ).as_dict()
    cas_root = result_root / "custody-cas"
    source_manifest = _source_descriptor(
        str(source_root.resolve(strict=True)), source_content, source_snapshot, cas_root
    )
    identity = input_identity(
        source=source_manifest,
        toolchains=toolchains,
        command=command,
        outputs=outputs,
        env=env,
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
            elif previous.get("state") in {
                "pending",
                "terminal-unsealed-reclaimable",
                "terminal-indeterminate-retained",
                "reclaimed",
                "reclaim-blocked",
            }:
                cold_reason = "prior-generation-unsealed"
            else:
                raise ValueError("Cargo cache generation state is invalid")
        generation = root / secrets.token_hex(8)
        generation.mkdir(exist_ok=False)
        file_publication.fsync_directory(root)
        target = generation / "target"
        owner_path = generation / "owner.json"
        file_publication.resolve_owned_path(owner_path)
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
            "disk_capacity_admission": capacity_admission,
            "generation_id": generation.name,
            "generation_owner": str(owner_path),
            "generation_run_id": run_id,
            "execution_nonce_sha256": execution_nonce_sha256,
        }
        owner: dict[str, object] = {
            "schema": GENERATION_SCHEMA,
            "generation_id": generation.name,
            "input_sha256": digest,
            "run_id": run_id,
            "execution_nonce_sha256": execution_nonce_sha256,
            "target": str(target),
            "inputs": inputs,
            "created_at": _utc_now(),
            "lifecycle": "leased",
            "publication": None,
            "terminal_receipt": None,
            "reclaim": None,
        }
        _write_owner(owner_path, owner)
        custody_cas._durable_makedirs(target)
        if any(target.iterdir()):
            raise ValueError("new Cargo cache generation is not empty")
        bound_environment = outputs.bind(env, target=target)
        validate_prelaunch(
            provenance,
            cas_root=cas_root,
            command=command,
            outputs=outputs,
            env=bound_environment,
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
                        "generation_owner": str(owner_path),
                    },
                    sort_keys=True,
                )
                + "\n"
            ).encode(),
        )
        return CargoCacheLease(
            target,
            pointer,
            owner_path,
            cas_root,
            lock,
            provenance,
            owner,
            bound_environment,
        )
    except BaseException:
        _release_file_lock(lock)
        raise
