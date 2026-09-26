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
    cargo_output_layout,
    command_identity,
    custody_cas,
    execution_custody,
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
_SEALED_RETIREMENT_EVIDENCE_KIND = (
    "molt.proof-cargo-cache-sealed-retirement-evidence.v1"
)
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
        and execution_custody.child_receipt_is_admitted(child_receipt)
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
    cargo_output_lifetime: str = "retain",
    cargo_output_root: Mapping[str, object] | None = None,
) -> None:
    """Validate cold custody; reject reuse without an enforced input closure."""
    if row.get("schema") != SCHEMA or row.get("run_owned") is not True:
        raise ValueError("Cargo cache has no exclusive prelaunch custody")
    if not cargo_output_layout.same_root(
        row.get("cargo_output_root"), cargo_output_root
    ):
        raise ValueError("Cargo cache output root differs from admitted envelope")
    layout = cargo_output_layout.CargoOutputLayout.create(
        result_root=cas_root.parent,
        declaration=cargo_output_root,
        source_root=Path(source_root),
    )
    if outputs.external_placement != (cargo_output_root is not None):
        raise ValueError("Cargo output roles differ from admitted placement")
    if _output_lifetime(row) != _output_lifetime(
        {"cargo_output_lifetime": cargo_output_lifetime}
    ):
        raise ValueError("Cargo cache output lifetime differs from admitted envelope")
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
    generation_id = row.get("generation_id")
    generation_owner = row.get("generation_owner")
    generation_run_id = row.get("generation_run_id")
    execution_nonce_sha256 = row.get("execution_nonce_sha256")
    if (
        not isinstance(generation_id, str)
        or _HEX_16.fullmatch(generation_id) is None
        or generation_owner != str(owned / str(generation_id) / "owner.json")
        or not isinstance(generation_run_id, str)
        or not generation_run_id
        or not isinstance(execution_nonce_sha256, str)
        or _HEX_64.fullmatch(execution_nonce_sha256) is None
    ):
        raise ValueError("Cargo cache generation provenance is incomplete")
    if target != layout.target(
        str(row["input_sha256"]),
        generation_id,
        version=cargo_output_layout.recorded_target_layout(row),
    ):
        raise ValueError("Cargo cache target is outside its identity-owned generation")
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
    layout = cargo_output_layout.CargoOutputLayout.create(
        result_root=result_root, declaration=provenance.get("cargo_output_root")
    )
    target = layout.target(
        digest,
        generation_id,
        version=cargo_output_layout.recorded_target_layout(provenance),
    )
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


def _output_lifetime(record: Mapping[str, object]) -> str:
    from tools.proof_queue_pkg.command_admission import parse_cargo_output_lifetime

    return parse_cargo_output_lifetime(record.get("cargo_output_lifetime", "retain"))


def _validate_owner(
    owner: Mapping[str, object], provenance: Mapping[str, object], target: Path
) -> None:
    cargo_output_layout.require_same_target_layout(owner, provenance)
    if not cargo_output_layout.same_root(
        owner.get("cargo_output_root"), provenance.get("cargo_output_root")
    ):
        raise ValueError("Cargo cache generation owner output root mismatch")
    if _output_lifetime(owner) != _output_lifetime(provenance):
        raise ValueError("Cargo cache generation owner output lifetime mismatch")
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


def _read_current_pointer(
    pointer: Path,
    *,
    target: Path,
    owner_path: Path,
    provenance: Mapping[str, object],
) -> dict[str, object] | None:
    if not pointer.exists():
        return None
    payload = loads_exact(pointer.read_text(encoding="utf-8"))
    if not isinstance(payload, dict) or payload.get("schema") != SCHEMA:
        raise ValueError("Cargo cache generation state is malformed")
    if payload.get("target") != str(target):
        return None
    cargo_output_layout.require_same_target_layout(payload, provenance)
    if (
        payload.get("generation_owner") != str(owner_path)
        or payload.get("run_id") != provenance.get("generation_run_id")
        or not cargo_output_layout.same_root(
            payload.get("cargo_output_root"), provenance.get("cargo_output_root")
        )
    ):
        raise ValueError("Cargo cache current pointer owner binding mismatch")
    return payload


