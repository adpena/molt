"""Drift and semantic teeth for the heap-kind lifetime authority."""

from __future__ import annotations

import importlib
import json
from pathlib import Path

import pytest


ROOT = Path(__file__).resolve().parents[1]


def _gen():
    return importlib.import_module("tools.gen_heap_kinds")


def test_generated_outputs_are_byte_exact() -> None:
    gen = _gen()
    rendered = gen.render_all(gen.load_table())
    for path, expected in rendered.items():
        assert path.read_bytes() == expected.encode("utf-8"), (
            f"{path.relative_to(ROOT)} is stale; run `python tools/gen_heap_kinds.py`"
        )


def test_inventory_is_sparse_object_plus_dense_builtin_domain() -> None:
    kinds = _gen().load_table()
    assert [(row["name"], row["id"]) for row in kinds if row["id"] < 200] == [
        ("OBJECT", 100)
    ]
    dense = [row["id"] for row in kinds if row["id"] >= 200]
    assert dense == list(range(200, 257))
    assert kinds[-1]["name"] == "WEAKREF"


def test_green_reference_holders_carry_closed_acyclic_capabilities() -> None:
    kinds = _gen().load_table()
    green_ref_holders = [
        row for row in kinds if row["cycle"] == "never" and row["edges"] != "none"
    ]
    assert green_ref_holders
    capability_holders = [
        row for row in green_ref_holders if row["publication"] == "python"
    ]
    assert {row["name"]: row["acyclic_capability"] for row in capability_holders} == {
        "RANGE": "int_triplet",
        "CODE": "code_metadata",
    }
    assert {
        row["name"]
        for row in green_ref_holders
        if row["publication"] == "linear_unpublished"
    } == {"LIST_BUILDER", "DICT_BUILDER", "SET_BUILDER", "CALLARGS"}
    by_name = {row["name"]: row for row in kinds}
    assert by_name["RANGE"]["acyclic_slots"] == {
        "start": "int",
        "stop": "int",
        "step": "int",
    }
    assert by_name["CODE"]["acyclic_slots"] == dict(_gen().ACYCLIC_SLOT_SCHEMAS["CODE"])


def test_cpython_weakref_policy_is_explicit_and_exact() -> None:
    by_name = {row["name"]: row for row in _gen().load_table()}
    allowed = {
        name for name, row in by_name.items() if row["weakref"] == "allow"
    }
    assert allowed == {
        "MEMORYVIEW",
        "FUNCTION",
        "BOUND_METHOD",
        "MODULE",
        "TYPE",
        "GENERATOR",
        "SET",
        "FROZENSET",
        "CODE",
        "GENERIC_ALIAS",
        "ASYNC_GENERATOR",
    }
    assert by_name["OBJECT"]["weakref"] == "class"
    assert by_name["FOREIGN"]["weakref"] == "class"
    for denied in ("LIST", "DICT", "EXCEPTION", "PROPERTY", "SUPER", "UNION"):
        assert by_name[denied]["weakref"] == "deny"


def test_cycle_policy_models_cpython_dynamic_container_tracking() -> None:
    by_name = {row["name"]: row for row in _gen().load_table()}
    assert by_name["DICT"]["cycle"] == "dynamic"
    assert by_name["TUPLE"]["cycle"] == "dynamic"
    assert by_name["SLICE"]["cycle"] == "always"
    assert by_name["RANGE"]["cycle"] == "never"
    assert by_name["CODE"]["cycle"] == "never"
    assert by_name["DICT"]["track"] == "dict_dynamic"
    assert by_name["TUPLE"]["track"] == "tuple_dynamic"
    assert by_name["OBJECT"]["track"] == "always"


