from __future__ import annotations

import json
from pathlib import Path
import tempfile
import zipfile

from molt.cli.atomic_io import _atomic_write_json


def _resolve_sidecar_path(output_path: Path, override: str | None, suffix: str) -> Path:
    if override:
        path = Path(override).expanduser()
        if not path.is_absolute():
            path = (output_path.parent / path).absolute()
        return path
    return output_path.with_name(output_path.stem + suffix)


def _resolve_extension_manifest_for_verify(
    wheel_path: Path,
) -> tuple[Path | None, tempfile.TemporaryDirectory[str] | None, str | None]:
    sibling_manifest = wheel_path.parent / "extension_manifest.json"
    if sibling_manifest.exists():
        return sibling_manifest, None, None
    try:
        with zipfile.ZipFile(wheel_path) as zf:
            manifest_bytes = zf.read("extension_manifest.json")
    except KeyError:
        return (
            None,
            None,
            "extension_manifest.json not found next to wheel or inside wheel.",
        )
    except (OSError, zipfile.BadZipFile) as exc:
        return None, None, f"Failed to inspect wheel {wheel_path}: {exc}"
    try:
        decoded = json.loads(manifest_bytes.decode("utf-8"))
    except (UnicodeDecodeError, json.JSONDecodeError) as exc:
        return None, None, f"Invalid embedded extension_manifest.json: {exc}"
    if not isinstance(decoded, dict):
        return None, None, "Embedded extension_manifest.json must be a JSON object."
    tmpdir = tempfile.TemporaryDirectory(prefix="molt_ext_manifest_")
    manifest_path = Path(tmpdir.name) / "extension_manifest.json"
    _atomic_write_json(manifest_path, decoded, sort_keys=True, indent=2)
    return manifest_path, tmpdir, None
