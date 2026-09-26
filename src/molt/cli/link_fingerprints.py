"""One input fingerprint and exact output-family receipt for every final link."""

from __future__ import annotations

import hashlib
import json
import os
from pathlib import Path
from typing import Any, Mapping, Sequence, cast

from molt.cli.artifact_state import _artifact_state_path
from molt.cli.atomic_io import _atomic_write_json
from molt.cli.runtime_fingerprints import (
    _artifact_needs_rebuild,
    _fingerprint_payload,
    _runtime_fingerprint_payload_is_valid,
    _stored_fingerprint_matches_source_metadata,
)
from molt.cli.runtime_identity_schema import RUNTIME_ARTIFACT_METADATA_MAX_BYTES
from molt.cli.static_archive_identity import (
    StaticArchiveIdentityError,
    artifact_content_identity,
    validate_artifact_content_identity,
)
from molt.file_hashing import _hash_source_tree_metadata, _iter_source_fingerprint_files
from molt.exact_json import read_exact
from molt.link_outputs import validate_link_output_paths


def _link_receipt_is_valid(payload: object) -> bool:
    if not isinstance(payload, dict) or set(payload) != {
        "schema",
        "fingerprint",
        "outputs",
    }:
        return False
    if payload["schema"] != "molt.final-link.v1":
        return False
    if not _runtime_fingerprint_payload_is_valid(payload["fingerprint"]):
        return False
    outputs = payload["outputs"]
    if not isinstance(outputs, dict) or not outputs:
        return False
    for role, output in outputs.items():
        if not isinstance(role, str) or not role or not isinstance(output, dict):
            return False
        if set(output) != {"path", "identity"}:
            return False
        if (
            not isinstance(output["path"], str)
            or not Path(output["path"]).is_absolute()
        ):
            return False
        try:
            validate_artifact_content_identity(output["identity"])
        except ValueError:
            return False
    return True


def _read_link_fingerprint(path: Path) -> dict[str, Any] | None:
    try:
        payload = read_exact(
            path,
            max_bytes=RUNTIME_ARTIFACT_METADATA_MAX_BYTES,
            label="final link receipt",
        )
    except (OSError, ValueError, UnicodeError):
        return None
    return payload if _link_receipt_is_valid(payload) else None


def _link_fingerprint_path(
    project_root: Path,
    artifact: Path,
    profile: str,
    target_triple: str | None,
) -> Path:
    target = (target_triple or "native").replace(os.sep, "_").replace(":", "_")
    return _artifact_state_path(
        project_root,
        artifact,
        subdir="link_fingerprints",
        stem_suffix=f"{profile}.{target}",
        extension="fingerprint",
    )


def _link_fingerprint(
    *,
    project_root: Path,
    inputs: list[Path],
    link_cmd: list[str],
    tool_facts: Sequence[Mapping[str, object]] = (),
    stored_fingerprint: dict[str, Any] | None = None,
) -> dict[str, str | None] | None:
    inputs_meta = _hash_source_tree_metadata(inputs, project_root)
    inputs_digest = inputs_meta[0] if inputs_meta is not None else None
    tool_identity = json.dumps(list(tool_facts), sort_keys=True, separators=(",", ":"))
    meta = "\0".join((*link_cmd, tool_identity))
    meta_digest = hashlib.sha256(meta.encode("utf-8")).hexdigest()
    if _stored_fingerprint_matches_source_metadata(
        stored_fingerprint,
        inputs_digest=inputs_digest,
        rustc=None,
        meta_digest=meta_digest,
    ):
        assert stored_fingerprint is not None
        return {
            "hash": cast(str, stored_fingerprint.get("hash")),
            "rustc": None,
            "inputs_digest": inputs_digest,
            "meta_digest": meta_digest,
        }
    hasher = hashlib.sha256()
    hasher.update(meta.encode("utf-8"))
    hasher.update(b"\0")
    try:
        for path in sorted(inputs, key=lambda item: str(item)):
            for item in _iter_source_fingerprint_files(path):
                try:
                    identity_path = item.relative_to(project_root)
                except ValueError:
                    identity_path = item
                hasher.update(str(identity_path).encode("utf-8"))
                hasher.update(b"\0")
                hasher.update(
                    json.dumps(
                        artifact_content_identity(item),
                        sort_keys=True,
                        separators=(",", ":"),
                    ).encode("utf-8")
                )
                hasher.update(b"\0")
    except (OSError, StaticArchiveIdentityError):
        return None
    return {
        "hash": hasher.hexdigest(),
        "rustc": None,
        "inputs_digest": inputs_digest,
        "meta_digest": meta_digest,
    }


def _link_outputs_match(
    *,
    outputs: Mapping[str, Path],
    fingerprint: dict[str, Any] | None,
    stored_fingerprint: dict[str, Any] | None,
) -> bool:
    """Reuse only the exact published family, never file presence or format alone."""
    if not outputs or not _link_receipt_is_valid(stored_fingerprint):
        return False
    assert stored_fingerprint is not None
    recorded = stored_fingerprint["outputs"]
    if recorded.keys() != outputs.keys():
        return False
    # The common artifact key check retains the version/build-key contract.
    # Output identity is independent: timestamps are not proof of these bytes.
    if _artifact_needs_rebuild(
        next(iter(outputs.values())), fingerprint, stored_fingerprint["fingerprint"]
    ):
        return False
    try:
        validate_link_output_paths(outputs)
        for role, path in outputs.items():
            if str(path.resolve()) != recorded[role]["path"]:
                return False
            if artifact_content_identity(path) != recorded[role]["identity"]:
                return False
    except (OSError, ValueError):
        return False
    return True


def _write_link_fingerprint_if_needed(
    *,
    link_skipped: bool,
    link_fingerprint: dict[str, Any] | None,
    link_fingerprint_path: Path,
    outputs: Mapping[str, Path],
) -> str | None:
    """Bind the input key to final bytes after linking, optimization and publication."""
    if link_skipped or link_fingerprint is None:
        return None
    try:
        validate_link_output_paths(outputs, inputs=(link_fingerprint_path,))
        identities = {
            role: {
                "path": str(path.resolve()),
                "identity": artifact_content_identity(path),
            }
            for role, path in sorted(outputs.items())
        }
        link_fingerprint_path.parent.mkdir(parents=True, exist_ok=True)
        _atomic_write_json(
            link_fingerprint_path,
            {
                "schema": "molt.final-link.v1",
                "fingerprint": _fingerprint_payload(link_fingerprint),
                "outputs": identities,
            },
            indent=2,
        )
    except (OSError, ValueError) as exc:
        return f"failed to write link fingerprint metadata: {exc}"
    return None
