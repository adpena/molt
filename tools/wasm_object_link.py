"""Link a standard LLVM wasm32 application object with wasm-ld.

An LLVM wasm32 object is a standard relocatable object: wasm-ld resolves its
runtime references against the relocatable runtime member and its stdlib
functions against the shared stdlib archive, with no object surgery, fact
scan or table layout of its own. A function pointer is a slot of the one
table wasm-ld lays out, and the runtime calls it directly (HF-699 K2).

``tools/wasm_link.py`` admits every input (producer digests, the trusted
runtime generation, the native link plan) and owns publication; this module
owns only the link and the optimizer pass of a standard object.
"""

from __future__ import annotations

import subprocess
import sys
import time
from collections.abc import Mapping, MutableMapping, Sequence
from dataclasses import dataclass
from pathlib import Path

from molt._wasm_runtime_exports import wasm_output_runtime_export_names
from molt.cli.app_export_contract import (
    app_export_symbol_map,
    exported_app_symbols,
    load_app_export_contract,
)
from molt._wasm_abi_generated import compiled_execution_symbol
from molt.cli.source_extension_link_requirements import (
    SourceExtensionLinkRequirements,
    render_source_extension_link_arguments,
)
from molt.wasm_artifact import (
    _collect_wasm_export_names,
    strip_wasm_publication_sections,
)
from molt.wasm_optimizer_identity import (
    build_wasm_optimizer_attestation,
    encode_wasm_optimizer_attestation,
    wasm_optimizer_attestation_path,
    wasm_optimizer_invocation_identity,
)
from molt.temporary_artifacts import OwnedTemporaryDirectory
from tools import wasm_link_native_inputs as _native_inputs
from tools.wasm_link_command import _run_external_tool
from tools.wasm_link_fact_provider import make_rust_wasm_facts_provider
from tools.wasm_link_format import (
    _ESSENTIAL_EXPORTS,
    _build_sections,
    _parse_sections,
    _read_string,
    _read_varuint,
    _write_string,
    _write_varuint,
)
from tools.wasm_link_optimizer_policy import _run_wasm_opt_via_optimize


@dataclass(frozen=True)
class NativeLink:
    """A link's source-extension inputs with their SDK provider closure, and
    the allowlist that admits the host imports those providers leave."""

    requirements: SourceExtensionLinkRequirements
    allowlist: Path


def resolve_native_link(
    requirements: SourceExtensionLinkRequirements,
    *,
    runtime: Path,
    wasm_facts_scanner: Path,
    base_allowlist: Path,
    scratch: OwnedTemporaryDirectory,
    evidence_root: Path,
    phase_timings_ms: dict[str, float],
) -> NativeLink:
    """Close the extension objects over the SDK members they need (libc, the
    long-double formatter, compiler-rt), as the final WASM link does
    (`wasm_link_native_inputs._resolve_native_link_requirements`). A link with
    no extension object passes through unchanged."""
    if not requirements.inputs:
        return NativeLink(requirements, base_allowlist)
    facts = make_rust_wasm_facts_provider(
        wasm_facts_scanner,
        Path(scratch.name),
        phase_timings_ms,
        evidence_root=evidence_root,
    )
    plan = _native_inputs._resolve_native_link_requirements(
        requirements,
        source_paths={Path(item.path): Path(item.path) for item in requirements.inputs},
        # The SDK plan admitted each provider member by digest; the link reads
        # them in place.
        capture_input=lambda item: item,
        facts_provider=facts,
        runtime_exports=frozenset(
            facts(runtime.read_bytes()).linking_symbols.defined_names
        ),
    )
    allowlist = _native_inputs._compose_wasm_ld_allowlist(
        base_allowlist=base_allowlist,
        native_link_requirements=plan.requirements,
        temp_dir=scratch,
        provider_symbols=plan.host_symbols,
    )
    return NativeLink(plan.requirements, allowlist)