def test_every_kind_gets_one_generated_direct_lifecycle_handler() -> None:
    gen = _gen()
    kinds = gen.load_table()
    rendered = gen.render_runtime(kinds)
    handler_body = rendered.split(
        "pub(crate) const fn heap_lifecycle_handler", 1
    )[1].split("pub(crate) const fn heap_kind_uses_object_layout", 1)[0]
    assert handler_body.count("=> Some(HeapLifecycleHandler::") == len(kinds)
    for row in kinds:
        variant = gen._variant(str(row["name"]).lower())
        assert (
            f"TYPE_ID_{row['name']} => Some(HeapLifecycleHandler::{variant})"
            in handler_body
        )

    for field in (
        "drop",
        "metrics",
        "weakref",
        "cycle",
        "layout",
        "shape",
        "publication",
        "external_gc",
        "acyclic_capability",
    ):
        policy_body = rendered.split(f"pub(crate) const fn heap_{field}_policy", 1)[
            1
        ].split("\n}\n", 1)[0]
        assert policy_body.count("=> Some(") == len(kinds), field


def test_runtime_visit_and_clear_dispatch_are_exhaustive_without_wildcards() -> None:
    gen = _gen()
    source = (
        ROOT / "runtime/molt-runtime/src/object/heap_lifecycle.rs"
    ).read_text(encoding="utf-8")
    visit = source.split("pub(crate) unsafe fn visit_owned_values", 1)[1].split(
        "pub(crate) unsafe fn visit_owned_edges", 1
    )[0]
    clear = source.split("pub(crate) unsafe fn clear_cycle_edges_with_sink", 1)[1].split(
        "pub(crate) unsafe fn detach_terminal_owned_edges", 1
    )[0]
    for row in gen.load_table():
        variant = f"HeapLifecycleHandler::{gen._variant(str(row['name']).lower())}"
        assert variant in visit, f"visit dispatch omits {row['name']}"
        assert variant in clear, f"clear dispatch omits {row['name']}"
    assert "_ =>" not in visit
    assert "_ =>" not in clear


def test_gc_deleted_legacy_type_id_traverse_and_clear_switches() -> None:
    source = (ROOT / "runtime/molt-runtime/src/object/gc.rs").read_text(
        encoding="utf-8"
    )
    traverse = source.split("pub(crate) unsafe fn molt_traverse", 1)[1].split(
        "/// molt's `tp_clear`", 1
    )[0]
    clear = source.split("pub(crate) unsafe fn molt_clear", 1)[1].split(
        "// The collector", 1
    )[0]
    assert "match type_id" not in traverse
    assert "match type_id" not in clear
    assert "heap_lifecycle::visit_owned_edges" in traverse
    assert "heap_lifecycle::clear_cycle_edges" in clear


def test_gc_reentrancy_is_runtime_owned_and_free_thread_fails_before_snapshot() -> None:
    gc = (ROOT / "runtime/molt-runtime/src/object/gc.rs").read_text(
        encoding="utf-8"
    )
    state = (ROOT / "runtime/molt-runtime/src/state/runtime_state.rs").read_text(
        encoding="utf-8"
    )
    assert "static GC_RUNNING" not in gc
    assert "pub(crate) gc_running: AtomicBool" in state
    assert "runtime_state(py).gc_running" in gc
    collector = gc.split("pub(crate) unsafe fn collect_generation", 1)[1].split(
        "pub(crate) unsafe fn collect_cycles", 1
    )[0]
    free_thread = collector.split('if cfg!(feature = "free-threaded")', 1)[1].split(
        "// Reentrancy guard", 1
    )[0]
    assert "GcCollectStatus::UnsupportedConcurrency" in free_thread
    assert "CollectStats::failure" in free_thread
    assert "snapshot_tracked_registry" not in free_thread


def test_dynamic_tracking_requires_explicit_projection(tmp_path: Path) -> None:
    gen = _gen()
    source = gen.TABLE.read_text(encoding="utf-8").replace(
        'track = "dict_dynamic"\n', "", 1
    )
    table = tmp_path / "heap_kinds.toml"
    table.write_text(source, encoding="utf-8")
    with pytest.raises(ValueError, match="dynamic heap kind DICT"):
        gen.load_table(table)


