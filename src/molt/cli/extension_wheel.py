from __future__ import annotations

import hashlib
import json
import zipfile
from collections.abc import Mapping, Sequence
from pathlib import Path, PurePosixPath
from typing import Any

from molt.cli.atomic_io import _atomic_zip_file
from molt.cli.extension_manifest import _wheel_record_line, _write_zip_member
from molt.file_hashing import _sha256_file


_EMBEDDED_EXTENSION_MANIFEST = "extension_manifest.json"
_SIDECAR_ONLY_MANIFEST_FIELDS = frozenset({"generated_at_utc", "wheel_sha256"})


class ExtensionWheelError(ValueError):
    pass


def _canonical_wheel_path(raw_path: str) -> str:
    path = raw_path.replace("\\", "/")
    parsed = PurePosixPath(path)
    if (
        not path
        or parsed.as_posix() == "."
        or parsed.is_absolute()
        or ".." in parsed.parts
        or path.endswith("/")
    ):
        raise ExtensionWheelError(f"invalid wheel member path: {raw_path!r}")
    return parsed.as_posix()


def _validated_wheel_entries(
    entries: Sequence[tuple[str, bytes]],
    *,
    record_path: str,
) -> tuple[tuple[str, bytes], ...]:
    canonical_record_path = _canonical_wheel_path(record_path)
    if not canonical_record_path.endswith(".dist-info/RECORD"):
        raise ExtensionWheelError(
            f"wheel RECORD path is not under .dist-info: {record_path!r}"
        )
    by_path: dict[str, bytes] = {}
    for raw_path, data in entries:
        path = _canonical_wheel_path(raw_path)
        if path == canonical_record_path:
            raise ExtensionWheelError("wheel entries must not provide RECORD directly")
        if path in by_path:
            raise ExtensionWheelError(f"duplicate wheel member path: {path}")
        by_path[path] = data
    if _EMBEDDED_EXTENSION_MANIFEST not in by_path:
        raise ExtensionWheelError("wheel is missing extension_manifest.json")
    dist_info = PurePosixPath(canonical_record_path).parent.as_posix()
    for required_name in ("WHEEL", "METADATA"):
        required_path = f"{dist_info}/{required_name}"
        if required_path not in by_path:
            raise ExtensionWheelError(
                f"wheel is missing required {required_path} member"
            )
    try:
        manifest = json.loads(by_path[_EMBEDDED_EXTENSION_MANIFEST])
    except (UnicodeDecodeError, json.JSONDecodeError) as exc:
        raise ExtensionWheelError(f"embedded extension manifest is invalid: {exc}") from exc
    if not isinstance(manifest, Mapping):
        raise ExtensionWheelError("embedded extension manifest must be an object")
    extension = manifest.get("extension")
    if not isinstance(extension, str) or _canonical_wheel_path(extension) not in by_path:
        raise ExtensionWheelError(
            "embedded extension manifest does not name a wheel member"
        )
    return tuple(sorted(by_path.items()))


def _write_extension_wheel(
    wheel_path: Path,
    *,
    entries: Sequence[tuple[str, bytes]],
    record_path: str,
) -> str:
    """Write one canonical extension wheel and its complete RECORD authority."""
    canonical_entries = _validated_wheel_entries(entries, record_path=record_path)
    canonical_record_path = _canonical_wheel_path(record_path)
    record_lines = [_wheel_record_line(path, data) for path, data in canonical_entries]
    record_lines.append(f"{canonical_record_path},,")
    record_bytes = ("\n".join(record_lines) + "\n").encode("utf-8")
    with _atomic_zip_file(wheel_path) as wheel:
        for path, data in canonical_entries:
            _write_zip_member(wheel, path, data)
        _write_zip_member(wheel, canonical_record_path, record_bytes)
    return _sha256_file(wheel_path)


def _rewrite_staged_extension_wheel(
    source_wheel: Path,
    destination_wheel: Path,
    *,
    canonical_embedded_manifest: Mapping[str, Any],
) -> tuple[str, dict[str, Any]]:
    """Replace the raw build manifest with the producer's canonical authority."""
    try:
        with zipfile.ZipFile(source_wheel) as wheel:
            infos = wheel.infolist()
            names = [info.filename for info in infos]
            if len(names) != len(set(names)):
                raise ExtensionWheelError("source wheel has duplicate member paths")
            record_paths = [name for name in names if name.endswith(".dist-info/RECORD")]
            if len(record_paths) != 1:
                raise ExtensionWheelError(
                    "source wheel must have exactly one .dist-info/RECORD member"
                )
            try:
                raw_embedded = json.loads(wheel.read(_EMBEDDED_EXTENSION_MANIFEST))
            except KeyError as exc:
                raise ExtensionWheelError(
                    "source wheel is missing extension_manifest.json"
                ) from exc
            except (UnicodeDecodeError, json.JSONDecodeError) as exc:
                raise ExtensionWheelError(
                    f"source wheel extension manifest is invalid: {exc}"
                ) from exc
            if not isinstance(raw_embedded, Mapping):
                raise ExtensionWheelError(
                    "source wheel extension manifest must be an object"
                )
            raw_extension = raw_embedded.get("extension")
            if not isinstance(raw_extension, str):
                raise ExtensionWheelError(
                    "source wheel extension manifest has no extension member"
                )
            entries = [
                (info.filename, wheel.read(info))
                for info in infos
                if info.filename
                not in {_EMBEDDED_EXTENSION_MANIFEST, record_paths[0]}
            ]
    except (OSError, zipfile.BadZipFile) as exc:
        raise ExtensionWheelError(f"cannot read source extension wheel: {exc}") from exc

    embedded_manifest = dict(canonical_embedded_manifest)
    for field in _SIDECAR_ONLY_MANIFEST_FIELDS:
        embedded_manifest.pop(field, None)
    canonical_extension = embedded_manifest.get("extension")
    if not isinstance(canonical_extension, str):
        raise ExtensionWheelError(
            "canonical extension manifest has no extension member"
        )
    canonical_extension = _canonical_wheel_path(canonical_extension)
    raw_extension = _canonical_wheel_path(raw_extension)
    if canonical_extension != raw_extension and any(
        path == canonical_extension for path, _data in entries
    ):
        raise ExtensionWheelError(
            "canonical extension member collides with an existing wheel member: "
            f"{canonical_extension}"
        )
    entries = [
        (canonical_extension if path == raw_extension else path, data)
        for path, data in entries
    ]
    embedded_manifest["extension"] = canonical_extension
    embedded_manifest["wheel"] = destination_wheel.name
    expected_extension_sha256 = embedded_manifest.get("extension_sha256")
    extension_members = dict(entries)
    extension_bytes = extension_members.get(canonical_extension)
    if extension_bytes is None:
        raise ExtensionWheelError(
            "source wheel extension manifest names a missing extension member"
        )
    if not isinstance(expected_extension_sha256, str) or (
        _sha256_bytes(extension_bytes) != expected_extension_sha256
    ):
        raise ExtensionWheelError(
            "source wheel extension member does not match extension_sha256"
        )
    embedded_bytes = (
        json.dumps(embedded_manifest, sort_keys=True, indent=2).encode("utf-8") + b"\n"
    )
    entries.append((_EMBEDDED_EXTENSION_MANIFEST, embedded_bytes))
    wheel_sha256 = _write_extension_wheel(
        destination_wheel,
        entries=entries,
        record_path=record_paths[0],
    )
    return wheel_sha256, embedded_manifest


def _sha256_bytes(data: bytes) -> str:
    return hashlib.sha256(data).hexdigest()
