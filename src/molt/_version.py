from __future__ import annotations

from functools import lru_cache
from importlib import metadata
from pathlib import Path


_PROJECT_NAME = "molt"
_SOURCE_TREE_VERSION = "0.0.001"


def _source_tree_pyproject() -> Path:
    return Path(__file__).resolve().parents[2] / "pyproject.toml"


@lru_cache(maxsize=1)
def version() -> str:
    pyproject = _source_tree_pyproject()
    if pyproject.exists():
        return _SOURCE_TREE_VERSION
    try:
        return metadata.version(_PROJECT_NAME)
    except metadata.PackageNotFoundError as exc:
        raise RuntimeError("unable to determine Molt package version") from exc
