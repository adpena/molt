from __future__ import annotations

import re
import shutil
import subprocess
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
CPYTHON_ABI_HOOKS_PATH = ROOT / "runtime/molt-cpython-abi/src/hooks.rs"
CPYTHON_ABI_TYPES_PATH = ROOT / "runtime/molt-cpython-abi/src/abi_types.rs"
CPYTHON_ABI_BUFFER_PATH = ROOT / "runtime/molt-cpython-abi/src/api/buffer.rs"
HTTP_BRIDGE_PATH = ROOT / "runtime/molt-runtime-http/src/bridge.rs"
NUMPY_HEADER_PATH = ROOT / "include/numpy/ndarrayobject.h"
NUMPY_UFUNC_HEADER_PATH = ROOT / "include/numpy/ufuncobject.h"

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
    assert _c_define_value(header_source, "MOLT_C_API_VERSION") == 3
    assert _rust_const_value(c_api_source, "MOLT_C_API_VERSION") == 3
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


def test_numpy_newshape_does_not_widen_same_pointer_metadata() -> None:
    source = NUMPY_HEADER_PATH.read_text(encoding="utf-8")
    newshape_body = _function_body(source, "PyArray_Newshape")

    assert "current_elements" in newshape_body
    assert "new_elements" in newshape_body
    assert "PyArray_NDIM(self)" in newshape_body
    assert "newdims->ptr" in newshape_body
    assert "current_elements != new_elements" in newshape_body
    assert "cannot reshape array to a different element count" in newshape_body
    assert "PyArray_IS_F_CONTIGUOUS(self)" in newshape_body
    assert "PyArray_IS_C_CONTIGUOUS(self)" in newshape_body
    assert "cannot reshape non-contiguous array without copy" in newshape_body


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
    assert "(*view).ndim } == 0" in compiled_c or "(*view).ndim } == 0" in compiled_c.replace("\n", " ")
    assert "(*view).ndim } <= 1" not in compiled_c.replace("\n", " ")
    assert "(*view).ndim } == 0" in compiled_f or "(*view).ndim } == 0" in compiled_f.replace("\n", " ")
    assert "(*view).ndim } <= 1" not in compiled_f.replace("\n", " ")


