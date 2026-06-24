from __future__ import annotations

from pathlib import Path


def _resolve_sidecar_path(output_path: Path, override: str | None, suffix: str) -> Path:
    if override:
        path = Path(override).expanduser()
        if not path.is_absolute():
            path = (output_path.parent / path).absolute()
        return path
    return output_path.with_name(output_path.stem + suffix)
