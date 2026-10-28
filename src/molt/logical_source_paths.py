"""The source paths that compiled programs embed (HF-372).

A module's logical source path is the one path its compiled code reports:
``__file__``, ``__spec__.origin``, the package ``__path__``, every
``co_filename``, traceback frames, deferred compile warnings and the module
registry origin. Every emitted path derives from it; nothing else names a
module's source location in an emitted artifact.

- A program module reports the absolute path it was compiled from, as CPython
  reports the file it imported. Moving the program changes its paths, as it
  does under CPython.
- A toolchain-owned module (a file of the compiler's own ``molt`` package: the
  bundled stdlib, ``molt.gpu``, ``molt.lib`` and ``molt`` itself) reports its
  place in that package below ``TOOLCHAIN_ROOT``, not the checkout, worktree or
  installed distribution that holds it. rustc reports
  ``/rustc/<commit>/library/...`` for std in the same way, and Go's
  ``-trimpath`` records ``runtime/proc.go``. The same compiler sources
  therefore give the same objects from every checkout.
- A generated module (such as a namespace stub) reports its
  path below ``GENERATED_ROOT``.

The design and its references are in
docs/spec/areas/core/0025_REPRODUCIBLE_AND_DETERMINISTIC_MODE.md, section 2.3.
"""

from __future__ import annotations

import re
from collections.abc import Sequence
from dataclasses import dataclass
from pathlib import Path

TOOLCHAIN_ROOT = "/__molt__"
GENERATED_ROOT = "/__molt_generated__"
_STDLIB_PREFIX = f"{TOOLCHAIN_ROOT}/stdlib/"


def toolchain_logical_path(package_relative_parts: Sequence[str]) -> str:
    """Return the path a toolchain-owned module reports.

    ``package_relative_parts`` are the module file's path parts below the
    compiler's ``molt`` package directory, for example
    ``("stdlib", "json", "__init__.py")``.
    """
    if not package_relative_parts or any(
        not part or part in {".", ".."} or "/" in part or "\\" in part
        for part in package_relative_parts
    ):
        raise ValueError(
            f"invalid toolchain module path parts: {tuple(package_relative_parts)!r}"
        )
    return "/".join((TOOLCHAIN_ROOT, *package_relative_parts))


def generated_module_logical_path(module_name: str) -> str:
    """Return the path a compiler-generated module reports."""
    safe = re.sub(r"[^0-9A-Za-z_]+", "_", module_name).strip("_")
    return f"{GENERATED_ROOT}/{safe or 'module'}.py"


def is_stdlib_logical_path(path: str) -> bool:
    """Whether ``path`` is the logical path of a bundled stdlib module."""
    return path.startswith(_STDLIB_PREFIX)


@dataclass(frozen=True, slots=True)
class ToolchainPackageRoot:
    """The resolved ``molt`` package directory of one compiler's sources.

    Every module file inside it (the bundled stdlib, ``molt.gpu``, ``molt.lib``
    and the ``molt`` package itself) is toolchain-owned. The CLI builds this
    once per module graph (``molt.cli.module_resolution``).
    """

    package: Path

    def logical_source_path(self, module_path: Path) -> str:
        """Return the path that ``module_path``'s compiled code reports.

        ``module_path`` must be resolved, as module graph paths are.
        """
        parts = module_path.parts
        package_parts = self.package.parts
        if len(parts) > len(package_parts) and parts[: len(package_parts)] == (
            package_parts
        ):
            return toolchain_logical_path(parts[len(package_parts) :])
        return str(module_path)