def test_numpy_ndarray_span_stride_and_alignment_use_shared_checked_authority() -> None:
    source = NUMPY_HEADER_PATH.read_text(encoding="utf-8")
    python_header_source = PYTHON_HEADER_PATH.read_text(encoding="utf-8")

    assert "#define PyArray_NBYTES(arr) _molt_pyarray_nbytes((PyArrayObject *)(arr))" in source
    assert "static inline int _molt_numpy_checked_nbytes_from_dims" in source
    assert "static inline int _molt_numpy_flat_index_offset" in source
    assert "static inline int _molt_numpy_array_is_aligned" in source
    assert "static inline char _molt_numpy_pep3118_format_code" in source
    assert "static inline int _molt_numpy_size_mul_overflow" in source
    assert "_MOLT_C_HEAP_TAG" not in python_header_source
    assert "_MOLT_C_HEAP_PAYLOAD_MASK" not in python_header_source
    assert "molt_c_heap_contains((uintptr_t)obj)" in python_header_source
    assert "molt_c_heap_register((uintptr_t)header)" in python_header_source
    assert "molt_c_heap_unregister((uintptr_t)header)" in python_header_source
    assert "molt_c_heap_type_canonicalize(kind, (uintptr_t)header)" in python_header_source
    assert "return header->type == type;" in python_header_source
    assert "header->type == (PyTypeObject *)obj" in python_header_source
    assert "static inline int _molt_c_heap_object_is_type_object" in python_header_source
    assert "_molt_c_heap_object_is_type_object(obj)" in python_header_source
    assert "header->kind == type_header->kind" not in python_header_source
    assert "#define PyArray_Type (*_molt_numpy_array_type())" in source
    assert "#define PyArrayDescr_Type (*_molt_numpy_descr_type())" in source
    assert '_molt_numpy_builtin_type_borrowed("object")' not in source
    assert "static inline PyTypeObject *_molt_numpy_public_c_heap_type" in source
    assert "static PyTypeObject *canonical = NULL" in source
    assert "_MOLT_NUMPY_C_HEAP_ARRAY_TYPE" in source
    assert "_MOLT_NUMPY_C_HEAP_DESCR_TYPE" in source
    assert "_MOLT_NUMPY_C_HEAP_DTYPE_META_TYPE" in source
    assert "_MOLT_NUMPY_C_HEAP_ITER_TYPE" in source
    assert "_MOLT_NUMPY_C_HEAP_NEIGHBORHOOD_ITER_TYPE" in source
    assert "_MOLT_NUMPY_C_HEAP_UFUNC_TYPE" in source
    assert "_molt_c_heap_static_type_init(type_obj, type_kind)" in source
    assert "molt_c_heap_type_canonicalize(\n                type_kind," in source
    assert "molt_cpython_abi_type_canonicalize(type_kind, type_obj)" in source
    assert "PyTypeObject **canonical_out" in source
    assert "_molt_numpy_abi_local_type(\n        &type_obj,\n        &canonical,\n        _MOLT_NUMPY_C_HEAP_ARRAY_TYPE" in source
    assert "_molt_numpy_abi_local_type(\n        &type_obj,\n        &canonical,\n        _MOLT_NUMPY_C_HEAP_DESCR_TYPE" in source
    assert "_molt_numpy_descr_init_header(descr)" in source
    assert "_molt_numpy_array_init_header(array_obj)" in source
    assert "_molt_numpy_dtype_meta_init_header(dtype)" in source
    assert "_molt_numpy_iter_init_header(iter)" in source
    assert "_molt_numpy_neighborhood_iter_init_header(neighborhood)" in source
    assert "array_obj->ob_base =" not in source
    assert "descr->ob_base =" not in source
    assert "dtype->ob_base =" not in source

    pyarray_size_body = _function_body(source, "_molt_pyarray_size")
    assert "array_obj->nd == 0" in pyarray_size_body
    assert "return 1" in pyarray_size_body

    aligned_realloc_body = _function_body(source, "PyArray_realloc_aligned")
    aligned_calloc_body = _function_body(source, "PyArray_calloc_aligned")
    assert "_molt_numpy_size_add_overflow" in aligned_realloc_body
    assert "old_size = *(((size_t *)ptr) - 2)" in aligned_realloc_body
    assert "old_size < size ? old_size : size" in aligned_realloc_body
    assert "*(((size_t *)aligned) - 2) = size" in aligned_realloc_body
    assert "_molt_numpy_size_mul_overflow" in aligned_calloc_body
    assert "memset(ptr, 0, nbytes)" in aligned_calloc_body

    format_body = _function_body(source, "_molt_numpy_typenum_from_buffer_format")
    assert "_molt_numpy_pep3118_format_code(format)" in format_body
    assert "return NPY_NOTYPE" in format_body

    descr_body = _function_body(source, "_molt_numpy_descr_from_buffer")
    from_any_body = _function_body(source, "PyArray_FromAny")
    assert "int *descr_owned_out" in source
    assert "int allow_cast" in source
    assert "source_typenum = _molt_numpy_typenum_from_buffer_format" in descr_body
    assert "source_typenum == NPY_NOTYPE" in descr_body
    assert "unsupported PEP 3118 buffer format" in descr_body
    assert "*descr_owned_out = 1" in descr_body
    assert "Py_DECREF((PyObject *)descr)" in descr_body
    assert "!allow_cast && requested != NULL && descr->type_num != source_typenum" in descr_body
    assert "buffer dtype does not match requested dtype" in descr_body
    assert "_molt_numpy_reject_ref_transfer_dtype(descr, \"buffer admission\")" in descr_body
    assert "resolved_descr_owned" in from_any_body
    assert "PyMem_Free(resolved_descr)" not in from_any_body
    assert "NPY_ARRAY_FORCECAST" in from_any_body
    assert "force-cast fallback" not in from_any_body
    assert "int forcecast = (requirements & NPY_ARRAY_FORCECAST) != 0" in from_any_body
    assert "forcecast ? NULL : descr" in from_any_body
    assert "forcecast,\n        &resolved_descr_owned" in from_any_body
    assert "PyArray_Cast((PyArrayObject *)array_obj, target_typenum)" in from_any_body
    assert "NPY_ARRAY_ENSURENOCOPY | NPY_ARRAY_WRITEBACKIFCOPY" in from_any_body
    assert "retained buffer lease support" in from_any_body
    assert "NPY_ARRAY_ENSURECOPY" not in from_any_body.split("NPY_ARRAY_FORCECAST", 1)[0]
    assert "view_requirements = requirements" in from_any_body
    assert "NPY_ARRAY_FORCECAST" in from_any_body.split("view_requirements = requirements", 1)[1]
    assert "NPY_ARRAY_ENSURENOCOPY" in from_any_body.split("view_requirements = requirements", 1)[1]
    assert "NPY_ARRAY_WRITEBACKIFCOPY" in from_any_body.split("view_requirements = requirements", 1)[1]
    assert "NPY_ARRAY_C_CONTIGUOUS" in from_any_body
    assert "NPY_ARRAY_F_CONTIGUOUS" in from_any_body
    assert "NPY_ARRAY_WRITEABLE" in from_any_body
    assert "_molt_numpy_buffer_flags_from_requirements(view_requirements)" in from_any_body
    assert "_molt_numpy_array_from_buffer_view(obj, &view, resolved_descr, requirements)" in from_any_body
    assert "_molt_numpy_array_from_buffer_view(obj, &view, resolved_descr, view_requirements)" not in from_any_body
    assert "needs_copy = 0" in from_any_body
    assert "needs_copy = (requirements & NPY_ARRAY_ENSURECOPY) != 0" not in from_any_body
    assert "PyArray_NewCopy((PyArrayObject *)array_obj, copy_order)" in from_any_body
    assert "Py_DECREF(array_obj)" in from_any_body
    assert "NPY_ARRAY_C_CONTIGUOUS | NPY_ARRAY_WRITEABLE" in source

    dims_body = _function_body(source, "_molt_numpy_dims_size")
    fill_body = _function_body(source, "_molt_numpy_fill_strides")
    flat_dims_body = _function_body(source, "_molt_numpy_flat_dims_offset")
    from_buffer_view_body = _function_body(source, "_molt_numpy_array_from_buffer_view")
    new_body = _function_body(source, "PyArray_NewFromDescr")
    resize_body = _function_body(source, "PyArray_Resize")
    flags_body = _function_body(source, "PyArray_UpdateFlags")
    copy_body = _function_body(source, "PyArray_NewCopy")
    cast_body = _function_body(source, "PyArray_Cast")
    dtype_transfer_body = _function_body(source, "PyArray_GetDTypeTransferFunction")
    numeric_cast_body = _function_body(source, "PyArray_GetStridedNumericCastFn")
    transfer_nd_body = _function_body(source, "PyArray_TransferNDimToStrided")
    transfer_to_nd_body = _function_body(source, "PyArray_TransferStridedToNDim")
    transfer_masked_body = _function_body(source, "PyArray_TransferMaskedStridedToNDim")
    iter_body = _function_body(source, "_molt_numpy_iter_next")
    neighborhood_body = _function_body(source, "PyArrayNeighborhoodIter_Next")

    for name, body in [
        ("_molt_numpy_dims_size", dims_body),
        ("_molt_numpy_fill_strides", fill_body),
        ("_molt_numpy_flat_dims_offset", flat_dims_body),
        ("PyArray_UpdateFlags", flags_body),
    ]:
        assert "*=" not in body, f"{name} must not use raw span multiplication"

    assert "array_obj->data = (char *)view->buf" not in from_buffer_view_body
    assert "array_obj->base = obj" not in from_buffer_view_body
    assert "array_obj->base = NULL" in from_buffer_view_body
    assert "NPY_ARRAY_OWNDATA | NPY_ARRAY_WRITEABLE" in from_buffer_view_body
    assert "PyMem_Calloc((size_t)(nbytes > 0 ? nbytes : 1), 1)" in from_buffer_view_body
    assert "_molt_numpy_flat_dims_offset(" in from_buffer_view_body
    assert "((const char *)view->buf) + src_offset" in from_buffer_view_body
    assert "array_obj->data + dst_offset" in from_buffer_view_body
    assert "buffer data pointer must not be NULL" in from_buffer_view_body

    assert "_molt_numpy_checked_nbytes_from_dims" in new_body
    assert "_molt_numpy_array_is_aligned" in new_body
    assert "PyArray_UpdateFlags(" in new_body
    assert "array_obj->flags = flags;" in new_body
    assert "array_obj->flags |= NPY_ARRAY_WRITEABLE" in new_body
    assert "_molt_numpy_checked_nbytes_from_dims" in resize_body
    assert "!PyArray_CHKFLAGS(self, NPY_ARRAY_OWNDATA)" in resize_body
    assert "cannot grow non-owned NumPy array backing storage" in resize_body
    assert "PyArray_UpdateFlags(" in resize_body
    assert "_molt_numpy_array_is_aligned" in flags_body
    assert "_molt_numpy_flat_index_offset(array_obj" in copy_body
    assert "_molt_numpy_flat_index_offset((PyArrayObject *)copy_obj" in copy_body
    assert "_molt_numpy_flat_index_offset(array_obj" in cast_body
    assert "_molt_numpy_flat_index_offset(iter->ao" in iter_body
    assert "_molt_numpy_flat_index_offset(iter->ao" in neighborhood_body
    assert "src_dtype->type_num != dst_dtype->type_num" in dtype_transfer_body
    assert "src_dtype->elsize != dst_dtype->elsize" in dtype_transfer_body
    assert "dtype transfer requires identical source and destination descriptors" in dtype_transfer_body
    assert "_molt_numpy_reject_ref_transfer_dtype(src_dtype, \"dtype transfer source\")" in dtype_transfer_body
    assert "_molt_numpy_reject_ref_transfer_dtype(dst_dtype, \"dtype transfer destination\")" in dtype_transfer_body
    assert "src_type_num != dst_type_num" in numeric_cast_body
    assert "numeric cast transfer requires identical source and destination types" in numeric_cast_body
    assert "src_type_num == NPY_OBJECT || dst_type_num == NPY_OBJECT" in numeric_cast_body
    assert "ndim != 1" in transfer_nd_body
    assert "N-D to strided transfer requires explicit coordinate lowering" in transfer_nd_body
    assert "ndim != 1" in transfer_to_nd_body
    assert "strided to N-D transfer requires explicit coordinate lowering" in transfer_to_nd_body
    assert "ndim != 1" in transfer_masked_body
    assert "masked strided to N-D transfer requires explicit coordinate lowering" in transfer_masked_body

    iter_new_body = _function_body(source, "PyArray_IterNew")
    neighborhood_new_body = _function_body(source, "PyArray_NeighborhoodIterNew")
    assert "Py_INCREF((PyObject *)array_obj)" in iter_new_body
    assert "Py_INCREF((PyObject *)iter->ao)" in neighborhood_new_body

    store_body = _function_body(source, "_molt_numpy_store_scalar")
    assert "_molt_numpy_signed_range_error" in store_body
    assert "_molt_numpy_unsigned_range_error" in store_body
    assert "raw < SCHAR_MIN || raw > SCHAR_MAX" in store_body
    assert "raw > UCHAR_MAX" in store_body

    ufunc_source = NUMPY_UFUNC_HEADER_PATH.read_text(encoding="utf-8")
    assert "#define PyUFunc_Type (*_molt_numpy_ufunc_type())" in ufunc_source
    assert "#define PyUFunc_Check(op) PyObject_TypeCheck((PyObject *)(op), &PyUFunc_Type)" in ufunc_source
    assert "#define PyUFunc_Type PyArray_Type" not in ufunc_source
    assert "PyObject_TypeCheck((PyObject *)(op), &PyArray_Type)" not in ufunc_source
    assert "static inline PyTypeObject *_molt_numpy_ufunc_type(void)" in ufunc_source
    assert "_molt_numpy_public_c_heap_type(&type_obj, &canonical, _MOLT_NUMPY_C_HEAP_UFUNC_TYPE)" in ufunc_source
    assert "_MOLT_NUMPY_C_HEAP_UFUNC,\n        _molt_numpy_ufunc_type()" in ufunc_source
    assert "_molt_numpy_abi_local_type(\n        &type_obj,\n        &canonical,\n        _MOLT_NUMPY_C_HEAP_UFUNC_TYPE" in ufunc_source
    assert '"numpy.ufunc"' in ufunc_source
    assert "ufunc->ob_refcnt = 1" in ufunc_source
    assert "ufunc->ob_type = _molt_numpy_ufunc_type()" in ufunc_source


