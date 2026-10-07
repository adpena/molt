"""Compiler input-source authority, independent of guest projects and outputs."""

from __future__ import annotations

import os
from pathlib import Path
import sysconfig


MOLT_SOURCE_ROOT_ENV = "MOLT_SOURCE_ROOT"
_DEFAULT_COMPILER_SOURCE_ROOT = Path(__file__).resolve().parents[2]


def resolve_path_override(env_var: str) -> Path | None:
    """Resolve an optional environment path without requiring it to exist."""
    override = os.environ.get(env_var)
    if not override:
        return None
    path = Path(override).expanduser()
    if not path.is_absolute():
        path = Path.cwd() / path
    return path.resolve(strict=False)


def compiler_source_root_override() -> Path | None:
    """Preserve explicit source selection, including invalid paths.

    Source-marker validation belongs to the consuming boundary. An invalid
    explicit selection must never silently fall back to another source tree.
    """
    return resolve_path_override(MOLT_SOURCE_ROOT_ENV)


# Install-scheme data location of a platform wheel's release bundle.
PACKAGED_DISTRIBUTION_PATH = ("share", "molt", "distribution")


def packaged_distribution_root() -> Path | None:
    """Return the release bundle a platform wheel installed beside this package.

    Only the install scheme that owns this executing package is consulted, so a
    nearby checkout or another environment is never adopted. The bundle is
    admitted by content in ``compiler_distribution``, never trusted by path.
    """
    package_parent = os.path.normcase(str(Path(__file__).resolve().parent.parent))
    for scheme in sysconfig.get_scheme_names():
        try:
            paths = sysconfig.get_paths(scheme)
        except KeyError:
            continue
        libraries = {
            os.path.normcase(os.path.realpath(paths[key]))
            for key in ("purelib", "platlib")
            if key in paths
        }
        if package_parent not in libraries or "data" not in paths:
            continue
        root = Path(paths["data"]).joinpath(*PACKAGED_DISTRIBUTION_PATH)
        if root.exists() or root.is_symlink():
            return root
    return None


def source_file_revision(path: Path) -> tuple[int, int, int]:
    """Identify one revision of a compiler input file for cache keys.

    A loader that caches a parsed manifest keys on this as well as the path,
    so a rewrite in the same process is never served from the old parse.
    """
    try:
        stat = path.stat()
    except OSError:
        return (-1, -1, -1)
    return (stat.st_mtime_ns, stat.st_size, stat.st_ino)


def compiler_source_root() -> Path:
    """Return compiler inputs, never a writable artifact or guest-project root."""
    override = compiler_source_root_override()
    if override is not None:
        return override
    packaged = packaged_distribution_root()
    return (
        packaged / "source" if packaged is not None else _DEFAULT_COMPILER_SOURCE_ROOT
    )
