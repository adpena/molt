from __future__ import annotations

import re
import runpy
from pathlib import Path

from molt.rust_source_scan import (
    mask_rust_comments_and_strings,
    mask_rust_test_items,
    rust_block_region,
    rust_delimiter_end,
    rust_token_range,
)


ROOT = Path(__file__).resolve().parents[1]


def test_cpython_buffer_export_lifetime_capsule() -> None:
    runpy.run_path(
        str(ROOT / "tests/differential/basic/memoryview_array_export_resize.py")
    )


MEMORYVIEW_OFFSET_FILES = [
    ROOT / "runtime/molt-runtime/src/object/ops/subscript.rs",
    ROOT / "runtime/molt-runtime/src/object/ops_memoryview.rs",
    ROOT / "runtime/molt-runtime/src/object/memoryview.rs",
]
MOLT_HEADER_PATH = ROOT / "include/molt/molt.h"
RUNTIME_MEMORYVIEW_PATH = ROOT / "runtime/molt-runtime/src/object/memoryview.rs"
RUNTIME_BUILDERS_PATH = ROOT / "runtime/molt-runtime/src/object/builders.rs"
RUNTIME_OPS_ITER_PATH = ROOT / "runtime/molt-runtime/src/object/ops_iter.rs"
C_API_MOLT_API_PATH = ROOT / "runtime/molt-runtime/src/c_api/molt_api.rs"
C_API_SURFACE_PATH = (
    ROOT / "docs/spec/areas/compat/surfaces/c_api/libmolt_c_api_surface.md"
)
CPYTHON_ABI_HOOKS_PATH = ROOT / "runtime/molt-cpython-abi/src/hooks.rs"
HTTP_BRIDGE_PATH = ROOT / "runtime/molt-runtime-http/src/bridge.rs"

MOLT_BUFFER_VIEW_FIELDS = [
    "data",
    "len",
    "backing_capacity",
    "readonly",
    "ndim",
    "itemsize",
    "offset",
    "owner",
    "base",
    "shape",
    "strides",
    "format",
]

FORBIDDEN_RAW_STRIDE_PATTERNS = [
    re.compile(r"\bas\s+isize\)\s*\*\s*strides?\b"),
    re.compile(r"\bstrides?\[[^\]]+\]\s*\*"),
    re.compile(r"\*\s*strides?\[[^\]]+\]"),
    re.compile(r"\.saturating_mul\(\s*\*?strides?\b"),
]


def _rust_block_body(source: str, header: str, *, depth: int | None = None) -> str:
    region = rust_block_region(source, header, depth=depth)
    assert region is not None, f"{header} is missing, ambiguous or unbalanced"
    return mask_rust_comments_and_strings(source[slice(*region)])


def _rust_function_body(source: str, name: str, *, owner: str | None = None) -> str:
    # Select one production declaration in its owning scope. Comments, literals
    # and test-only stand-ins cannot satisfy these source contracts.
    code = mask_rust_comments_and_strings(mask_rust_test_items(source))
    if owner is not None:
        code = _rust_block_body(code, owner, depth=0)
    declaration = rust_token_range(code, f"fn {name}", depth=0)
    assert declaration is not None, f"{name} is missing or ambiguous in {owner}"
    opening = code.find("{", declaration[1])
    assert opening >= 0 and ";" not in code[declaration[1] : opening], (
        f"{name} has no function body"
    )
    end = rust_delimiter_end(code, opening)
    assert end is not None, f"{name} body is unbalanced"
    return code[opening + 1 : end - 1]


def _c_molt_buffer_fields(source: str) -> list[str]:
    match = re.search(
        r"typedef\s+struct\s+MoltBufferView\s*\{(?P<body>.*?)\}\s*MoltBufferView;",
        source,
        re.S,
    )
    assert match is not None, "C MoltBufferView typedef is missing"
    fields: list[str] = []
    for raw_line in match.group("body").splitlines():
        line = raw_line.strip()
        if not line or line.startswith("/*"):
            continue
        field = re.sub(r"\[[^\]]+\]", "", line.rstrip(";")).split()[-1].lstrip("*")
        fields.append(field)
    return fields


