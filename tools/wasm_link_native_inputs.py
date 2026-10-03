"""Native-extension input resolution and wasm-ld allowlist authority."""

from __future__ import annotations

from wasm_link_fact_provider import WasmFactsProvider

from molt.wasm_artifact import skip_wasm_import_description as _parse_import_desc

from collections.abc import Mapping, Sequence
import json
from pathlib import Path
import tempfile

from molt._wasm_abi_generated import (
    WASM_EXTERNAL_NATIVE_LINK_IMPORT_PRIMITIVE_CLASSES,
    WASM_EXTERNAL_NATIVE_LINK_IMPORTS,
)
from molt._wasm_runtime_exports import _CPYTHON_ABI_LINK_IMPORT_CLASS
from molt.cli import wasm_link_inputs
from molt.cli.external_link_providers import (
    WASM_COMPILER_RT_LINK_IMPORT_CLASS,
    WASM_LIBCXX_LINK_IMPORT_CLASS,
    WASM_LIBC_LINK_IMPORT_CLASS,
    wasm_external_link_provider_symbols,
)
from molt.cli.source_extension_link_requirements import (
    SourceExtensionLinkRequirements,
    SourceExtensionLinkLoadingPolicy,
    merge_source_extension_link_requirements,
    render_source_extension_link_arguments,
    source_extension_link_file,
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


def _external_native_host_link_imports() -> tuple[str, ...]:
    generated = {
        symbol
        for symbol in WASM_EXTERNAL_NATIVE_LINK_IMPORTS
        if WASM_EXTERNAL_NATIVE_LINK_IMPORT_PRIMITIVE_CLASSES.get(symbol)
        not in {
            WASM_COMPILER_RT_LINK_IMPORT_CLASS,
            _CPYTHON_ABI_LINK_IMPORT_CLASS,
        }
    }
    provider_symbols = wasm_external_link_provider_symbols(
        primitive_classes=frozenset(
            {WASM_LIBC_LINK_IMPORT_CLASS, WASM_LIBCXX_LINK_IMPORT_CLASS}
        )
    )
    return tuple(sorted(generated | provider_symbols))


def _compiler_rt_link_imports() -> frozenset[str]:
    generated = {
        symbol
        for symbol, primitive_class in (
            WASM_EXTERNAL_NATIVE_LINK_IMPORT_PRIMITIVE_CLASSES.items()
        )
        if primitive_class == WASM_COMPILER_RT_LINK_IMPORT_CLASS
    }
    return frozenset(
        generated
        | wasm_external_link_provider_symbols(
            primitive_classes=frozenset({WASM_COMPILER_RT_LINK_IMPORT_CLASS})
        )
    )


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


def _compiler_rt_imports_required_by_native_objects(
    native_objects: Sequence[Path],
    *,
    facts_provider: WasmFactsProvider,
) -> frozenset[str]:
    compiler_rt_imports = _compiler_rt_link_imports()
    return frozenset(
        sorted(
            symbol
            for native_object in native_objects
            for symbol in _compiler_rt_imports_from_wasm(
                native_object,
                compiler_rt_imports,
                facts_provider=facts_provider,
            )
        )
    )


def _eager_native_link_paths(
    requirements: SourceExtensionLinkRequirements,
) -> tuple[Path, ...]:
    """Object files and whole archives are obligations before linker selection."""
    eager = []
    for item in requirements.inputs:
        path = Path(item.path)
        with path.open("rb") as stream:
            archive = stream.read(len(AR_MAGIC)) == AR_MAGIC
        if not archive or item.loading is SourceExtensionLinkLoadingPolicy.ALL_MEMBERS:
            eager.append(path)
    return tuple(dict.fromkeys(eager))


def _compiler_rt_provider_inputs(
    native_objects: Sequence[Path],
    required_symbols: frozenset[str],
    candidate_symbols: frozenset[str],
) -> tuple[Path, ...]:
    if not candidate_symbols:
        return ()
    provider = wasm_link_inputs.wasm_compiler_builtins_archive()
    if provider is None:
        if not required_symbols:
            return ()
        missing = ", ".join(sorted(required_symbols))
        raise ValueError(
            "wasm_compiler_rt_link_import symbols require Rust wasm32-wasip1 "
            f"libcompiler_builtins provider; missing provider for: {missing}"
        )
    try:
        provider = provider.resolve(strict=True)
    except OSError as exc:
        raise ValueError(
            f"wasm_compiler_rt_link_import provider does not exist: {provider}"
        ) from exc
    for native_object in native_objects:
        if native_object.expanduser().absolute() == provider:
            return ()
    return (provider,)


def _resolve_native_link_requirements(
    requirements: SourceExtensionLinkRequirements,
    *,
    source_paths: Mapping[Path, Path],
    facts_provider: WasmFactsProvider,
) -> SourceExtensionLinkRequirements:
    """Resolve providers from captured bytes and exact original input identities."""
    native_inputs = tuple(Path(item.path) for item in requirements.inputs)
    candidates = _compiler_rt_imports_required_by_native_objects(
        native_inputs, facts_provider=facts_provider
    )
    required = _compiler_rt_imports_required_by_native_objects(
        _eager_native_link_paths(requirements),
        facts_provider=facts_provider,
    )
    providers = _compiler_rt_provider_inputs(
        tuple(source_paths[path] for path in native_inputs), required, candidates
    )
    if not providers:
        return requirements
    return merge_source_extension_link_requirements(
        (
            requirements,
            SourceExtensionLinkRequirements(
                requirements.target_triple,
                tuple(source_extension_link_file(path) for path in providers),
            ),
        ),
        target_triple=requirements.target_triple,
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
) -> list[str]:
    """wasm-ld args for the SPLIT app link, overriding wasi-libc's ``%L`` stub.

    The split ``app.wasm`` statically links numpy/scipy + their own wasi-libc
    ``libc.a`` but â€” unlike the combined ``output_linked.wasm`` â€” does NOT link
    the reloc runtime object, so numpy's ``NumPyOS_ascii_formatl`` ->
    ``snprintf("%Lg")`` binds ``libc.a``'s ``long_double_not_supported`` stub
    (raw ``unreachable`` trap at ``_multiarray_umath`` import).

    Applies the SINGLE long-double link authority
    (:func:`wasm_link_inputs.resolve_long_double_link_policy` +
    :func:`wasm_link_inputs.long_double_whole_archive_link_argv`) â€” the SAME policy
    the reloc runtime and deploy cdylib links apply: whole-archive
    ``libc-printscan-long-double.a`` ahead of ``libc.a`` so its real
    ``vfprintf``/``__floatscan``/``strtold`` override the stub objects (they stay
    lazy once defined), + the binary128 soft-float builtins. Scoped to the split
    app ONLY: the combined link already carries these from the reloc runtime, so
    whole-archiving there duplicate-symbols. Non-numpy builds (no ``libc.a``) get
    the plain passthrough.
    """
    inputs = [Path(item.path) for item in requirements.inputs]
    if not any(path.name == "libc.a" for path in inputs):
        return list(render_source_extension_link_arguments(requirements))
    # libc.a present => numpy/scipy static tier: a missing formatter archive is a
    # HARD ERROR (relinking the abort stub would trap at import).
    policy = wasm_link_inputs.resolve_long_double_link_policy(required=True)
    if policy.error is not None:
        raise ValueError(policy.error)
    return [
        *wasm_link_inputs.long_double_whole_archive_link_argv(
            policy, whole_archive=[], trailing=[]
        ),
        *render_source_extension_link_arguments(requirements),
    ]


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
    temp_dir: tempfile.TemporaryDirectory[str],
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
    temp_dir: tempfile.TemporaryDirectory,
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
            *_external_native_host_link_imports(),
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
    temp_dir: tempfile.TemporaryDirectory,
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
            *_external_native_host_link_imports(),
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
