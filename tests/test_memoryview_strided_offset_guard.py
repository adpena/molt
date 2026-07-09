from __future__ import annotations

import re
from pathlib import Path


ROOT = Path(__file__).resolve().parents[1]

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
C_API_SURFACE_PATH = ROOT / "docs/spec/areas/compat/surfaces/c_api/libmolt_c_api_surface.md"
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
    match = re.search(r"pub\s+struct\s+MoltBufferView\s*\{(?P<body>.*?)\n\}", source, re.S)
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
        for lineno, line in enumerate(path.read_text(encoding="utf-8").splitlines(), start=1):
            if any(pattern.search(line) for pattern in FORBIDDEN_RAW_STRIDE_PATTERNS):
                offenders.append(f"{path.relative_to(ROOT)}:{lineno}: {line.strip()}")

    assert not offenders, (
        "memoryview offset math must use memoryview_linear_offset or "
        "memoryview_strided_offset instead of raw stride multiplication:\n"
        + "\n".join(offenders)
    )


def test_memoryview_contains_uses_strided_search_without_materializing_view() -> None:
    subscript_source = (ROOT / "runtime/molt-runtime/src/object/ops/subscript.rs").read_text(
        encoding="utf-8"
    )
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
    c_api_source = C_API_MOD_PATH.read_text(encoding="utf-8")
    c_api_symbols_source = C_API_MOLT_API_PATH.read_text(encoding="utf-8")

    assert _c_molt_buffer_fields(header_source) == MOLT_BUFFER_VIEW_FIELDS
    assert _rust_molt_buffer_fields(runtime_source) == MOLT_BUFFER_VIEW_FIELDS
    assert _rust_molt_buffer_fields(cpython_abi_source) == MOLT_BUFFER_VIEW_FIELDS
    assert _canonical_buffer_fields(
        _rust_molt_buffer_fields(http_bridge_source.replace("BufferExport", "MoltBufferView"))
    ) == MOLT_BUFFER_VIEW_FIELDS
    assert _c_define_value(header_source, "MOLT_C_API_VERSION") == 4
    assert _rust_const_value(c_api_source, "MOLT_C_API_VERSION") == 4
    assert "int32_t molt_buffer_export(MoltHandle obj_bits, MoltBufferView *out_view);" in header_source
    assert '#define molt_buffer_export ((int32_t (*)(MoltHandle, MoltBufferView *))_molt_host_abi_symbol("molt_buffer_export"))' in header_source
    assert "int32_t molt_c_heap_register(uintptr_t ptr);" in header_source
    assert "int32_t molt_c_heap_unregister(uintptr_t ptr);" in header_source
    assert "int32_t molt_c_heap_contains(uintptr_t ptr);" in header_source
    assert "uintptr_t molt_c_heap_type_canonicalize(uint32_t kind, uintptr_t ptr);" in header_source
    assert "pub extern \"C\" fn molt_c_heap_register(ptr: usize) -> i32" in c_api_symbols_source
    assert "pub extern \"C\" fn molt_c_heap_unregister(ptr: usize) -> i32" in c_api_symbols_source
    assert "pub extern \"C\" fn molt_c_heap_contains(ptr: usize) -> i32" in c_api_symbols_source
    assert "pub extern \"C\" fn molt_c_heap_type_canonicalize(kind: u32, ptr: usize) -> usize" in c_api_symbols_source


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
    assert "storage.base_bits == 0 && storage.min_offset < 0" in runtime_source
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
    assert "storage.fits_in_base_len(base_slice.len())" in from_buffer_body
    assert "data_matches_base" in from_buffer_body
    assert "base_slice.as_ptr().add(offset).cast_mut()" in from_buffer_body
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

    assert "Canonical bool: 0 writable, 1 read-only; other values are rejected." in header_source
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


