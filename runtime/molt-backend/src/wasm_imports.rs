//! Static import registry and op→import dependency table for WASM backend.
//!
//! Adding a new runtime import: add to `wasm_abi_manifest.toml` + OP_IMPORT_DEPS,
//! then run `python tools/gen_wasm_abi.py`.
//! The codegen declares its own dependencies through OP_IMPORT_DEPS.

use crate::SimpleIR;
use crate::representation_plan::ScalarRepresentationPlan;
use crate::wasm_plan::wasm_specialized_container_import;
use std::collections::{BTreeMap, BTreeSet};

/// (import_name, wasm_type_idx) for every host import.
///
/// Generated from `wasm_abi_manifest.toml` so Rust codegen, Python runtime
/// export validation, and tools consume one import-name/type authority.
pub(crate) use crate::wasm_abi_generated::IMPORT_REGISTRY;

pub(crate) const OP_IMPORT_DEPS: &[(&str, &[&str])] = &[
    // ── Core structural imports ──
    // Absolute minimum needed by ANY program regardless of IR content.
    // Everything else is discovered by IR-scanning below.
    // Target: <80 for hello-world. Current: ~50.
    (
        "__structural__",
        &[
            // Memory management
            "alloc",
            "dec_ref_obj",
            "inc_ref_obj",
            // Call dispatch (call_func uses call_func_dispatch)
            "call_func_dispatch",
            // Exceptions — any program can raise
            "exception_active",
            "exception_current",
            "exception_enter_handler",
            "exception_class",
            "exception_clear",
            "exception_context_set",
            "exception_kind",
            "exception_last",
            "exception_last_pending",
            "exception_resolve_captured",
            "exception_message",
            "exception_new",
            "exception_new_from_class",
            "exception_pending",
            "exception_pop",
            "exception_push",
            "exception_set_cause",
            "exception_set_last",
            "exception_set_value",
            "exception_stack_clear",
            // Function creation (func_new is used by every module)
            "func_new",
            // Pointer resolution
            "handle_resolve",
            // Identity and truthiness
            "is",
            "is_truthy",
            "not",
            // Module system
            "module_cache_get",
            "module_cache_set",
            "module_get_attr",
            "module_new",
            "module_set_attr",
            // Output
            "print_newline",
            "print_obj",
            // Exception raising
            "raise",
            // Runtime lifecycle
            "runtime_init",
            "runtime_shutdown",
            // Bootstrap
            "set_intrinsic_manifest",
            "sys_set_version_info",
        ],
    ),
    // ── On-demand: fast-path truthiness ──
    // Pulled in when if/while_test ops exist; codegen may select the integer
    // variant only from explicit representation flags.
    ("if", &["is_truthy_int", "is_truthy_bool"]),
    ("while_test", &["is_truthy_int", "is_truthy_bool"]),
    // ── On-demand: comparison ops ──
    // Pulled in when comparison ops appear in IR.
    ("eq", &["eq"]),
    ("ne", &["ne"]),
    ("lt", &["lt"]),
    ("le", &["le"]),
    ("gt", &["gt"]),
    ("ge", &["ge"]),
    // ── On-demand: context managers ──
    // Pulled in when with-statement ops appear in IR.
    (
        "context_enter",
        &[
            "context_enter",
            "context_exit",
            "context_depth",
            "context_closing",
            "context_null",
            "context_unwind",
            "context_unwind_to",
        ],
    ),
    (
        "context_exit",
        &["context_exit", "context_depth", "context_unwind_to"],
    ),
    ("context_closing", &["context_closing"]),
    ("context_null", &["context_null"]),
    (
        "context_unwind",
        &["context_unwind", "context_unwind_to", "context_depth"],
    ),
    // ── On-demand: class infrastructure ──
    // Pulled in when class-definition ops appear in IR.
    (
        "alloc_class",
        &[
            "alloc_class",
            "class_new",
            "class_set_base",
            "class_apply_set_name",
            "class_layout_version",
            "class_merge_layout",
            "class_set_layout_version",
        ],
    ),
    (
        "alloc_class_static",
        &[
            "alloc_class_static",
            "class_new",
            "class_set_base",
            "class_apply_set_name",
            "class_layout_version",
            "class_merge_layout",
            "class_set_layout_version",
        ],
    ),
    (
        "alloc_class_trusted",
        &[
            "alloc_class_trusted",
            "class_new",
            "class_set_base",
            "class_apply_set_name",
            "class_layout_version",
            "class_merge_layout",
            "class_set_layout_version",
        ],
    ),
    (
        "class_new",
        &[
            "class_new",
            "class_set_base",
            "class_apply_set_name",
            "class_layout_version",
            "class_merge_layout",
            "class_set_layout_version",
        ],
    ),
    (
        "class_def",
        &[
            "guarded_class_def",
            "class_layout_version",
            "class_set_layout_version",
        ],
    ),
    // ── On-demand: object field access ──
    // Pulled in when load/store/object ops appear in IR.
    ("object_new", &["object_new", "object_set_class"]),
    ("object_new_bound", &[]),
    ("object_new_bound_stack", &["object_new_bound_sized"]),
    ("object_field_get", &["object_field_get"]),
    ("object_field_get_ptr", &["object_field_get_ptr"]),
    ("object_field_init", &["object_field_init"]),
    ("object_field_init_ptr", &["object_field_init_ptr"]),
    ("object_field_set", &["object_field_set"]),
    ("object_field_set_ptr", &["object_field_set_ptr"]),
    // ── On-demand: guards and inline caches ──
    // Pulled in when guard/guarded ops appear in IR. (Also via guarded_field_* entries below.)
    ("guard_type", &["guard_type"]),
    ("guard_layout_ptr", &["guard_layout_ptr"]),
    // ── On-demand: closures ──
    // Already have entries below; removed from structural.
    // ── On-demand: exception groups ──
    ("exceptiongroup_combine", &["exceptiongroup_combine"]),
    ("exceptiongroup_match", &["exceptiongroup_match"]),
    // ── On-demand: singletons ──
    ("ellipsis", &["ellipsis"]),
    ("missing", &["missing"]),
    ("not_implemented", &["not_implemented"]),
    ("pending", &["pending"]),
    // ── On-demand: function variants ──
    ("func_new_builtin", &["func_new_builtin"]),
    ("func_new_closure", &["func_new_closure"]),
    ("fn_ptr_code_set", &["fn_ptr_code_set"]),
    ("builtin_func", &["func_new_builtin"]),
    // ── On-demand: call variants ──
    ("call_bind", &["call_bind"]),
    ("call_bind_ic", &["call_bind_ic"]),
    // Fused method-dispatch ICs: a single op kind may lower to any of the five
    // arity variants (the codegen selects by positional-arg count), so pull in
    // the whole family when the fused op is present. Mirrors the context_enter
    // family-expansion convention.
    (
        "call_method_ic",
        &[
            "call_method_ic0",
            "call_method_ic1",
            "call_method_ic2",
            "call_method_ic3",
            "call_method_ic4",
        ],
    ),
    (
        "call_super_method_ic",
        &[
            "call_super_method_ic0",
            "call_super_method_ic1",
            "call_super_method_ic2",
            "call_super_method_ic3",
            "call_super_method_ic4",
        ],
    ),
    ("call_arity_error", &["call_arity_error"]),
    ("call_indirect", &["call_indirect_ic"]),
    ("call_indirect_ic", &["call_indirect_ic"]),
    ("invoke_ffi_ic", &["invoke_ffi_ic"]),
    // ── On-demand: module system extras ──
    ("module_cache_del", &["module_cache_del"]),
    ("module_del_global", &["module_del_global"]),
    (
        "module_del_global_if_present",
        &["module_del_global_if_present"],
    ),
    ("module_get_global", &["module_get_global"]),
    ("module_get_name", &["module_get_name"]),
    ("module_import_from", &["module_import_from"]),
    ("module_import", &["module_import"]),
    ("module_import_star", &["module_import_star"]),
    // ── On-demand: code objects and tracing ──
    ("code_new", &["code_new"]),
    ("code_slot_set", &["code_slot_set"]),
    ("code_slots_init", &["code_slots_init"]),
    ("trace_enter_slot", &["trace_enter_slot"]),
    ("trace_exit", &["trace_exit"]),
    ("line", &["trace_set_line"]),
    ("trace_set_line", &["trace_set_line"]),
    // ── On-demand: check_exception ──
    ("check_exception", &["exception_pending"]),
    // ── Defaults-devirt deopt guard: reads the function defaults version ──
    ("function_defaults_version", &["function_defaults_version"]),
    // ── Arithmetic ops ──
    // Auto-discovered by kind match, declared here for completeness
    // so the scanner has explicit dep table hits.
    ("add", &["add", "str_concat"]),
    ("str_concat", &["str_concat"]),
    ("sub", &["sub"]),
    ("mul", &["mul"]),
    ("div", &["div"]),
    ("floordiv", &["floordiv"]),
    ("mod", &["mod"]),
    ("pow", &["pow"]),
    ("pow_mod", &["pow_mod"]),
    ("lshift", &["lshift"]),
    ("rshift", &["rshift"]),
    ("bit_and", &["bit_and"]),
    ("bit_or", &["bit_or"]),
    ("bit_xor", &["bit_xor"]),
    ("invert", &["invert"]),
    ("neg", &["neg"]),
    ("pos", &["pos"]),
    ("matmul", &["matmul"]),
    ("inplace_add", &["inplace_add", "str_concat"]),
    ("inplace_sub", &["inplace_sub"]),
    ("inplace_mul", &["inplace_mul"]),
    ("inplace_div", &["inplace_div"]),
    ("inplace_floordiv", &["inplace_floordiv"]),
    ("inplace_mod", &["inplace_mod"]),
    ("inplace_pow", &["inplace_pow"]),
    ("inplace_lshift", &["inplace_lshift"]),
    ("inplace_rshift", &["inplace_rshift"]),
    ("inplace_matmul", &["inplace_matmul"]),
    ("inplace_bit_and", &["inplace_bit_and"]),
    ("inplace_bit_or", &["inplace_bit_or"]),
    ("inplace_bit_xor", &["inplace_bit_xor"]),
    ("abs_builtin", &["abs_builtin"]),
    ("round", &["round"]),
    ("trunc", &["trunc"]),
    // Length ops
    ("len", &["len"]),
    ("len_dict", &["len_dict"]),
    ("len_list", &["len_list"]),
    ("len_set", &["len_set"]),
    ("len_str", &["len_str"]),
    ("len_tuple", &["len_tuple"]),
    // Attribute access ops
    ("get_attr_generic", &["get_attr_generic"]),
    ("get_attr_name", &["get_attr_name"]),
    ("get_attr_name_default", &["get_attr_name_default"]),
    ("get_attr_object", &["get_attr_object"]),
    ("get_attr_special", &["get_attr_special"]),
    ("set_attr_generic", &["set_attr_generic"]),
    ("set_attr_name", &["set_attr_name"]),
    ("del_attr_generic", &["del_attr_generic"]),
    ("del_attr_name", &["del_attr_name"]),
    ("del_attr_object", &["del_attr_object"]),
    ("has_attr_name", &["has_attr_name"]),
    // Iterator ops
    ("iter", &["iter"]),
    ("iter_next", &["iter_next"]),
    ("iter_next_unboxed", &["index", "iter_next"]),
    ("iter_sentinel", &["iter_sentinel"]),
    ("contains", &["contains"]),
    // Comparison/identity ops
    ("guard_tag", &["guard_type"]),
    ("isinstance", &["isinstance"]),
    ("exception_match_builtin", &["exception_match_builtin"]),
    ("issubclass", &["issubclass"]),
    ("is_bound_method", &["is_bound_method"]),
    ("is_callable", &["is_callable"]),
    ("is_function_obj", &["is_function_obj"]),
    ("is_generator", &["is_generator"]),
    ("exception_new_builtin", &["exception_new_builtin"]),
    (
        "exception_new_builtin_empty",
        &["exception_new_builtin_empty"],
    ),
    ("exception_new_builtin_one", &["exception_new_builtin_one"]),
    // Generator ops
    ("generator_close", &["generator_close"]),
    ("generator_send", &["generator_send"]),
    ("generator_throw", &["generator_throw"]),
    // Builtins and type ops
    ("builtin_type", &["builtin_type"]),
    ("type_of", &["type_of"]),
    ("bound_method_new", &["bound_method_new"]),
    ("bridge_unavailable", &["bridge_unavailable"]),
    ("classmethod_new", &["classmethod_new"]),
    ("staticmethod_new", &["staticmethod_new"]),
    ("property_new", &["property_new"]),
    ("super_new", &["super_new"]),
    ("repr_builtin", &["repr_builtin"]),
    ("repr_from_obj", &["repr_from_obj"]),
    ("int_from_str_of_obj", &["int_from_str_of_obj"]),
    ("ord_at", &["ord_at"]),
    ("str_from_obj", &["str_from_obj"]),
    ("format_builtin", &["format_builtin"]),
    ("string_eq", &["string_eq"]),
    ("frame_locals_set", &["frame_locals_set"]),
    (
        "recursion_guard_enter",
        &["recursion_guard_enter", "recursion_guard_exit"],
    ),
    ("recursion_guard_exit", &["recursion_guard_exit"]),
    // Call support ops
    ("callargs_new", &["callargs_new"]),
    ("callargs_push_pos", &["callargs_push_pos"]),
    ("callargs_push_kw", &["callargs_push_kw"]),
    ("callargs_expand_star", &["callargs_expand_star"]),
    ("callargs_expand_kwstar", &["callargs_expand_kwstar"]),
    // Function metadata ops
    ("function_default_kind", &["function_default_kind"]),
    ("function_is_coroutine", &["function_is_coroutine"]),
    ("function_is_generator", &["function_is_generator"]),
    (
        "function_init_metadata_packed",
        &["function_init_metadata_packed"],
    ),
    ("function_set_builtin", &["function_set_builtin"]),
    ("function_set_defaults", &["function_set_defaults"]),
    (
        "async_generator_yield",
        &[
            "asyncgen_hooks_get",
            "asyncgen_hooks_set",
            "asyncgen_locals",
            "asyncgen_locals_register",
            "asyncgen_new",
            "asyncgen_shutdown",
        ],
    ),
    (
        "asyncgen_new",
        &[
            "asyncgen_hooks_get",
            "asyncgen_hooks_set",
            "asyncgen_locals",
            "asyncgen_locals_register",
            "asyncgen_new",
            "asyncgen_shutdown",
        ],
    ),
    ("call_async", &["handle_resolve", "inc_ref_obj", "task_new"]),
    (
        "call_guarded",
        &[
            "call_bind_ic",
            "call_func_dispatch",
            "callargs_new",
            "callargs_push_pos",
            "handle_resolve",
            "is_function_obj",
            "is_truthy",
            "recursion_guard_enter",
            "recursion_guard_exit",
            "trace_enter_slot",
            "trace_exit",
        ],
    ),
    (
        "call_method",
        &[
            "call_bind_ic",
            "callargs_new",
            "callargs_push_pos",
            "fast_dict_get",
            "fast_list_append",
            "fast_str_join",
            "fast_str_startswith",
            "fast_str_upper",
            "fast_str_lower",
            "fast_str_strip",
        ],
    ),
    (
        "cbor_parse",
        &[
            "alloc",
            "cbor_parse_scalar",
            "cbor_parse_scalar_obj",
            "handle_resolve",
        ],
    ),
    ("closure_load", &["closure_load", "handle_resolve"]),
    ("closure_store", &["closure_store", "handle_resolve"]),
    ("const_bigint", &["bigint_from_str"]),
    ("const_bytes", &["bytes_from_bytes"]),
    ("const_ellipsis", &["ellipsis"]),
    ("const_not_implemented", &["not_implemented"]),
    ("const_str", &["string_from_bytes"]),
    (
        "coroutine",
        &[
            "cancel_token_get_current",
            "handle_resolve",
            "inc_ref_obj",
            "task_new",
            "task_register_token_owned",
        ],
    ),
    (
        "alloc_task",
        &[
            "cancel_token_get_current",
            "handle_resolve",
            "inc_ref_obj",
            "task_new",
            "task_register_token_owned",
        ],
    ),
    ("del_attr_generic_ptr", &["del_attr_ptr", "handle_resolve"]),
    ("dict_new", &["dict_new", "dict_set"]),
    ("frozenset_new", &["frozenset_add", "frozenset_new"]),
    (
        "function_closure_bits",
        &["function_closure_bits", "inc_ref_obj"],
    ),
    (
        "gen_locals_register",
        &["gen_locals", "gen_locals_register"],
    ),
    ("get_attr_generic_obj", &["get_attr_object_ic"]),
    ("get_attr_generic_ptr", &["get_attr_ptr", "handle_resolve"]),
    ("get_attr_special_obj", &["get_attr_special"]),
    ("guard_layout", &["guard_layout_ptr", "handle_resolve"]),
    (
        "guarded_field_get",
        &[
            "guard_layout_ptr",
            "guarded_field_get_ptr",
            "handle_resolve",
            "inc_ref_obj",
        ],
    ),
    (
        "guarded_field_init",
        &[
            "guard_layout_ptr",
            "guarded_field_init_ptr",
            "handle_resolve",
            "object_field_init_ptr",
        ],
    ),
    (
        "guarded_field_set",
        &[
            "guard_layout_ptr",
            "guarded_field_set_ptr",
            "handle_resolve",
            "object_field_set_ptr",
        ],
    ),
    ("guarded_load", &["inc_ref_obj", "object_field_get"]),
    (
        "invoke_ffi",
        &["callargs_new", "callargs_push_pos", "invoke_ffi_ic"],
    ),
    ("io_wait", &["io_wait", "io_wait_new"]),
    ("io_wait_new", &["io_wait", "io_wait_new"]),
    (
        "json_parse",
        &[
            "alloc",
            "handle_resolve",
            "json_parse_scalar",
            "json_parse_scalar_obj",
        ],
    ),
    ("list_int_new", &["list_int_new"]),
    ("list_fill_new", &["list_fill_new"]),
    (
        "build_list",
        &[
            "list_builder_append",
            "list_builder_finish",
            "list_builder_new",
        ],
    ),
    (
        "list_new",
        &[
            "list_builder_append",
            "list_builder_finish",
            "list_builder_new",
        ],
    ),
    ("load", &["inc_ref_obj", "object_field_get"]),
    (
        "msgpack_parse",
        &[
            "alloc",
            "handle_resolve",
            "msgpack_parse_scalar",
            "msgpack_parse_scalar_obj",
        ],
    ),
    ("object_set_class", &["handle_resolve", "object_set_class"]),
    ("process_spawn", &["process_poll", "process_spawn"]),
    ("set_attr_generic_obj", &["set_attr_object"]),
    ("set_attr_generic_ptr", &["handle_resolve", "set_attr_ptr"]),
    ("set_new", &["set_add", "set_new"]),
    (
        "state_transition",
        &[
            "closure_store",
            "future_poll",
            "handle_resolve",
            "obj_get_state",
            "obj_set_state",
            "sleep_register",
        ],
    ),
    ("state_yield", &["inc_ref_obj", "obj_set_state"]),
    ("chan_send_yield", &["chan_send", "obj_set_state"]),
    ("chan_recv_yield", &["chan_recv", "obj_set_state"]),
    ("store", &["object_field_set"]),
    ("store_init", &["object_field_init"]),
    ("string_format", &["format_builtin"]),
    ("task_new", &["task_new", "task_register_token_owned"]),
    ("thread_spawn", &["thread_poll", "thread_spawn"]),
    ("thread_submit", &["thread_poll", "thread_submit"]),
    (
        "tuple_new",
        &[
            "list_builder_append",
            "list_builder_new",
            "tuple_builder_finish",
        ],
    ),
    ("unpack_sequence", &["index"]),
    ("yield_await", &["future_poll"]),
    ("yield_send", &["future_poll"]),
    // Websocket group: any ws_ op requires the full websocket import set.
    // Each individual ws_ op maps to the same complete group because the
    // runtime's ws implementation is tightly coupled (connect needs wait,
    // send needs wait, etc.).
    (
        "ws_connect",
        &[
            "ws_close",
            "ws_connect",
            "ws_connect_obj",
            "ws_drop",
            "ws_pair",
            "ws_pair_obj",
            "ws_recv",
            "ws_send",
            "ws_send_obj",
            "ws_wait",
            "ws_wait_new",
        ],
    ),
    (
        "ws_connect_obj",
        &[
            "ws_close",
            "ws_connect",
            "ws_connect_obj",
            "ws_drop",
            "ws_pair",
            "ws_pair_obj",
            "ws_recv",
            "ws_send",
            "ws_send_obj",
            "ws_wait",
            "ws_wait_new",
        ],
    ),
    (
        "ws_pair",
        &[
            "ws_close",
            "ws_connect",
            "ws_connect_obj",
            "ws_drop",
            "ws_pair",
            "ws_pair_obj",
            "ws_recv",
            "ws_send",
            "ws_send_obj",
            "ws_wait",
            "ws_wait_new",
        ],
    ),
    (
        "ws_pair_obj",
        &[
            "ws_close",
            "ws_connect",
            "ws_connect_obj",
            "ws_drop",
            "ws_pair",
            "ws_pair_obj",
            "ws_recv",
            "ws_send",
            "ws_send_obj",
            "ws_wait",
            "ws_wait_new",
        ],
    ),
    (
        "ws_send",
        &[
            "ws_close",
            "ws_connect",
            "ws_connect_obj",
            "ws_drop",
            "ws_pair",
            "ws_pair_obj",
            "ws_recv",
            "ws_send",
            "ws_send_obj",
            "ws_wait",
            "ws_wait_new",
        ],
    ),
    (
        "ws_send_obj",
        &[
            "ws_close",
            "ws_connect",
            "ws_connect_obj",
            "ws_drop",
            "ws_pair",
            "ws_pair_obj",
            "ws_recv",
            "ws_send",
            "ws_send_obj",
            "ws_wait",
            "ws_wait_new",
        ],
    ),
    (
        "ws_recv",
        &[
            "ws_close",
            "ws_connect",
            "ws_connect_obj",
            "ws_drop",
            "ws_pair",
            "ws_pair_obj",
            "ws_recv",
            "ws_send",
            "ws_send_obj",
            "ws_wait",
            "ws_wait_new",
        ],
    ),
    (
        "ws_close",
        &[
            "ws_close",
            "ws_connect",
            "ws_connect_obj",
            "ws_drop",
            "ws_pair",
            "ws_pair_obj",
            "ws_recv",
            "ws_send",
            "ws_send_obj",
            "ws_wait",
            "ws_wait_new",
        ],
    ),
    (
        "ws_drop",
        &[
            "ws_close",
            "ws_connect",
            "ws_connect_obj",
            "ws_drop",
            "ws_pair",
            "ws_pair_obj",
            "ws_recv",
            "ws_send",
            "ws_send_obj",
            "ws_wait",
            "ws_wait_new",
        ],
    ),
    (
        "ws_wait",
        &[
            "ws_close",
            "ws_connect",
            "ws_connect_obj",
            "ws_drop",
            "ws_pair",
            "ws_pair_obj",
            "ws_recv",
            "ws_send",
            "ws_send_obj",
            "ws_wait",
            "ws_wait_new",
        ],
    ),
    (
        "ws_wait_new",
        &[
            "ws_close",
            "ws_connect",
            "ws_connect_obj",
            "ws_drop",
            "ws_pair",
            "ws_pair_obj",
            "ws_recv",
            "ws_send",
            "ws_send_obj",
            "ws_wait",
            "ws_wait_new",
        ],
    ),
];