def _update_pointer_if_current(
    pointer: Path,
    *,
    target: Path,
    state: str,
    owner_path: Path,
    provenance: Mapping[str, object],
    terminal_receipt: object | None = None,
) -> None:
    payload = _read_current_pointer(
        pointer, target=target, owner_path=owner_path, provenance=provenance
    )
    if payload is None:
        return
    updated: dict[str, object] = {
        "schema": SCHEMA,
        **cargo_output_layout.target_layout_fields(provenance),
        "state": state,
        "run_id": payload.get("run_id"),
        "target": str(target),
        "generation_owner": str(owner_path),
        **(
            {"cargo_output_root": payload["cargo_output_root"]}
            if "cargo_output_root" in payload
            else {}
        ),
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
        cargo_output_layout.validate_root(self.provenance.get("cargo_output_root"))
        current_pointer = _read_current_pointer(
            self.pointer,
            target=self.target,
            owner_path=self.owner_path,
            provenance=self.provenance,
        )
        if current_pointer is None:
            raise ValueError("active Cargo lease lost its current pointer")
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
                        **cargo_output_layout.target_layout_fields(self.provenance),
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
        cargo_output_layout.validate_root(self.provenance.get("cargo_output_root"))
        self._persist_publication(publication)
        if publication.get("state") == "sealed":
            _atomic_json(
                self.pointer,
                {
                    "schema": SCHEMA,
                    **cargo_output_layout.target_layout_fields(self.provenance),
                    "state": "sealed",
                    "seal": publication["seal"],
                    "target": str(self.target),
                    "run_id": self.provenance["generation_run_id"],
                    "generation_owner": str(self.owner_path),
                    **(
                        {"cargo_output_root": self.provenance["cargo_output_root"]}
                        if "cargo_output_root" in self.provenance
                        else {}
                    ),
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
    cargo_output_layout.require_same_target_layout(receipt, provenance)
    if not cargo_output_layout.same_root(
        receipt.get("cargo_output_root"), provenance.get("cargo_output_root")
    ):
        raise ValueError("Cargo cache terminal output root mismatch")
    if _output_lifetime(receipt) != _output_lifetime(provenance):
        raise ValueError("Cargo cache terminal output lifetime mismatch")
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
        **cargo_output_layout.target_layout_fields(provenance),
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
        _read_current_pointer(
            identity_root / "state.json",
            target=target,
            owner_path=owner_path,
            provenance=provenance,
        )
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
            provenance=provenance,
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
    # Revalidate under the identity lock as well as during path resolution.
    # Missing/remounted media is never evidence that a target was deleted.
    cargo_output_layout.validate_root(provenance.get("cargo_output_root"))
    owner = _read_owner(owner_path)
    _validate_owner(owner, provenance, target)
    _read_current_pointer(
        owner_path.parent.parent / "state.json",
        target=target,
        owner_path=owner_path,
        provenance=provenance,
    )
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


def _sealed_retirement_rejection(
    *,
    owner: Mapping[str, object],
    projection: Mapping[str, object],
    receipt: Mapping[str, object],
    publication: Mapping[str, object],
    target_present: bool,
    allow_passed: bool,
) -> str | None:
    """One sealed-retirement admission policy for inspection and locked apply."""
    if owner.get("lifecycle") != "terminal-sealed-retained":
        return "owner-lifecycle-not-retirable"
    if projection.get("state") != "terminal-sealed-retained":
        return "terminal-projection-not-retirable"
    if publication.get("state") != "sealed":
        return "publication-not-sealed"
    if (
        publication.get("purpose") != "preserved-candidate"
        or publication.get("reusable") is not False
        or publication.get("reuse_rejection_code") != "cargo-input-closure-unproven"
    ):
        return "publication-may-be-reusable"
    if receipt.get("process_cleanup_safe") is not True:
        return "terminal-receipt-not-cleanup-safe"
    terminal = receipt.get("queue_terminal")
    status = terminal.get("status") if isinstance(terminal, Mapping) else None
    if status not in _sealed_retirement_statuses(allow_passed=allow_passed):
        return "terminal-run-not-failed"
    if not target_present:
        return "target-absent"
    return None


def _sealed_retirement_statuses(*, allow_passed: bool) -> tuple[str, ...]:
    if not isinstance(allow_passed, bool):
        raise ValueError("Cargo sealed retirement allow_passed policy must be boolean")
    return ("failed", "passed") if allow_passed else ("failed",)


def sealed_retirement_policy(*, allow_passed: bool) -> dict[str, object]:
    """Project the exact admission policy into receipt and agent-facing evidence."""
    return {
        "allow_passed": allow_passed,
        "eligible_terminal_statuses": list(
            _sealed_retirement_statuses(allow_passed=allow_passed)
        ),
    }


def _terminal_status(receipt: Mapping[str, object]) -> object:
    terminal = receipt.get("queue_terminal")
    return terminal.get("status") if isinstance(terminal, Mapping) else None


def inspect_terminal_sealed_retirement(
    *,
    result_root: Path,
    provenance: Mapping[str, object],
    projection: Mapping[str, object],
    allow_passed: bool = False,
) -> dict[str, object]:
    """Inspect whether one policy-admitted sealed generation may be retired."""
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
    policy = sealed_retirement_policy(allow_passed=allow_passed)
    terminal_status = _terminal_status(immutable_receipt)
    rejection = _sealed_retirement_rejection(
        owner=owner,
        projection=projection,
        receipt=immutable_receipt,
        publication=publication,
        target_present=target_present,
        allow_passed=allow_passed,
    )
    return {
        "state": lifecycle,
        "target": str(target),
        "owner": str(owner_path),
        "target_present": target_present,
        "terminal_state": projection.get("state"),
        "publication_state": publication.get("state"),
        "process_cleanup_safe": immutable_receipt.get("process_cleanup_safe"),
        "retirement_policy": policy,
        "terminal_status": terminal_status,
        "retirement_eligible": rejection is None,
        "retirement_reason": rejection
        or (
            "terminal-sealed-passed-cleanup-safe"
            if terminal_status == "passed"
            else "terminal-sealed-failed-cleanup-safe"
        ),
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


def _preserve_terminal_output_evidence(
    *,
    target: Path,
    cas_root: Path,
    kind: str,
    publication: Mapping[str, object],
    terminal_receipt: Mapping[str, object],
    provenance: Mapping[str, object] | None = None,
    retirement_policy: Mapping[str, object] | None = None,
    terminal_status: object = None,
) -> tuple[dict[str, object], int | None, int]:
    """Capture immutable output evidence before either terminal payload transition."""
    output_before = command_identity._directory_manifest_identity(
        target, label="terminal Cargo output", strict_owned=True
    )
    if provenance is not None:
        seal = _read(publication.get("seal"), cas_root, _SEAL_KIND)
        cargo_output_layout.require_same_target_layout(seal, provenance)
        if (
            seal.get("input_sha256") != provenance.get("input_sha256")
            or seal.get("target") != str(target)
            or seal.get("manifest") != output_before
        ):
            raise ValueError("sealed Cargo output differs from its immutable seal")
    timings = _preserve_timings(target, cas_root=cas_root)
    output_after = command_identity._directory_manifest_identity(
        target, label="terminal Cargo output", strict_owned=True
    )
    if output_after != output_before:
        raise ValueError("terminal Cargo output changed during evidence capture")
    evidence_payload: dict[str, object] = {
        "target": str(target),
        "publication": dict(publication),
        "terminal_receipt": dict(terminal_receipt),
        "output_manifest": output_before,
        "timings": timings,
    }
    if provenance is not None:
        if retirement_policy is None:
            raise ValueError("sealed Cargo retirement policy is missing")
        evidence_payload.update(
            retirement_policy=dict(retirement_policy),
            terminal_status=terminal_status,
        )
    evidence = _artifact(cas_root, kind, **evidence_payload)
    files = output_before.get("files")
    file_rows = files if isinstance(files, list) else []
    return (
        evidence,
        (
            output_before.get("file_count")
            if isinstance(output_before.get("file_count"), int)
            else None
        ),
        sum(
            int(row.get("size", 0))
            for row in file_rows
            if isinstance(row, Mapping) and isinstance(row.get("size"), int)
        ),
    )


@dataclass(frozen=True)
class _TerminalOutputDisposition:
    eligible_lifecycle: str
    in_progress_lifecycle: str
    completed_lifecycle: str
    blocked_lifecycle: str
    projection_state: str
    publication_state: str
    evidence_kind: str
    detail_field: str
    action: str
    started_at_field: str
    completed_at_field: str
    blocked_at_field: str
    prior_failure_reason: str
    require_nonreusable_seal: bool = False


_UNSEALED_RECLAIM = _TerminalOutputDisposition(
    "terminal-unsealed-reclaimable",
    "reclaiming",
    "reclaimed",
    "reclaim-blocked",
    "terminal-unsealed-reclaimable",
    "unsealed",
    _RECLAIM_EVIDENCE_KIND,
    "reclaim",
    "reclaim",
    "reclaim_started_at",
    "reclaimed_at",
    "reclaim_blocked_at",
    "prior-reclaim-failure",
)
_SEALED_RETIREMENT = _TerminalOutputDisposition(
    "terminal-sealed-retained",
    "retiring-sealed",
    "retired-sealed",
    "retire-blocked",
    "terminal-sealed-retained",
    "sealed",
    _SEALED_RETIREMENT_EVIDENCE_KIND,
    "retirement",
    "retire",
    "retire_started_at",
    "retired_at",
    "retire_blocked_at",
    "prior-retirement-failure",
    True,
)


def _terminal_target_present(target: Path, provenance: Mapping[str, object]) -> bool:
    """Only typed absence on still-admitted media can complete disposal."""
    try:
        target.stat(follow_symlinks=False)
    except FileNotFoundError:
        cargo_output_layout.validate_root(provenance.get("cargo_output_root"))
        return False
    return True


def _transition_terminal_output(
    *,
    result_root: Path,
    provenance: Mapping[str, object],
    projection: Mapping[str, object],
    disposition: _TerminalOutputDisposition,
    timeout_s: float,
    allow_passed: bool = False,
) -> dict[str, object]:
    """One locked, receipt-preserving target-removal state machine."""
    identity_root, _generation, target, owner_path = _generation_paths(
        result_root, provenance
    )
    lock = _acquire_file_lock(
        identity_root / "target.lock",
        timeout_s=max(0.0, min(timeout_s, 30.0)),
        timeout_message=f"Cargo cache target is busy: {identity_root}",
    )
    not_state = "not-retirable" if disposition.action == "retire" else "not-reclaimable"
    try:
        owner, receipt, publication = _validate_terminal_generation_authority(
            result_root=result_root,
            provenance=provenance,
            projection=projection,
            target=target,
            owner_path=owner_path,
        )
        terminal_receipt = projection.get("terminal_receipt")
        assert isinstance(terminal_receipt, Mapping)
        lifecycle = owner.get("lifecycle")
        sealed = disposition.require_nonreusable_seal
        policy_fields: dict[str, object] = {}
        retirement_policy: dict[str, object] | None = None
        terminal_status: object = None
        if sealed:
            retirement_policy = sealed_retirement_policy(allow_passed=allow_passed)
            terminal_status = _terminal_status(receipt)
            policy_fields = {
                "retirement_policy": retirement_policy,
                "terminal_status": terminal_status,
            }
        if lifecycle == disposition.completed_lifecycle:
            _update_pointer_if_current(
                identity_root / "state.json",
                target=target,
                state=disposition.completed_lifecycle,
                owner_path=owner_path,
                provenance=provenance,
                terminal_receipt=terminal_receipt,
            )
            return {
                "state": disposition.completed_lifecycle,
                "target": str(target),
                "owner": str(owner_path),
                "idempotent": True,
                **policy_fields,
            }
        if lifecycle == disposition.blocked_lifecycle:
            return {
                "state": disposition.blocked_lifecycle,
                "target": str(target),
                "owner": str(owner_path),
                "reason": disposition.prior_failure_reason,
                **policy_fields,
            }
        if lifecycle == disposition.in_progress_lifecycle:
            if not _terminal_target_present(target, provenance):
                owner.update(
                    lifecycle=disposition.completed_lifecycle,
                    **{disposition.completed_at_field: _utc_now()},
                )
                _write_owner(owner_path, owner)
                _update_pointer_if_current(
                    identity_root / "state.json",
                    target=target,
                    state=disposition.completed_lifecycle,
                    owner_path=owner_path,
                    provenance=provenance,
                    terminal_receipt=terminal_receipt,
                )
                return {
                    "state": disposition.completed_lifecycle,
                    "target": str(target),
                    "owner": str(owner_path),
                    "idempotent": True,
                    **policy_fields,
                }
            owner.update(
                lifecycle=disposition.blocked_lifecycle,
                **{
                    disposition.blocked_at_field: _utc_now(),
                    f"{disposition.action}_error": f"prior {disposition.action} was interrupted; automatic retry refused",
                },
            )
            _write_owner(owner_path, owner)
            return {
                "state": disposition.blocked_lifecycle,
                "target": str(target),
                "owner": str(owner_path),
                "reason": f"prior-{disposition.action}-interrupted",
                **policy_fields,
            }
        if lifecycle != disposition.eligible_lifecycle:
            return {
                "state": not_state,
                "lifecycle": lifecycle,
                "target": str(target),
                "owner": str(owner_path),
                **policy_fields,
            }
        eligible = (
            projection.get("state") == disposition.projection_state
            and publication.get("state") == disposition.publication_state
            and receipt.get("process_cleanup_safe") is True
        )
        if sealed:
            eligible = (
                _sealed_retirement_rejection(
                    owner=owner,
                    projection=projection,
                    receipt=receipt,
                    publication=publication,
                    target_present=target.exists(),
                    allow_passed=allow_passed,
                )
                is None
            )
        if not eligible or not target.exists():
            return {
                "state": not_state,
                "lifecycle": lifecycle,
                "target": str(target),
                "owner": str(owner_path),
                **policy_fields,
            }
        try:
            evidence, file_count, size_bytes = _preserve_terminal_output_evidence(
                target=target,
                cas_root=result_root / "custody-cas",
                kind=disposition.evidence_kind,
                publication=publication,
                terminal_receipt=terminal_receipt,
                provenance=provenance if sealed else None,
                retirement_policy=retirement_policy,
                terminal_status=terminal_status,
            )
        except (OSError, ValueError) as exc:
            owner.update(
                lifecycle=disposition.blocked_lifecycle,
                **{
                    disposition.blocked_at_field: _utc_now(),
                    f"{disposition.action}_error": f"{type(exc).__name__}: {exc}",
                },
            )
            _write_owner(owner_path, owner)
            return {
                "state": disposition.blocked_lifecycle,
                "target": str(target),
                "owner": str(owner_path),
                "reason": "evidence-preservation-failed",
                "error": owner[f"{disposition.action}_error"],
                **policy_fields,
            }
        detail: dict[str, object] = {
            "evidence": evidence,
            "file_count": file_count,
            "size_bytes": size_bytes,
        }
        if retirement_policy is not None:
            detail.update(
                retirement_policy=retirement_policy,
                terminal_status=terminal_status,
            )
        owner.update(
            lifecycle=disposition.in_progress_lifecycle,
            **{
                disposition.started_at_field: _utc_now(),
                disposition.detail_field: detail,
            },
        )
        _write_owner(owner_path, owner)
        # Revalidate selected media after evidence preservation, immediately
        # before entering the existing identity-locked deletion primitive.
        cargo_output_layout.validate_root(provenance.get("cargo_output_root"))
        deleted, error = delete_path(target)
        if not deleted:
            owner.update(
                lifecycle=disposition.blocked_lifecycle,
                **{
                    disposition.blocked_at_field: _utc_now(),
                    f"{disposition.action}_error": error,
                },
            )
            _write_owner(owner_path, owner)
            _update_pointer_if_current(
                identity_root / "state.json",
                target=target,
                state=disposition.blocked_lifecycle,
                owner_path=owner_path,
                provenance=provenance,
                terminal_receipt=terminal_receipt,
            )
            return {
                "state": disposition.blocked_lifecycle,
                "target": str(target),
                "owner": str(owner_path),
                "reason": "delete-failed",
                "error": error,
                **policy_fields,
            }
        owner.update(
            lifecycle=disposition.completed_lifecycle,
            **{disposition.completed_at_field: _utc_now()},
        )
        _write_owner(owner_path, owner)
        _update_pointer_if_current(
            identity_root / "state.json",
            target=target,
            state=disposition.completed_lifecycle,
            owner_path=owner_path,
            provenance=provenance,
            terminal_receipt=terminal_receipt,
        )
        detail = owner[disposition.detail_field]
        assert isinstance(detail, Mapping)
        return {
            "state": disposition.completed_lifecycle,
            "target": str(target),
            "owner": str(owner_path),
            "file_count": detail.get("file_count"),
            "size_bytes": detail.get("size_bytes"),
            "evidence": detail.get("evidence"),
            **policy_fields,
        }
    finally:
        _release_file_lock(lock)


def reclaim_terminal_unsealed(
    *,
    result_root: Path,
    provenance: Mapping[str, object],
    projection: Mapping[str, object],
    timeout_s: float = 30.0,
) -> dict[str, object]:
    """Reclaim one terminal unsealed target through shared disposition custody."""
    return _transition_terminal_output(
        result_root=result_root,
        provenance=provenance,
        projection=projection,
        disposition=_UNSEALED_RECLAIM,
        timeout_s=timeout_s,
    )


def retire_terminal_sealed(
    *,
    result_root: Path,
    provenance: Mapping[str, object],
    projection: Mapping[str, object],
    timeout_s: float = 30.0,
    allow_passed: bool = False,
) -> dict[str, object]:
    """Retire one policy-admitted non-reusable sealed target through custody."""
    return _transition_terminal_output(
        result_root=result_root,
        provenance=provenance,
        projection=projection,
        disposition=_SEALED_RETIREMENT,
        timeout_s=timeout_s,
        allow_passed=allow_passed,
    )


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
    cargo_output_lifetime: str = "retain",
    cargo_output_root: Mapping[str, object] | None = None,
) -> CargoCacheLease:
    cargo_output_lifetime = _output_lifetime(
        {"cargo_output_lifetime": cargo_output_lifetime}
    )
    started = time.perf_counter()
    if _HEX_64.fullmatch(execution_nonce_sha256) is None:
        raise ValueError("Cargo cache generation requires an execution nonce digest")
    layout = cargo_output_layout.CargoOutputLayout.create(
        result_root=result_root, declaration=cargo_output_root, source_root=source_root
    )
    if outputs.external_placement != (cargo_output_root is not None):
        raise ValueError("Cargo output roles differ from admitted placement")
    capacity_admission = disk_capacity.require_build_capacity(
        layout.capacity_paths(),
        env=env,
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
            cargo_output_layout.recorded_target_layout(previous)
            if previous.get("state") == "sealed":
                seed = previous.get("seal")
                target = Path(str(previous.get("target")))
                previous_owner = Path(str(previous.get("generation_owner")))
                previous_layout = cargo_output_layout.CargoOutputLayout.create(
                    result_root=result_root,
                    declaration=previous.get("cargo_output_root"),
                )
                if (
                    previous_owner.parent.parent != root
                    or previous_owner.name != "owner.json"
                    or target
                    != previous_layout.target(
                        digest,
                        previous_owner.parent.name,
                        version=cargo_output_layout.recorded_target_layout(previous),
                    )
                ):
                    raise ValueError("Cargo cache seal target escaped its generation")
                raise CargoInputClosureUnproven(candidate=target, seal=seed)
            elif previous.get("state") in {
                "pending",
                "terminal-unsealed-reclaimable",
                "terminal-indeterminate-retained",
                "reclaimed",
                "reclaim-blocked",
                "retired-sealed",
                "retire-blocked",
            }:
                cold_reason = (
                    "prior-generation-retired-sealed"
                    if previous.get("state") == "retired-sealed"
                    else "prior-generation-unsealed"
                )
            else:
                raise ValueError("Cargo cache generation state is invalid")
        generation = root / secrets.token_hex(8)
        generation.mkdir(exist_ok=False)
        file_publication.fsync_directory(root)
        target = layout.target(digest, generation.name)
        owner_path = generation / "owner.json"
        file_publication.resolve_owned_path(owner_path)
        inputs = _artifact(cas_root, _INPUT_KIND, identity=identity)
        provenance: dict[str, object] = {
            "schema": SCHEMA,
            "cargo_target_layout": cargo_output_layout.TARGET_LAYOUT,
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
            "cargo_output_lifetime": cargo_output_lifetime,
            **(
                {"cargo_output_root": dict(cargo_output_root)}
                if cargo_output_root is not None
                else {}
            ),
            "execution_nonce_sha256": execution_nonce_sha256,
        }
        owner: dict[str, object] = {
            "schema": GENERATION_SCHEMA,
            "cargo_target_layout": cargo_output_layout.TARGET_LAYOUT,
            "cargo_output_lifetime": cargo_output_lifetime,
            **(
                {"cargo_output_root": dict(cargo_output_root)}
                if cargo_output_root is not None
                else {}
            ),
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
        custody_cas._durable_makedirs(target.parent)
        # The compact address is outside the exclusively created metadata
        # directory. Even an empty pre-existing payload has no fresh custody.
        target.mkdir(exist_ok=False)
        file_publication.fsync_directory(target.parent)
        bound_environment = outputs.bind(env, target=target)
        validate_prelaunch(
            provenance,
            cargo_output_lifetime=cargo_output_lifetime,
            cargo_output_root=cargo_output_root,
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
                        "cargo_target_layout": cargo_output_layout.TARGET_LAYOUT,
                        "state": "pending",
                        "run_id": run_id,
                        "target": str(target),
                        "generation_owner": str(owner_path),
                        **(
                            {"cargo_output_root": dict(cargo_output_root)}
                            if cargo_output_root is not None
                            else {}
                        ),
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