def _rust_molt_buffer_fields(source: str, name: str = "MoltBufferView") -> list[str]:
    body = _rust_block_body(mask_rust_test_items(source), f"pub struct {name}", depth=0)
    fields: list[str] = []
    for raw_line in body.splitlines():
        line = raw_line.strip()
        if line.startswith("pub "):
            fields.append(line.removeprefix("pub ").split(":", 1)[0].strip())
    return fields


def _canonical_buffer_fields(fields: list[str]) -> list[str]:
    return ["data" if field == "ptr" else field for field in fields]


def _c_define_value(source: str, name: str) -> int:
    match = re.search(rf"^\s*#define\s+{re.escape(name)}\s+([0-9]+)u?\b", source, re.M)
    assert match is not None, f"{name} is missing from C header"
    return int(match.group(1))


def test_memoryview_offsets_use_checked_stride_primitives() -> None:
    offenders: list[str] = []
    for path in MEMORYVIEW_OFFSET_FILES:
        code = mask_rust_comments_and_strings(
            mask_rust_test_items(path.read_text(encoding="utf-8"))
        )
        for lineno, line in enumerate(code.splitlines(), start=1):
            if any(pattern.search(line) for pattern in FORBIDDEN_RAW_STRIDE_PATTERNS):
                offenders.append(f"{path.relative_to(ROOT)}:{lineno}: {line.strip()}")

    assert not offenders, (
        "memoryview offset math must use memoryview_linear_offset or "
        "memoryview_strided_offset instead of raw stride multiplication:\n"
        + "\n".join(offenders)
    )


def test_memoryview_contains_streams_shared_iterator_and_scalar_access() -> None:
    subscript_source = (
        ROOT / "runtime/molt-runtime/src/object/ops/subscript.rs"
    ).read_text(encoding="utf-8")
    iter_source = RUNTIME_OPS_ITER_PATH.read_text(encoding="utf-8")
    contains_body = _rust_function_body(subscript_source, "contains_impl")
    search_body = _rust_function_body(subscript_source, "iterable_contains")

    for entry, builtin in (
        ("molt_contains", "false"),
        ("molt_contains_builtin", "true"),
    ):
        assert f"contains_impl(container_bits, item_bits, {builtin})" in (
            _rust_function_body(subscript_source, entry)
        )
    assert "TYPE_ID_MEMORYVIEW" not in contains_body
    assert "return iterable_contains(_py, container_bits, item_bits)" in contains_body
    assert "memoryview_strided_contains_" not in mask_rust_comments_and_strings(
        subscript_source
    )
    assert "OwnedIterator::new(_py, container_bits)" in search_body
    assert "match iter.next()" in search_body
    assert "compare_object_eq_bool(_py, obj_from_bits(value), item)" in search_body
    step_body = _rust_block_body(
        _rust_function_body(iter_source, "molt_iter_next"),
        "if target_type == TYPE_ID_MEMORYVIEW",
    )
    assert "molt_getitem_builtin(" in step_body
    for body in (contains_body, search_body, step_body):
        for materialization in ("Vec::", ".collect(", ".to_vec(", "memoryview_tobytes"):
            assert materialization not in body


