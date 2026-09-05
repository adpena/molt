"""Content-addressed custody for source-extension compilation inputs."""

from __future__ import annotations

import os
from collections.abc import Mapping, Sequence
from pathlib import Path
from pathlib import PurePosixPath
from typing import Any, cast

from molt.cli.atomic_io import _atomic_copy_file
from molt.cli.source_extension_manifest_codec import (
    _expand_source_extension_manifest_authorities,
    _manifest_dependencies,
)
from molt.cli.source_extension_object_closure import (
    SourceExtensionObjectClosureError,
    _require_canonical_sha256,
    finalize_source_extension_object_closure,
)
from molt.file_hashing import _sha256_bytes, _sha256_file


_CUSTODY_ROOT = Path("provenance") / "compiled-inputs" / "sha256"
_CUSTODY_LAYOUT = "provenance/compiled-inputs/sha256/{prefix}/{sha256}"


class SourceExtensionInputCustodyError(ValueError):
    """A compilation input is missing, corrupt, or ambiguously described."""


def source_extension_input_custody_manifest() -> dict[str, object]:
    """Return the canonical path-neutral source-input custody descriptor."""

    return {
        "schema_version": 1,
        "algorithm": "sha256",
        "layout": _CUSTODY_LAYOUT,
    }


def validate_source_extension_manifest_input_custody(
    manifest: Mapping[str, Any],
) -> None:
    """Require every declared input to use its digest-derived relative address."""

    descriptor = manifest.get("input_custody")
    if (
        not isinstance(descriptor, Mapping)
        or type(descriptor.get("schema_version")) is not int
        or descriptor != source_extension_input_custody_manifest()
    ):
        raise SourceExtensionInputCustodyError(
            "source-extension manifest has no canonical input_custody authority"
        )
    rows = source_extension_manifest_input_rows(manifest)
    object_sources = [
        raw_path for field, raw_path, _digest in rows if field.endswith(".source")
    ]
    if manifest.get("sources") != object_sources:
        raise SourceExtensionInputCustodyError(
            "source-extension manifest sources must exactly project object closure order"
        )
    for field, raw_path, digest in rows:
        if "\\" in raw_path:
            raise SourceExtensionInputCustodyError(
                f"source-extension custody path is not portable: {field}"
            )
        path = PurePosixPath(raw_path)
        if path.is_absolute() or path.as_posix() != raw_path:
            raise SourceExtensionInputCustodyError(
                f"source-extension custody path is not canonical: {field}"
            )
        suffix = (*_CUSTODY_ROOT.parts, digest[:2], digest)
        prefix = path.parts[: -len(suffix)]
        if path.parts[-len(suffix) :] != suffix or any(part != ".." for part in prefix):
            raise SourceExtensionInputCustodyError(
                f"source-extension custody path does not match its digest: {field}"
            )


def require_source_extension_sha256(value: object, *, field: str) -> str:
    try:
        return _require_canonical_sha256(value, field=field)
    except SourceExtensionObjectClosureError as exc:
        raise SourceExtensionInputCustodyError(str(exc)) from exc


def source_extension_input_custody_path(sha256: str) -> Path:
    """Return the path-neutral custody address for one input digest."""

    digest = require_source_extension_sha256(sha256, field="input digest")
    return _CUSTODY_ROOT / digest[:2] / digest


def source_extension_manifest_input_references(
    *,
    output_manifest_path: Path,
    publish_root: Path,
    staged_inputs: Mapping[Path, Path],
) -> dict[Path, str]:
    """Project staged input custody into one manifest's relative namespace."""

    return {
        source: os.path.relpath(
            publish_root / relative,
            output_manifest_path.parent,
        ).replace("\\", "/")
        for source, relative in staged_inputs.items()
    }


def _manifest_input_candidates(
    raw_path: str,
    *,
    manifest_path: Path | None,
    input_roots: Sequence[tuple[str, Path]],
) -> tuple[tuple[Path, ...], list[str]]:
    normalized = raw_path.replace("\\", "/")
    candidates: list[Path] = []
    for raw_token, raw_root in input_roots:
        token = raw_token.rstrip("/")
        if normalized == token:
            suffix = ""
        elif normalized.startswith(f"{token}/"):
            suffix = normalized[len(token) + 1 :]
        else:
            continue
        relative = PurePosixPath(suffix)
        if relative.is_absolute() or any(
            part in {".", ".."} for part in relative.parts
        ):
            return (), [
                f"extension_manifest.json input escapes canonical root: {raw_path}"
            ]
        candidates.append(raw_root.joinpath(*relative.parts))
    if not candidates:
        if normalized.startswith("@"):
            return (), [f"extension_manifest.json input has no bound root: {raw_path}"]
        source_path = Path(raw_path).expanduser()
        if not source_path.is_absolute():
            if manifest_path is None:
                return (), [
                    "extension_manifest.json relative input requires an explicit "
                    f"manifest path: {raw_path}"
                ]
            source_path = manifest_path.parent / source_path
        candidates.append(source_path)
    return (
        tuple(dict.fromkeys(candidate.resolve() for candidate in candidates)),
        [],
    )


