"""Admit external link obligations from extracted members, never inventories.

The linker chooses members. The shared symbol reader owns their byte identities,
and the existing C-API surface owns support. This module joins those authorities
before publication; it does not implement a second archive-resolution algorithm.
"""

from __future__ import annotations

import hashlib
import contextlib
from dataclasses import dataclass
from pathlib import Path
from typing import Iterator, Mapping

from molt.c_api_symbols import (
    is_c_api_external_requirement,
    is_cpython_abi_dynamic_import_symbol,
)
from molt.cli.extension_scan_surface import _load_c_api_scan_surface
from molt.cli.extension_scan_surface import _ExtensionScanSurface
from molt.cli.extension_scan_surface import runtime_owned_link_symbol
from molt.cli.native_link_plan import NativeLinkPlan, NativeLinkerKind, LinkDialect
from molt.cli.link_member_selection import selected_archive_members
from molt.cli.native_symbol_inspection import (
    _NativeGlobalSymbolFacts,
    _native_archive_global_symbol_facts,
    _native_object_global_symbol_facts,
    _symbol_artifact_members,
)
from molt.cli.source_extension_link_requirements import (
    SourceExtensionLinkLoadingPolicy,
    SourceExtensionLinkRequirements,
    SourceExtensionLinkProviderKind,
)
from molt.exact_json import encode_exact, read_exact
from molt.source_root import compiler_source_root
from molt.toolchain_identity import (
    StableRegularFileIdentity,
    stable_regular_file_identity,
    verify_stable_regular_file_identity,
)


LINK_SELECTION_SCHEMA = "molt.link-member-selection.v1"


def link_selection_policy(
    surface: _ExtensionScanSurface | None = None,
) -> dict[str, str]:
    """Include current support and admission semantics in final-link cache keys."""
    surface = surface or _support_surface()
    payload = {
        "schema": LINK_SELECTION_SCHEMA,
        "runtime_backed": sorted(surface.runtime_backed),
        "source_compile_only": sorted(surface.source_compile_only),
        "fail_fast": sorted(surface.fail_fast),
    }
    return {
        "selection_policy": hashlib.sha256(
            encode_exact(payload, indent=None)
        ).hexdigest()
    }


def _support_surface() -> _ExtensionScanSurface:
    root = compiler_source_root()
    surface, header, error = _load_c_api_scan_surface(
        root, header_path=root / "runtime/molt-cpython-abi/include/Python.h"
    )
    if surface is None:
        raise ValueError(
            f"Cannot admit final-link C-API requirements: {header}: {error}"
        )
    return surface


