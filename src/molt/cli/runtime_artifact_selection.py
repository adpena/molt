from __future__ import annotations

from dataclasses import dataclass
from enum import StrEnum


class RuntimeCrateType(StrEnum):
    """Cargo crate types that may materialize a Molt runtime artifact."""

    RLIB = "rlib"
    STATICLIB = "staticlib"
    CDYLIB = "cdylib"


@dataclass(frozen=True)
class RuntimeArtifactSelection:
    """Exact Cargo-level crate-type selection for one runtime producer.

    Cargo's ``cargo rustc --crate-type`` option replaces the manifest crate-type
    plan.  Passing ``--crate-type`` after Cargo's ``--`` separator instead adds a
    rustc crate type to the manifest plan, which is precisely the accidental
    multi-artifact codegen this authority prevents.
    """

    crate_types: tuple[RuntimeCrateType, ...]

    def __post_init__(self) -> None:
        if not self.crate_types:
            raise ValueError("a runtime artifact selection cannot be empty")
        if len(set(self.crate_types)) != len(self.crate_types):
            raise ValueError("a runtime artifact selection cannot contain duplicates")

    @property
    def cargo_value(self) -> str:
        return ",".join(crate_type.value for crate_type in self.crate_types)

    @property
    def source_identity(self) -> str:
        return f"molt.runtime-artifact-selection.v1:{self.cargo_value}"

    def cargo_args(self) -> tuple[str, str]:
        return ("--crate-type", self.cargo_value)

    def select_in(self, command: list[str]) -> None:
        """Append the selector at Cargo level, failing closed after ``--``."""

        if "--" in command:
            raise ValueError(
                "runtime crate types must be selected before Cargo's -- separator"
            )
        command.extend(self.cargo_args())

    def includes(self, crate_type: RuntimeCrateType) -> bool:
        return crate_type in self.crate_types


RUNTIME_RLIB_ARTIFACTS = RuntimeArtifactSelection((RuntimeCrateType.RLIB,))
RUNTIME_STATICLIB_ARTIFACTS = RuntimeArtifactSelection((RuntimeCrateType.STATICLIB,))
RUNTIME_CDYLIB_ARTIFACTS = RuntimeArtifactSelection((RuntimeCrateType.CDYLIB,))
RUNTIME_WASM_COMBINED_ARTIFACTS = RuntimeArtifactSelection(
    (RuntimeCrateType.STATICLIB, RuntimeCrateType.CDYLIB)
)
