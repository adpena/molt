"""Native-extension input resolution and wasm-ld allowlist authority."""

from __future__ import annotations

from wasm_link_fact_provider import WasmFactsProvider

from molt.wasm_artifact import skip_wasm_import_description as _parse_import_desc

from collections.abc import Callable, Mapping, Sequence
from dataclasses import dataclass, replace
from types import MappingProxyType
import json
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
    SourceExtensionLinkCyclicGroup,
    SourceExtensionLinkInput,
    SourceExtensionLinkLoadingPolicy,
    merge_source_extension_link_requirements,
    render_source_extension_link_arguments,
)
from wasm_link_export_contract import (
    _TRAP_FUNC_BODY,
    _function_body_payloads_by_index,
)
from wasm_archive import AR_MAGIC, iter_wasm_object_members
from wasm_link_format import (
    _read_string,
    _read_varuint,
    _write_string,
    _write_varuint,
)
from wasm_link_operations import build_sections, parse_sections


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


def _compiler_rt_imports_from_wasm(
    path: Path,
    compiler_rt_imports: frozenset[str],
    *,
    facts_provider: WasmFactsProvider,
) -> frozenset[str]:
    return frozenset(
        fact.name
        for member in iter_wasm_object_members(path)
        for fact in facts_provider(member.data).imports
        if fact.kind == 0 and fact.name in compiler_rt_imports
    )


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


def _sealed_native_init_symbols(native_objects: Sequence[Path]) -> tuple[str, ...]:
    symbols: set[str] = set()
    for native_object in native_objects:
        manifest_path = native_object.with_name(
            native_object.name + ".extension_manifest.json"
        )
        if not manifest_path.exists():
            continue
        try:
            payload = json.loads(manifest_path.read_text(encoding="utf-8"))
        except (OSError, json.JSONDecodeError) as exc:
            raise ValueError(
                f"sealed native extension manifest is unreadable: {manifest_path}: {exc}"
            ) from exc
        init_symbol = payload.get("init_symbol")
        if not isinstance(init_symbol, str) or not init_symbol.startswith("PyInit_"):
            raise ValueError(
                "sealed native extension manifest has invalid init_symbol: "
                f"{manifest_path}: {init_symbol!r}"
            )
        symbols.add(init_symbol)
    return tuple(sorted(symbols))


def _split_app_native_link_args(
    requirements: SourceExtensionLinkRequirements,
    *,
    provider_paths: Mapping[str, Path],
) -> list[str]:
    """Force the captured formatter once in the app; other providers stay lazy."""
    formatter = provider_paths.get("long_double")
    if formatter is None:
        return list(render_source_extension_link_arguments(requirements))
    formatter_items = tuple(
        item for item in requirements.inputs if Path(item.path) == formatter
    )
    if len(formatter_items) != 1:
        raise ValueError("split app formatter must be captured exactly once")

    def retained(item):
        return (
            not isinstance(item, SourceExtensionLinkInput)
            or Path(item.path) != formatter
        )

    rest = []
    for item in requirements.items:
        if isinstance(item, SourceExtensionLinkCyclicGroup):
            members = tuple(member for member in item.members if retained(member))
            if members:
                rest.append(replace(item, members=members))
        elif retained(item):
            rest.append(item)
    selected = replace(
        requirements,
        items=(
            replace(
                formatter_items[0], loading=SourceExtensionLinkLoadingPolicy.ALL_MEMBERS
            ),
            *rest,
        ),
    )
    return list(render_source_extension_link_arguments(selected))


def _required_native_direct_symbols(
    output_data: bytes, *, facts_provider: WasmFactsProvider
) -> tuple[str, ...]:
    return tuple(
        sorted(
            {
                wasm_import.name
                for wasm_import in facts_provider(output_data).imports
                if wasm_import.module == "molt_native" and wasm_import.kind == 0
            }
        )
    )


