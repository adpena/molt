"""Read-only Cython requirement admission and receipt-bound execution."""

from __future__ import annotations

from collections.abc import Iterator, Mapping
from contextlib import ExitStack, contextmanager
from dataclasses import dataclass
import json
import os
from pathlib import Path
import subprocess
import sys
from typing import Any

from packaging.requirements import Requirement
from packaging.utils import canonicalize_name

from molt import process_guard
from molt.cli.source_build_environment import (
    active_source_build_requirements,
    canonical_source_marker_environment,
)
from molt.cli.source_build_inventory import SourceBuildInventory
from molt.cli.source_build_requirements import current_build_requirements
from molt.toolchain_identity import stable_executable_probe


def require_cython(
    *,
    pyproject: Mapping[str, Any],
    inventory: SourceBuildInventory | None = None,
) -> tuple[str | None, str | None]:
    """Validate every active Cython constraint without provisioning an environment."""
    try:
        build_system = pyproject.get("build-system", {})
        if not isinstance(build_system, Mapping):
            raise ValueError("source [build-system] must be a table")
        requirements = build_system.get("requires", [])
        if not isinstance(requirements, list) or any(
            not isinstance(raw, str) or not raw.strip() for raw in requirements
        ):
            raise ValueError("source [build-system].requires must be a string array")
        if (
            inventory is not None
            and Path(sys.executable).absolute() != inventory.python_executable
        ):
            raise ValueError(
                "Cython inventory belongs to a different selected interpreter"
            )
        markers = canonical_source_marker_environment(
            inventory.marker_environment if inventory is not None else None
        )
        active = tuple(
            (raw, requirement)
            for raw, requirement in active_source_build_requirements(
                requirements, markers
            )
            if canonicalize_name(requirement.name) == "cython"
        ) or (("Cython", Requirement("Cython")),)
        if inventory is None:
            resolved = current_build_requirements(active, markers)
        else:
            resolved = []
            for raw, requirement in active:
                installed = inventory.requirement(raw, requirement)
                if installed is None:
                    raise ValueError(
                        f"locked source-build inventory does not satisfy {raw!r}"
                    )
                resolved.append(installed)
        return resolved[0].version, None
    except (OSError, ValueError, subprocess.SubprocessError) as exc:
        return None, (
            f"Cython admission failed for {sys.executable!r}: {exc}. "
            "No packages were installed or changed. Prepare a compatible environment "
            "explicitly with uv; locked builds require an updated dependency group and lock."
        )


_CYTHON_IMPORT_CHECK = """
import importlib.metadata as metadata
import json
from pathlib import Path
import sys
import sysconfig
import Cython
import cython

distribution = metadata.distribution('Cython')
owned = {Path(distribution.locate_file(item)).resolve() for item in distribution.files or ()}
roots = {Path(sysconfig.get_path(name)).resolve() for name in ('purelib', 'platlib')}
origins = {}
for name, module in tuple(sys.modules.items()):
    if name not in ('Cython', 'cython') and not name.startswith('Cython.'):
        continue
    origin = Path(module.__file__).resolve()
    if origin.suffix == '.pyc' or origin not in owned or not any(origin.is_relative_to(root) for root in roots):
        raise RuntimeError(f'Cython import is not owned by the selected interpreter distribution: {origin}')
    origins[name] = str(origin)
if Cython.__version__ != distribution.version:
    raise RuntimeError('Cython imported version differs from installed distribution')
print(json.dumps({'version': distribution.version, 'origins': origins}))
"""


@dataclass(frozen=True)
class CythonTool:
    """An admitted tool used inside its plan-scoped execution fence."""

    version: str
    python_executable: str
    bytecode_prefix: Path

    @property
    def command(self) -> tuple[str, ...]:
        return (
            self.python_executable,
            "-B",
            "-I",
            "-X",
            f"pycache_prefix={self.bytecode_prefix}",
        )

    def run(self, args: list[str], **kwargs: Any) -> subprocess.CompletedProcess[str]:
        # -B disables cache writes, not reads. Redirect source-cache lookups to
        # this absent path; never delete or reuse somebody else's cached code.
        def check_cache() -> None:
            if os.path.lexists(self.bytecode_prefix):
                raise ValueError(
                    f"Cython bytecode isolation path must remain absent: {self.bytecode_prefix}"
                )

        check_cache()
        try:
            return process_guard.run_completed_command([*self.command, *args], **kwargs)
        finally:
            check_cache()


@contextmanager
def cython_execution(
    *,
    pyproject: Mapping[str, Any],
    working_directory: Path,
    inventory: SourceBuildInventory | None = None,
) -> Iterator[CythonTool]:
    """Fence the installed generator once across a complete regeneration plan.

    Runtime/site startup remains under the owning environment contract. This
    binds Cython's distribution, not arbitrary undeclared dynamic imports.
    """
    version, error = require_cython(pyproject=pyproject, inventory=inventory)
    if error is not None:
        raise ValueError(error)
    assert version is not None
    tool = CythonTool(
        version,
        sys.executable,
        working_directory.resolve() / ".molt-cython-no-bytecode",
    )
    with ExitStack() as stack:
        owned: set[Path] | None = None
        if inventory is not None:
            base_executable = getattr(sys, "_base_executable", None)
            if not isinstance(base_executable, str) or not base_executable:
                raise ValueError("selected interpreter has no base-runtime executable")
            stack.enter_context(
                stable_executable_probe(
                    inventory.python_executable,
                    label="Cython interpreter",
                    identity=inventory.python_identity(
                        base_executable=Path(base_executable)
                    ),
                )
            )
            files = inventory.files(distribution="Cython")
            owned = {file.path.resolve(strict=True) for file in files}
            for file in files:
                stack.enter_context(
                    stable_executable_probe(
                        file.path,
                        label="Cython distribution",
                        identity=file.content,
                    )
                )
        probe = tool.run(
            ["-c", _CYTHON_IMPORT_CHECK],
            capture_output=True,
            text=True,
            timeout=60,
            check=False,
        )
        if probe.returncode != 0:
            raise ValueError(
                (probe.stderr or probe.stdout).strip()[-4000:]
                or f"Cython import check exited {probe.returncode}"
            )
        result = json.loads(probe.stdout)
        if not isinstance(result, dict) or result.get("version") != version:
            raise ValueError(
                "isolated Cython import differs from admitted distribution version"
            )
        origins = result.get("origins")
        if (
            not isinstance(origins, dict)
            or not {"Cython", "cython"} <= origins.keys()
            or any(not isinstance(path, str) for path in origins.values())
        ):
            raise ValueError("isolated Cython import has no complete origin inventory")
        if owned is not None and any(
            Path(path) not in owned for path in origins.values()
        ):
            raise ValueError(
                "isolated Cython import differs from locked distribution files"
            )
        yield tool
