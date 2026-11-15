"""Link a standard LLVM wasm32 application object with wasm-ld.

An LLVM wasm32 object is a standard relocatable object: wasm-ld resolves its
runtime references against the relocatable runtime member and its stdlib
functions against the shared stdlib archive, with no object surgery, fact
scan or table layout of its own. A function pointer is a slot of the one
table wasm-ld lays out, and the runtime calls it directly (HF-699 K2), so the
module carries no callable-table attestation.

``tools/wasm_link.py`` admits every input (producer digests, the trusted
runtime generation, the native link plan) and owns publication; this module
owns only the link and the optimizer pass of a standard object.
"""

from __future__ import annotations

import sys
import time
from collections.abc import MutableMapping, Sequence
from pathlib import Path

from molt._wasm_runtime_exports import wasm_output_runtime_export_names
from molt.cli.app_export_contract import exported_app_symbols, load_app_export_contract
from molt._wasm_abi_generated import compiled_execution_symbol
from molt.cli.source_extension_link_requirements import (
    SourceExtensionLinkRequirements,
    render_source_extension_link_arguments,
)
from molt.wasm_artifact import _collect_wasm_export_names
from molt.wasm_optimizer_identity import (
    build_wasm_optimizer_attestation,
    encode_wasm_optimizer_attestation,
    wasm_optimizer_attestation_path,
    wasm_optimizer_invocation_identity,
)
from tools.wasm_link_command import _run_external_tool
from tools.wasm_link_format import _ESSENTIAL_EXPORTS
from tools.wasm_link_optimizer_policy import _run_wasm_opt_via_optimize

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
) -> int:
    """Link, optionally optimize, and publish ``linked``. Returns an exit code."""
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
    result = _run_external_tool(command, capture_output=True, text=True)
    phase_timings_ms["wasm_link_wasm_ld"] = (time.perf_counter() - started) * 1000.0
    if result.returncode != 0:
        sys.stderr.write(result.stderr)
        print("wasm-ld failed to link the standard object", file=sys.stderr)
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
            required_exports=set(exports) & _collect_wasm_export_names(staged),
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