def test_public_python_h_rebuilds_public_pybuffer_and_trusts_runtime_capacity_only() -> None:
    header_source = PYTHON_HEADER_PATH.read_text(encoding="utf-8")
    body = _function_body(header_source, "PyMemoryView_FromBuffer")

    assert "view = info->_molt_view" not in body
    assert "memset(&view, 0, sizeof(view))" in body
    assert "view.data = (uint8_t *)info->buf" in body
    assert "info->suboffsets != NULL" in body
    assert "indirect buffers are not supported" in body
    assert "trusted_molt_view = info->internal == &info->_molt_view" in body
    assert "ndim = info->ndim" in body
    assert "info->ndim > 0 ? info->ndim : 1" not in body
    assert "view.backing_capacity = trusted_molt_view ? info->_molt_view.backing_capacity : view.len" in body
    assert "view.offset = trusted_molt_view ? info->_molt_view.offset : 0" in body
    assert "PyBuffer_FillContiguousStrides((int)ndim, view.shape, view.strides, (int)view.itemsize, 'C')" in body
    assert "strided foreign buffer requires runtime backing capacity" in body
    assert "? info->_molt_view.base" in body
    assert ": (info->obj != NULL ? _molt_py_handle(info->obj) : 0)" in body
    assert "view.base = info->obj != NULL ? _molt_py_handle(info->obj) : 0" not in body


def test_public_and_compiled_headers_expose_cpython_memoryview_flags() -> None:
    header_source = PYTHON_HEADER_PATH.read_text(encoding="utf-8")
    abi_source = CPYTHON_ABI_TYPES_PATH.read_text(encoding="utf-8")

    expected_public = [
        "#define PyBUF_READ 0x0100",
        "#define PyBUF_WRITE 0x0200",
        "#define PyBUF_INDIRECT (0x0100 | PyBUF_STRIDES)",
        "#define PyBUF_CONTIG_RO PyBUF_ND",
        "#define PyBUF_CONTIG (PyBUF_ND | PyBUF_WRITABLE)",
        "#define PyBUF_RECORDS_RO (PyBUF_STRIDES | PyBUF_FORMAT)",
        "#define PyBUF_RECORDS (PyBUF_STRIDES | PyBUF_FORMAT | PyBUF_WRITABLE)",
        "#define PyBUF_FULL_RO (PyBUF_INDIRECT | PyBUF_FORMAT)",
        "#define PyBUF_FULL (PyBUF_INDIRECT | PyBUF_FORMAT | PyBUF_WRITABLE)",
    ]
    for expected in expected_public:
        assert expected in header_source
        assert header_source.count(expected) == 2

    expected_compiled = [
        "pub const PyBUF_READ: c_int = 0x0100;",
        "pub const PyBUF_WRITE: c_int = 0x0200;",
        "pub const PyBUF_INDIRECT: c_int = 0x0100 | PyBUF_STRIDES;",
        "pub const PyBUF_CONTIG_RO: c_int = PyBUF_ND;",
        "pub const PyBUF_CONTIG: c_int = PyBUF_ND | PyBUF_WRITABLE;",
        "pub const PyBUF_RECORDS_RO: c_int = PyBUF_STRIDES | PyBUF_FORMAT;",
        "pub const PyBUF_RECORDS: c_int = PyBUF_STRIDES | PyBUF_FORMAT | PyBUF_WRITABLE;",
        "pub const PyBUF_FULL_RO: c_int = PyBUF_INDIRECT | PyBUF_FORMAT;",
        "pub const PyBUF_FULL: c_int = PyBUF_INDIRECT | PyBUF_FORMAT | PyBUF_WRITABLE;",
    ]
    for expected in expected_compiled:
        assert expected in abi_source

    from_memory_body = _function_body(header_source, "PyMemoryView_FromMemory")
    assert "view.backing_capacity = (uint64_t)size" in from_memory_body
    assert "flags & PyBUF_WRITE" in from_memory_body


def test_compiled_abi_rejects_indirect_pybuffer_topology() -> None:
    abi_buffer_source = CPYTHON_ABI_BUFFER_PATH.read_text(encoding="utf-8")
    descriptor_body = _rust_function_body(abi_buffer_source, "descriptor_from_pybuffer")

    assert "!info.suboffsets.is_null()" in descriptor_body
    assert "return Err(())" in descriptor_body
    assert "is_registered_buffer_internal(info.internal)" in descriptor_body
    assert "(*info.internal.cast::<BufferInternal>()).descriptor" in descriptor_body
    assert "descriptor.backing_capacity = info.len as u64" in descriptor_body
    assert descriptor_body.find("(*info.internal.cast::<BufferInternal>()).descriptor") < descriptor_body.find(
        "descriptor.backing_capacity = info.len as u64"
    )
    assert "let ndim = info.ndim as usize" in descriptor_body
    assert "info.ndim == 0" not in descriptor_body
    assert "!pybuffer_is_c_contiguous" in descriptor_body

    assert "BUFFER_INTERNAL_REGISTRY" in abi_buffer_source
    assert "register_buffer_internal(internal_ptr)" in abi_buffer_source
    assert "unregister_buffer_internal((*view).internal)" in abi_buffer_source

    getbuffer_body = _rust_function_body(abi_buffer_source, "PyObject_GetBuffer")
    assert "let mut descriptor = MoltBufferView::default()" in getbuffer_body
    assert "Box::new(MoltBufferView::default())" not in getbuffer_body
    assert "BufferInternal::runtime(descriptor)" in getbuffer_body


