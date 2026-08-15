"""Custodied monolithic and split-runtime WASM link orchestration."""

from __future__ import annotations

from collections.abc import Mapping, Sequence
import contextlib
import json
from pathlib import Path
import shlex
import sys
import tempfile
import time
from typing import Any, Literal

from wasm_link_format import CallableTableLayout

RuntimeLinkInputRole = Literal["reloc", "shared"]


def run_wasm_ld_with_custodied_inputs(
    api: Mapping[str, Any],
    wasm_ld: str,
    runtime: Path,
    output: Path,
    linked: Path,
    *,
    runtime_role: RuntimeLinkInputRole,
    allowlist_override: Path | None = None,
    optimize: bool = False,
    optimize_level: str = "Oz",
    freestanding: bool = False,
    split_runtime: bool = False,
    split_output_dir: Path | None = None,
    deploy_runtime_override: Path | None = None,
    native_objects: Sequence[Path] = (),
    native_link_arguments: Sequence[str] = (),
    preserve_debug_sections: bool = False,
    phase_timings_file: Path | None = None,
    wasm_facts_scanner: Path,
    app_export_contract_path: Path | None = None,
) -> int:
    phase_timings_ms: dict[str, float] = {}
    facts_metrics: dict[str, float] = {}
    operation_counts: dict[str, int | float] = {
        "wasm_whole_artifact_full_binary_parses": 0,
        "wasm_whole_artifact_section_walks": 0,
        "wasm_whole_artifact_reserializations": 0,
        "wasm_whole_artifact_redundant_parses_eliminated": 0,
        **api["_empty_wasm_link_cache_metrics"](),
    }
    total_start = time.perf_counter()
    # The finally block records partial-failure timing even when lld rejects an
    # input before split processing begins.
    split_runtime_start = total_start
    for native_object in native_objects:
        if not native_object.exists():
            print(f"Native WASM link input not found: {native_object}", file=sys.stderr)
            return 1
    try:
        native_objects = api["_resolve_native_link_inputs"](tuple(native_objects))
        native_link_requirements = api["source_extension_link_requirements"](
            native_link_arguments,
            target_triple="wasm32-wasip1",
        )
        native_link_arguments = api["render_source_extension_link_arguments"](
            native_link_requirements
        )
    except ValueError as exc:
        print(f"Wasm link failed: {exc}", file=sys.stderr)
        return 1
    runtime_exports: set[str]
    try:
        runtime_data = runtime.read_bytes()
        runtime_exports = (
            set(api["parse_wasm_linking_symbols"](runtime_data).defined_names)
            if runtime_role == "reloc"
            else api["_collect_exports"](runtime_data)
        )
    except (OSError, UnicodeDecodeError, ValueError) as exc:
        print(
            f"Failed to parse {runtime_role} runtime symbols ({runtime}): {exc}",
            file=sys.stderr,
        )
        runtime_exports = set()
    if not runtime_exports:
        print("Runtime exports unavailable for linking.", file=sys.stderr)
        return 1
    output_data = output.read_bytes()
    app_export_contract: dict[str, object] | None = None
    app_call_abi: dict[str, object] | None = None
    if app_export_contract_path is not None:
        try:
            app_export_contract = api["load_app_export_contract"](
                app_export_contract_path
            )
            app_call_abi = api["app_export_call_abi"](app_export_contract)
        except ValueError as exc:
            print(f"Wasm link failed: {exc}", file=sys.stderr)
            return 1
    temp_dir = tempfile.TemporaryDirectory(prefix="molt-wasm-link-")
    try:
        facts_provider = api["_make_rust_wasm_facts_provider"](
            wasm_facts_scanner,
            Path(temp_dir.name),
            facts_metrics,
        )
        output_facts = facts_provider(output_data)
        output_callable_layout = api["_callable_layout_from_wasm_facts"](output_facts)
    except ValueError as exc:
        temp_dir.cleanup()
        print(f"Wasm link failed: {exc}", file=sys.stderr)
        return 1
    output_memory_min = api["_memory_import_min"](output_data)
    output_table_min = api["_table_import_min"](output_data)
    callable_entry_export_names = (
        tuple(
            api["_callable_entry_export_name"](slot)
            for slot in range(
                output_callable_layout.fixed_prefix_len
                + output_callable_layout.app_entry_count
            )
        )
        if output_callable_layout is not None
        else ()
    )
    reserved_runtime_link_exports = tuple(
        runtime_name
        for _index, runtime_name, _import_name, _arity, dispatch in (
            api["WASM_RESERVED_RUNTIME_CALLABLES"]
        )
        if dispatch == "direct"
    )
    split_callable_layout: CallableTableLayout | None = None
    monolithic_callable_layout = output_callable_layout
    deploy_runtime_path: Path | None = None
    if split_runtime:
        if output_callable_layout is None:
            print(
                "Split app is missing explicit callable-table layout authority.",
                file=sys.stderr,
            )
            temp_dir.cleanup()
            return 1
        try:
            deploy_runtime_path = api["_resolve_deploy_runtime"](
                deploy_runtime_override
            )
            runtime_callable_layout = api["read_wasm_split_runtime_callable_layout"](
                deploy_runtime_path
            )
        except (OSError, ValueError) as exc:
            print(
                f"Split runtime executable callable layout is invalid: {exc}",
                file=sys.stderr,
            )
            temp_dir.cleanup()
            return 1
        try:
            split_callable_layout = api["_reconcile_split_callable_layout"](
                output_callable_layout,
                runtime_callable_layout,
            )
        except ValueError as exc:
            print(f"Split callable-table layout is invalid: {exc}", file=sys.stderr)
            temp_dir.cleanup()
            return 1
        expected_table_min = split_callable_layout.finalized_app_base + (
            split_callable_layout.app_entry_count
        )
        table_boundary_matches = output_table_min == expected_table_min or (
            expected_table_min == 0 and output_table_min is None
        )
        if expected_table_min > 0xFFFF_FFFF or not table_boundary_matches:
            print(
                "Split app table import boundary does not match explicit callable layout: "
                f"import_min={output_table_min}, expected={expected_table_min}",
                file=sys.stderr,
            )
            temp_dir.cleanup()
            return 1
    required_native_direct_symbols = tuple(
        sorted(
            set(api["_required_native_direct_symbols"](output_data))
            | set(api["_sealed_native_init_symbols"](native_objects))
        )
    )
    try:
        export_symbol_map = api["_collect_output_export_symbol_map"](output_data)
    except ValueError as exc:
        print(f"Wasm link failed: {exc}", file=sys.stderr)
        temp_dir.cleanup()
        return 1
    callable_entry_export_map = {
        name: export_symbol_map[name]
        for name in callable_entry_export_names
        if name in export_symbol_map
    }
    if len(callable_entry_export_map) != len(callable_entry_export_names):
        missing_callable_exports = sorted(
            set(callable_entry_export_names) - callable_entry_export_map.keys()
        )
        print(
            "Wasm link failed: callable-table entry exports are missing linker symbols: "
            + ", ".join(missing_callable_exports),
            file=sys.stderr,
        )
        return 1
    callable_entry_symbol_names = tuple(
        dict.fromkeys(callable_entry_export_map.values())
    )
    callable_entry_symbol_names_by_slot = tuple(
        callable_entry_export_map[name] for name in callable_entry_export_names
    )
    contract_app_exports = (
        api["exported_app_symbols"](app_export_contract)
        if app_export_contract is not None
        else ()
    )
    app_target_symbol_map = {
        name: export_symbol_map[name]
        for name in contract_app_exports
        if name in export_symbol_map
    }
    app_adapter_symbol_map: dict[str, str] = {}
    app_adapter_identity_map: dict[str, str] = {}
    app_target_identity_map: dict[str, str] = {}
    app_identity_exports: dict[str, str] = {}
    preserved_output_exports = list(
        dict.fromkeys(
            [
                *contract_app_exports,
                *(
                    name
                    for name in api["WASM_OUTPUT_RUNTIME_EXPORT_ALIASES"]
                    if name in export_symbol_map
                ),
                *(
                    entry.canonical_name
                    for entry in api["_split_runtime_export_contract"]("app")
                    if entry.kind == 0 and entry.canonical_name in export_symbol_map
                ),
            ]
        )
    )
    missing_contract_exports = sorted(
        set(contract_app_exports) - export_symbol_map.keys()
    )
    if missing_contract_exports:
        print(
            "Wasm link failed: frontend app export contract names are absent from "
            "the relocatable app artifact: " + ", ".join(missing_contract_exports),
            file=sys.stderr,
        )
        return 1
    rewritten = api["_rewrite_output_imports"](output, runtime_exports, temp_dir)
    if rewritten is None:
        temp_dir.cleanup()
        return 1
    rewritten_path, temp_dir, force_exports = rewritten
    try:
        rewritten_path = api["_rewrite_required_native_direct_imports"](
            rewritten_path,
            required_native_direct_symbols,
            temp_dir,
        )
    except ValueError as exc:
        print(f"Failed to rewrite native direct imports: {exc}", file=sys.stderr)
        return 1
    if app_call_abi is not None:
        try:
            rewritten_path, app_adapter_symbol_map = api["_inject_app_export_adapters"](
                rewritten_path,
                temp_dir,
                public_export_names=contract_app_exports,
                call_abi=app_call_abi,
            )
        except (OSError, ValueError) as exc:
            print(f"Wasm link failed: {exc}", file=sys.stderr)
            return 1
        export_symbol_map.update(app_adapter_symbol_map)
        try:
            (
                app_adapter_identity_map,
                app_target_identity_map,
                app_identity_exports,
            ) = api["_app_export_identity_maps"](
                app_adapter_symbol_map,
                app_target_symbol_map,
            )
        except ValueError as exc:
            print(f"Wasm link failed: {exc}", file=sys.stderr)
            return 1
    user_export_symbol_names = [
        export_symbol_map[name]
        for name in preserved_output_exports
        if name in export_symbol_map
    ]
    native_link_inputs, native_force_exports = api["_rewrite_native_runtime_imports"](
        tuple(native_objects),
        runtime_exports,
        temp_dir,
        split_runtime=split_runtime,
    )
    force_exports.extend(native_force_exports)
    rewritten_path = api["_inject_call_indirect_alias"](
        rewritten_path, runtime, temp_dir
    )
    if allowlist_override is not None:
        base_allowlist = allowlist_override
    else:
        base_allowlist = Path(api["__file__"]).parent / "wasm_allowed_imports.txt"
    if not base_allowlist.exists():
        print(f"Allowlist not found: {base_allowlist}", file=sys.stderr)
        return 1
    allowlist = api["_compose_wasm_ld_allowlist"](
        base_allowlist=base_allowlist,
        native_objects=native_objects,
        temp_dir=temp_dir,
    )
    linked_rewritten_path = rewritten_path
    linked_native_inputs = native_link_inputs
    if split_runtime and native_objects:
        # The published linked artifact resolves Molt ABI imports in the normal
        # wasm-ld symbol namespace. The deployed split app keeps the same
        # imports as ``molt_runtime`` ABI edges.
        linked_rewrite = api["_rewrite_runtime_import_module_namespace"](
            rewritten_path,
            source_module="molt_runtime",
            target_module="env",
            runtime_exports=runtime_exports,
            temp_dir=temp_dir,
            filename="output_linked_runtime_imports.wasm",
        )
        if linked_rewrite is None:
            return 1
        linked_rewritten_path, linked_force_exports = linked_rewrite
        force_exports.extend(linked_force_exports)
        linked_native_inputs = tuple(native_objects)
    staged_outputs: list[Path] = []
    work_linked = api["artifact_publish"].staged_output_path(linked)
    staged_outputs.append(work_linked)
    app_wasm: Path | None = None
    rt_wasm: Path | None = None
    app_stage: Path | None = None
    rt_stage: Path | None = None
    size_attestation: dict[str, object] = {}
    size_attestation_path: Path | None = None
    size_attestation_stage: Path | None = None

    # When imports were rewritten to prefixed names that are missing from
    # the non-relocatable runtime's export section (e.g. inlined away by
    # LTO), check whether the actual link runtime is a relocatable object
    # that retains the symbols in its linking section.  If so, wasm-ld
    # will resolve them â€” no extra action needed.  If the link runtime is
    # the non-relocatable module itself, we need the relocatable variant.
    if force_exports:
        if runtime_role == "reloc":
            # The relocatable runtime retains all symbols â€” the pre-check
            # against the non-reloc export list was overly conservative.
            pass
        else:
            missing_list = ", ".join(sorted(set(force_exports)))
            print(
                f"Wasm link failed: {len(force_exports)} import(s) missing from "
                f"the explicitly selected shared runtime: {missing_list}",
                file=sys.stderr,
            )
            return 1

    # The published linked artifact is a runnable Node/WASI artifact, even when
    # the deployment output is split-runtime. Keep split app deforestation in
    # split_app_cmd below; never link output_linked.wasm against an
    # unreachable runtime stub.
    link_runtime_path = runtime

    preflight_error = (
        api["_preflight_relocatable_runtime"](wasm_ld, link_runtime_path, temp_dir)
        if runtime_role == "reloc"
        else None
    )
    if preflight_error is not None:
        print(f"Wasm link failed: {preflight_error}", file=sys.stderr)
        return 1

    cmd = [
        wasm_ld,
        "--no-entry",
        "--gc-sections",
        f"--allow-undefined-file={str(allowlist)}",
        "--import-table",
        # Place the stack before data segments in linear memory so that the
        # stack (which grows downward from __stack_pointer) cannot overwrite
        # data segments.  Without this flag wasm-ld may place data segments
        # in the address range reserved for the stack, causing corruption
        # when function calls push frames that overlap string constants and
        # other read-only data (manifests as NameError / AttributeError with
        # null-byte names).
        "--stack-first",
        "-z",
        "stack-size=1048576",
        "--export=molt_main",
        "--export-if-defined=molt_memory",
        "--export-if-defined=memory",
        "--export-if-defined=molt_table",
        "--export-if-defined=__indirect_function_table",
        "--export-if-defined=molt_set_wasm_table_base",
    ]
    linked_callable_growth_base = (
        api["_monolithic_linked_callable_growth_base"](output_callable_layout)
        if output_callable_layout is not None
        else None
    )
    if linked_callable_growth_base is not None:
        cmd.insert(
            cmd.index("--import-table") + 1,
            f"--table-base={linked_callable_growth_base}",
        )
    # Force-export symbols that were rewritten but missing from the
    # non-relocatable runtime â€” they exist in the relocatable runtime
    # and wasm-ld needs to know to keep them in the linked output.
    cmd.extend(
        api["_deduplicated_export_flags"](
            (f"--export-if-defined={sym}" for sym in force_exports),
            (
                f"--export-if-defined={sym}"
                for sym in sorted(
                    api["_ESSENTIAL_EXPORTS"]
                    - {"__indirect_function_table", "memory", "molt_main"}
                )
            ),
            (f"--export={sym}" for sym in required_native_direct_symbols),
            (f"--export={sym}" for sym in user_export_symbol_names),
            (f"--export-if-defined={name}" for name in callable_entry_symbol_names),
            (f"--export-if-defined={name}" for name in reserved_runtime_link_exports),
        )
    )
    cmd += [
        "-o",
        str(work_linked),
        str(linked_rewritten_path),
        str(link_runtime_path),
    ]
    cmd.extend(str(native_object) for native_object in linked_native_inputs)
    cmd.extend(native_link_arguments)

    split_linked_app_path: Path | None = None
    split_app_cmd: list[str] | None = None
    split_app_required_table_min: int | None = None
    split_app_got_runtime_addresses: dict[str, int] = {}
    if split_runtime:
        split_native_inputs = native_link_inputs
        # Keep the active-data-segment alias: it defines each CPython-ABI data
        # symbol (Py_None/Py_False/PyExc_*/Py*_Type/...) so the split app link
        # resolves both a PIC extension's GOT references (numpy, via
        # R_WASM_GLOBAL_INDEX_LEB -> wasm-ld emits a defined
        # `GOT.data.internal.molt_<sym>` global) and a non-PIC extension's
        # absolute references (scipy, via R_WASM_MEMORY_ADDR_SLEB). wasm-ld
        # relocates the alias's zero-size segments into the split app's own
        # region, so every one of those GOT globals initialises to the same
        # app-local placeholder address (ob_type==NULL). The GOT globals are the
        # exact word numpy reads at run time, so after the link we retarget each
        # `GOT.data.internal.molt_<sym>` global to the shared runtime's canonical
        # linear-memory address (see _rewrite_split_app_got_data_globals). This
        # keeps the split app non-PIC (no import-dynamic, so no GOT.func / no
        # loader change) and does not disturb any statically resolved symbol.
        if native_objects:
            try:
                assert deploy_runtime_path is not None
                data_alias_object = api["_split_runtime_data_alias_object"](
                    native_objects=native_link_inputs,
                    deploy_runtime=deploy_runtime_path,
                    temp_dir=temp_dir,
                    reloc_runtime=link_runtime_path,
                )
                split_app_got_runtime_addresses = api[
                    "_runtime_exported_data_symbol_addresses"
                ](deploy_runtime_path.read_bytes())
            except ValueError as exc:
                print(str(exc), file=sys.stderr)
                return 1
            if data_alias_object is not None:
                split_native_inputs = (*native_link_inputs, data_alias_object)
        split_native_allowlist = api["_compose_split_runtime_native_allowlist"](
            base_allowlist=base_allowlist,
            native_objects=split_native_inputs,
            runtime_exports=runtime_exports,
            temp_dir=temp_dir,
        )
        split_linked_app_path = Path(temp_dir.name) / "app_split_linked.wasm"
        try:
            split_app_data_base = api["_split_app_global_base"](output_data)
        except ValueError as exc:
            print(f"WASM split app memory layout is invalid: {exc}", file=sys.stderr)
            return 1
        assert output_callable_layout is not None
        assert split_callable_layout is not None
        split_app_table_base = api["_callable_app_end"](split_callable_layout)
        split_app_prefix = [
            f"--allow-undefined-file={split_native_allowlist}"
            if part.startswith("--allow-undefined-file=")
            else "--no-stack-first"
            if part == "--stack-first"
            else part
            for part in cmd[: cmd.index("-o")]
            if part != "--export=molt_main" and not part.startswith("--table-base=")
        ]
        try:
            split_app_link_args = api["_split_app_native_link_args"](
                split_native_inputs
            )
        except ValueError as exc:
            print(f"WASM split app native link failed: {exc}", file=sys.stderr)
            return 1
        split_app_cmd = [
            *split_app_prefix,
            "--import-memory",
            f"--global-base={split_app_data_base}",
            f"--table-base={split_app_table_base}",
            "-o",
            str(split_linked_app_path),
            str(rewritten_path),
            *split_app_link_args,
            *native_link_arguments,
        ]
        operation_counts["split_app_data_base_bytes"] = split_app_data_base

    res = api["_run_external_tool"](cmd, capture_output=True, text=True)
    whole_artifact_counts_token = api["_WHOLE_ARTIFACT_OPERATION_COUNTS"].set(
        operation_counts
    )
    try:
        if res.returncode != 0:
            err = res.stderr.strip() or res.stdout.strip()
            if err:
                print(err, file=sys.stderr)
            return res.returncode
        signature_mismatch = api["_wasm_ld_signature_mismatch_warning"](res.stderr)
        if signature_mismatch is not None:
            print(signature_mismatch, file=sys.stderr)
            return 1
        if not work_linked.exists():
            print(
                "wasm-ld exited successfully but produced no linked output: "
                f"{work_linked}",
                file=sys.stderr,
            )
            return 1
        linked_bytes = api["_read_wasm_bytes_with_retry"](work_linked)
        if not api["_is_wasm_binary"](linked_bytes):
            print(
                "wasm-ld produced non-wasm linked output "
                f"({work_linked}, size={len(linked_bytes)} bytes)",
                file=sys.stderr,
            )
            return 1
        # wasm-ld 22 emits the merged type section as a GC-proposal recursive
        # type group even when every member is a plain MVP func type. Flatten it
        # back to standalone types before ANY downstream step: the molt host
        # runner / Cloudflare V8 / wasm-opt all reject the `0x4E` rec-group
        # encoding without the GC proposal, and the linker's own
        # `_parse_type_section` assumes the standalone-`func` form. Doing this
        # first keeps every later type-section-aware pass operating on a
        # canonical MVP type section.
        try:
            canonical_linked_bytes = api["_canonicalize_wasm_ld_output"](
                linked_bytes, description="linked"
            )
        except ValueError as exc:
            print(str(exc), file=sys.stderr)
            return 1
        if canonical_linked_bytes != linked_bytes:
            work_linked.write_bytes(canonical_linked_bytes)
            linked_bytes = canonical_linked_bytes
        if output_callable_layout is not None:
            try:
                raw_linked_facts = facts_provider(linked_bytes)
                raw_callable_entries = raw_linked_facts.get("callable_table_entries")
                entry_plan = api["_resolve_callable_table_entry_plan"](
                    linked_bytes,
                    output_callable_layout,
                    entry_symbol_names=callable_entry_symbol_names_by_slot,
                    include_fixed_prefix=True,
                    override_reserved_direct=True,
                )
                api["_merge_linked_callable_table"](
                    raw_callable_entries,
                    output_callable_layout,
                    entry_plan,
                )
                linked_bytes = api["_install_callable_table_layout"](
                    linked_bytes,
                    output_callable_layout,
                    entry_symbol_names=callable_entry_symbol_names_by_slot,
                    entry_plan=entry_plan,
                )
            except ValueError as exc:
                print(
                    f"Failed to publish linked callable table: {exc}",
                    file=sys.stderr,
                )
                return 1
            work_linked.write_bytes(linked_bytes)
        public_export_map = api["_public_output_export_symbol_map"](
            output_data,
            preserved_output_exports=preserved_output_exports,
            export_symbol_map=export_symbol_map,
        )
        restored_linked_bytes = api["_restore_public_output_exports"](
            linked_bytes,
            public_export_map,
            preserved_symbol_names=required_native_direct_symbols,
        )
        if restored_linked_bytes != linked_bytes:
            work_linked.write_bytes(restored_linked_bytes)
            linked_bytes = restored_linked_bytes
        if app_adapter_symbol_map:
            try:
                linked_bytes = api["_publish_app_export_identity_markers"](
                    linked_bytes,
                    public_export_names=contract_app_exports,
                    adapter_symbol_map=app_adapter_symbol_map,
                    target_symbol_map=app_target_symbol_map,
                    identity_exports=app_identity_exports,
                )
            except ValueError as exc:
                print(f"Wasm link failed: {exc}", file=sys.stderr)
                return 1
            work_linked.write_bytes(linked_bytes)
        try:
            native_link_error = api["_validate_required_native_direct_symbols"](
                linked_bytes,
                required_native_direct_symbols,
                description="Wasm native link",
            )
        except ValueError as exc:
            print(f"Failed to inspect native direct symbols: {exc}", file=sys.stderr)
            return 1
        if native_link_error is not None:
            print(native_link_error, file=sys.stderr)
            return 1

        # MOL-183/MOL-186: Post-link optimization to reduce V8 OOM risk.
        # Strip debug sections, internal exports, and report data duplicates.
        # Pass the original user module as reference_data so the type-index
        # repair can use exact signature matching (Strategy 1) instead of
        # the heuristic body-scan fallback.
        pre_opt_size = len(linked_bytes)
        try:
            output_reference = output.read_bytes()
        except OSError:
            output_reference = None
        split_app_contract_keep_set = api["_split_artifact_contract_keep_set"](
            "app",
            public_export_map=public_export_map,
            required_native_direct_symbols=required_native_direct_symbols,
        )
        post_link_preserve_exports = set(split_app_contract_keep_set)
        post_link_preserve_exports.update(app_identity_exports)
        if not split_runtime:
            post_link_preserve_exports.update(preserved_output_exports)
        linked_bytes = api["_post_link_optimize"](
            linked_bytes,
            reference_data=output_reference,
            preserve_exports=post_link_preserve_exports,
            facts_provider=facts_provider,
        )
        post_opt_size = len(linked_bytes)
        if post_opt_size < pre_opt_size:
            savings = pre_opt_size - post_opt_size
            print(
                f"Post-link optimization: stripped {savings:,} bytes "
                f"({savings / 1024:.1f} KB, "
                f"{savings / pre_opt_size * 100:.1f}% reduction)",
                file=sys.stderr,
            )
            work_linked.write_bytes(linked_bytes)

        if optimize:
            if not api["_run_wasm_opt_via_optimize"](
                work_linked,
                level=optimize_level,
                converge=False,
                apply_level=not split_runtime,
                required_exports=(
                    set(api["_collect_function_exports"](linked_bytes))
                    & post_link_preserve_exports
                ),
            ):
                print("Required linked WASM optimization failed.", file=sys.stderr)
                return 1
            # Re-read after optimization since the file changed on disk.
            linked_bytes = work_linked.read_bytes()

        if app_adapter_identity_map:
            try:
                api["_validate_app_export_adapters"](
                    linked_bytes,
                    contract_app_exports,
                    adapter_symbol_map=app_adapter_identity_map,
                    target_symbol_map=app_target_identity_map,
                )
            except ValueError as exc:
                print(
                    f"Wasm link failed after post-link optimization: {exc}",
                    file=sys.stderr,
                )
                return 1

        required_table_min = api["_required_linked_table_min"](
            linked_bytes,
            output_table_min,
            facts_provider(linked_bytes),
        )
        if required_table_min is not None:
            try:
                updated = api["_rewrite_table_import_min"](
                    linked_bytes, required_table_min
                )
            except ValueError as exc:
                print(f"Failed to rewrite linked table min: {exc}", file=sys.stderr)
                return 1
            if updated is not None:
                work_linked.write_bytes(updated)
                linked_bytes = updated
        if output_memory_min is not None:
            try:
                updated = api["_rewrite_memory_min"](linked_bytes, output_memory_min)
            except ValueError as exc:
                print(f"Failed to rewrite linked memory min: {exc}", file=sys.stderr)
                return 1
            if updated is not None:
                work_linked.write_bytes(updated)
                linked_bytes = updated
        try:
            updated = api["_ensure_table_export"](linked_bytes)
        except ValueError as exc:
            print(f"Failed to ensure table export: {exc}", file=sys.stderr)
            return 1
        if updated is not None:
            work_linked.write_bytes(updated)
            linked_bytes = updated
        if not any(entry[2] == 2 for entry in api["_collect_imports"](linked_bytes)):
            try:
                updated = api["_ensure_defined_memory_export"](linked_bytes)
            except ValueError as exc:
                print(f"Failed to ensure memory export: {exc}", file=sys.stderr)
                return 1
            if updated is not None:
                work_linked.write_bytes(updated)
                linked_bytes = updated
        if not split_runtime:
            try:
                stripped_linked_bytes = api["_strip_app_export_identity_markers"](
                    linked_bytes,
                    identity_exports=app_identity_exports,
                    preserve_exports=set(preserved_output_exports),
                )
            except ValueError as exc:
                print(f"Wasm link failed: {exc}", file=sys.stderr)
                return 1
            if stripped_linked_bytes != linked_bytes:
                work_linked.write_bytes(stripped_linked_bytes)
                linked_bytes = stripped_linked_bytes
        if freestanding:
            try:
                import importlib.util as _ilu

                stub_path = Path(api["__file__"]).parent / "wasm_stub_wasi.py"
                spec = _ilu.spec_from_file_location("wasm_stub_wasi", stub_path)
                if spec is None or spec.loader is None:
                    print("wasm_stub_wasi.py not found", file=sys.stderr)
                    return 1
                stub_mod = _ilu.module_from_spec(spec)
                spec.loader.exec_module(stub_mod)
                linked_bytes, n_stubbed = stub_mod.stub_wasi_imports(linked_bytes)
                if n_stubbed > 0:
                    work_linked.write_bytes(linked_bytes)
                    print(
                        f"Freestanding: stubbed {n_stubbed} WASI imports",
                        file=sys.stderr,
                    )
            except Exception as exc:
                print(f"Freestanding WASI stubbing failed: {exc}", file=sys.stderr)
                return 1

        # -- Split-runtime: emit app.wasm + molt_runtime.wasm ---------------
        split_runtime_start = time.perf_counter()
        if split_runtime:
            out_dir = split_output_dir or linked.parent
            out_dir.mkdir(parents=True, exist_ok=True)

            app_wasm = out_dir / "app.wasm"
            rt_wasm = out_dir / "molt_runtime.wasm"
            app_stage = api["artifact_publish"].staged_output_path(app_wasm)
            rt_stage = api["artifact_publish"].staged_output_path(rt_wasm)
            size_attestation_path = out_dir / "wasm_size_attestation.json"
            size_attestation_stage = api["artifact_publish"].staged_output_path(
                size_attestation_path
            )
            staged_outputs.extend([app_stage, rt_stage, size_attestation_stage])

            if split_app_cmd is not None:
                assert split_linked_app_path is not None
                split_app_res = api["_run_external_tool"](
                    split_app_cmd,
                    capture_output=True,
                    text=True,
                )
                if split_app_res.returncode != 0:
                    err = split_app_res.stderr.strip() or split_app_res.stdout.strip()
                    if err:
                        print(err, file=sys.stderr)
                    return split_app_res.returncode
                signature_mismatch = api["_wasm_ld_signature_mismatch_warning"](
                    split_app_res.stderr
                )
                if signature_mismatch is not None:
                    print(signature_mismatch, file=sys.stderr)
                    return 1
                if not split_linked_app_path.exists():
                    print(
                        "wasm-ld exited successfully but produced no split app "
                        f"linked output: {split_linked_app_path}",
                        file=sys.stderr,
                    )
                    return 1
                rewritten_data = api["_read_wasm_bytes_with_retry"](
                    split_linked_app_path
                )
                if not api["_is_wasm_binary"](rewritten_data):
                    print(
                        "wasm-ld produced non-wasm split app linked output "
                        f"({split_linked_app_path}, size={len(rewritten_data)} bytes)",
                        file=sys.stderr,
                    )
                    return 1
                try:
                    output_intervals, linked_intervals = api[
                        "_validate_split_app_data_layout"
                    ](
                        output_data,
                        rewritten_data,
                        planned_base=split_app_data_base,
                    )
                except ValueError as exc:
                    print(
                        f"WASM split app memory layout is invalid: {exc}",
                        file=sys.stderr,
                    )
                    return 1
                operation_counts["split_app_output_data_segment_count"] = len(
                    output_intervals
                )
                output_extent = (
                    output_intervals[0][0],
                    max(end for _start, end in output_intervals),
                )
                operation_counts["split_app_output_data_min_bytes"] = output_extent[0]
                operation_counts["split_app_output_data_end_bytes"] = output_extent[1]
                operation_counts["split_app_linked_data_segment_count"] = len(
                    linked_intervals
                )
                linked_extent = (
                    linked_intervals[0][0],
                    max(end for _start, end in linked_intervals),
                )
                operation_counts["split_app_linked_data_min_bytes"] = linked_extent[0]
                operation_counts["split_app_linked_data_end_bytes"] = linked_extent[1]
                size_attestation["split_app_data_layout"] = {
                    "alignment_bytes": 16,
                    "planned_native_base": split_app_data_base,
                    "output_active_intervals": output_intervals,
                    "output_extent": output_extent,
                    "linked_active_intervals": linked_intervals,
                    "linked_extent": linked_extent,
                }
                try:
                    canonical_rewritten_data = api["_canonicalize_wasm_ld_output"](
                        rewritten_data, description="split app linked"
                    )
                except ValueError as exc:
                    print(str(exc), file=sys.stderr)
                    return 1
                if canonical_rewritten_data != rewritten_data:
                    split_linked_app_path.write_bytes(canonical_rewritten_data)
                    rewritten_data = canonical_rewritten_data
                assert split_callable_layout is not None
                try:
                    raw_split_facts = facts_provider(rewritten_data)
                    raw_split_entries = raw_split_facts.get("callable_table_entries")
                    entry_plan = api["_resolve_callable_table_entry_plan"](
                        rewritten_data,
                        split_callable_layout,
                        entry_symbol_names=callable_entry_symbol_names_by_slot,
                        include_fixed_prefix=False,
                        override_reserved_direct=False,
                    )
                    split_app_required_table_min = api["_merge_linked_callable_table"](
                        raw_split_entries,
                        split_callable_layout,
                        entry_plan,
                    )
                    rewritten_data = api["_install_callable_table_layout"](
                        rewritten_data,
                        split_callable_layout,
                        entry_symbol_names=callable_entry_symbol_names_by_slot,
                        include_fixed_prefix=False,
                        override_reserved_direct=False,
                        entry_plan=entry_plan,
                    )
                except ValueError as exc:
                    print(
                        f"Failed to publish split-app callable table: {exc}",
                        file=sys.stderr,
                    )
                    return 1
                split_linked_app_path.write_bytes(rewritten_data)
                try:
                    restored_rewritten_data = api[
                        "_restore_split_runtime_contract_exports"
                    ](
                        rewritten_data,
                        artifact="app",
                        stage="native-link",
                        public_export_map=public_export_map,
                        required_native_direct_symbols=required_native_direct_symbols,
                        operation_counts=operation_counts,
                    )
                except ValueError as exc:
                    print(str(exc), file=sys.stderr)
                    return 1
                if restored_rewritten_data != rewritten_data:
                    split_linked_app_path.write_bytes(restored_rewritten_data)
                rewritten_data = restored_rewritten_data
                try:
                    native_link_error = api["_validate_required_native_direct_symbols"](
                        rewritten_data,
                        required_native_direct_symbols,
                        description="Split-runtime native app link",
                    )
                except ValueError as exc:
                    print(
                        f"Failed to inspect split-runtime native direct symbols: {exc}",
                        file=sys.stderr,
                    )
                    return 1
                if native_link_error is not None:
                    print(native_link_error, file=sys.stderr)
                    failure_artifact = linked.with_name(
                        linked.stem + ".split-native-link-failure.wasm"
                    )
                    failure_artifact.write_bytes(rewritten_data)
                    print(
                        f"Split-runtime native app failure artifact: {failure_artifact}",
                        file=sys.stderr,
                    )
                    print(
                        "Split-runtime native app linker argv: "
                        + shlex.join(split_app_cmd),
                        file=sys.stderr,
                    )
                    return 1
                # Retarget the CPython-ABI GOT data globals wasm-ld emitted for
                # the PIC extension(s) from the app-local placeholder address to
                # the shared runtime's canonical singleton/type/exception copy.
                # Must run before the split-app optimizer strips the name
                # section that identifies each `GOT.data.internal.molt_<sym>`.
                try:
                    rewritten_data, got_retargeted = api[
                        "_rewrite_split_app_got_data_globals"
                    ](
                        rewritten_data,
                        runtime_addresses=split_app_got_runtime_addresses,
                        description="Split-runtime native app link",
                    )
                except ValueError as exc:
                    print(str(exc), file=sys.stderr)
                    return 1
                if got_retargeted:
                    split_linked_app_path.write_bytes(rewritten_data)
                    print(
                        "Split-runtime GOT data bridge: retargeted "
                        f"{got_retargeted} CPython-ABI GOT data global(s) to the "
                        "shared runtime's canonical addresses",
                        file=sys.stderr,
                    )
            if app_adapter_symbol_map:
                try:
                    rewritten_data = api["_publish_app_export_identity_markers"](
                        rewritten_data,
                        public_export_names=contract_app_exports,
                        adapter_symbol_map=app_adapter_symbol_map,
                        target_symbol_map=app_target_symbol_map,
                        identity_exports=app_identity_exports,
                    )
                except ValueError as exc:
                    print(f"Wasm split-app link failed: {exc}", file=sys.stderr)
                    return 1
            try:
                optimized_app = api["_optimize_split_app_module"](
                    rewritten_data,
                    reference_data=output_data,
                    optimize=optimize,
                    optimize_level=optimize_level,
                    contract_keep_set=(
                        split_app_contract_keep_set | set(app_identity_exports)
                    ),
                    attestation=size_attestation,
                    operation_counts=operation_counts,
                    facts_provider=facts_provider,
                )
            except RuntimeError as exc:
                print(str(exc), file=sys.stderr)
                return 1
            assert split_callable_layout is not None
            assert split_app_required_table_min is not None
            try:
                updated = api["_rewrite_table_import_min"](
                    optimized_app,
                    split_app_required_table_min,
                )
            except ValueError as exc:
                print(
                    f"Failed to rewrite split app table min: {exc}",
                    file=sys.stderr,
                )
                return 1
            if updated is not None:
                optimized_app = updated
            if output_memory_min is not None:
                try:
                    updated = api["_rewrite_memory_min"](
                        optimized_app, output_memory_min
                    )
                except ValueError as exc:
                    print(
                        f"Failed to rewrite split app memory min: {exc}",
                        file=sys.stderr,
                    )
                    return 1
                if updated is not None:
                    optimized_app = updated
            try:
                optimized_app = api["_restore_split_runtime_contract_exports"](
                    optimized_app,
                    artifact="app",
                    stage="optimized-app",
                    public_export_map=public_export_map,
                    required_native_direct_symbols=required_native_direct_symbols,
                    operation_counts=operation_counts,
                )
            except ValueError as exc:
                print(str(exc), file=sys.stderr)
                return 1
            if app_adapter_identity_map:
                try:
                    api["_validate_app_export_adapters"](
                        optimized_app,
                        contract_app_exports,
                        adapter_symbol_map=app_adapter_identity_map,
                        target_symbol_map=app_target_identity_map,
                    )
                    optimized_app = api["_strip_app_export_identity_markers"](
                        optimized_app,
                        identity_exports=app_identity_exports,
                        preserve_exports=split_app_contract_keep_set,
                    )
                except ValueError as exc:
                    print(
                        f"Wasm link failed after split-app optimization: {exc}",
                        file=sys.stderr,
                    )
                    return 1
            if native_objects:
                native_imports = api["_collect_module_imports"](
                    optimized_app, "molt_native"
                )
                if native_imports:
                    print(
                        "Split-runtime native link left unresolved molt_native "
                        "import(s): " + ", ".join(sorted(native_imports)),
                        file=sys.stderr,
                    )
                    return 1
            app_stage.write_bytes(optimized_app)

            # Resolve the deploy-ready (non-relocatable) runtime.
            assert deploy_runtime_path is not None
            deploy_runtime = deploy_runtime_path

            # Tree-shake the runtime against its OWN canonical, app-independent
            # public export surface â€” never against the current app's import
            # subset.  The shared runtime is a single artifact cached once by the
            # CDN and reused by every app, so it MUST be byte-identical across
            # builds (see test_runtime_hash_identical).  Shaking by per-app
            # imports made appA (a class) and appB (fib) keep different export
            # sets, which produced divergent runtime bytes and silently broke CDN
            # cacheability. Keeping the full canonical ABI lets the linker's
            # structural reachability cleanup remove only functions unreachable
            # from ANY public export while every app's import surface still
            # resolves. Per-app shrinkage comes entirely from
            # app.wasm (the intrinsic manifest + wasm-ld --gc-sections), which is
            # the correct split-runtime model: one large cached runtime + a tiny
            # per-app payload.
            full_rt_size = deploy_runtime.stat().st_size
            deploy_runtime_data = deploy_runtime.read_bytes()
            size_attestation["runtime_before"] = api["wasm_metrics"](
                deploy_runtime_data
            )
            try:
                canonical_required_exports = api[
                    "_canonical_split_runtime_required_exports"
                ](deploy_runtime_data)
                app_imports = api["_collect_module_imports"](
                    app_stage.read_bytes(), "molt_runtime"
                )
                missing_runtime_imports: list[str] = []
                for name in app_imports:
                    export_name = api["wasm_split_runtime_export_name_for_import"](name)
                    if (
                        export_name is not None
                        and export_name in canonical_required_exports
                    ):
                        continue
                    if export_name is None and name in canonical_required_exports:
                        continue
                    if name in api["_ESSENTIAL_EXPORTS"]:
                        continue
                    missing_runtime_imports.append(name)
                missing_runtime_imports.sort()
                if missing_runtime_imports:
                    # The app imports a runtime symbol the canonical export
                    # surface does not advertise.  This is a hard ABI contract
                    # violation (the shared runtime cannot satisfy the app), so
                    # publication fails closed with the exact missing symbols.
                    # A per-app reshake or full-copy fallback would hide drift in
                    # the canonical runtime ABI and destroy the split contract.
                    raise ValueError(
                        "split-runtime app imports runtime symbols absent from the "
                        f"canonical shared-runtime export surface: {missing_runtime_imports}"
                    )
                print(
                    f"App imports {len(app_imports)} functions from molt_runtime; "
                    f"shaking shared runtime against {len(canonical_required_exports)} "
                    "canonical exports (app-independent, CDN-cacheable)",
                    file=sys.stderr,
                )
                shaken_runtime = api["_tree_shake_runtime"](
                    deploy_runtime_data,
                    canonical_required_exports,
                    facts_provider=facts_provider,
                    operation_counts=operation_counts,
                )
                rt_stage.write_bytes(shaken_runtime)
            except Exception as exc:
                print(
                    f"Required runtime tree-shake failed: {exc}",
                    file=sys.stderr,
                )
                return 1

            app_size = app_stage.stat().st_size
            rt_size = rt_stage.stat().st_size
            total = app_size + rt_size
            print(
                f"Split-runtime output: "
                f"{app_wasm.name} ({app_size:,} bytes, {app_size // 1024}KB) + "
                f"{rt_wasm.name} ({rt_size:,} bytes, {rt_size // 1024}KB) = "
                f"{total:,} bytes total "
                f"(runtime: {full_rt_size:,} -> {rt_size:,}, "
                f"{(1 - rt_size / full_rt_size) * 100:.0f}% reduction)",
                file=sys.stderr,
            )
        if split_runtime:
            phase_timings_ms["split_runtime_processing"] = round(
                max(0.0, (time.perf_counter() - split_runtime_start) * 1000.0), 6
            )

        validation_start = time.perf_counter()
        if freestanding:
            if not api["_validate_freestanding"](linked_bytes):
                return 1
        phase_timings_ms["fail_closed_validation"] = round(
            max(0.0, (time.perf_counter() - validation_start) * 1000.0), 6
        )
        strip_start = time.perf_counter()
        stripped_debug = api["_strip_debug_sections"](linked_bytes)
        if stripped_debug is not None:
            work_linked.write_bytes(stripped_debug)
            linked_bytes = stripped_debug
        canonical_sections = api["_canonicalize_standard_section_order"](linked_bytes)
        if canonical_sections is not None:
            work_linked.write_bytes(canonical_sections)
            linked_bytes = canonical_sections
        published_linked = api["strip_wasm_publication_sections"](
            work_linked.read_bytes(),
            final_artifact=True,
            preserve_debug=preserve_debug_sections,
        )
        work_linked.write_bytes(published_linked)
        try:
            api["_publish_rust_wasm_link_facts"](
                wasm_facts_scanner,
                work_linked,
                layout=monolithic_callable_layout,
            )
        except ValueError as exc:
            print(
                f"Failed to attest final linked callable table: {exc}", file=sys.stderr
            )
            return 1
        if split_runtime:
            assert app_stage is not None
            assert rt_stage is not None
            try:
                published_app = api["_strip_and_restore_split_artifact"](
                    app_stage.read_bytes(),
                    artifact="app",
                    stage="publication-strip",
                    preserve_debug=preserve_debug_sections,
                    public_export_map=public_export_map,
                    required_native_direct_symbols=required_native_direct_symbols,
                    operation_counts=operation_counts,
                )
            except ValueError as exc:
                print(str(exc), file=sys.stderr)
                return 1
            published_runtime = api["strip_wasm_publication_sections"](
                rt_stage.read_bytes(),
                final_artifact=True,
                preserve_debug=preserve_debug_sections,
            )
            app_stage.write_bytes(published_app)
            rt_stage.write_bytes(published_runtime)
            try:
                assert split_callable_layout is not None
                app_facts = api["_publish_rust_wasm_link_facts"](
                    wasm_facts_scanner,
                    app_stage,
                    layout=split_callable_layout,
                    role="app",
                )
                final_split_callable_layout = api["_callable_layout_from_wasm_facts"](
                    app_facts
                )
                if final_split_callable_layout is None:
                    raise ValueError(
                        "final split app publication omitted callable-table layout"
                    )
                api["_publish_rust_wasm_link_facts"](
                    wasm_facts_scanner,
                    rt_stage,
                    layout=final_split_callable_layout,
                    role="runtime",
                )
            except ValueError as exc:
                print(
                    f"Failed to attest final split callable table: {exc}",
                    file=sys.stderr,
                )
                return 1
            assert size_attestation_stage is not None
            size_attestation["published"] = {
                "app": api["wasm_metrics"](app_stage.read_bytes()),
                "runtime": api["wasm_metrics"](rt_stage.read_bytes()),
            }
            size_attestation_stage.write_text(
                json.dumps(size_attestation, indent=2, sort_keys=True) + "\n",
                encoding="utf-8",
            )
        phase_timings_ms["wasm_strip"] = round(
            max(0.0, (time.perf_counter() - strip_start) * 1000.0), 6
        )

        app_export_error = api["_app_export_surface_error"](
            work_linked.read_bytes(),
            app_export_contract,
            stage="linked-publication",
        )
        if app_export_error is not None:
            print(app_export_error, file=sys.stderr)
            return 1
        if split_runtime:
            assert app_stage is not None
            app_export_error = api["_app_export_surface_error"](
                app_stage.read_bytes(),
                app_export_contract,
                stage="split-app-publication",
            )
            if app_export_error is not None:
                print(app_export_error, file=sys.stderr)
                return 1

        linked_ok = api["_validate_linked"](work_linked)
        if not linked_ok:
            failed_validation = linked.with_name(
                f"{linked.stem}.failed-validation.wasm"
            )
            failed_validation.write_bytes(work_linked.read_bytes())
            print(
                f"Preserved failed linked validation artifact: {failed_validation}",
                file=sys.stderr,
            )
            if split_runtime:
                print(
                    "Linked wasm validation failed before split-runtime publication; "
                    "failing because linked validation is the canonical table/memory/import guard.",
                    file=sys.stderr,
                )
            return 1

        publish_pairs = [(work_linked, linked)]
        validation_start = time.perf_counter()
        if split_runtime:
            assert app_stage is not None
            assert rt_stage is not None
            assert app_wasm is not None
            assert rt_wasm is not None
            assert size_attestation_path is not None
            assert size_attestation_stage is not None
            if not api["_validate_split_runtime_outputs"](app_stage, rt_stage):
                return 1
            publish_pairs.extend(
                [
                    (rt_stage, rt_wasm),
                    (app_stage, app_wasm),
                    (size_attestation_stage, size_attestation_path),
                ]
            )
        phase_timings_ms["fail_closed_validation"] = round(
            phase_timings_ms.get("fail_closed_validation", 0.0)
            + max(0.0, (time.perf_counter() - validation_start) * 1000.0),
            6,
        )
        try:
            api["artifact_publish"].publish_validated_outputs(publish_pairs)
        except OSError as exc:
            print(f"Failed to publish wasm linker outputs: {exc}", file=sys.stderr)
            return 1

        return 0
    finally:
        if split_runtime and "split_runtime_processing" not in phase_timings_ms:
            phase_timings_ms["split_runtime_processing"] = round(
                max(0.0, (time.perf_counter() - split_runtime_start) * 1000.0), 6
            )
        phase_timings_ms.setdefault("wasm_strip", 0.0)
        phase_timings_ms.setdefault("fail_closed_validation", 0.0)
        phase_timings_ms.update(
            {name: round(value, 6) for name, value in facts_metrics.items()}
        )
        phase_timings_ms.update(operation_counts)
        phase_timings_ms["wasm_link_total"] = round(
            max(0.0, (time.perf_counter() - total_start) * 1000.0), 6
        )
        if phase_timings_file is not None:
            phase_timings_file.parent.mkdir(parents=True, exist_ok=True)
            phase_timings_file.write_text(
                json.dumps(phase_timings_ms, sort_keys=True) + "\n",
                encoding="utf-8",
            )
        api["_WHOLE_ARTIFACT_OPERATION_COUNTS"].reset(whole_artifact_counts_token)
        for staged_output in staged_outputs:
            with contextlib.suppress(OSError):
                staged_output.unlink()
        temp_dir.cleanup()