def test_audit_json_preserves_every_schema_row() -> None:
    gen = _gen()
    audit = json.loads(gen.OUT_AUDIT.read_text(encoding="utf-8"))
    assert audit["source"] == "runtime/heap_kinds.toml"
    assert audit["kinds"] == gen.load_table()
    assert audit["object_shapes"] == gen.load_object_shapes()


def test_object_shape_abi_is_generated_from_same_authority() -> None:
    gen = _gen()
    shapes = gen.load_object_shapes()
    assert shapes[0] == {
        "name": "PLAIN",
        "id": 0,
        "family": "plain",
        "resource_slot0": "none",
    }
    assert {row["name"] for row in shapes} >= {
        "GENERIC_TASK_PAYLOAD",
        "DICT_SUBCLASS",
        "FUNCTOOLS_PARTIAL",
        "ITERTOOLS_ZIP_LONGEST",
    }
    rendered = gen.render_all(gen.load_table())[gen.OUT_CORE]
    assert "pub enum ObjectShapeId" in rendered
    assert rendered.count("=> Self::") == len(shapes)
    core = (ROOT / "runtime/molt-runtime-core/src/lib.rs").read_text(encoding="utf-8")
    assert "pub enum ObjectShapeId" not in core
    assert "ObjectShapeId" in core
    assert "object_shape_lifecycle_family" in core


def test_no_manual_heap_policy_or_shape_authority_survives() -> None:
    runtime = ROOT / "runtime"
    object_shape_enums = []
    policy_enums = []
    for path in runtime.rglob("*.rs"):
        source = path.read_text(encoding="utf-8")
        if "enum ObjectShapeId" in source:
            object_shape_enums.append(path.relative_to(ROOT).as_posix())
        if "enum HeapLifecycleHandler" in source:
            policy_enums.append(path.relative_to(ROOT).as_posix())
    assert sorted(object_shape_enums) == [
        "runtime/molt-runtime-core/src/heap_kinds_generated.rs"
    ]
    assert sorted(policy_enums) == [
        "runtime/molt-runtime/src/object/heap_kinds_generated.rs"
    ]
    assert all(
        set(row) == {"name", "id", "family", "resource_slot0"}
        for row in _gen().load_object_shapes()
    ), "shape lifecycle and native-resource facts must remain generated"
    descriptor_consumers = []
    for path in (runtime / "molt-runtime/src").rglob("*.rs"):
        if path.name == "heap_kinds_generated.rs":
            continue
        if "heap_kind_descriptor(" in path.read_text(encoding="utf-8"):
            descriptor_consumers.append(path.relative_to(ROOT).as_posix())
    assert descriptor_consumers == [], (
        "cold audit descriptor leaked back into runtime dispatch: "
        f"{descriptor_consumers}"
    )


def test_every_generated_object_shape_has_visit_clear_and_terminal_dispatch() -> None:
    gen = _gen()
    source = (ROOT / "runtime/molt-runtime/src/object/mod.rs").read_text(
        encoding="utf-8"
    )
    generated = (ROOT / "runtime/molt-runtime-core/src/heap_kinds_generated.rs").read_text(
        encoding="utf-8"
    )
    visit = source.split("pub(crate) unsafe fn object_shape_visit_owned_edges", 1)[
        1
    ].split("pub(crate) unsafe fn object_shape_clear_cycle_edges", 1)[0]
    clear = source.split("pub(crate) unsafe fn object_shape_clear_cycle_edges", 1)[1].split(
        "pub(crate) unsafe fn dec_ref_ptr", 1
    )[0]
    for row in gen.load_object_shapes():
        variant = f"ObjectShapeId::{gen._variant(str(row['name']).lower())}"
        assert generated.count(variant) >= 2, f"generated projections omit {row['name']}"
    assert "object_shape_lifecycle_family(shape)" in visit
    assert "object_shape_lifecycle_family(shape)" in clear
    assert "DetachedResource::Functools" in clear
    assert "DetachedResource::Itertools" in clear
    assert "Vec::with_capacity" not in clear
    assert "DetachedEdgeSink" in clear


