from __future__ import annotations

from dataclasses import dataclass
from pathlib import Path
from typing import Mapping

from molt.cli.runtime_identity_schema import RuntimeBuildIdentity
from molt.cli.native_link_manifest import (
    NativeLinkProductFacts,
    native_link_dependency_manifest_path,
)
from molt.cli.native_link_custody import NativeLinkCustodyObservation
from molt.toolchain_identity import (
    StableRegularFileIdentity,
    verify_stable_regular_file_identity,
)


@dataclass(frozen=True, slots=True)
class NativeRuntimeCodegenBinding:
    """The admitted runtime and callable input consumed by one app's codegen.

    This owns no global cache and is not a live source/toolchain identity.
    Final admission must independently recapture those inputs.
    """

    runtime_lib: Path
    build_identity: RuntimeBuildIdentity
    archive: StableRegularFileIdentity
    callable_symbols: StableRegularFileIdentity
    semantic_digest: str
    manifest: StableRegularFileIdentity
    link_facts: NativeLinkProductFacts
    custody: NativeLinkCustodyObservation

    def verify(self) -> None:
        if self.link_facts.build_identity != self.build_identity:
            raise ValueError("native runtime binding changed build identity")
        if self.manifest.path != native_link_dependency_manifest_path(
            self.archive.path
        ):
            raise ValueError("native runtime binding changed receipt coordinate")
        verify_stable_regular_file_identity(self.manifest, label="native link receipt")
        if self.runtime_lib.resolve(strict=True) != self.archive.path:
            raise ValueError(
                "native runtime path no longer names its codegen generation"
            )
        verify_stable_regular_file_identity(
            self.archive, label="native runtime codegen archive"
        )
        verify_stable_regular_file_identity(
            self.callable_symbols, label="native runtime codegen callable symbols"
        )


def native_runtime_codegen_environment(
    base: Mapping[str, str],
    binding: NativeRuntimeCodegenBinding | None,
) -> dict[str, str]:
    """Project one operation's callable authority, discarding ambient inputs.

    The environment is transport only. Neither a caller's mutable mapping nor
    process-global state may choose a different file/digest from the binding.
    """
    result = dict(base)
    result.pop("MOLT_RUNTIME_CALLABLE_SYMBOLS", None)
    result.pop("MOLT_RUNTIME_CALLABLE_SYMBOLS_SHA256", None)
    if binding is not None:
        binding.verify()
        result["MOLT_RUNTIME_CALLABLE_SYMBOLS"] = str(binding.callable_symbols.path)
        result["MOLT_RUNTIME_CALLABLE_SYMBOLS_SHA256"] = binding.callable_symbols.sha256
    return result