def resolve_source_extension_manifest_input(
    raw_path: str,
    *,
    manifest_path: Path | None,
    expected_sha256: str | None = None,
    input_roots: Sequence[tuple[str, Path]] = (),
) -> tuple[Path | None, list[str]]:
    """Resolve only the path explicitly named by a manifest.

    Relative paths are anchored to an explicit manifest. Absolute paths are accepted at
    the pre-publication build boundary. This function deliberately performs no
    basename, ancestor, current-directory, or well-known-directory search.
    """

    if not isinstance(raw_path, str) or not raw_path.strip():
        return None, ["extension_manifest.json input path must be non-empty"]
    candidates, candidate_errors = _manifest_input_candidates(
        raw_path,
        manifest_path=manifest_path,
        input_roots=input_roots,
    )
    if candidate_errors:
        return None, candidate_errors
    existing = tuple(path for path in candidates if path.is_file())
    if not existing:
        return None, []
    if expected_sha256 is not None:
        try:
            digest = require_source_extension_sha256(
                expected_sha256, field="input checksum"
            )
        except SourceExtensionInputCustodyError as exc:
            return None, [str(exc)]
        for source_path in existing:
            if _sha256_file(source_path) == digest:
                return source_path, []
        return None, [f"extension_manifest.json source checksum mismatch: {raw_path}"]
    return existing[0], []


def read_source_extension_manifest_input(
    raw_path: str,
    *,
    manifest_path: Path,
    expected_sha256: str,
) -> tuple[Path | None, bytes | None, list[str]]:
    """Return the exact verified bytes to scan, without a hash/read race."""
    try:
        digest = require_source_extension_sha256(
            expected_sha256, field="input checksum"
        )
    except SourceExtensionInputCustodyError as exc:
        return None, None, [str(exc)]
    source, errors = resolve_source_extension_manifest_input(
        raw_path, manifest_path=manifest_path
    )
    if errors or source is None:
        return None, None, errors
    try:
        content = source.read_bytes()
    except OSError as exc:
        return (
            None,
            None,
            [f"extension_manifest.json input is unreadable: {raw_path}: {exc}"],
        )
    if _sha256_bytes(content) != digest:
        return (
            None,
            None,
            [f"extension_manifest.json source checksum mismatch: {raw_path}"],
        )
    return source, content, []


def source_extension_manifest_input_rows(
    manifest: Mapping[str, Any],
) -> tuple[tuple[str, str, str], ...]:
    """Validate and enumerate every owned input, including compact dependencies.

    Each row carries its diagnostic field, explicit path and canonical SHA256.
    An empty or malformed object closure cannot describe an inspected input set.
    """
    closure = manifest.get("object_closure")
    objects = closure.get("objects") if isinstance(closure, Mapping) else None
    if not isinstance(objects, list) or not objects:
        raise SourceExtensionInputCustodyError(
            "source-extension manifest requires non-empty object_closure.objects"
        )
    rows: list[tuple[str, str, str]] = []
    for object_index, item in enumerate(objects):
        if not isinstance(item, Mapping):
            raise SourceExtensionInputCustodyError(
                f"object_closure.objects[{object_index}] must be an object"
            )
        item = cast(Mapping[str, Any], item)
        source = item.get("source")
        if not isinstance(source, str) or not source.strip():
            raise SourceExtensionInputCustodyError(
                f"object_closure.objects[{object_index}].source must be non-empty"
            )
        source_sha256 = require_source_extension_sha256(
            item.get("source_sha256"),
            field=f"object_closure.objects[{object_index}].source_sha256",
        )
        rows.append(
            (
                f"object_closure.objects[{object_index}].source",
                source,
                source_sha256,
            )
        )
        if "dependencies" not in item and "dependencies_ref" not in item:
            dependencies: list[dict[str, str]] = []
        else:
            try:
                dependencies = _manifest_dependencies(manifest, item)
            except ValueError as exc:
                raise SourceExtensionInputCustodyError(str(exc)) from exc
        for dependency_index, dependency in enumerate(dependencies):
            dependency_path = dependency.get("path")
            if not isinstance(dependency_path, str) or not dependency_path.strip():
                raise SourceExtensionInputCustodyError(
                    "source-extension dependency path must be non-empty at "
                    f"objects[{object_index}].dependencies[{dependency_index}]"
                )
            dependency_sha256 = require_source_extension_sha256(
                dependency.get("sha256"),
                field=(
                    f"object_closure.objects[{object_index}].dependencies"
                    f"[{dependency_index}].sha256"
                ),
            )
            rows.append(
                (
                    (
                        f"object_closure.objects[{object_index}].dependencies"
                        f"[{dependency_index}].path"
                    ),
                    dependency_path,
                    dependency_sha256,
                )
            )
    return tuple(rows)