def test_buffer_support_probe_does_not_require_simple_contiguity() -> None:
    header_source = PYTHON_HEADER_PATH.read_text(encoding="utf-8")
    abi_buffer_source = CPYTHON_ABI_BUFFER_PATH.read_text(encoding="utf-8")
    public_body = _function_body(header_source, "PyObject_CheckBuffer")
    compiled_body = _rust_function_body(abi_buffer_source, "PyObject_CheckBuffer")

    assert "MoltBufferView tmp" in public_body
    assert "molt_buffer_acquire(_molt_py_handle(obj), &tmp)" in public_body
    assert "_molt_pybuffer_release_exported_view(obj, &tmp)" in public_body
    assert "PyBUF_SIMPLE" not in public_body
    assert "PyObject_GetBuffer" not in public_body

    assert "let mut descriptor = MoltBufferView::default()" in compiled_body
    assert "(hooks.buffer_acquire)(bits, &mut descriptor as *mut MoltBufferView)" in compiled_body
    assert "(hooks.buffer_release)(&mut descriptor as *mut MoltBufferView)" in compiled_body
    assert "PyBUF_SIMPLE" not in compiled_body
    assert "PyObject_GetBuffer" not in compiled_body


def test_public_pybuffer_routes_c_heap_objects_through_lease_authority() -> None:
    # Derived from PR #31 "Guard C-heap buffer route authority": pin the public
    # source-recompiled PyObject_GetBuffer / PyObject_CheckBuffer / PyBuffer_Release
    # so C-heap ndarray-style objects flow through molt_c_heap_export_buffer /
    # molt_c_heap_release_buffer while runtime objects keep going through
    # molt_buffer_acquire / molt_buffer_release. Without this route split a stale
    # or forged C-heap object would be treated as a runtime handle.
    header_source = PYTHON_HEADER_PATH.read_text(encoding="utf-8")
    public_check_body = _function_body(header_source, "PyObject_CheckBuffer")
    public_getbuffer_body = _function_body(header_source, "PyObject_GetBuffer")
    public_release_body = _function_body(header_source, "PyBuffer_Release")
    public_release_exported_body = _function_body(
        header_source, "_molt_pybuffer_release_exported_view"
    )

    assert "molt_c_heap_export_buffer((uintptr_t)obj, &tmp)" in public_check_body
    assert "molt_buffer_acquire(_molt_py_handle(obj), &tmp)" in public_check_body
    assert (
        "molt_c_heap_export_buffer((uintptr_t)obj, &view->_molt_view)"
        in public_getbuffer_body
    )
    assert (
        "molt_buffer_acquire(_molt_py_handle(obj), &view->_molt_view)"
        in public_getbuffer_body
    )
    assert "view->internal == &view->_molt_view" in public_release_body
    assert (
        "_molt_pybuffer_release_exported_view(view->obj, &view->_molt_view)"
        in public_release_body
    )
    assert "molt_buffer_release(view)" in public_release_exported_body
    assert "molt_c_heap_release_buffer((uintptr_t)obj, view)" in public_release_exported_body


def test_noncontiguous_buffers_require_stride_metadata() -> None:
    header_source = PYTHON_HEADER_PATH.read_text(encoding="utf-8")
    abi_buffer_source = CPYTHON_ABI_BUFFER_PATH.read_text(encoding="utf-8")

    public_descriptor_body = _function_body(header_source, "_molt_pybuffer_descriptor_satisfies_flags")
    public_getbuffer_body = _function_body(header_source, "PyObject_GetBuffer")
    compiled_descriptor_body = _rust_function_body(abi_buffer_source, "descriptor_satisfies_flags")
    compiled_install_body = _rust_function_body(abi_buffer_source, "install_buffer_internal")

    assert "(flags & PyBUF_STRIDES) == 0" in public_descriptor_body
    assert "!_molt_buffer_view_is_c_contiguous(view)" in public_descriptor_body
    assert "_molt_pybuffer_descriptor_satisfies_flags(&view->_molt_view, flags)" in public_getbuffer_body
    assert "non-contiguous buffers require PyBUF_STRIDES" in public_getbuffer_body

    assert "(flags & PyBUF_STRIDES) == 0" in compiled_descriptor_body
    assert "!descriptor_is_c_contiguous(descriptor)" in compiled_descriptor_body
    assert "descriptor_satisfies_flags(&internal.descriptor, flags)" in compiled_install_body
    assert "non-contiguous buffers require PyBUF_STRIDES" in compiled_install_body