def run_selected_link(
    command: Sequence[str],
    requirements: SourceExtensionLinkRequirements,
    *,
    role: str,
    why_extract: Path,
    selection_roles: MutableMapping[str, Mapping[str, object]],
) -> subprocess.CompletedProcess[str]:
    """Run one wasm-ld command and record which extension archive members it
    extracted (`link_selection_admission`), as the final WASM link does. A
    link with no extension input records nothing."""
    from molt.cli.link_selection_admission import LinkSelectionAdmission

    selection = (
        LinkSelectionAdmission.capture(requirements) if requirements.items else None
    )
    arguments = list(command)
    if selection is not None and selection.lazy_archives:
        arguments.extend(("--trace", f"--why-extract={why_extract}"))
    try:
        result = _run_external_tool(arguments, capture_output=True, text=True)
        if result.returncode == 0 and selection is not None:
            selection_roles[role] = selection.admit(
                dialect="wasm",
                stdout=result.stdout,
                stderr=result.stderr,
                why_extract=why_extract.read_text(encoding="utf-8")
                if selection.lazy_archives
                else None,
            )
    finally:
        why_extract.unlink(missing_ok=True)
    return result


# The linear-memory shadow stack, placed first so an overflow traps on the
# null page instead of corrupting static data.
SHADOW_STACK_BYTES = 1 << 20


def standard_object_exports(app_export_contract: Path) -> tuple[str, ...]:
    """The module's export roots: the host ABI, the app's compiled entries
    and the user exports its contract names."""
    contract = load_app_export_contract(app_export_contract)
    return tuple(
        sorted(
            {
                *_ESSENTIAL_EXPORTS,
                *wasm_output_runtime_export_names(),
                *exported_app_symbols(contract),
            }
        )
    )


def publish_app_export_names(data: bytes, app_export_contract: Path) -> bytes:
    """Export each app function under its public name, not its symbol.

    wasm-ld exports a definition under its symbol only. The contract maps
    each public export name (external ABI) to the compiled symbol; this pass
    renames those export entries. A public name another export already holds,
    or a contract export the link did not keep, is refused.
    """
    public_by_symbol = {
        symbol: public
        for public, symbol in app_export_symbol_map(
            load_app_export_contract(app_export_contract)
        ).items()
    }
    if not public_by_symbol:
        return data
    sections = _parse_sections(data)
    rebuilt: list[tuple[int, bytes]] = []
    renamed: set[str] = set()
    for section_id, payload in sections:
        if section_id != 7:
            rebuilt.append((section_id, bytes(payload)))
            continue
        count, offset = _read_varuint(payload, 0)
        entries: list[tuple[str, int, int]] = []
        for _ in range(count):
            name, offset = _read_string(payload, offset)
            kind = payload[offset]
            index, offset = _read_varuint(payload, offset + 1)
            entries.append((name, kind, index))
        names = {name for name, _, _ in entries}
        export = bytearray(_write_varuint(len(entries)))
        for name, kind, index in entries:
            public = public_by_symbol.get(name) if kind == 0 else None
            if public is not None:
                if public in names:
                    raise ValueError(
                        f"app export {public!r} collides with an existing export"
                    )
                renamed.add(name)
                name = public
            export.extend(_write_string(name))
            export.append(kind)
            export.extend(_write_varuint(index))
        rebuilt.append((section_id, bytes(export)))
    missing = sorted(set(public_by_symbol) - renamed)
    if missing:
        raise ValueError(
            "the link kept no export for app symbol(s): " + ", ".join(missing)
        )
    return _build_sections(rebuilt)


def _publish_app_exports(
    staged: Path, app_export_contract: Path, *, strip_debug: bool = False
) -> bool:
    try:
        data = publish_app_export_names(staged.read_bytes(), app_export_contract)
        if strip_debug:
            data = strip_wasm_publication_sections(
                data, final_artifact=True, preserve_debug=False
            )
        staged.write_bytes(data)
    except ValueError as exc:
        print(f"wasm app exports: {exc}", file=sys.stderr)
        staged.unlink(missing_ok=True)
        return False
    return True


def freestanding_import_error(data: bytes) -> str | None:
    """Why a freestanding module is not freestanding, else None.

    A freestanding module imports only from `env`: the WASI stub pass must have
    replaced every `wasi_snapshot_preview1` import, and no other host
    namespace (`molt_runtime` or one a package added) may remain.
    """
    from tools.wasm_link_format import _collect_imports

    foreign = [
        f"{entry.module}::{entry.name}"
        for entry in _collect_imports(data)
        if entry.module != "env"
    ]
    if foreign:
        return "imports outside env remain: " + ", ".join(foreign)
    return None


