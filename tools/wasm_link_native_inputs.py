"""Native-extension input resolution and wasm-ld allowlist authority."""

from __future__ import annotations


from tools.wasm_link_fact_provider import WasmFactsProvider


from collections.abc import Callable, Mapping
from dataclasses import dataclass
from types import MappingProxyType
from pathlib import Path
from molt.temporary_artifacts import OwnedTemporaryDirectory

from molt._wasm_abi_generated import (
    WASM_EXTERNAL_NATIVE_LINK_IMPORT_PRIMITIVE_CLASSES,
    WASM_EXTERNAL_NATIVE_LINK_IMPORTS,
)
from molt._wasm_runtime_exports import _CPYTHON_ABI_LINK_IMPORT_CLASS
from molt.cli import wasm_link_inputs
from molt.wasi_sdk_identity import WasiCAbiProjection
from molt.cli.external_link_providers import (
    WASM_COMPILER_RT_LINK_IMPORT_CLASS,
    wasm_external_link_provider_symbols,
)
from molt.cli.source_extension_link_requirements import (
    SourceExtensionLinkRequirements,
    SourceExtensionLinkInput,
    SourceExtensionLinkLoadingPolicy,
    merge_source_extension_link_requirements,
)
from tools.wasm_archive import AR_MAGIC, iter_wasm_object_members


def _read_link_allowlist_symbols(path: Path) -> list[str]:
    return [
        line.strip()
        for line in path.read_text(encoding="utf-8").splitlines()
        if line.strip() and not line.strip().startswith("#")
    ]


def _external_native_host_link_imports(
    provider_symbols: frozenset[str] = frozenset(),
) -> tuple[str, ...]:
    generated = {
        symbol
        for symbol in WASM_EXTERNAL_NATIVE_LINK_IMPORTS
        if WASM_EXTERNAL_NATIVE_LINK_IMPORT_PRIMITIVE_CLASSES.get(symbol)
        not in {WASM_COMPILER_RT_LINK_IMPORT_CLASS, _CPYTHON_ABI_LINK_IMPORT_CLASS}
    }
    return tuple(sorted(generated | provider_symbols))


@dataclass(frozen=True)
class ResolvedNativeLinkInputs:
    requirements: SourceExtensionLinkRequirements
    provider_paths: Mapping[str, Path]
    provider_symbols: frozenset[str]
    host_symbols: frozenset[str]


def native_link_input_is_eager(item: SourceExtensionLinkInput) -> bool:
    """An object is eager regardless of suffix; archives obey declared loading."""
    with Path(item.path).open("rb") as stream:
        archive = stream.read(len(AR_MAGIC)) == AR_MAGIC
    return not archive or item.loading is SourceExtensionLinkLoadingPolicy.ALL_MEMBERS