def test_buffer_format_metadata_fails_closed_instead_of_truncating() -> None:
    header_source = PYTHON_HEADER_PATH.read_text(encoding="utf-8")
    abi_buffer_source = CPYTHON_ABI_BUFFER_PATH.read_text(encoding="utf-8")

    public_memoryview_body = _function_body(header_source, "PyMemoryView_FromBuffer")
    compiled_descriptor_body = _rust_function_body(abi_buffer_source, "descriptor_from_pybuffer")

    assert "size_t cap = MOLT_BUFFER_FORMAT_CAP - 1u" in public_memoryview_body
    assert "n > cap" in public_memoryview_body
    assert "buffer format exceeds Molt ABI capacity" in public_memoryview_body
    assert "n = cap" not in public_memoryview_body

    assert "bytes.len() >= MOLT_BUFFER_FORMAT_CAP" in compiled_descriptor_body
    assert "copy_len = bytes.len().min" in compiled_descriptor_body


def test_public_python_h_memoryview_get_buffer_has_per_object_cache() -> None:
    header_source = PYTHON_HEADER_PATH.read_text(encoding="utf-8")
    get_buffer_body = _function_body(header_source, "PyMemoryView_GET_BUFFER")
    cache_body = _function_body(header_source, "_molt_memoryview_export_cache_get")
    refresh_body = _function_body(header_source, "_molt_memoryview_export_refresh")

    assert "_molt_memoryview_export_slot(" not in header_source
    assert "typedef struct _molt_memoryview_export_slot" in header_source
    assert "#define _MOLT_MEMORYVIEW_EXPORT_SLOT_COUNT 64u" in header_source
    assert "static inline _MoltMemoryViewExportSlot *_molt_memoryview_export_slots" in header_source
    assert "static inline size_t *_molt_memoryview_export_next_slot" in header_source
    assert "slots[i].mview == mview" in cache_body
    assert "PyMem_Calloc" not in cache_body
    assert "PyObject_GetBuffer" not in cache_body
    assert "molt_buffer_export(_molt_py_handle(slot->mview), &slot->view._molt_view)" in refresh_body
    assert "slot->view.internal = &slot->view._molt_view" in refresh_body
    assert "slot->view.obj = NULL" in refresh_body
    assert "PyBuffer_Release" not in get_buffer_body
    assert "return _molt_memoryview_export_cache_get(mview)" in get_buffer_body


def test_public_and_compiled_contiguity_reject_one_dimensional_gaps() -> None:
    header_source = PYTHON_HEADER_PATH.read_text(encoding="utf-8")
    abi_source = CPYTHON_ABI_BUFFER_PATH.read_text(encoding="utf-8")

    public_c = _function_body(header_source, "_molt_pybuffer_is_c_contiguous")
    public_f = _function_body(header_source, "_molt_pybuffer_is_f_contiguous")
    assert "view->ndim == 0" in public_c
    assert "view->ndim <= 1" not in public_c
    assert "view->ndim == 0" in public_f
    assert "view->ndim <= 1" not in public_f
    assert "expected *=" not in public_c
    assert "expected *=" not in public_f

    compiled_c = _rust_function_body(abi_source, "pybuffer_is_c_contiguous")
    compiled_f = _rust_function_body(abi_source, "pybuffer_is_f_contiguous")
    compiled_c_flat = compiled_c.replace("\n", " ")
    compiled_f_flat = compiled_f.replace("\n", " ")
    assert "(*view).ndim } == 0" in compiled_c or (
        "(*view).ndim } == 0" in compiled_c_flat
    )
    assert "(*view).ndim } <= 1" not in compiled_c_flat
    assert "(*view).ndim } == 0" in compiled_f or (
        "(*view).ndim } == 0" in compiled_f_flat
    )
    assert "(*view).ndim } <= 1" not in compiled_f_flat


def test_numpy_header_overlay_is_not_memoryview_authority() -> None:
    assert not (ROOT / "include" / "numpy").exists()
    assert not (ROOT / "include" / "_numpyconfig.h").exists()