/// Scan the IR for imports that must be declared before relocatable linking.
/// Non-relocatable Auto registers the canonical import registry and strips
/// from TrackedImportIds after codegen, because the emitted-use ledger is
/// the only exact authority for final import retention. Relocatable Auto
/// cannot use that post-serialization strip phase, so it still needs this
/// conservative pre-emission set for declarations handed to the linker.
pub(crate) fn collect_reloc_required_imports(ir: &SimpleIR) -> BTreeSet<String> {
    let mut required: BTreeSet<String> = BTreeSet::new();

    // Build a lookup from the deps table.
    let deps_map: BTreeMap<&str, &[&str]> = OP_IMPORT_DEPS.iter().map(|&(k, v)| (k, v)).collect();

    // Structural imports: always needed regardless of IR content.
    if let Some(structural) = deps_map.get("__structural__") {
        for &name in *structural {
            required.insert(name.to_string());
        }
    }

    if let Ok(extra_required) = std::env::var("MOLT_WASM_EXTRA_REQUIRED_IMPORTS") {
        for name in extra_required
            .split(',')
            .map(str::trim)
            .filter(|name| !name.is_empty())
        {
            required.insert(name.to_string());
        }
    }

    // Build the set of all defined function/import names so Auto mode can
    // distinguish internal direct calls from imported runtime direct calls.
    let defined_function_names: BTreeSet<&str> =
        ir.functions.iter().map(|func| func.name.as_str()).collect();

    // Build the set of all known import names for auto-discovery.
    let known_imports: BTreeSet<&str> = IMPORT_REGISTRY.iter().map(|&(name, _)| name).collect();

    // Scan IR ops: each op declares its own import dependencies.
    for func_ir in &ir.functions {
        let scalar_plan = ScalarRepresentationPlan::for_function_ir(func_ir);
        for (op_index, op) in func_ir.ops.iter().enumerate() {
            let kind = op.kind.as_str();
            if matches!(
                std::env::var("MOLT_DEBUG_WASM_IMPORTS").ok().as_deref(),
                Some("1")
            ) && op.s_value.as_deref() == Some("__main_____f_poll")
            {
                eprintln!(
                    "WASM_IMPORTS saw_op kind={} s_value={:?} task_kind={:?} args={:?} func={}",
                    kind, op.s_value, op.task_kind, op.args, func_ir.name
                );
            }

            // `object_new_bound` chooses its runtime entry from the
            // static payload-size metadata attached to the op. Keep the
            // auto import set as precise as the codegen path: typed class
            // allocation sites with a positive payload size use the sized
            // constructor, while hand-built legacy SimpleIR without the
            // metadata keeps the unsized verifier/allocator path.
            if kind == "object_new_bound" {
                let import_name = if op.value.is_some_and(|size| size > 0) {
                    "object_new_bound_sized"
                } else {
                    "object_new_bound"
                };
                required.insert(import_name.to_string());
                continue;
            }

            // Check explicit dependency table first.
            if let Some(deps) = deps_map.get(kind) {
                if matches!(
                    std::env::var("MOLT_DEBUG_WASM_IMPORTS").ok().as_deref(),
                    Some("1")
                ) && kind == "alloc_task"
                {
                    eprintln!(
                        "WASM_IMPORTS alloc_task deps={deps:?} func={}",
                        func_ir.name
                    );
                }
                for &dep in *deps {
                    required.insert(dep.to_string());
                }
            }
            // Auto-discovery: if op kind matches a known import name, include it.
            else if known_imports.contains(kind) {
                required.insert(kind.to_string());
            }
            if crate::tir::op_kinds_generated::kind_result_mints_owned_selected_operand_table(kind)
                && op.out.is_some()
            {
                required.insert("inc_ref_obj".to_string());
            }

            // builtin_func ops reference imports by s_value (with molt_ prefix).
            if kind == "builtin_func"
                && let Some(name) = op.s_value.as_ref()
            {
                let import_name = name.strip_prefix("molt_").unwrap_or(name.as_str());
                required.insert(import_name.to_string());
            }
            if kind == "call"
                && let Some(name) = op.s_value.as_ref()
                && !defined_function_names.contains(name.as_str())
            {
                let import_name = name.strip_prefix("molt_").unwrap_or(name.as_str());
                if name.starts_with("molt_") || known_imports.contains(import_name) {
                    required.insert(import_name.to_string());
                }
            }
            if kind == "call_async"
                && let Some(name) = op.s_value.as_ref()
            {
                let import_name = name.strip_prefix("molt_").unwrap_or(name.as_str());
                if known_imports.contains(import_name) {
                    required.insert(import_name.to_string());
                }
            }

            // Task allocation semantics are keyed off task_kind metadata in
            // practice; keep the required runtime imports even if an
            // intermediate op kind rewrite obscures the original alloc_task
            // key before import collection runs.
            if let Some(task_kind) = op.task_kind.as_deref() {
                if matches!(
                    std::env::var("MOLT_DEBUG_WASM_IMPORTS").ok().as_deref(),
                    Some("1")
                ) {
                    eprintln!(
                        "WASM_IMPORTS task_meta kind={} task_kind={} args={} func={}",
                        kind,
                        task_kind,
                        op.args.as_ref().map(|a| a.len()).unwrap_or(0),
                        func_ir.name
                    );
                }
                required.insert("task_new".to_string());
                let has_args = op.args.as_ref().is_some_and(|args| !args.is_empty());
                if has_args {
                    required.insert("handle_resolve".to_string());
                    required.insert("inc_ref_obj".to_string());
                }
                if matches!(task_kind, "future" | "coroutine") {
                    required.insert("cancel_token_get_current".to_string());
                    required.insert("task_register_token_owned".to_string());
                }
            }

            // Some ops lower to specialized runtime imports selected from
            // final TIR/LIR container facts at codegen time. Mirror that
            // logic here so Auto mode keeps the same import lane that
            // compile_func will actually emit.
            let specialized_import =
                wasm_specialized_container_import(&scalar_plan, op_index, kind, op);
            if let Some(import_name) = specialized_import {
                required.insert(import_name.to_string());
            }

            // Prefix-based discovery for stdlib groups.
            // If the op kind starts with a known stdlib prefix, include it.
            // Group expansions (e.g., ws_ -> all websocket imports) are handled
            // by OP_IMPORT_DEPS entries above, not here.
            for prefix in [
                "os_",
                "path_",
                "time_",
                "struct_",
                "importlib_",
                "asyncio_",
                "contextlib_async",
                "socket_",
                "file_",
                "stream_",
                "lock_",
                "rlock_",
                "thread_",
                "process_",
                "db_",
                "ws_",
                "cancel_token_",
                "chan_",
                "string_",
                "bytes_",
                "bytearray_",
                "math_",
                "json_",
                "msgpack_",
                "cbor_",
                "vec_",
                "heapq_",
                "buffer2d_",
                "statistics_",
                "weakref_",
                "memoryview_",
                "taq_",
                "sys_",
                "dataclass_",
            ] {
                if kind.starts_with(prefix) {
                    required.insert(kind.to_string());
                    break;
                }
            }

            // Special singleton matches.
            match kind {
                "socketpair" | "cancelled" | "cancel_current" | "spawn" | "block_on"
                | "sleep_register" | "intarray_from_seq" | "enumerate" | "aiter" | "anext"
                | "open_builtin" | "compile_builtin" | "getargv" | "getpid" | "getframe"
                | "getcwd" | "getrecursionlimit" | "setrecursionlimit" | "env_get"
                | "env_snapshot" | "os_name" | "errno_constants" => {
                    required.insert(kind.to_string());
                }
                _ => {}
            }
        }

        // Scan poll functions for poll import references.
        if func_ir.name.ends_with("_poll") {
            for op in &func_ir.ops {
                if (op.kind == "call_func" || op.kind == "invoke_ffi")
                    && let Some(s) = op.s_value.as_ref()
                    && s.ends_with("_poll")
                {
                    let import_name = s.strip_prefix("molt_").unwrap_or(s.as_str());
                    required.insert(import_name.to_string());
                }
            }
        }
    }

    required
}

