"""Captured source-extension link-input identity and validation contract."""

from __future__ import annotations

from dataclasses import dataclass
from pathlib import Path
import re

from molt.wasi_sdk_identity import WasiCAbiProjection
from molt.exact_json import string_keyed_mapping

SOURCE_EXTENSION_LINK_INPUTS_ENV = "MOLT_PROOF_SOURCE_EXTENSION_LINK_INPUTS"
_SCHEMA = "molt.source-extension-link-inputs.v2"


@dataclass(frozen=True, slots=True)
class SourceExtensionLinkInputs:
    target_triple: str
    compiler_rt: Path | None
    sha256: str | None
    size: int | None

    def verify_c_abi(self, plan: WasiCAbiProjection | None) -> None:
        if self.target_triple != "wasm32-wasip1":
            if plan is not None:
                raise ValueError(
                    "non-WASI source extension has an unexpected C ABI plan"
                )
            return
        if plan is None:
            raise ValueError("WASI source extension requires its selected C ABI plan")
        expected = next(
            (path, digest, size)
            for role, path, size, digest in plan.files
            if role == "compiler_rt"
        )
        if (self.compiler_rt, self.sha256, self.size) != expected:
            raise ValueError(
                "captured source-extension compiler-rt differs from the selected SDK plan"
            )

    def metadata(self) -> dict[str, object]:
        return {
            "schema": _SCHEMA,
            "target_triple": self.target_triple,
            "compiler_rt": (
                {
                    "path": str(self.compiler_rt),
                    "sha256": self.sha256,
                    "size": self.size,
                }
                if self.compiler_rt is not None
                else None
            ),
        }


def project_source_extension_link_inputs(
    target_triple: str,
    plan: WasiCAbiProjection | None,
) -> SourceExtensionLinkInputs:
    """Project the admitted managed generation; do not reread its archive."""
    if target_triple != "wasm32-wasip1":
        if plan is not None:
            raise ValueError(
                "non-WASI source-extension target has unexpected compiler-rt"
            )
        return SourceExtensionLinkInputs(target_triple, None, None, None)
    if plan is None:
        raise ValueError("WASI source-extension target requires compiler-rt")
    path, size, digest = next(
        (path, size, digest)
        for role, path, size, digest in plan.files
        if role == "compiler_rt"
    )
    return SourceExtensionLinkInputs(target_triple, path, digest, size)


def validate_source_extension_link_inputs(
    payload: object,
    *,
    target_triple: str,
) -> SourceExtensionLinkInputs:
    """Verify a captured selection without running tools or rediscovering files."""
    contract = string_keyed_mapping(payload)
    if (
        contract is None
        or set(contract) != {"schema", "target_triple", "compiler_rt"}
        or contract["schema"] != _SCHEMA
        or contract["target_triple"] != target_triple
    ):
        raise ValueError("source-extension link-input contract differs from the target")
    archive_payload = contract["compiler_rt"]
    if target_triple != "wasm32-wasip1":
        if archive_payload is not None:
            raise ValueError(
                "non-WASI source-extension target has unexpected compiler-rt"
            )
        return SourceExtensionLinkInputs(target_triple, None, None, None)
    archive = string_keyed_mapping(archive_payload)
    if archive is None:
        raise ValueError("WASI source-extension compiler-rt identity is malformed")
    path_value = archive.get("path")
    sha256_value = archive.get("sha256")
    size_value = archive.get("size")
    if (
        set(archive) != {"path", "sha256", "size"}
        or not isinstance(path_value, str)
        or not isinstance(sha256_value, str)
        or re.fullmatch(r"[0-9a-f]{64}", sha256_value) is None
        or type(size_value) is not int
        or size_value < 0
    ):
        raise ValueError("WASI source-extension compiler-rt identity is malformed")
    path = Path(path_value)
    if not path.is_absolute() or ".." in path.parts or str(path) != path_value:
        raise ValueError(
            "source-extension compiler-rt requires canonical absolute syntax"
        )
    return SourceExtensionLinkInputs(target_triple, path, sha256_value, size_value)