def test_molt_buffer_view_v2_layout_is_mirrored() -> None:
    header_source = MOLT_HEADER_PATH.read_text(encoding="utf-8")
    runtime_source = RUNTIME_MEMORYVIEW_PATH.read_text(encoding="utf-8")
    cpython_abi_source = CPYTHON_ABI_HOOKS_PATH.read_text(encoding="utf-8")
    http_bridge_source = HTTP_BRIDGE_PATH.read_text(encoding="utf-8")
    c_api_symbols_source = C_API_MOLT_API_PATH.read_text(encoding="utf-8")

    assert _c_molt_buffer_fields(header_source) == MOLT_BUFFER_VIEW_FIELDS
    assert _rust_molt_buffer_fields(runtime_source) == MOLT_BUFFER_VIEW_FIELDS
    assert _rust_molt_buffer_fields(cpython_abi_source) == MOLT_BUFFER_VIEW_FIELDS
    assert (
        _canonical_buffer_fields(
            _rust_molt_buffer_fields(http_bridge_source, "BufferExport")
        )
        == MOLT_BUFFER_VIEW_FIELDS
    )
    assert _c_define_value(header_source, "MOLT_C_API_VERSION") == 5
    assert (
        "int32_t molt_buffer_export(MoltHandle obj_bits, MoltBufferView *out_view);"
        in header_source
    )
    assert (
        '#define molt_buffer_export ((int32_t (*)(MoltHandle, MoltBufferView *))_molt_host_abi_symbol("molt_buffer_export"))'
        in header_source
    )
    assert "int32_t molt_c_heap_register(uintptr_t ptr);" in header_source
    assert "int32_t molt_c_heap_unregister(uintptr_t ptr);" in header_source
    assert "int32_t molt_c_heap_contains(uintptr_t ptr);" in header_source
    assert (
        "uintptr_t molt_c_heap_type_canonicalize(uint32_t kind, uintptr_t ptr);"
        in header_source
    )
    assert (
        'pub extern "C" fn molt_c_heap_register(ptr: usize) -> i32'
        in c_api_symbols_source
    )
    assert (
        'pub extern "C" fn molt_c_heap_unregister(ptr: usize) -> i32'
        in c_api_symbols_source
    )
    assert (
        'pub extern "C" fn molt_c_heap_contains(ptr: usize) -> i32'
        in c_api_symbols_source
    )
    assert (
        'pub extern "C" fn molt_c_heap_type_canonicalize(kind: u32, ptr: usize) -> usize'
        in c_api_symbols_source
    )


def test_molt_buffer_backing_capacity_is_runtime_admission_authority() -> None:
    runtime_source = RUNTIME_MEMORYVIEW_PATH.read_text(encoding="utf-8")
    builders_source = RUNTIME_BUILDERS_PATH.read_text(encoding="utf-8")
    c_api_source = C_API_MOLT_API_PATH.read_text(encoding="utf-8")

    storage_body = _rust_block_body(
        mask_rust_test_items(runtime_source),
        "pub(crate) struct TypedStridedStorage",
        depth=0,
    )
    assert "pub(crate) span_len: usize" in storage_body
    assert "pub(crate) min_offset: isize" in storage_body
    assert "pub(crate) max_end_offset: isize" in storage_body
    assert "fn memoryview_strided_span_len" not in mask_rust_comments_and_strings(
        runtime_source
    )
    storage_new = _rust_function_body(
        runtime_source, "new", owner="impl TypedStridedStorage"
    )
    assert (
        "memoryview_strided_bounds(shape.as_slice(), strides.as_slice(), itemsize)?"
        in (storage_new)
    )
    assert "span_len: bounds.span_len" in storage_new
    assert "self.fits_in_base_len(base_slice.len())" in _rust_function_body(
        runtime_source, "backing_capacity_len", owner="impl TypedStridedStorage"
    )
    span_body = _rust_function_body(runtime_source, "memoryview_strided_bounds")
    assert "stride < 0" not in span_body
    assert "min_offset" in span_body
    assert "max_end_offset" in span_body

    wrapper_body = _rust_function_body(builders_source, "alloc_memoryview_from_storage")
    assert "match PinnedMemoryViewStorage::new(py, storage)" in wrapper_body
    assert "Ok(pinned) => pinned.allocate()" in wrapper_body
    assert "alloc_object(" not in wrapper_body
    owner = "impl<'a, 'py> PinnedMemoryViewStorage<'a, 'py>"
    pin_body = _rust_function_body(builders_source, "new", owner=owner)
    assert "ScopedBufferExport::new(py, storage.owner_bits)?" in pin_body
    alloc_body = _rust_function_body(builders_source, "allocate", owner=owner)
    # The existing runtime null/span/capacity cases own behavior. These guards
    # keep backing admission in the pinned allocation authority after the move.
    assert "storage.base_bits == 0 && storage.span_len == 0" in alloc_body
    assert "std::ptr::NonNull::<u8>::dangling().as_ptr()" in alloc_body
    assert "storage.fits_in_base_len(base_slice.len())" in alloc_body

    from_buffer_body = _rust_function_body(c_api_source, "molt_memoryview_from_buffer")
    assert "usize::try_from(view.backing_capacity)" in from_buffer_body
    source_body = _rust_block_body(from_buffer_body, "if source_owner != 0")
    assert "TypedStridedStorage::from_object_bits(source_owner)" in source_body
    assert "array_storage_from_object_bits(" in source_body
    assert "storage.fits_in_backing_len(backing_capacity)" in from_buffer_body
    admission_body = _rust_block_body(
        from_buffer_body, "if logical_len == view.len && valid"
    )
    assert "alloc_memoryview_from_storage(_py, storage)" in admission_body
    assert "storage.fits_in_backing_len(backing_len)" not in from_buffer_body


