from __future__ import annotations

from functools import lru_cache
from importlib import metadata
from pathlib import Path
import tomllib


_PROJECT_NAME = "molt"


def _source_tree_pyproject() -> Path:
    return Path(__file__).resolve().parents[2] / "pyproject.toml"


def _read_pyproject_version(pyproject: Path) -> str:
    with pyproject.open("rb") as handle:
        data = tomllib.load(handle)
    project = data.get("project")
    if not isinstance(project, dict):
        raise RuntimeError(f"invalid Molt pyproject metadata: {pyproject}")
    version = project.get("version")
    if not isinstance(version, str) or not version:
        raise RuntimeError(f"invalid Molt project version in {pyproject}")
    return version


@lru_cache(maxsize=1)
def version() -> str:
    pyproject = _source_tree_pyproject()
    if pyproject.exists():
        return _read_pyproject_version(pyproject)
    try:
        return metadata.version(_PROJECT_NAME)
    except metadata.PackageNotFoundError as exc:
        raise RuntimeError("unable to determine Molt package version") from exc
