"""One selected archive identity from producer planning through materialization."""

from __future__ import annotations

from collections.abc import Mapping
from dataclasses import dataclass
import os
from pathlib import Path
import re

from molt.cli import wasm_link_inputs
from molt.dx import _reject_onedrive
from molt.exact_json import loads_exact
from molt.toolchain_identity import stable_file_content_identity

SOURCE_EXTENSION_LINK_INPUTS_ENV = "MOLT_PROOF_SOURCE_EXTENSION_LINK_INPUTS"
_SCHEMA = "molt.source-extension-link-inputs.v1"


@dataclass(frozen=True, slots=True)
class SourceExtensionLinkInputs:
    target_triple: str
    compiler_builtins: Path | None
    sha256: str | None
    size: int | None

    def metadata(self) -> dict[str, object]:
        return {
            "schema": _SCHEMA,
            "target_triple": self.target_triple,
            "compiler_builtins": (
                {
                    "path": str(self.compiler_builtins),
                    "sha256": self.sha256,
                    "size": self.size,
                }
                if self.compiler_builtins is not None
                else None
            ),
        }


def _archive_identity(path: Path) -> dict[str, str | int]:
    if not path.is_absolute() or str(path.resolve(strict=True)) != str(path):
        raise ValueError(
            "source-extension compiler-builtins must use its canonical absolute path"
        )
    _reject_onedrive(path, "source-extension compiler-builtins")
    return stable_file_content_identity(
        path, label="source-extension compiler-builtins"
    )


def validate_source_extension_link_inputs(
    payload: object,
    *,
    target_triple: str,
) -> SourceExtensionLinkInputs:
    """Verify a captured selection without running rustc or rediscovering files."""
    if (
        not isinstance(payload, Mapping)
        or set(payload) != {"schema", "target_triple", "compiler_builtins"}
        or payload["schema"] != _SCHEMA
        or payload["target_triple"] != target_triple
    ):
        raise ValueError("source-extension link-input contract differs from the target")
    archive = payload["compiler_builtins"]
    if target_triple != "wasm32-wasip1":
        if archive is not None:
            raise ValueError(
                "non-WASI source-extension target has unexpected compiler-builtins"
            )
        return SourceExtensionLinkInputs(target_triple, None, None, None)
    if (
        not isinstance(archive, Mapping)
        or set(archive) != {"path", "sha256", "size"}
        or not isinstance(archive["path"], str)
        or not isinstance(archive["sha256"], str)
        or re.fullmatch(r"[0-9a-f]{64}", archive["sha256"]) is None
        or type(archive["size"]) is not int
        or archive["size"] < 0
    ):
        raise ValueError(
            "WASI source-extension compiler-builtins identity is malformed"
        )
    path = Path(archive["path"])
    actual = _archive_identity(path)
    if actual["sha256"] != archive["sha256"] or actual["size"] != archive["size"]:
        raise ValueError("captured source-extension compiler-builtins content changed")
    return SourceExtensionLinkInputs(
        target_triple, path, archive["sha256"], archive["size"]
    )


def resolve_source_extension_link_inputs(
    target_triple: str,
    *,
    environment: Mapping[str, str] | None = None,
) -> SourceExtensionLinkInputs:
    environment = os.environ if environment is None else environment
    if SOURCE_EXTENSION_LINK_INPUTS_ENV in environment:
        try:
            payload = loads_exact(environment[SOURCE_EXTENSION_LINK_INPUTS_ENV])
        except (TypeError, ValueError) as exc:
            raise ValueError(
                "invalid captured source-extension link-input environment"
            ) from exc
        return validate_source_extension_link_inputs(
            payload, target_triple=target_triple
        )
    if target_triple != "wasm32-wasip1":
        return SourceExtensionLinkInputs(target_triple, None, None, None)
    path = wasm_link_inputs.wasm_compiler_builtins_archive(
        target_triple, environment=environment
    )
    if path is None:
        raise ValueError(
            "WASI source-extension target requires the selected Rust compiler-builtins archive for Meson configure links"
        )
    path = path.resolve(strict=True)
    identity = _archive_identity(path)
    return SourceExtensionLinkInputs(
        target_triple, path, str(identity["sha256"]), int(identity["size"])
    )
