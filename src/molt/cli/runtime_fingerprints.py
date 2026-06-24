from __future__ import annotations

import json
from pathlib import Path
from typing import Any, Literal

from molt.cli.file_hashing import _sha256_file
from molt.cli.json_cache import _read_cached_json_object, _write_cached_json_object


def _read_runtime_fingerprint(path: Path) -> dict[str, Any] | None:
    payload = _read_cached_json_object(path)
    if payload is not None:
        data = payload
    else:
        try:
            text = path.read_text().strip()
        except OSError:
            return None
        if not text:
            return None
        try:
            json.loads(text)
        except json.JSONDecodeError:
            return {"hash": text, "rustc": None, "inputs_digest": None}
        return None
    hash_value = data.get("hash")
    if not isinstance(hash_value, str) or not hash_value:
        return None
    rustc_value = data.get("rustc")
    inputs_digest = data.get("inputs_digest")
    meta_digest = data.get("meta_digest")
    if (
        (rustc_value is None or isinstance(rustc_value, str))
        and (inputs_digest is None or isinstance(inputs_digest, str))
        and (meta_digest is None or isinstance(meta_digest, str))
    ):
        return data
    if rustc_value is not None and not isinstance(rustc_value, str):
        rustc_value = None
    if inputs_digest is not None and not isinstance(inputs_digest, str):
        inputs_digest = None
    if meta_digest is not None and not isinstance(meta_digest, str):
        meta_digest = None
    return {
        "hash": hash_value,
        "rustc": rustc_value,
        "inputs_digest": inputs_digest,
        "meta_digest": meta_digest,
    }


def _write_runtime_fingerprint(
    path: Path,
    fingerprint: dict[str, str | None],
    *,
    artifact: Path | None = None,
) -> None:
    payload = {
        "version": 2,
        "hash": fingerprint.get("hash"),
        "rustc": fingerprint.get("rustc"),
        "inputs_digest": fingerprint.get("inputs_digest"),
        "meta_digest": fingerprint.get("meta_digest"),
    }
    if artifact is not None:
        payload["artifact_sha256"] = _sha256_file(artifact)
    _write_cached_json_object(path, payload)


def _artifact_needs_rebuild(
    artifact: Path,
    fingerprint: dict[str, str | None] | None,
    stored_fingerprint: dict[str, str | None] | None,
) -> bool:
    try:
        artifact.stat()
    except OSError:
        return True
    if not _artifact_content_looks_valid(artifact):
        return True
    if fingerprint is None or stored_fingerprint is None:
        return True
    if stored_fingerprint.get("hash") != fingerprint.get("hash"):
        return True
    meta_digest = fingerprint.get("meta_digest")
    if meta_digest:
        stored_meta_digest = stored_fingerprint.get("meta_digest")
        if stored_meta_digest is None or stored_meta_digest != meta_digest:
            return True
    rustc = fingerprint.get("rustc")
    if rustc:
        stored_rustc = stored_fingerprint.get("rustc")
        return stored_rustc is None or stored_rustc != rustc
    return False


def _runtime_artifact_fingerprint_matches(
    artifact: Path,
    fingerprint: dict[str, str | None] | None,
    fingerprint_path: Path,
    *,
    require_artifact_digest: bool,
) -> bool:
    stored_fingerprint = _read_runtime_fingerprint(fingerprint_path)
    if _artifact_needs_rebuild(artifact, fingerprint, stored_fingerprint):
        return False
    if not require_artifact_digest:
        return True
    if stored_fingerprint is None:
        return False
    artifact_digest = stored_fingerprint.get("artifact_sha256")
    if not isinstance(artifact_digest, str) or not artifact_digest:
        return False
    try:
        return _sha256_file(artifact) == artifact_digest
    except OSError:
        return False


def _is_valid_static_library_artifact(path: Path) -> bool:
    if path.suffix not in {".a", ".lib"}:
        return True
    try:
        with path.open("rb") as handle:
            return handle.read(8) == b"!<arch>\n"
    except OSError:
        return False


def _artifact_content_looks_valid(path: Path) -> bool:
    if path.suffix in {".a", ".lib"}:
        return _is_valid_static_library_artifact(path)
    if path.suffix == ".wasm":
        return _is_valid_wasm_binary(path)
    return True


def _is_valid_wasm_binary(path: Path) -> bool:
    return _inspect_wasm_binary(path) == "valid"


def _inspect_wasm_binary(path: Path) -> Literal["missing", "invalid", "valid"]:
    try:
        with path.open("rb") as handle:
            magic = handle.read(8)
    except OSError:
        return "missing"
    if magic != b"\x00asm\x01\x00\x00\x00":
        return "invalid"
    return "valid"
