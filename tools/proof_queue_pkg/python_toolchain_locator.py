"""Locate Python ownership roots without constructing or hashing its inventory."""

from __future__ import annotations

import importlib.metadata as metadata
import json
from pathlib import Path
import site
import sys
import sysconfig
import urllib.parse


def _editable_root(distribution: metadata.Distribution) -> Path | None:
    try:
        payload = json.loads(distribution.read_text("direct_url.json") or "{}")
    except (json.JSONDecodeError, UnicodeError):
        return None
    if not isinstance(payload, dict):
        return None
    directory_info = payload.get("dir_info")
    url = payload.get("url")
    if not isinstance(directory_info, dict) or directory_info.get("editable") is not True:
        return None
    if not isinstance(url, str):
        return None
    parsed = urllib.parse.urlparse(url)
    if parsed.scheme != "file":
        return None
    raw = urllib.parse.unquote(parsed.path)
    if parsed.netloc:
        raw = f"//{parsed.netloc}{raw}"
    if sys.platform == "win32" and raw.startswith("/") and len(raw) > 2:
        if raw[2] == ":":
            raw = raw[1:]
    try:
        return Path(raw).resolve(strict=True)
    except OSError:
        return None


def main() -> None:
    roots: set[Path] = set()
    for raw in (
        sys.prefix,
        sys.base_prefix,
        sys.exec_prefix,
        sys.base_exec_prefix,
        *sys.path,
        *site.getsitepackages(),
        sysconfig.get_path("stdlib"),
        sysconfig.get_path("platstdlib"),
        sysconfig.get_path("purelib"),
        sysconfig.get_path("platlib"),
    ):
        if not raw:
            continue
        try:
            candidate = Path(raw).resolve(strict=True)
        except OSError:
            continue
        roots.add(candidate if candidate.is_dir() else candidate.parent)
    editable_roots: set[Path] = set()
    for distribution in metadata.distributions():
        root = _editable_root(distribution)
        if root is not None:
            editable_roots.add(root)
    print(
        json.dumps(
            {
                "schema": "molt.proof-python-toolchain-location.v1",
                "executable": sys.executable,
                "base_executable": str(
                    Path(getattr(sys, "_base_executable", sys.executable)).resolve(
                        strict=True
                    )
                ),
                "roots": sorted(str(path) for path in roots),
                "editable_roots": sorted(str(path) for path in editable_roots),
            },
            sort_keys=True,
            separators=(",", ":"),
        )
    )


if __name__ == "__main__":
    main()