def test_numpy_abi_local_types_are_canonical_across_translation_units(tmp_path: Path) -> None:
    clang = shutil.which("clang")
    if clang is None:
        raise AssertionError("clang is required for the NumPy ABI cross-TU type proof")

    tu_a = tmp_path / "numpy_abi_tu_a.c"
    tu_b = tmp_path / "numpy_abi_tu_b.c"
    main = tmp_path / "numpy_abi_main.c"
    exe = tmp_path / ("numpy_abi_cross_tu.exe" if shutil.which("cmd") else "numpy_abi_cross_tu")

    tu_a.write_text(
        "\n".join(
            [
                "#include <Python.h>",
                "#include <numpy/arrayobject.h>",
                "#include <numpy/ufuncobject.h>",
                "",
                "static PyArrayObject_fields arr;",
                "static PyArray_Descr descr;",
                "static PyUFuncObject ufunc;",
                "",
                "PyObject *tu_a_array_object(void) {",
                "    _molt_numpy_array_init_header(&arr);",
                "    return (PyObject *)&arr;",
                "}",
                "",
                "PyObject *tu_a_descr_object(void) {",
                "    _molt_numpy_descr_init_header(&descr);",
                "    return (PyObject *)&descr;",
                "}",
                "",
                "PyObject *tu_a_ufunc_object(void) {",
                "    _molt_numpy_ufunc_init_header(&ufunc);",
                "    return (PyObject *)&ufunc;",
                "}",
                "",
                "PyTypeObject *tu_a_array_type(void) { return &PyArray_Type; }",
                "PyTypeObject *tu_a_descr_type(void) { return &PyArrayDescr_Type; }",
                "PyTypeObject *tu_a_ufunc_type(void) { return &PyUFunc_Type; }",
                "",
            ]
        ),
        encoding="utf-8",
    )
    tu_b.write_text(
        "\n".join(
            [
                "#include <Python.h>",
                "#include <numpy/arrayobject.h>",
                "#include <numpy/ufuncobject.h>",
                "",
                "extern PyObject *tu_a_array_object(void);",
                "extern PyObject *tu_a_descr_object(void);",
                "extern PyObject *tu_a_ufunc_object(void);",
                "extern PyTypeObject *tu_a_array_type(void);",
                "extern PyTypeObject *tu_a_descr_type(void);",
                "extern PyTypeObject *tu_a_ufunc_type(void);",
                "",
                "int tu_b_verify(void) {",
                "    PyObject *arr = tu_a_array_object();",
                "    PyObject *descr = tu_a_descr_object();",
                "    PyObject *ufunc = tu_a_ufunc_object();",
                "    if (!PyArray_Check(arr)) return 10;",
                "    if (Py_TYPE(arr) != &PyArray_Type) return 11;",
                "    if (tu_a_array_type() != &PyArray_Type) return 12;",
                "    if (!PyArray_DescrCheck(descr)) return 20;",
                "    if (Py_TYPE(descr) != &PyArrayDescr_Type) return 21;",
                "    if (tu_a_descr_type() != &PyArrayDescr_Type) return 22;",
                "    if (PyArray_Check(ufunc)) return 30;",
                "    if (!PyUFunc_Check(ufunc)) return 31;",
                "    if (Py_TYPE(ufunc) != &PyUFunc_Type) return 32;",
                "    if (tu_a_ufunc_type() != &PyUFunc_Type) return 33;",
                "    if (PyObject_TypeCheck((PyObject *)&PyArray_Type, &PyArray_Type)) return 40;",
                "    return 0;",
                "}",
                "",
            ]
        ),
        encoding="utf-8",
    )
    main.write_text(
        "\n".join(
            [
                "#include <Python.h>",
                "#include <stdint.h>",
                "#include <stdlib.h>",
                "",
                "PyTypeObject PyType_Type;",
                "PyTypeObject PyBaseObject_Type;",
                "",
                "typedef struct CanonType {",
                "    uint32_t kind;",
                "    PyTypeObject *type;",
                "} CanonType;",
                "",
                "static CanonType canon_types[32];",
                "static int canon_type_count = 0;",
                "",
                "PyTypeObject *molt_cpython_abi_type_canonicalize(uint32_t kind, PyTypeObject *type_obj) {",
                "    int i;",
                "    if (kind == 0 || type_obj == NULL) return NULL;",
                "    for (i = 0; i < canon_type_count; i++) {",
                "        if (canon_types[i].kind == kind) return canon_types[i].type;",
                "    }",
                "    if (canon_type_count >= 32) abort();",
                "    canon_types[canon_type_count].kind = kind;",
                "    canon_types[canon_type_count].type = type_obj;",
                "    canon_type_count++;",
                "    return type_obj;",
                "}",
                "",
                "#undef PyObject_TypeCheck",
                "int PyObject_TypeCheck(PyObject *op, PyTypeObject *tp) {",
                "    return op != NULL && tp != NULL && Py_TYPE(op) == tp;",
                "}",
                "",
                "#undef PyType_Check",
                "int PyType_Check(PyObject *op) {",
                "    return op != NULL && (op == (PyObject *)&PyType_Type || Py_TYPE(op) == &PyType_Type);",
                "}",
                "",
                "#undef Py_INCREF",
                "void Py_INCREF(PyObject *op) {",
                "    if (op != NULL && op->ob_refcnt < _Py_IMMORTAL_REFCNT_LOCAL) op->ob_refcnt++;",
                "}",
                "",
                "#undef Py_DECREF",
                "void Py_DECREF(PyObject *op) {",
                "    if (op != NULL && op->ob_refcnt > 0 && op->ob_refcnt < _Py_IMMORTAL_REFCNT_LOCAL) op->ob_refcnt--;",
                "}",
                "",
                "void PyMem_Free(void *ptr) { free(ptr); }",
                "",
                "extern int tu_b_verify(void);",
                "",
                "int main(void) {",
                "    PyType_Type.ob_refcnt = _Py_IMMORTAL_REFCNT_LOCAL;",
                "    PyType_Type.ob_type = &PyType_Type;",
                "    PyType_Type.tp_name = \"type\";",
                "    PyBaseObject_Type.ob_refcnt = _Py_IMMORTAL_REFCNT_LOCAL;",
                "    PyBaseObject_Type.ob_type = &PyType_Type;",
                "    PyBaseObject_Type.tp_name = \"object\";",
                "    return tu_b_verify();",
                "}",
                "",
            ]
        ),
        encoding="utf-8",
    )

    result = subprocess.run(
        [
            clang,
            "-Wall",
            "-Wextra",
            "-Werror",
            f"-I{ROOT / 'runtime' / 'molt-cpython-abi' / 'include'}",
            f"-I{ROOT / 'include'}",
            str(tu_a),
            str(tu_b),
            str(main),
            "-o",
            str(exe),
        ],
        capture_output=True,
        text=True,
        check=False,
    )
    assert result.returncode == 0, result.stderr

    run = subprocess.run([str(exe)], capture_output=True, text=True, check=False)
    assert run.returncode == 0, f"cross-TU NumPy ABI type proof failed rc={run.returncode}"