def test_variable_gc_edges_use_one_prereserved_detach_sink() -> None:
    lifecycle = (
        ROOT / "runtime/molt-runtime/src/object/heap_lifecycle.rs"
    ).read_text(encoding="utf-8")
    gc = (ROOT / "runtime/molt-runtime/src/object/gc.rs").read_text(
        encoding="utf-8"
    )
    generator_clear = lifecycle.split("unsafe fn detach_generator_owned_edges", 1)[
        1
    ].split("pub(crate) unsafe fn clear_cycle_edges", 1)[0]
    assert "detach_if_heap" in generator_clear
    assert "generator_exception_stack_take" in generator_clear
    assert "generator_context_stack_take" in generator_clear
    assert "GEN_CONTROL_SIZE..payload_size" in generator_clear
    delete_garbage = gc.split("pub(crate) unsafe fn collect_generation", 1)[1].split(
        "pub(crate) unsafe fn collect_cycles", 1
    )[0]
    assert "DetachedEdgeSink::try_with_capacities" in delete_garbage
    assert "clear_cycle_edges_with_sink" in delete_garbage
    assert "detached.release_all(py)" in delete_garbage
    clear_pos = delete_garbage.rfind("clear_cycle_edges_with_sink")
    release_pos = delete_garbage.rfind("detached.release_all(py)")
    assert clear_pos < release_pos, "release must follow the whole detach phase"


def test_gc_clear_handlers_are_detach_only_until_sink_release() -> None:
    lifecycle = (
        ROOT / "runtime/molt-runtime/src/object/heap_lifecycle.rs"
    ).read_text(encoding="utf-8")
    clear = lifecycle.split("pub(crate) unsafe fn clear_cycle_edges_with_sink", 1)[1]
    for forbidden in (
        "dec_ref_bits(",
        "dec_ref_ptr(",
        "weakref_object_release(",
        "weakcontainer_clear_state(",
        "asyncgen_clear_owned_edges(",
        "exception_release_detached_edges(",
    ):
        assert forbidden not in clear
    assert "weakref_object_detach_owned_edges" in clear
    assert "weakcontainer_detach_state" in clear
    assert "asyncgen_detach_owned_edges" in clear


def test_terminal_object_shape_dealloc_has_no_rediscovery_lane() -> None:
    source = (ROOT / "runtime/molt-runtime/src/object/mod.rs").read_text(
        encoding="utf-8"
    )
    physical = source.split("match heap_drop_policy(type_id)", 1)[1].split(
        "release_ptr(ptr)", 1
    )[0]
    assert "HeapDropPolicy::ObjectShape" in physical
    for forbidden in (
        "object_poll_fn",
        "poll_fn_addr",
        "issubclass_bits",
        "operator_drop_instance",
        "itertools_drop_instance",
        "functools_drop_instance",
        "types_drop_instance",
    ):
        assert forbidden not in physical


def test_terminal_dealloc_has_one_detach_authority_and_no_release_switch() -> None:
    source = (ROOT / "runtime/molt-runtime/src/object/mod.rs").read_text(
        encoding="utf-8"
    )
    terminal = source.split("let (terminal_edge_count, terminal_resource_count) =", 1)[1].split(
        "release_ptr(ptr);", 1
    )[0]
    assert "terminal_detach_capacity(py, ptr)" in terminal
    assert "detach_terminal_owned_edges(py, ptr" in terminal
    assert terminal.index("detach_terminal_owned_edges") < terminal.index(
        "terminal_edges.release_all(py)"
    )
    physical = terminal.split("match heap_drop_policy(type_id)", 1)[1]
    assert "dec_ref_bits(" not in physical
    assert "dec_ref_ptr(" not in physical
    assert "object_shape_terminal_drop" not in source
    assert "object_shape_drop_detached_resources" not in source
    assert "DetachedResource::RuntimeView" in terminal
    assert "runtime_object_destroyed" not in source


