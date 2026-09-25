from __future__ import annotations

import functools
from pathlib import Path

from molt.cli.output import fail as _fail
from molt.compiler_distribution import installed_compiler
from molt.source_root import (
    MOLT_SOURCE_ROOT_ENV,
    compiler_source_root_override,
    resolve_path_override,
)


def _is_path_within(path: Path, container: Path) -> bool:
    """Return True when ``path`` is (resolves to) inside ``container``.

    A pure path-containment predicate. It lives in this low-level path module so
    both frontend module-resolution and post-lowering setup/readiness code can
    share one authority without either dragging the other's import closure onto
    its path.
    """
    try:
        path.resolve().relative_to(container.resolve())
    except ValueError:
        return False
    return True


def _resolve_root_override(var: str) -> Path | None:
    path = resolve_path_override(var)
    if path is None:
        return None
    if path.exists():
        return path
    return None


def _has_molt_repo_markers(path: Path) -> bool:
    return all(
        (path / marker).is_file()
        for marker in (
            "Cargo.toml",
            "runtime/molt-runtime/Cargo.toml",
            "runtime/molt-backend/Cargo.toml",
            "src/molt/cli/__init__.py",
        )
    )


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
        return Path(override_text)
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
    override = compiler_source_root_override()
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
    if _has_molt_repo_markers(molt_root):
        try:
            installed = installed_compiler(molt_root)
            if installed is not None:
                installed.verify_sources()
        except (OSError, ValueError) as exc:
            return _fail(
                f"Molt installed compiler sources are invalid: {exc}",
                json_output,
                command=command,
            )
        return None
    message = (
        f"Molt compiler/runtime sources not found under {molt_root}. "
        f"Set {MOLT_SOURCE_ROOT_ENV} to the compiler source root or use a "
        "release bundle with its verified source payload."
    )
    return _fail(message, json_output, command=command)