def test_numpy_public_c_heap_types_are_canonical_across_translation_units(tmp_path: Path) -> None:
    clang = shutil.which("clang")
    if clang is None:
        raise AssertionError("clang is required for the NumPy public cross-TU type proof")

    tu_a = tmp_path / "numpy_public_tu_a.c"
    tu_b = tmp_path / "numpy_public_tu_b.c"
    main = tmp_path / "numpy_public_main.c"
    exe = tmp_path / ("numpy_public_cross_tu.exe" if shutil.which("cmd") else "numpy_public_cross_tu")

    tu_a.write_text(
        "\n".join(
            [
                "#include <Python.h>",
                "#include <numpy/arrayobject.h>",
                "#include <numpy/ufuncobject.h>",
                "",
                "static PyArrayObject_fields arr;",
                "static PyArray_Descr descr;",
                "static PyUFuncObject ufunc;",
                "",
                "PyObject *public_tu_a_array_object(void) {",
                "    _molt_numpy_array_init_header(&arr);",
                "    return (PyObject *)&arr;",
                "}",
                "",
                "PyObject *public_tu_a_descr_object(void) {",
                "    _molt_numpy_descr_init_header(&descr);",
                "    return (PyObject *)&descr;",
                "}",
                "",
                "PyObject *public_tu_a_ufunc_object(void) {",
                "    _molt_numpy_ufunc_init_header(&ufunc);",
                "    return (PyObject *)&ufunc;",
                "}",
                "",
                "PyTypeObject *public_tu_a_array_type(void) { return &PyArray_Type; }",
                "PyTypeObject *public_tu_a_descr_type(void) { return &PyArrayDescr_Type; }",
                "PyTypeObject *public_tu_a_ufunc_type(void) { return &PyUFunc_Type; }",
                "",
            ]
        ),
        encoding="utf-8",
    )
    tu_b.write_text(
        "\n".join(
            [
                "#include <Python.h>",
                "#include <numpy/arrayobject.h>",
                "#include <numpy/ufuncobject.h>",
                "",
                "extern PyObject *public_tu_a_array_object(void);",
                "extern PyObject *public_tu_a_descr_object(void);",
                "extern PyObject *public_tu_a_ufunc_object(void);",
                "extern PyTypeObject *public_tu_a_array_type(void);",
                "extern PyTypeObject *public_tu_a_descr_type(void);",
                "extern PyTypeObject *public_tu_a_ufunc_type(void);",
                "",
                "int public_tu_b_verify(void) {",
                "    PyObject *arr = public_tu_a_array_object();",
                "    PyObject *descr = public_tu_a_descr_object();",
                "    PyObject *ufunc = public_tu_a_ufunc_object();",
                "    if (!PyArray_Check(arr)) return 10;",
                "    if (PyArray_Check((PyObject *)&PyArray_Type)) return 11;",
                "    if (!PyType_Check((PyObject *)&PyArray_Type)) return 12;",
                "    if (!PyType_CheckExact((PyObject *)&PyArray_Type)) return 13;",
                "    if (PyObject_TypeCheck((PyObject *)&PyArray_Type, &PyArray_Type)) return 14;",
                "    if (public_tu_a_array_type() != &PyArray_Type) return 15;",
                "    if (!PyArray_DescrCheck(descr)) return 20;",
                "    if (PyArray_DescrCheck((PyObject *)&PyArrayDescr_Type)) return 21;",
                "    if (!PyType_Check((PyObject *)&PyArrayDescr_Type)) return 22;",
                "    if (PyObject_TypeCheck((PyObject *)&PyArrayDescr_Type, &PyArrayDescr_Type)) return 23;",
                "    if (public_tu_a_descr_type() != &PyArrayDescr_Type) return 24;",
                "    if (PyArray_Check(ufunc)) return 30;",
                "    if (!PyUFunc_Check(ufunc)) return 31;",
                "    if (PyUFunc_Check((PyObject *)&PyUFunc_Type)) return 32;",
                "    if (!PyType_Check((PyObject *)&PyUFunc_Type)) return 33;",
                "    if (PyObject_TypeCheck((PyObject *)&PyUFunc_Type, &PyUFunc_Type)) return 34;",
                "    if (public_tu_a_ufunc_type() != &PyUFunc_Type) return 35;",
                "    return 0;",
                "}",
                "",
            ]
        ),
        encoding="utf-8",
    )
    main.write_text(
        "\n".join(
            [
                "#include <Python.h>",
                "#include <stdint.h>",
                "#include <stdlib.h>",
                "#include <string.h>",
                "",
                "typedef struct CanonPtr {",
                "    uint32_t kind;",
                "    uintptr_t ptr;",
                "} CanonPtr;",
                "",
                "static uintptr_t registered[128];",
                "static size_t registered_count = 0;",
                "static CanonPtr canon_types[32];",
                "static int canon_type_count = 0;",
                "",
                "static void remember_registered(uintptr_t ptr) {",
                "    size_t i;",
                "    if (ptr == 0) return;",
                "    for (i = 0; i < registered_count; i++) {",
                "        if (registered[i] == ptr) return;",
                "    }",
                "    if (registered_count >= 128) abort();",
                "    registered[registered_count++] = ptr;",
                "}",
                "",
                "uint32_t molt_c_api_version(void) { return MOLT_C_API_VERSION; }",
                "int32_t molt_c_heap_register(uintptr_t ptr) { remember_registered(ptr); return ptr == 0 ? -1 : 0; }",
                "int32_t molt_c_heap_unregister(uintptr_t ptr) { (void)ptr; return ptr == 0 ? -1 : 0; }",
                "int32_t molt_c_heap_contains(uintptr_t ptr) {",
                "    size_t i;",
                "    for (i = 0; i < registered_count; i++) {",
                "        if (registered[i] == ptr) return 1;",
                "    }",
                "    return 0;",
                "}",
                "",
                "uintptr_t molt_c_heap_type_canonicalize(uint32_t kind, uintptr_t ptr) {",
                "    int i;",
                "    if (kind == 0 || ptr == 0) return 0;",
                "    for (i = 0; i < canon_type_count; i++) {",
                "        if (canon_types[i].kind == kind) {",
                "            remember_registered(canon_types[i].ptr);",
                "            return canon_types[i].ptr;",
                "        }",
                "    }",
                "    if (canon_type_count >= 32) abort();",
                "    canon_types[canon_type_count].kind = kind;",
                "    canon_types[canon_type_count].ptr = ptr;",
                "    canon_type_count++;",
                "    remember_registered(ptr);",
                "    return ptr;",
                "}",
                "",
                "int32_t molt_c_heap_register_buffer_exporter(uint32_t kind, uintptr_t type_ptr, MoltCHeapBufferExporter exporter) {",
                "    (void)kind; (void)type_ptr; (void)exporter; return 0;",
                "}",
                "int32_t molt_c_heap_register_buffer_releaser(uint32_t kind, uintptr_t type_ptr, MoltCHeapBufferReleaser releaser) {",
                "    (void)kind; (void)type_ptr; (void)releaser; return 0;",
                "}",
                "int32_t molt_c_heap_export_buffer(uintptr_t ptr, MoltBufferView *out_view) {",
                "    (void)ptr; (void)out_view; return -1;",
                "}",
                "int32_t molt_c_heap_release_buffer(uintptr_t ptr, MoltBufferView *view) {",
                "    (void)ptr; (void)view; return 0;",
                "}",
                "",
                "MoltHandle molt_none(void) { return 1; }",
                "MoltHandle molt_string_from(const uint8_t *ptr, uint64_t len) { (void)ptr; (void)len; return 2; }",
                "MoltHandle molt_int_from_i64(int64_t value) { (void)value; return 4; }",
                "void molt_handle_incref(MoltHandle handle) { (void)handle; }",
                "void molt_handle_decref(MoltHandle handle) { (void)handle; }",
                "MoltHandle molt_builtin_class_lookup(MoltHandle name_bits) { (void)name_bits; return 0; }",
                "MoltHandle molt_object_getattr_bytes(MoltHandle obj_bits, const uint8_t *name_ptr, uint64_t name_len) {",
                "    (void)obj_bits; (void)name_ptr; (void)name_len; return 0;",
                "}",
                "int32_t molt_object_equal(MoltHandle lhs_bits, MoltHandle rhs_bits) {",
                "    return lhs_bits == rhs_bits ? 1 : 0;",
                "}",
                "int64_t molt_sequence_length(MoltHandle seq_bits) { (void)seq_bits; return 0; }",
                "MoltHandle molt_sequence_getitem(MoltHandle seq_bits, MoltHandle key_bits) {",
                "    (void)seq_bits; (void)key_bits; return 0;",
                "}",
                "int32_t molt_err_pending(void) { return 0; }",
                "int32_t molt_err_clear(void) { return 0; }",
                "int32_t molt_err_set(MoltHandle exc_type_bits, const uint8_t *message_ptr, uint64_t message_len) {",
                "    (void)exc_type_bits; (void)message_ptr; (void)message_len; return 0;",
                "}",
                "MoltHandle molt_exception_class(MoltHandle kind_bits) { (void)kind_bits; return 0; }",
                "uint64_t molt_type_of_borrowed(uint64_t obj_bits) { (void)obj_bits; return 0; }",
                "MoltHandle molt_object_call(MoltHandle callable, MoltHandle args, MoltHandle kwargs) {",
                "    (void)callable; (void)args; (void)kwargs; return 0;",
                "}",
                "MoltHandle molt_tuple_from_array(const MoltHandle *items, uint64_t len) {",
                "    (void)items; (void)len; return 3;",
                "}",
                "",
                "extern int public_tu_b_verify(void);",
                "",
                "int main(void) {",
                "    return public_tu_b_verify();",
                "}",
                "",
            ]
        ),
        encoding="utf-8",
    )

    result = subprocess.run(
        [
            clang,
            "-Wall",
            "-Wextra",
            "-Werror",
            f"-I{ROOT / 'include'}",
            str(tu_a),
            str(tu_b),
            str(main),
            "-o",
            str(exe),
        ],
        capture_output=True,
        text=True,
        check=False,
    )
    assert result.returncode == 0, result.stderr

    run = subprocess.run([str(exe)], capture_output=True, text=True, check=False)
    assert run.returncode == 0, f"public cross-TU NumPy type proof failed rc={run.returncode}"