def _rewrite_required_native_direct_imports(
    module_path: Path,
    required_symbols: Sequence[str],
    temp_dir: OwnedTemporaryDirectory,
) -> Path:
    required = set(required_symbols)
    if not required:
        return module_path
    changed = False
    rebuilt_sections: list[tuple[int, bytes]] = []
    for section_id, payload in parse_sections(module_path.read_bytes()):
        if section_id != 2:
            rebuilt_sections.append((section_id, payload))
            continue
        count, offset = _read_varuint(payload, 0)
        rebuilt = bytearray(_write_varuint(count))
        for _ in range(count):
            module, offset = _read_string(payload, offset)
            name, offset = _read_string(payload, offset)
            if offset >= len(payload):
                raise ValueError("Unexpected EOF while reading import kind")
            kind = payload[offset]
            desc_start = offset + 1
            offset = _parse_import_desc(payload, desc_start, kind)
            desc = payload[desc_start:offset]
            if module == "molt_native" and kind == 0 and name in required:
                module = "env"
                changed = True
            rebuilt.extend(_write_string(module))
            rebuilt.extend(_write_string(name))
            rebuilt.append(kind)
            rebuilt.extend(desc)
        rebuilt_sections.append((section_id, bytes(rebuilt)))
    if not changed:
        return module_path
    rewritten_path = Path(temp_dir.name) / "output_native_direct_imports.wasm"
    rewritten_path.write_bytes(build_sections(rebuilt_sections))
    return rewritten_path


def _validate_required_native_direct_symbols(
    linked_data: bytes,
    required_symbols: Sequence[str],
    *,
    description: str,
    facts_provider: WasmFactsProvider,
) -> str | None:
    if not required_symbols:
        return None
    exports = facts_provider(linked_data).function_exports
    bodies = _function_body_payloads_by_index(
        linked_data, facts_provider=facts_provider
    )
    missing: list[str] = []
    unresolved: list[str] = []
    trap_stubs: list[str] = []
    for symbol in required_symbols:
        func_index = exports.get(symbol)
        if func_index is None:
            missing.append(symbol)
            continue
        body = bodies.get(func_index)
        if body is None:
            unresolved.append(symbol)
        elif body == _TRAP_FUNC_BODY:
            trap_stubs.append(symbol)
    if not (missing or unresolved or trap_stubs):
        return None
    parts: list[str] = []
    if missing:
        parts.append("missing export(s): " + ", ".join(missing))
    if unresolved:
        parts.append("exported unresolved import(s): " + ", ".join(unresolved))
    if trap_stubs:
        parts.append("trap stub(s): " + ", ".join(trap_stubs))
    return f"{description} did not link required native direct symbol(s): " + "; ".join(
        parts
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


def _compose_split_runtime_native_allowlist(
    *,
    base_allowlist: Path,
    native_link_requirements: SourceExtensionLinkRequirements,
    split_runtime_exports: set[str],
    temp_dir: OwnedTemporaryDirectory,
    provider_symbols: frozenset[str] = frozenset(),
) -> Path:
    """Return the deployed split-app allowlist for static native extensions.

    The monolithic validation link resolves Molt ABI symbols against the
    relocatable runtime under their canonical C names. The deployed split app
    deliberately leaves those same symbols as ``molt_runtime`` imports under
    their split export names, so wasm-ld must allow exactly the export surface
    of the runtime the app deploys with (``split_runtime_exports``, the deploy
    runtime's export section) for that transaction-local app link. The
    relocatable runtime's defined names are the wrong authority here: they
    spell the CPython ABI canonically (``PyType_Ready``) while the split app
    imports ``molt_PyType_Ready``.
    """
    if not native_link_requirements.inputs:
        return base_allowlist
    symbols = sorted(
        {
            *_read_link_allowlist_symbols(base_allowlist),
            *_external_native_host_link_imports(provider_symbols),
            *split_runtime_exports,
        }
    )
    composed = Path(temp_dir.name) / "wasm_allowed_imports.split_runtime_native.txt"
    composed.write_text(
        "\n".join(
            [
                "# @generated transaction-local by tools/wasm_link_native_inputs.py",
                "# split-runtime native app imports: host + external-native + runtime ABI",
                *symbols,
                "",
            ]
        ),
        encoding="utf-8",
    )
    return composed
