from __future__ import annotations

import re
import runpy
from pathlib import Path


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
PYTHON_HEADER_PATH = ROOT / "include/molt/Python.h"
RUNTIME_MEMORYVIEW_PATH = ROOT / "runtime/molt-runtime/src/object/memoryview.rs"
RUNTIME_BUILDERS_PATH = ROOT / "runtime/molt-runtime/src/object/builders.rs"
C_API_MOLT_API_PATH = ROOT / "runtime/molt-runtime/src/c_api/molt_api.rs"
C_API_MOD_PATH = ROOT / "runtime/molt-runtime/src/c_api/mod.rs"
C_API_SURFACE_PATH = (
    ROOT / "docs/spec/areas/compat/surfaces/c_api/libmolt_c_api_surface.md"
)
CPYTHON_ABI_HOOKS_PATH = ROOT / "runtime/molt-cpython-abi/src/hooks.rs"
CPYTHON_ABI_TYPES_PATH = ROOT / "runtime/molt-cpython-abi/src/abi_types.rs"
CPYTHON_ABI_BUFFER_PATH = ROOT / "runtime/molt-cpython-abi/src/api/buffer.rs"
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


def _function_body(source: str, name: str) -> str:
    match = re.search(rf"\b{name}\s*\([^)]*\)\s*\{{", source)
    assert match is not None, f"{name} is missing"
    depth = 1
    pos = match.end()
    while pos < len(source) and depth:
        char = source[pos]
        if char == "{":
            depth += 1
        elif char == "}":
            depth -= 1
        pos += 1
    assert depth == 0, f"{name} body is unbalanced"
    return source[match.end() : pos - 1]


def _rust_function_body(source: str, name: str) -> str:
    match = re.search(rf"\bfn\s+{re.escape(name)}\b[^\{{]*\{{", source)
    assert match is not None, f"{name} is missing"
    depth = 1
    pos = match.end()
    while pos < len(source) and depth:
        char = source[pos]
        if char == "{":
            depth += 1
        elif char == "}":
            depth -= 1
        pos += 1
    assert depth == 0, f"{name} body is unbalanced"
    return source[match.end() : pos - 1]


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


def _rust_molt_buffer_fields(source: str) -> list[str]:
    match = re.search(
        r"pub\s+struct\s+MoltBufferView\s*\{(?P<body>.*?)\n\}", source, re.S
    )
    assert match is not None, "Rust MoltBufferView struct is missing"
    fields: list[str] = []
    for raw_line in match.group("body").splitlines():
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


def _rust_const_value(source: str, name: str) -> int:
    match = re.search(
        rf"^\s*pub(?:\(crate\))?\s+const\s+{re.escape(name)}\s*:\s*\w+\s*=\s*([0-9]+)\s*;",
        source,
        re.M,
    )
    assert match is not None, f"{name} is missing from Rust source"
    return int(match.group(1))


def test_memoryview_offsets_use_checked_stride_primitives() -> None:
    offenders: list[str] = []
    for path in MEMORYVIEW_OFFSET_FILES:
        for lineno, line in enumerate(
            path.read_text(encoding="utf-8").splitlines(), start=1
        ):
            if any(pattern.search(line) for pattern in FORBIDDEN_RAW_STRIDE_PATTERNS):
                offenders.append(f"{path.relative_to(ROOT)}:{lineno}: {line.strip()}")

    assert not offenders, (
        "memoryview offset math must use memoryview_linear_offset or "
        "memoryview_strided_offset instead of raw stride multiplication:\n"
        + "\n".join(offenders)
    )


def test_memoryview_contains_uses_strided_search_without_materializing_view() -> None:
    subscript_source = (
        ROOT / "runtime/molt-runtime/src/object/ops/subscript.rs"
    ).read_text(encoding="utf-8")
    contains_body = _rust_function_body(subscript_source, "molt_contains")

    assert "unsafe fn memoryview_strided_contains_byte" in subscript_source
    assert "unsafe fn memoryview_strided_contains_bytes" in subscript_source
    assert "memoryview_strided_contains_byte(" in contains_body
    assert "memoryview_strided_contains_bytes(" in contains_body
    assert "Vec::with_capacity(len)" not in contains_body


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
            _rust_molt_buffer_fields(
                http_bridge_source.replace("BufferExport", "MoltBufferView")
            )
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

    assert "pub(crate) span_len: usize" in runtime_source
    assert "pub(crate) min_offset: isize" in runtime_source
    assert "pub(crate) max_end_offset: isize" in runtime_source
    assert "pub(crate) fn memoryview_strided_bounds" in runtime_source
    assert "fn memoryview_strided_span_len" not in runtime_source
    assert "span_len: bounds.span_len" in runtime_source
    assert "backing_capacity_len" in runtime_source
    span_body = _rust_function_body(runtime_source, "memoryview_strided_bounds")
    assert "stride < 0" not in span_body
    assert "min_offset" in span_body
    assert "max_end_offset" in span_body

    alloc_body = _rust_function_body(builders_source, "alloc_memoryview_from_storage")
    assert "storage.span_len" in alloc_body
    assert "storage.fits_in_base_len(base_slice.len())" in alloc_body
    assert "std::ptr::NonNull::<u8>::dangling().as_ptr()" in alloc_body

    from_buffer_body = _rust_function_body(c_api_source, "molt_memoryview_from_buffer")
    assert "view.backing_capacity" in from_buffer_body
    assert "storage.fits_in_backing_len(backing_capacity)" in from_buffer_body
    assert "storage.fits_in_base_len(base_len)" in from_buffer_body
    assert "array_storage_from_object_bits" in from_buffer_body
    assert "data_matches_base" in from_buffer_body
    assert "base_data.add(offset).cast_mut()" in from_buffer_body
    assert "== view.data" in from_buffer_body
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
