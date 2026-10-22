"""The launch facts of one native program, for every code owner.

An AOT executable installs them in its generated C ``main``
(``native_main_stub.py``). The development JIT runner installs them from its
JIT image's header (``runtime/molt-jit/src/launch.rs``), in the same order. Both
read this plan, so the two tiers start a program in the same environment.
"""

from __future__ import annotations

from collections.abc import Mapping, Sequence
from dataclasses import dataclass
from pathlib import Path
from types import MappingProxyType
from typing import Any

from molt.capability_manifest import ResolvedRuntimePolicy

EXECUTION_TARGET_ENV = "MOLT_EXECUTION_TARGET"
NATIVE_EXECUTION_TARGET = "native"


@dataclass(frozen=True, slots=True)
class NativeLaunchPlan:
    """What a native program's launch installs before the runtime starts."""

    # Installed with overwrite: the resolved capability policy and the target.
    environment: Mapping[str, str]
    # External native artifact roots, prepended to MOLT_MODULE_ROOTS.
    runtime_module_roots: tuple[str, ...]
    # The program's own source files, for tracebacks without `fs.read`.
    compiled_source_paths: tuple[str, ...]

    @classmethod
    def for_program(
        cls,
        *,
        resolved_capability_policy: ResolvedRuntimePolicy,
        compiled_source_paths: Sequence[str] = (),
        runtime_module_roots: Sequence[Path] = (),
    ) -> NativeLaunchPlan:
        environment = {
            **resolved_capability_policy.to_env_vars(),
            EXECUTION_TARGET_ENV: NATIVE_EXECUTION_TARGET,
        }
        return cls(
            environment=MappingProxyType(dict(sorted(environment.items()))),
            runtime_module_roots=tuple(
                str(path.resolve()) for path in dict.fromkeys(runtime_module_roots)
            ),
            compiled_source_paths=tuple(compiled_source_paths),
        )

    def image_header_payload(self) -> dict[str, Any]:
        """The ``launch`` object of a JIT image's header (``jit_image.py``)."""
        return {
            "environment": dict(self.environment),
            "runtime_module_roots": list(self.runtime_module_roots),
            "compiled_source_paths": list(self.compiled_source_paths),
        }