def _resolve_native_link_requirements(
    requirements: SourceExtensionLinkRequirements,
    *,
    source_paths: Mapping[Path, Path],
    capture_input: Callable[[SourceExtensionLinkInput], SourceExtensionLinkInput],
    facts_provider: WasmFactsProvider,
    runtime_exports: frozenset[str] = frozenset(),
    wasi_plan: WasiCAbiProjection | None = None,
) -> ResolvedNativeLinkInputs:
    """Plan the complete provider closure before the snapshot transaction closes.

    Object/member facts are scanned once. Only unresolved, noncanonical names
    justify archive discovery; dormant lazy members alone never require an SDK.
    Original paths admit SDK roles, while requirements retain captured bytes.
    """
    originals = tuple(source_paths[Path(item.path)] for item in requirements.inputs)
    plan = wasi_plan or wasm_link_inputs.admit_wasi_provider_inputs(originals)
    imported: set[str] = set()
    eager_imported: set[str] = set()
    defined: set[str] = set()
    input_definitions: dict[Path, set[str]] = {}
    for item in requirements.inputs:
        path = Path(item.path)
        eager = native_link_input_is_eager(item)
        local_definitions = input_definitions.setdefault(path, set())
        for member in iter_wasm_object_members(path):
            facts = facts_provider(member.data)
            names = {fact.name for fact in facts.imports if fact.kind == 0}
            names.update(facts.linking_symbols.undefined_functions)
            imported.update(names)
            if eager:
                eager_imported.update(names)
            local_definitions.update(facts.linking_symbols.defined_functions)
            imported_functions = sum(fact.kind == 0 for fact in facts.imports)
            local_definitions.update(
                name
                for name, index in facts.function_exports.items()
                if index >= imported_functions
            )
        defined.update(local_definitions)
    resolved = defined | set(WASM_EXTERNAL_NATIVE_LINK_IMPORTS) | set(runtime_exports)
    candidates = imported - resolved
    required = eager_imported - resolved
    if plan is None and candidates:
        if required or wasm_link_inputs.resolve_wasi_sysroot() is not None:
            plan = wasm_link_inputs.resolve_wasi_c_abi_plan()
    if plan is None:
        return ResolvedNativeLinkInputs(
            requirements, MappingProxyType({}), frozenset(), frozenset()
        )

    member_digests = {path: digest for _role, path, _size, digest in plan.files}
    for item, original in zip(requirements.inputs, originals, strict=True):
        if original in member_digests and item.sha256 != member_digests[original]:
            raise ValueError(
                f"captured SDK provider differs from admitted C ABI member: {original}"
            )
    original_items = {
        source_paths[Path(item.path)]: item for item in requirements.inputs
    }
    roles = {
        role: path
        for role, path, _size, _digest in plan.files
        if role in {"libc", "long_double", "compiler_rt"} and path in original_items
    }
    if "libc" in roles or "long_double" in roles:
        roles.update(
            (role, plan.path(role)) for role in ("libc", "long_double", "compiler_rt")
        )
    captured: dict[Path, SourceExtensionLinkInput] = {}

    def capture_provider(path: Path) -> SourceExtensionLinkInput:
        if path not in captured:
            captured[path] = original_items.get(path) or capture_input(
                SourceExtensionLinkInput(str(path), member_digests[path])
            )
        return captured[path]

    for path in roles.values():
        capture_provider(path)
    compiler_symbols = frozenset()
    if candidates or "compiler_rt" in roles:
        provider = plan.path("compiler_rt")
        item = capture_provider(provider)
        if provider in original_items:
            compiler_symbols = frozenset(input_definitions[Path(item.path)])
        else:
            compiler_symbols = wasm_external_link_provider_symbols(
                primitive_classes=frozenset({WASM_COMPILER_RT_LINK_IMPORT_CLASS}),
                plan=plan,
                archive_paths={provider: Path(item.path)},
            )
        if candidates & compiler_symbols:
            roles["compiler_rt"] = provider
    host_symbols: set[str] = set()
    for original, item in original_items.items():
        if original.name in {
            "libc.a",
            "libc-printscan-long-double.a",
            "libc++.a",
            "libc++abi.a",
            "libunwind.a",
        }:
            host_symbols.update(input_definitions[Path(item.path)])
    # Staged C providers use captured members once; no live provider can join
    # after this plan. Originally supplied members already have symbol facts.
    for role in ("libc", "long_double"):
        path = roles.get(role)
        if path is None or path in original_items:
            continue
        for member in iter_wasm_object_members(Path(captured[path].path)):
            host_symbols.update(
                facts_provider(member.data).linking_symbols.defined_functions
            )
    providers = tuple(
        captured[path]
        for path in dict.fromkeys(roles.values())
        if path not in original_items
    )
    merged = (
        merge_source_extension_link_requirements(
            (
                requirements,
                SourceExtensionLinkRequirements(requirements.target_triple, providers),
            ),
            target_triple=requirements.target_triple,
        )
        if providers
        else requirements
    )
    return ResolvedNativeLinkInputs(
        merged,
        MappingProxyType(
            {role: Path(captured[path].path) for role, path in roles.items()}
        ),
        frozenset(host_symbols) | compiler_symbols,
        frozenset(host_symbols),
    )


def _compose_wasm_ld_allowlist(
    *,
    base_allowlist: Path,
    native_link_requirements: SourceExtensionLinkRequirements,
    temp_dir: OwnedTemporaryDirectory,
    provider_symbols: frozenset[str] = frozenset(),
) -> Path:
    """Return the wasm-ld allowlist for this link transaction.

    The checked-in allowlist is the runtime/user-program import contract.  Native
    package objects need the generated external-native toolchain/libc/C++ import
    surface too; keep that authority generated and transaction-local so the base
    runtime allowlist does not grow a second copy of package closure policy.
    """
    if not native_link_requirements.inputs:
        return base_allowlist
    symbols = sorted(
        {
            *_read_link_allowlist_symbols(base_allowlist),
            *_external_native_host_link_imports(provider_symbols),
        }
    )
    composed = Path(temp_dir.name) / "wasm_allowed_imports.external_native.txt"
    composed.write_text(
        "\n".join(
            [
                "# @generated transaction-local by tools/wasm_link_native_inputs.py",
                "# runtime allowlist + generated external native link imports",
                *symbols,
                "",
            ]
        ),
        encoding="utf-8",
    )
    return composed