@dataclass(frozen=True)
class LinkSelectionAdmission:
    requirements: SourceExtensionLinkRequirements
    identities: Mapping[Path, StableRegularFileIdentity]
    facts: Mapping[Path, _NativeGlobalSymbolFacts]
    surface: _ExtensionScanSurface

    @classmethod
    def capture(
        cls,
        requirements: SourceExtensionLinkRequirements,
        *,
        surface: _ExtensionScanSurface | None = None,
    ) -> LinkSelectionAdmission:
        unbound = [
            provider.name
            for provider in requirements.providers
            if provider.provider_kind
            is not SourceExtensionLinkProviderKind.THREAD_RUNTIME
        ]
        if unbound:
            raise ValueError(
                "External link admission requires checksummed static inputs; "
                "unbound library/archive/framework providers cannot be verified: "
                + ", ".join(unbound)
                + ". Record the resolved static files in the extension link plan."
            )
        identities: dict[Path, StableRegularFileIdentity] = {}
        facts: dict[Path, _NativeGlobalSymbolFacts] = {}
        for item in requirements.inputs:
            path = Path(item.path).resolve(strict=True)
            if path not in identities:
                identity = stable_regular_file_identity(
                    path, label="external link input"
                )
                identities[path] = identity
                with path.open("rb") as stream:
                    is_wasm_object = stream.read(8) == b"\0asm\x01\0\0\0"
                if is_wasm_object:
                    from molt.cli.source_extensions import (
                        _inspect_source_extension_artifact_symbols,
                        _wasm_relocatable_external_symbols,
                    )

                    inspection = _inspect_source_extension_artifact_symbols(path)
                    if (
                        inspection is None
                        or inspection.artifact_digest != identity.sha256
                    ):
                        raise ValueError(
                            f"Cannot inspect stable WASM link input: {path}"
                        )
                    facts[path] = _NativeGlobalSymbolFacts(
                        defined=inspection.defined_symbols,
                        undefined=frozenset(
                            _wasm_relocatable_external_symbols(inspection) or ()
                        ),
                        defined_functions=inspection.defined_function_symbols,
                    )
                    verify_stable_regular_file_identity(
                        identity, label="external link input"
                    )
                else:
                    reader = (
                        _native_archive_global_symbol_facts
                        if _symbol_artifact_members(path) is not None
                        else _native_object_global_symbol_facts
                    )
                    facts[path] = reader(
                        path,
                        target_triple=requirements.target_triple,
                        identity=identity,
                    )
            if identities[path].sha256 != item.sha256:
                raise ValueError(
                    f"External link input changed before selection: {path}"
                )
        return cls(requirements, identities, facts, surface or _support_surface())

    @property
    def lazy_archives(self) -> Mapping[Path, _NativeGlobalSymbolFacts]:
        eager = {
            Path(item.path).resolve()
            for item in self.requirements.inputs
            if item.loading is SourceExtensionLinkLoadingPolicy.ALL_MEMBERS
        }
        return {
            path: facts
            for path, facts in self.facts.items()
            if facts.members is not None and path not in eager
        }

    def admit(
        self,
        *,
        dialect: str,
        stdout: str,
        stderr: str,
        why_extract: str | None = None,
    ) -> dict[str, object]:
        archives = self.lazy_archives
        extracted = (
            selected_archive_members(
                archives,
                dialect=dialect,
                stdout=stdout,
                stderr=stderr,
                why_extract=why_extract,
            )
            if archives
            else {}
        )
        selected: list[_NativeGlobalSymbolFacts] = []
        inputs: list[dict[str, object]] = []
        for input_index, (path, facts) in enumerate(self.facts.items()):
            identity = self.identities[path]
            verify_stable_regular_file_identity(identity, label="external link input")
            row: dict[str, object] = {
                "input_index": input_index,
                "sha256": identity.sha256,
            }
            if facts.members is None:
                selected.append(facts)
                row["members"] = None
            else:
                ordinals = (
                    tuple(member.identity.ordinal for member in facts.members)
                    if path not in archives
                    else extracted[path]
                )
                by_ordinal = {
                    member.identity.ordinal: member for member in facts.members
                }
                members = [by_ordinal[ordinal] for ordinal in ordinals]
                selected.extend(member.symbols for member in members)
                row["members"] = [
                    {
                        "ordinal": member.identity.ordinal,
                        "name": member.identity.member.name,
                        "sha256": member.identity.sha256,
                    }
                    for member in members
                ]
            inputs.append(row)
        defined = frozenset().union(*(fact.defined for fact in selected))
        requirements = frozenset().union(*(fact.undefined for fact in selected))
        weak_requirements = frozenset().union(
            *(fact.weak_undefined for fact in selected)
        )
        # Check ownership before subtraction: a dependency's weak or common
        # definition must not hide a runtime collision or CPython import library.
        dynamic = {
            symbol
            for symbol in defined | requirements | weak_requirements
            if is_cpython_abi_dynamic_import_symbol(symbol)
        }
        reserved = {
            symbol
            for symbol in defined
            if runtime_owned_link_symbol(
                symbol,
                self.surface,
                wasm=self.requirements.target_triple.startswith("wasm"),
            )
        }
        if dynamic or reserved:
            raise ValueError(
                "Selected external link members collide with canonical runtime/link/C-API "
                "authority or retain dynamic CPython imports: "
                + ", ".join(sorted(dynamic | reserved))
            )
        undefined = requirements - defined
        rejected = {
            symbol: self.surface.link_status_for(symbol)
            for symbol in sorted(undefined)
            if is_c_api_external_requirement(symbol)
            and self.surface.link_status_for(symbol) != "runtime_backed"
        }
        if rejected:
            raise ValueError(
                "Selected external link members require unsupported C-API symbols: "
                + ", ".join(
                    f"{symbol} ({status})" for symbol, status in rejected.items()
                )
            )
        return {
            "schema": LINK_SELECTION_SCHEMA,
            "selection": "extracted-before-dead-stripping",
            **link_selection_policy(self.surface),
            "inputs": inputs,
        }