def test_c_heap_buffer_export_admission_uses_memoryview_format_authority() -> None:
    # Derived from PR #44 "Validate C-heap buffer formats" + "Reject noncanonical
    # C buffer readonly flags": C-heap lease admission must route the PEP 3118
    # format through the shared memoryview_format_from_str authority (rejecting
    # unsupported codes and itemsize disagreement) and decode the readonly flag
    # through the canonical 0/1 decoder rather than the lossy `readonly != 0`.
    c_api_source = C_API_MOLT_API_PATH.read_text(encoding="utf-8")

    view_body = _rust_function_body(c_api_source, "c_heap_buffer_view_is_valid")
    format_body = _rust_function_body(c_api_source, "c_heap_buffer_format_is_valid")
    readonly_body = _rust_function_body(c_api_source, "buffer_readonly_from_flag")
    from_buffer_body = _rust_function_body(c_api_source, "molt_memoryview_from_buffer")

    assert "c_heap_buffer_format_is_valid(view, itemsize)" in view_body
    assert "view.format.iter().position" in format_body
    assert "std::str::from_utf8" in format_body
    assert "memoryview_format_from_str(format)" in format_body
    assert "format.itemsize == itemsize" in format_body
    assert "default_buffer_format" not in format_body
    assert "buffer_readonly_from_flag(view.readonly)" in view_body
    assert "buffer_readonly_from_flag(view.readonly)" in from_buffer_body
    assert "view.readonly != 0" not in view_body
    assert "view.readonly != 0" not in from_buffer_body
    assert "0 => Some(false)" in readonly_body
    assert "1 => Some(true)" in readonly_body
    assert "_ => None" in readonly_body


def test_molt_buffer_view_readonly_contract_is_canonical() -> None:
    # Derived from PR #44 "Document canonical C buffer readonly domain": the 0/1
    # readonly ABI domain must be documented at the public header, the Rust
    # descriptor, and the libmolt C-API surface so exporters and importers agree.
    header_source = MOLT_HEADER_PATH.read_text(encoding="utf-8")
    runtime_source = RUNTIME_MEMORYVIEW_PATH.read_text(encoding="utf-8")
    surface_source = C_API_SURFACE_PATH.read_text(encoding="utf-8")

    assert (
        "Canonical bool: 0 writable, 1 read-only; other values are rejected."
        in header_source
    )
    assert (
        "Canonical bool exported as 0/1; importers reject every other value."
        in runtime_source
    )
    normalized_surface = re.sub(r"\s+", " ", surface_source)
    assert (
        "`readonly` is a canonical u32 boolean: `0` means writable, `1` means "
        "read-only, and every other value fails descriptor admission."
        in normalized_surface
    )


def test_numpy_header_overlay_is_not_memoryview_authority() -> None:
    assert not (ROOT / "include" / "numpy").exists()
    assert not (ROOT / "include" / "_numpyconfig.h").exists()
