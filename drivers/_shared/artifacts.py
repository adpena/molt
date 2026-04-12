from __future__ import annotations

import hashlib
from pathlib import Path
from typing import Any


def sha256_file(path: Path) -> str:
    digest = hashlib.sha256()
    with path.open("rb") as fh:
        for chunk in iter(lambda: fh.read(1024 * 1024), b""):
            digest.update(chunk)
    return digest.hexdigest()


def artifact_record(*, kind: str, path: Path, root: Path | None = None) -> dict[str, Any]:
    resolved = path.resolve()
    payload = {
        "kind": kind,
        "path": str(resolved),
        "size_bytes": resolved.stat().st_size,
        "sha256": sha256_file(resolved),
    }
    if root is not None:
        payload["relative_path"] = str(resolved.relative_to(root.resolve()))
    return payload


def directory_records(
    *,
    kind: str,
    root: Path,
    include_hidden: bool = False,
) -> list[dict[str, Any]]:
    resolved_root = root.resolve()
    if not resolved_root.exists():
        raise FileNotFoundError(f"artifact directory not found: {resolved_root}")
    records: list[dict[str, Any]] = []
    for path in sorted(p for p in resolved_root.rglob("*") if p.is_file()):
        if not include_hidden and any(part.startswith(".") for part in path.relative_to(resolved_root).parts):
            continue
        records.append(artifact_record(kind=kind, path=path, root=resolved_root))
    return records