def test_terminal_metrics_commit_only_after_no_unwind_resource_teardown() -> None:
    source = (ROOT / "runtime/molt-runtime/src/object/mod.rs").read_text(
        encoding="utf-8"
    )
    terminal = source.split("let total_size =", 1)[1].split("#[cfg(test)]", 1)[0]
    teardown = terminal.index("terminal_resource_drop_no_unwind")
    release = terminal.index("release_ptr(ptr)")
    metrics = terminal.index("record_terminal_deallocation")
    assert teardown < release < metrics
    guard = source.split("fn terminal_resource_drop_no_unwind", 1)[1].split(
        "fn record_terminal_deallocation", 1
    )[0]
    assert "catch_unwind" in guard
    assert "std::process::abort()" in guard


def test_opaque_external_custody_is_explicit_not_silently_dynamic() -> None:
    by_name = {row["name"]: row for row in _gen().load_table()}
    assert by_name["NATIVE_HANDLE"]["external_gc"] == "opaque_rust_arc"
    assert by_name["FOREIGN"]["external_gc"] == "cpython_bridge"
    for name in ("NATIVE_HANDLE", "FOREIGN"):
        assert by_name[name]["edges"] == "none"
        assert by_name[name]["cycle"] == "never"
    foreign = (ROOT / "runtime/molt-runtime/src/object/foreign.rs").read_text(
        encoding="utf-8"
    )
    bridge = (ROOT / "runtime/molt-cpython-abi/src/bridge.rs").read_text(
        encoding="utf-8"
    )
    assert "molt_foreign_object_is_gc_capable(c_ptr)" in foreign
    assert "tp_is_gc" in bridge and "Py_TPFLAGS_HAVE_GC" in bridge
    native = (ROOT / "runtime/molt-runtime/src/object/native_handle.rs").read_text(
        encoding="utf-8"
    )
    assert "unsafe trait NativeHandleNoMoltEdges" in native
    assert "native_handle_new<T: NativeHandleNoMoltEdges>" in native


def test_generator_rejects_missing_or_unknown_acyclic_capability(tmp_path: Path) -> None:
    gen = _gen()
    source = gen.TABLE.read_text(encoding="utf-8").replace(
        'acyclic_capability = "int_triplet"\n',
        "",
        1,
    )
    table = tmp_path / "heap_kinds.toml"
    table.write_text(source, encoding="utf-8")
    with pytest.raises(ValueError, match="GREEN ref-holder RANGE"):
        gen.load_table(table)
    table.write_text(
        gen.TABLE.read_text(encoding="utf-8").replace(
            'acyclic_capability = "int_triplet"',
            'acyclic_capability = "hand_wavy_prose"',
            1,
        ),
        encoding="utf-8",
    )
    with pytest.raises(ValueError, match="invalid acyclic capability for RANGE"):
        gen.load_table(table)
    table.write_text(
        gen.TABLE.read_text(encoding="utf-8").replace(
            'linetable = "bytes_or_none"',
            'linetable = "nested_tuple"',
            1,
        ),
        encoding="utf-8",
    )
    with pytest.raises(ValueError, match="invalid acyclic edge domains for CODE"):
        gen.load_table(table)


