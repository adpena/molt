from __future__ import annotations

from pathlib import Path

from molt.cli.output import fail as _fail
from molt.compiler_distribution import MANIFEST_NAME
from molt.cli.compiler_identity import installed_compiler_admission
from molt.source_root import (
    MOLT_SOURCE_ROOT_ENV,
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


def _find_project_root(start: Path) -> Path:
    """Discover the live user project independently of compiler inputs.

    Directory topology is mutable, so a path-only cache cannot own this fact.
    An explicit selection is preserved even when invalid; consumers diagnose it.
    """
    override = resolve_path_override("MOLT_PROJECT_ROOT")
    if override is not None:
        return override
    start = start.expanduser().resolve()
    directory = start if start.is_dir() else start.parent
    for parent in (directory, *directory.parents):
        if _has_project_markers(parent):
            return parent
    return directory


def _require_project_root(
    root: Path,
    json_output: bool,
    command: str,
    *,
    pyproject: bool = False,
) -> int | None:
    if not root.is_dir():
        return _fail(
            f"Project directory not found: {root}",
            json_output,
            command=command,
        )
    if (root / MANIFEST_NAME).exists():
        return _fail(
            f"Installed compiler inputs are immutable: {root}. Select your user project.",
            json_output,
            command=command,
        )
    if pyproject and not (root / "pyproject.toml").is_file():
        return _fail(
            f"Project pyproject.toml not found under {root}. "
            "Run this command in your project or set MOLT_PROJECT_ROOT.",
            json_output,
            command=command,
        )
    return None


def _require_molt_root(
    molt_root: Path,
    json_output: bool,
    command: str,
) -> int | None:
    if _has_molt_repo_markers(molt_root):
        try:
            installed_compiler_admission(molt_root)
        except (OSError, ValueError) as exc:
            return _fail(
                f"Molt installed compiler sources are invalid: {exc}",
                json_output,
                command=command,
            )
        return None
    message = (
        f"Molt compiler/runtime sources not found under {molt_root}. This "
        "installation has no prebuilt Molt distribution for this platform: "
        "install the platform wheel, a package-manager release or a release "
        f"bundle, or set {MOLT_SOURCE_ROOT_ENV} to a Molt source checkout for "
        "an explicit source build."
    )
    return _fail(message, json_output, command=command)