def write_link_selection(path: Path, roles: Mapping[str, Mapping[str, object]]) -> None:
    """Write only a private candidate; final-link publication owns its visibility."""
    path.write_bytes(
        encode_exact({"schema": LINK_SELECTION_SCHEMA, "roles": roles}, indent=None)
    )


def validate_link_selection_policy(
    path: Path, policy: Mapping[str, str], *, roles: set[str]
) -> None:
    """Bind subprocess admission to the support snapshot used for cache identity."""
    payload = read_exact(path, max_bytes=64 * 1024 * 1024, label="link selection")
    recorded = payload.get("roles") if isinstance(payload, dict) else None
    if (
        not isinstance(recorded, dict)
        or payload.get("schema") != LINK_SELECTION_SCHEMA
        or set(recorded) != roles
        or any(
            not isinstance(row, dict)
            or row.get("schema") != LINK_SELECTION_SCHEMA
            or row.get("selection_policy") != policy["selection_policy"]
            for row in recorded.values()
        )
    ):
        raise ValueError(
            "WASM link selection policy or role evidence differs from the planned generation"
        )


@dataclass(frozen=True)
class NativeLinkSelection:
    admission: LinkSelectionAdmission
    dialect: str
    arguments: tuple[str, ...]
    why_extract: Path | None

    def admit(self, *, stdout: str, stderr: str, output: Path) -> None:
        proof = self.admission.admit(
            dialect=self.dialect,
            stdout=stdout,
            stderr=stderr,
            why_extract=self.why_extract.read_text(encoding="utf-8")
            if self.why_extract
            else None,
        )
        write_link_selection(output, {"binary": proof})


@contextlib.contextmanager
def native_link_selection(
    plan: NativeLinkPlan,
    candidate: Path,
    *,
    surface: _ExtensionScanSurface | None = None,
) -> Iterator[NativeLinkSelection | None]:
    requirements = plan.selection_requirements
    if requirements is None:
        yield None
        return
    admission = LinkSelectionAdmission.capture(requirements, surface=surface)
    if not admission.lazy_archives:
        yield NativeLinkSelection(admission, plan.target.link_dialect.value, (), None)
        return
    # Selection formats are a linker capability, not an OS/host-name heuristic.
    # Other linker families need their own demonstrated extraction contract.
    if plan.capabilities.linker is not NativeLinkerKind.LLD:
        raise ValueError(
            "External member admission requires the LLVM linker extraction capability"
        )
    dialect = plan.target.link_dialect
    why_extract = None
    if dialect is LinkDialect.ELF_GNU:
        why_extract = candidate.with_name(candidate.name + ".why-extract")
        flags = ("--trace", f"--why-extract={why_extract}")
    elif dialect is LinkDialect.MACHO:
        flags = ("-t",)
    elif dialect is LinkDialect.COFF_MSVC:
        flags = ("/verbose",)
    else:
        raise ValueError(
            f"External member extraction is not verified for {dialect.value}"
        )
    selection = NativeLinkSelection(
        admission,
        dialect.value,
        tuple(part for flag in flags for part in ("-Xlinker", flag)),
        why_extract,
    )
    try:
        yield selection
    finally:
        if why_extract is not None:
            why_extract.unlink(missing_ok=True)