def stage_source_extension_manifest_inputs(
    manifest: Mapping[str, Any],
    *,
    manifest_path: Path,
    publish_root: Path,
) -> dict[Path, Path]:
    """Validate and stage the complete object-input closure exactly once.

    The returned mapping is from each original resolved path to its path under
    ``publish_root``. Byte-identical inputs share one digest address regardless
    of producer checkout, build root, filename, platform path syntax, or object.
    """

    staged: dict[Path, Path] = {}
    for field, raw_path, expected_sha256 in source_extension_manifest_input_rows(
        manifest
    ):
        source, errors = resolve_source_extension_manifest_input(
            raw_path,
            manifest_path=manifest_path,
            expected_sha256=expected_sha256,
        )
        if errors:
            raise SourceExtensionInputCustodyError("; ".join(errors))
        if source is None:
            raise SourceExtensionInputCustodyError(
                f"source-extension manifest input is missing before custody: "
                f"{field}: {raw_path}"
            )
        relative = source_extension_input_custody_path(expected_sha256)
        destination = publish_root / relative
        if destination.exists():
            if (
                not destination.is_file()
                or _sha256_file(destination) != expected_sha256
            ):
                raise SourceExtensionInputCustodyError(
                    f"content-addressed source-extension input collision: {destination}"
                )
        else:
            try:
                _atomic_copy_file(source, destination, expected_sha256=expected_sha256)
            except ValueError as exc:
                raise SourceExtensionInputCustodyError(
                    f"source-extension input changed while staging: {source}"
                ) from exc
        staged[source] = relative
    return staged


def project_source_extension_manifest_inputs(
    manifest: Mapping[str, Any],
    *,
    source_manifest_path: Path,
    output_manifest_path: Path,
    publish_root: Path,
    staged_inputs: Mapping[Path, Path],
) -> dict[str, Any]:
    """Derive one manifest view of the retained inputs and bind its identity."""
    projected = dict(manifest)
    rewrite_source_extension_manifest_input_references(
        projected,
        source_manifest_path=source_manifest_path,
        output_manifest_path=output_manifest_path,
        publish_root=publish_root,
        staged_inputs=staged_inputs,
    )
    projected["sources"] = [
        item["source"] for item in projected["object_closure"]["objects"]
    ]
    projected["input_custody"] = source_extension_input_custody_manifest()
    validate_source_extension_manifest_input_custody(projected)
    finalize_source_extension_object_closure(projected)
    return projected


def rewrite_source_extension_manifest_input_references(
    manifest: dict[str, Any],
    *,
    source_manifest_path: Path,
    output_manifest_path: Path,
    publish_root: Path,
    staged_inputs: Mapping[Path, Path],
) -> None:
    """Atomically rewrite the complete validated input closure, expanding refs."""

    rows = source_extension_manifest_input_rows(manifest)
    projected = _expand_source_extension_manifest_authorities(manifest)

    references = source_extension_manifest_input_references(
        output_manifest_path=output_manifest_path,
        publish_root=publish_root,
        staged_inputs=staged_inputs,
    )

    def reference(raw_path: str, expected_sha256: str, *, field: str) -> str:
        candidates, errors = _manifest_input_candidates(
            raw_path,
            manifest_path=source_manifest_path,
            input_roots=(),
        )
        if errors:
            raise SourceExtensionInputCustodyError("; ".join(errors))
        matches = [source for source in candidates if source in staged_inputs]
        if len(matches) != 1:
            raise SourceExtensionInputCustodyError(
                f"source-extension staged input mapping is incomplete: {field}"
            )
        source = matches[0]
        if staged_inputs[source] != source_extension_input_custody_path(
            expected_sha256
        ):
            raise SourceExtensionInputCustodyError(
                f"source-extension staged input digest differs: {field}"
            )
        return references[source]

    rewritten = {
        (raw_path, digest): reference(raw_path, digest, field=field)
        for field, raw_path, digest in rows
    }
    for item in projected["object_closure"]["objects"]:
        item["source"] = rewritten[(item["source"], item["source_sha256"])]
        for dependency in item.get("dependencies", []):
            dependency["path"] = rewritten[(dependency["path"], dependency["sha256"])]
    manifest.clear()
    manifest.update(projected)
