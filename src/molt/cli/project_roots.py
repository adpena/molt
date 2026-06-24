from __future__ import annotations

import functools
import os
from pathlib import Path

from molt.cli.output import fail as _fail


def _resolve_root_override(var: str) -> Path | None:
    override = os.environ.get(var)
    if not override:
        return None
    path = Path(override).expanduser()
    if not path.is_absolute():
        path = (Path.cwd() / path).absolute()
    if path.exists():
        return path
    return None


def _has_molt_repo_markers(path: Path) -> bool:
    return (path / "runtime/molt-runtime/Cargo.toml").exists() and (
        path / "src/molt/cli/__init__.py"
    ).exists()


def _has_project_markers(path: Path) -> bool:
    return (
        (path / "pyproject.toml").exists()
        or (path / ".git").exists()
        or _has_molt_repo_markers(path)
    )


@functools.lru_cache(maxsize=64)
def _find_project_root_cached(start_text: str, override_text: str | None) -> Path:
    if override_text:
        override = Path(override_text)
        if override.exists():
            return override
    start = Path(start_text)
    for parent in [start] + list(start.parents):
        if _has_project_markers(parent):
            return parent
    return start.parent


def _find_project_root(start: Path) -> Path:
    override = _resolve_root_override("MOLT_PROJECT_ROOT")
    override_text = str(override) if override is not None else None
    return _find_project_root_cached(str(start), override_text)


@functools.lru_cache(maxsize=64)
def _find_molt_root_cached(
    candidate_texts: tuple[str, ...],
    override_text: str | None,
) -> Path:
    if override_text:
        override = Path(override_text)
        if override.exists():
            return override
    candidates = tuple(Path(text) for text in candidate_texts)
    for candidate in candidates:
        for parent in [candidate] + list(candidate.parents):
            if _has_molt_repo_markers(parent):
                return parent
    module_path = Path(__file__).resolve()
    for parent in [module_path] + list(module_path.parents):
        if _has_molt_repo_markers(parent):
            return parent
    if candidates:
        return candidates[0]
    return Path.cwd()


def _find_molt_root(*candidates: Path) -> Path:
    override = _resolve_root_override("MOLT_PROJECT_ROOT")
    override_text = str(override) if override is not None else None
    return _find_molt_root_cached(
        tuple(str(candidate) for candidate in candidates),
        override_text,
    )


def _require_molt_root(
    molt_root: Path,
    json_output: bool,
    command: str,
) -> int | None:
    runtime_toml = molt_root / "runtime/molt-runtime/Cargo.toml"
    backend_toml = molt_root / "runtime/molt-backend/Cargo.toml"
    if runtime_toml.exists() and backend_toml.exists():
        return None
    message = (
        f"Molt runtime sources not found under {molt_root}. "
        "Set MOLT_PROJECT_ROOT to the Molt repo root or run from within the Molt repo."
    )
    return _fail(message, json_output, command=command)