def standard_object_link_command(
    *,
    wasm_ld: str,
    app_object: Path,
    stdlib_archive: Path | None,
    runtime: Path,
    allowlist: Path,
    output: Path,
    exports: Sequence[str],
    native_link_requirements: SourceExtensionLinkRequirements,
    preserve_debug: bool,
) -> tuple[str, ...]:
    main = compiled_execution_symbol("molt_main")
    return (
        wasm_ld,
        "--no-entry",
        "--gc-sections",
        "--error-limit=0",
        f"--allow-undefined-file={allowlist}",
        "--import-table",
        "--stack-first",
        "-z",
        f"stack-size={SHADOW_STACK_BYTES}",
        *(() if preserve_debug else ("--strip-debug",)),
        f"--export={main}",
        *(f"--export-if-defined={name}" for name in exports if name != main),
        "-o",
        str(output),
        str(app_object),
        # Every stdlib member belongs to the program: the module registry
        # reaches each member's initializer.
        *(
            ("--whole-archive", str(stdlib_archive), "--no-whole-archive")
            if stdlib_archive is not None
            else ()
        ),
        str(runtime),
        *render_source_extension_link_arguments(native_link_requirements),
    )


def link_standard_object(
    *,
    wasm_ld: str,
    app_object: Path,
    stdlib_archive: Path | None,
    runtime: Path,
    allowlist: Path,
    linked: Path,
    app_export_contract: Path,
    native_link_requirements: SourceExtensionLinkRequirements,
    optimize: bool,
    optimize_level: str,
    preserve_debug: bool,
    phase_timings_ms: MutableMapping[str, float],
    selection_roles: MutableMapping[str, Mapping[str, object]],
    freestanding: bool = False,
) -> int:
    """Link, optionally optimize, and publish ``linked``. Returns an exit code.

    A freestanding build replaces each WASI import with a trapping stub before
    the optimizer runs, so the optimizer removes the dead continuations.
    """
    exports = standard_object_exports(app_export_contract)
    staged = linked.with_name(f".{linked.name}.staged")
    command = standard_object_link_command(
        wasm_ld=wasm_ld,
        app_object=app_object,
        stdlib_archive=stdlib_archive,
        runtime=runtime,
        allowlist=allowlist,
        output=staged,
        exports=exports,
        native_link_requirements=native_link_requirements,
        preserve_debug=preserve_debug,
    )
    started = time.perf_counter()
    result = run_selected_link(
        command,
        native_link_requirements,
        role="linked",
        why_extract=linked.with_name(f".{linked.name}.why-extract"),
        selection_roles=selection_roles,
    )
    phase_timings_ms["wasm_link_wasm_ld"] = (time.perf_counter() - started) * 1000.0
    if result.returncode != 0:
        sys.stderr.write(result.stderr)
        print("wasm-ld failed to link the standard object", file=sys.stderr)
        staged.unlink(missing_ok=True)
        return 1
    if not _publish_app_exports(staged, app_export_contract):
        return 1
    if freestanding:
        from tools.wasm_stub_wasi import stub_wasi_imports

        stubbed, count = stub_wasi_imports(staged.read_bytes())
        if count:
            staged.write_bytes(stubbed)
        error = freestanding_import_error(stubbed)
        if error is not None:
            print(f"Freestanding WASM link failed: {error}", file=sys.stderr)
            staged.unlink(missing_ok=True)
            return 1
    if optimize:
        identity = wasm_optimizer_invocation_identity()
        attestation: dict[str, object] = {}
        started = time.perf_counter()
        optimized = _run_wasm_opt_via_optimize(
            staged,
            optimize_level,
            # The export-if-defined roots the link kept; a table alias the
            # imported table has no definition for is not a root.
            required_exports=(
                set(exports)
                | set(
                    app_export_symbol_map(load_app_export_contract(app_export_contract))
                )
            )
            & _collect_wasm_export_names(staged),
            attestation=attestation,
            optimizer_identity=identity,
            preserve_debug=preserve_debug,
        )
        phase_timings_ms["wasm_link_wasm_opt"] = (
            time.perf_counter() - started
        ) * 1000.0
        if not optimized:
            staged.unlink(missing_ok=True)
            return 1
        payload = build_wasm_optimizer_attestation(
            attestation, published_output=staged.read_bytes()
        )
        wasm_optimizer_attestation_path(linked).write_text(
            encode_wasm_optimizer_attestation(payload), encoding="utf-8"
        )
    staged.replace(linked)
    return 0