def test_canonical_heap_cache_is_runtime_owned_and_has_no_losing_candidate_lane() -> None:
    builders = (ROOT / "runtime/molt-runtime/src/object/builders.rs").read_text(
        encoding="utf-8"
    )
    runtime_state_source = (
        ROOT / "runtime/molt-runtime/src/state/runtime_state.rs"
    ).read_text(encoding="utf-8")
    classifier = (
        ROOT / "runtime/molt-runtime/src/object/string_intern.rs"
    ).read_text(encoding="utf-8")
    sys_ext = (ROOT / "runtime/molt-runtime/src/builtins/sys_ext.rs").read_text(
        encoding="utf-8"
    )
    lifecycle = (
        ROOT / "runtime/molt-runtime/src/state/lifecycle.rs"
    ).read_text(encoding="utf-8")

    assert "pub(crate) struct CanonicalObjectCache" in builders
    assert "canonical_objects: CanonicalObjectCache" in runtime_state_source
    assert "alloc_interned_string(_py, s.as_bytes())" in sys_ext
    assert "clear_builder_singletons(_py, state)" in lifecycle
    for deleted in (
        "EMPTY_TUPLE_PTR",
        "EMPTY_STRING_PTR",
        "EMPTY_BYTES_PTR",
        "ASCII_CHARS",
        "molt_string_intern_pool",
        "InternedPtr",
        "leaks harmlessly",
        "intern_table",
    ):
        assert deleted not in builders + sys_ext
    assert "Box::leak" not in classifier
    clear = builders.split("pub(crate) fn clear_builder_singletons", 1)[1].split(
        "pub(crate) fn alloc_bytearray", 1
    )[0]
    assert "std::collections::HashSet" not in clear and "released.insert" not in clear
    assert "singleton_ptrs" in clear and "interned.into_values()" in clear


def test_raw_object_publication_is_explicit_on_every_backend_representation() -> None:
    builders = (ROOT / "runtime/molt-runtime/src/object/builders.rs").read_text(
        encoding="utf-8"
    )
    allocator = (ROOT / "runtime/molt-runtime/src/object/mod.rs").read_text(
        encoding="utf-8"
    )
    arena = (ROOT / "runtime/molt-runtime/src/arena.rs").read_text(encoding="utf-8")
    cranelift = (
        ROOT
        / "runtime/molt-backend-native/src/native_backend/function_compiler/fc/memory.rs"
    ).read_text(encoding="utf-8")
    llvm = (
        ROOT / "runtime/molt-backend-native/src/llvm_backend/lowering/op_dispatch.rs"
    ).read_text(encoding="utf-8")
    wasm = (
        ROOT
        / "runtime/molt-backend-wasm/src/wasm/op_loop/core_runtime_ops/allocation_ops.rs"
    ).read_text(encoding="utf-8")
    wasm_manifest = (
        ROOT / "runtime/molt-backend-wasm/src/wasm_abi_manifest.toml"
    ).read_text(encoding="utf-8")
    rust = (ROOT / "runtime/molt-backend-rust/src/rust/op_emitter.rs").read_text(
        encoding="utf-8"
    )
    luau = (ROOT / "runtime/molt-backend-luau/src/luau/op_objects.rs").read_text(
        encoding="utf-8"
    )

    assert builders.count("alloc_object_zeroed_unpublished_with_aux(") >= 2
    assert "pub extern \"C\" fn molt_object_publish_initialized" in builders
    unpublished_allocator = allocator.split(
        "fn alloc_object_zeroed_with_aux_policy", 1
    )[1].split("pub(crate) fn alloc_object(", 1)[0]
    assert unpublished_allocator.index(
        "initialize_flags_gc_unpublished(header, 0)"
    ) < unpublished_allocator.index("gc_track_if_cyclic")
    assert "gc_mark_unpublished" not in allocator
    assert "initialize_flags_gc_unpublished" in arena
    assert "gc_mark_unpublished" not in arena
    assert cranelift.count('"molt_object_publish_initialized"') >= 2
    assert 'get_function("molt_object_publish_initialized")' in llvm
    assert "WasmRuntimeImport::ObjectPublishInitialized" in wasm
    assert '"alloc" | "stack_alloc" | "alloc_class"' in wasm
    assert 'runtime_name = "molt_object_publish_initialized"' in wasm_manifest
    # Rust and Luau allocate their own fully initialized value/table
    # representations; they never observe the native unpublished pointer ABI.
    assert '"build_list" | "list_new" | "alloc" => self.emit_op_build_list(op)' in rust
    assert '"alloc" | "alloc_task" =>' in luau
