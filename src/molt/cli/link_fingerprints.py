"""One input fingerprint and exact output-family receipt for every final link."""

from __future__ import annotations

import hashlib
import json
from dataclasses import dataclass
from pathlib import Path
from typing import Any, Mapping, Sequence, cast

from molt import artifact_publication
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
from molt.exact_json import encode_exact, read_exact
from molt.link_outputs import validate_link_output_paths


def _link_receipt_is_valid(payload: object) -> bool:
    if not isinstance(payload, dict) or set(payload) != {
        "schema",
        "fingerprint",
        "outputs",
    }:
        return False
    if payload["schema"] != "molt.final-link.v2":
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


def _link_fingerprint_path(artifact: Path) -> Path:
    return artifact_publication.publication_receipt_path(artifact)


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
    receipt_path: Path,
) -> bool:
    """Reuse only the exact published family, never file presence or format alone."""
    if not outputs or fingerprint is None:
        return False
    try:
        with artifact_publication.publication_locks((*outputs.values(), receipt_path)):
            stored_fingerprint = _read_link_fingerprint(receipt_path)
            if stored_fingerprint is None:
                return False
            recorded = stored_fingerprint["outputs"]
            if recorded.keys() != outputs.keys():
                return False
            if _artifact_needs_rebuild(
                next(iter(outputs.values())),
                fingerprint,
                stored_fingerprint["fingerprint"],
            ):
                return False
            validate_link_output_paths(outputs)
            for role, path in outputs.items():
                if str(path.resolve()) != recorded[role]["path"]:
                    return False
                if artifact_content_identity(path) != recorded[role]["identity"]:
                    return False
    except (OSError, ValueError, RuntimeError):
        return False
    return True


@dataclass(frozen=True)
class FinalLinkReceiptRequest:
    """The producer's input key and receipt destination, never observed outputs."""

    path: Path
    fingerprint: dict[str, Any]

    def __post_init__(self) -> None:
        if not self.path.is_absolute():
            raise ValueError("final link receipt path must be absolute")
        if not _runtime_fingerprint_payload_is_valid(self.fingerprint):
            raise ValueError("invalid final link input fingerprint")

    @classmethod
    def from_fingerprint(
        cls, path: Path, fingerprint: dict[str, Any] | None
    ) -> FinalLinkReceiptRequest | None:
        return (
            cls(path.resolve(), _fingerprint_payload(fingerprint))
            if fingerprint
            else None
        )

    def encode(self) -> bytes:
        return encode_exact(
            {
                "schema": "molt.final-link-request.v1",
                "path": str(self.path),
                "fingerprint": self.fingerprint,
            },
            indent=None,
        )

    @classmethod
    def read(cls, path: Path) -> FinalLinkReceiptRequest:
        payload = read_exact(
            path,
            max_bytes=RUNTIME_ARTIFACT_METADATA_MAX_BYTES,
            label="final link receipt request",
        )
        if (
            not isinstance(payload, dict)
            or set(payload) != {"schema", "path", "fingerprint"}
            or payload["schema"] != "molt.final-link-request.v1"
            or not isinstance(payload["path"], str)
        ):
            raise ValueError("invalid final link receipt request")
        return cls(Path(payload["path"]), payload["fingerprint"])


def publish_link_outputs(
    candidates: Mapping[str, tuple[Path, Path]],
    *,
    receipt: FinalLinkReceiptRequest | None = None,
    removals: tuple[Path, ...] = (),
    retire_previous_outputs_under: Path | None = None,
) -> None:
    """Publish finalized private bytes and their input binding as one generation.

    Identities are taken from the producer's candidates, never from destinations
    that another producer can replace. The receipt joins the same locked,
    rollback-protected publication set, after all output roles.
    """
    outputs = {role: final for role, (_, final) in candidates.items()}
    stages = tuple(stage for stage, _ in candidates.values())
    validate_link_output_paths(outputs, inputs=stages)
    pairs = list(candidates.values())
    if retire_previous_outputs_under is not None and receipt is None:
        raise ValueError("retiring a prior link generation requires its receipt")
    retirement_root = (
        retire_previous_outputs_under.resolve(strict=True)
        if retire_previous_outputs_under is not None
        else None
    )

    def previous_outputs(locked: frozenset[Path]) -> tuple[Path, ...]:
        assert receipt is not None and retirement_root is not None
        if not receipt.path.exists():
            return ()
        previous = _read_link_fingerprint(receipt.path)
        if previous is None:
            # An invalid/obsolete cache record forces rebuilding; it cannot
            # authorize deletion of any additional path. The new current-schema
            # receipt supersedes it together with the explicit output family.
            return ()
        current = {path.resolve() for path in outputs.values()}
        retired = []
        for output in previous["outputs"].values():
            path = Path(output["path"])
            if path in current:
                continue
            if (
                not path.is_relative_to(retirement_root)
                or path.resolve() != path
                or path == receipt.path
            ):
                raise ValueError(
                    f"prior link output is outside retirement custody: {path}"
                )
            if (
                path.parent in locked
                and path.exists()
                and artifact_content_identity(path) != output["identity"]
            ):
                raise ValueError(
                    f"prior link output changed outside publication: {path}"
                )
            retired.append(path)
        return tuple(retired)

    receipt_stage: Path | None = None
    try:
        if receipt is not None:
            if "receipt" in outputs:
                raise ValueError("receipt is a reserved final link output role")
            validate_link_output_paths(
                {**outputs, "receipt": receipt.path}, inputs=stages
            )
            payload = {
                "schema": "molt.final-link.v2",
                "fingerprint": receipt.fingerprint,
                "outputs": {
                    role: {
                        "path": str(final.resolve()),
                        "identity": artifact_content_identity(
                            stage, logical_path=final
                        ),
                    }
                    for role, (stage, final) in sorted(candidates.items())
                },
            }
            receipt_stage = artifact_publication.staged_output_path(
                receipt.path, purpose="link-receipt"
            )
            receipt_stage.write_bytes(encode_exact(payload, indent=2))
            pairs.append((receipt_stage, receipt.path))
        artifact_publication.publish_validated_outputs(
            pairs,
            removals=removals,
            select_removals=previous_outputs if retirement_root is not None else None,
        )
    finally:
        if receipt_stage is not None:
            artifact_publication.discard_staged_output(receipt_stage)