# The imports a side module may name: the shared memory, table and shadow
# stack, its placement bases, the runtime namespace and the GOT entries the
# host resolves against the runtime's exports.
_SIDE_MODULE_ENV_IMPORTS = frozenset(
    {
        "memory",
        "__indirect_function_table",
        "__stack_pointer",
        "__memory_base",
        "__table_base",
    }
)
_SIDE_MODULE_IMPORT_MODULES = frozenset({"molt_runtime", "GOT.mem", "GOT.func"})


def side_module_link_command(
    *,
    wasm_ld: str,
    app_object: Path,
    stdlib_archive: Path | None,
    sdk_archives: Sequence[Path],
    output: Path,
    exports: Sequence[str],
    native_link_requirements: SourceExtensionLinkRequirements,
    preserve_debug: bool,
) -> tuple[str, ...]:
    main = compiled_execution_symbol("molt_main")
    native_paths = {
        Path(item.path).resolve() for item in native_link_requirements.inputs
    }
    return (
        wasm_ld,
        "-shared",
        "--experimental-pic",
        "--unresolved-symbols=import-dynamic",
        "-Bsymbolic",
        "--no-export-dynamic",
        "--gc-sections",
        "--error-limit=0",
        # Shorten the relocated LEBs. Relocation compression changes code
        # offsets and requires the linker to discard DWARF.
        *(() if preserve_debug else ("--strip-debug", "--compress-relocations")),
        f"--export={main}",
        *(f"--export-if-defined={name}" for name in exports if name != main),
        "-o",
        str(output),
        str(app_object),
        *(
            ("--whole-archive", str(stdlib_archive), "--no-whole-archive")
            if stdlib_archive is not None
            else ()
        ),
        # Extension objects belong to the application; the side module links
        # them, as the linked module does.
        *render_source_extension_link_arguments(native_link_requirements),
        # LLVM can emit C library calls as well as compiler-rt builtins. Extract
        # only demanded members of the selected SDK archives.
        *(str(path) for path in sdk_archives if path not in native_paths),
    )


def side_module_import_error(module: Path) -> str | None:
    """Why `module` is not a side module this host contract can place."""
    from molt.wasm_artifact import parse_wasm_imports

    unexpected = sorted(
        f"{entry.module}.{entry.name}"
        for entry in parse_wasm_imports(module.read_bytes())
        if not (
            entry.module in _SIDE_MODULE_IMPORT_MODULES
            or (entry.module == "env" and entry.name in _SIDE_MODULE_ENV_IMPORTS)
        )
    )
    if unexpected:
        return "side module imports outside the runtime contract: " + ", ".join(
            unexpected
        )
    return None


def link_side_module(
    *,
    wasm_ld: str,
    app_object: Path,
    stdlib_archive: Path | None,
    app: Path,
    app_export_contract: Path,
    native_link_requirements: SourceExtensionLinkRequirements,
    sdk_archives: Sequence[Path],
    preserve_debug: bool,
    phase_timings_ms: MutableMapping[str, float],
    selection_roles: MutableMapping[str, Mapping[str, object]],
) -> int:
    """Link the split application as a side module and publish ``app``.

    It keeps the linked module's export roots: wasm-ld exports only the ones
    the side module defines (the app's compiled entries, `molt_host_init`
    among them, and its user exports); the runtime member exports the rest.
    """
    exports = standard_object_exports(app_export_contract)
    staged = app.with_name(f".{app.name}.staged")
    command = side_module_link_command(
        wasm_ld=wasm_ld,
        app_object=app_object,
        stdlib_archive=stdlib_archive,
        sdk_archives=sdk_archives,
        output=staged,
        exports=exports,
        native_link_requirements=native_link_requirements,
        preserve_debug=preserve_debug,
    )
    started = time.perf_counter()
    result = run_selected_link(
        command,
        native_link_requirements,
        role="app",
        why_extract=app.with_name(f".{app.name}.why-extract"),
        selection_roles=selection_roles,
    )
    phase_timings_ms["wasm_link_side_module"] = (time.perf_counter() - started) * 1000.0
    if result.returncode != 0:
        sys.stderr.write(result.stderr)
        print("wasm-ld failed to link the side module", file=sys.stderr)
        staged.unlink(missing_ok=True)
        return 1
    if not _publish_app_exports(
        staged, app_export_contract, strip_debug=not preserve_debug
    ):
        return 1
    error = side_module_import_error(staged)
    if error is not None:
        print(error, file=sys.stderr)
        staged.unlink(missing_ok=True)
        return 1
    staged.replace(app)
    return 0
