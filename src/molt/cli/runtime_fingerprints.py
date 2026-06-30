from __future__ import annotations

import functools
import hashlib
import json
import os
from pathlib import Path
from typing import Any, cast

from molt.cli.capability_spec import _dedupe_preserve_order
from molt.cli.compiler_metadata import _rustc_version
from molt.cli.file_hashing import _sha256_file
from molt.cli.json_cache import _read_cached_json_object, _write_cached_json_object
from molt.wasm_artifact import is_valid_wasm_binary


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


def _hash_runtime_file(path: Path, root: Path, hasher: Any) -> None:
    try:
        rel_path = path.relative_to(root)
        rel_bytes = str(rel_path).encode("utf-8")
    except ValueError:
        rel_bytes = str(path).encode("utf-8")
    hasher.update(rel_bytes)
    hasher.update(b"\0")
    with path.open("rb") as handle:
        while True:
            chunk = handle.read(65536)
            if not chunk:
                break
            hasher.update(chunk)
    hasher.update(b"\0")


_SOURCE_FINGERPRINT_IGNORED_DIRS = frozenset({"__pycache__"})
_SOURCE_FINGERPRINT_IGNORED_SUFFIXES = frozenset({".pyc", ".pyo"})


def _source_fingerprint_should_skip(path: Path) -> bool:
    return (
        any(part in _SOURCE_FINGERPRINT_IGNORED_DIRS for part in path.parts)
        or path.suffix in _SOURCE_FINGERPRINT_IGNORED_SUFFIXES
    )


def _source_fingerprint_files(path: Path) -> list[Path]:
    if path.is_dir():
        return [
            item
            for item in sorted(path.rglob("*"), key=lambda p: str(p))
            if item.is_file() and not _source_fingerprint_should_skip(item)
        ]
    if path.exists() and path.is_file() and not _source_fingerprint_should_skip(path):
        return [path]
    return []


def _hash_source_tree_metadata(
    paths: list[Path],
    root: Path,
) -> tuple[str, int] | None:
    hasher = hashlib.sha256()
    file_count = 0
    try:
        for path in sorted(paths, key=lambda p: str(p)):
            for item in _source_fingerprint_files(path):
                try:
                    stat = item.stat()
                except OSError:
                    return None
                try:
                    rel_path = item.relative_to(root)
                    rel_text = str(rel_path)
                except ValueError:
                    rel_text = str(item)
                hasher.update(rel_text.encode("utf-8"))
                hasher.update(b"\0")
                hasher.update(str(stat.st_size).encode("utf-8"))
                hasher.update(b"\0")
                hasher.update(str(stat.st_mtime_ns).encode("utf-8"))
                hasher.update(b"\0")
                hasher.update(str(stat.st_ctime_ns).encode("utf-8"))
                hasher.update(b"\0")
                file_count += 1
    except OSError:
        return None
    return hasher.hexdigest(), file_count


def _stored_fingerprint_matches_source_metadata(
    stored_fingerprint: dict[str, Any] | None,
    *,
    inputs_digest: str | None,
    rustc: str | None,
    meta_digest: str | None,
) -> bool:
    if stored_fingerprint is None or not inputs_digest:
        return False
    if stored_fingerprint.get("inputs_digest") != inputs_digest:
        return False
    if meta_digest is not None:
        stored_meta = stored_fingerprint.get("meta_digest")
        if stored_meta is None or stored_meta != meta_digest:
            return False
    if rustc:
        stored_rustc = stored_fingerprint.get("rustc")
        if stored_rustc is None or stored_rustc != rustc:
            return False
    return isinstance(stored_fingerprint.get("hash"), str) and bool(
        stored_fingerprint.get("hash")
    )


def _runtime_fingerprint(
    project_root: Path,
    *,
    cargo_profile: str,
    target_triple: str | None,
    rustflags: str,
    runtime_features: tuple[str, ...] = (),
    stored_fingerprint: dict[str, Any] | None = None,
) -> dict[str, str | None] | None:
    feature_list = tuple(_dedupe_preserve_order(sorted(runtime_features)))
    meta = f"profile:{cargo_profile}\ntarget:{target_triple or 'native'}\n"
    meta += "build-schema:runtime-feature-profile-v3\n"
    meta += f"rustflags:{rustflags}\n"
    meta += f"features:{','.join(feature_list)}\n"
    meta_digest = hashlib.sha256(meta.encode("utf-8")).hexdigest()
    source_paths = _runtime_source_paths(project_root)
    rustc_info = _rustc_version()
    inputs_meta = _hash_source_tree_metadata(source_paths, project_root)
    inputs_digest = inputs_meta[0] if inputs_meta is not None else None
    if _stored_fingerprint_matches_source_metadata(
        stored_fingerprint,
        inputs_digest=inputs_digest,
        rustc=rustc_info,
        meta_digest=meta_digest,
    ):
        assert stored_fingerprint is not None
        return {
            "hash": cast(str, stored_fingerprint.get("hash")),
            "rustc": rustc_info,
            "inputs_digest": inputs_digest,
            "meta_digest": meta_digest,
        }

    hasher = hashlib.sha256()
    hasher.update(meta.encode("utf-8"))
    try:
        for path in sorted(source_paths, key=lambda p: str(p)):
            if path.is_dir():
                for item in sorted(path.rglob("*"), key=lambda p: str(p)):
                    if item.is_file():
                        _hash_runtime_file(item, project_root, hasher)
            elif path.exists():
                _hash_runtime_file(path, project_root, hasher)
    except OSError:
        return None
    return {
        "hash": hasher.hexdigest(),
        "rustc": rustc_info,
        "inputs_digest": inputs_digest,
        "meta_digest": meta_digest,
    }


@functools.lru_cache(maxsize=128)
def _runtime_source_paths_cached(project_root_str: str) -> tuple[Path, ...]:
    project_root = Path(project_root_str)
    runtime_root = project_root / "runtime"
    paths: list[Path] = [
        project_root / "runtime/molt-runtime/src",
        project_root / "runtime/molt-runtime/Cargo.toml",
        project_root / "runtime/molt-runtime/build.rs",
        project_root / "runtime/molt-cpython-abi/src",
        project_root / "runtime/molt-cpython-abi/shims",
        project_root / "runtime/molt-cpython-abi/Cargo.toml",
        project_root / "runtime/molt-cpython-abi/build.rs",
        project_root / "runtime/molt-obj-model/src",
        project_root / "runtime/molt-obj-model/Cargo.toml",
        project_root / "runtime/molt-obj-model/build.rs",
    ]
    for crate_dir in sorted(runtime_root.glob("molt-runtime-*")):
        paths.extend(
            (
                crate_dir / "src",
                crate_dir / "Cargo.toml",
                crate_dir / "build.rs",
            )
        )
    paths.extend(
        (
            project_root / "runtime/Cargo.toml",
            project_root / "runtime/Cargo.lock",
            project_root / "Cargo.toml",
            project_root / "Cargo.lock",
        )
    )
    deduped: list[Path] = []
    seen: set[Path] = set()
    for path in paths:
        if path in seen:
            continue
        seen.add(path)
        deduped.append(path)
    return tuple(deduped)


def _runtime_source_paths(project_root: Path) -> list[Path]:
    return list(_runtime_source_paths_cached(os.fspath(project_root)))


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
        return is_valid_wasm_binary(path)
    return True