#[cfg(test)]
mod tests {
    use super::{IMPORT_REGISTRY, OP_IMPORT_DEPS};

    #[test]
    fn module_cache_del_is_registered_as_on_demand_wasm_import() {
        let import_type = IMPORT_REGISTRY
            .iter()
            .find_map(|&(name, type_idx)| (name == "module_cache_del").then_some(type_idx));
        assert_eq!(
            import_type,
            Some(2),
            "module_cache_del must use the unary i64 -> i64 host import ABI"
        );

        let structural = OP_IMPORT_DEPS
            .iter()
            .find_map(|&(kind, deps)| (kind == "__structural__").then_some(deps))
            .expect("structural WASM import deps must exist");
        assert!(
            !structural.contains(&"module_cache_del"),
            "module_cache_del is cleanup-only and must not inflate every Auto-profile WASM binary"
        );

        let op_deps = OP_IMPORT_DEPS
            .iter()
            .find_map(|&(kind, deps)| (kind == "module_cache_del").then_some(deps))
            .expect("module_cache_del op must declare its WASM import dependency");
        assert_eq!(
            op_deps,
            ["module_cache_del"],
            "module_cache_del codegen must request its runtime import explicitly"
        );
    }

    #[test]
    fn object_new_bound_declares_wasm_imports() {
        let bound_type = IMPORT_REGISTRY
            .iter()
            .find_map(|&(name, type_idx)| (name == "object_new_bound").then_some(type_idx));
        assert_eq!(
            bound_type,
            Some(2),
            "object_new_bound must use the unary i64 -> i64 host import ABI"
        );

        let sized_type = IMPORT_REGISTRY
            .iter()
            .find_map(|&(name, type_idx)| (name == "object_new_bound_sized").then_some(type_idx));
        assert_eq!(
            sized_type,
            Some(3),
            "object_new_bound_sized must use the binary i64,i64 -> i64 host import ABI"
        );

        let op_deps = OP_IMPORT_DEPS
            .iter()
            .find_map(|&(kind, deps)| (kind == "object_new_bound").then_some(deps))
            .expect("object_new_bound op must declare its WASM import dependencies");
        assert!(
            op_deps.is_empty(),
            "object_new_bound dependencies are selected from payload-size metadata during import collection"
        );

        let stack_deps = OP_IMPORT_DEPS
            .iter()
            .find_map(|&(kind, deps)| (kind == "object_new_bound_stack").then_some(deps))
            .expect("object_new_bound_stack op must declare its WASM import dependencies");
        assert_eq!(
            stack_deps,
            ["object_new_bound_sized"],
            "WASM has no native stack object representation; stack-eligible class allocation lowers to the sized heap constructor"
        );
    }
}
