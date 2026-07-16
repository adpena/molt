"""Prevent in-tree and FFI itertools class construction from drifting apart."""

from pathlib import Path


ROOT = Path(__file__).resolve().parents[1]
DIRECT = ROOT / "runtime/molt-runtime/src/bridge/itertools.rs"
CANONICAL = ROOT / "runtime/molt-runtime/src/itertools_class.rs"
FFI = ROOT / "runtime/molt-runtime/src/itertools_bridge.rs"


def test_itertools_class_construction_has_one_runtime_authority() -> None:
    canonical = CANONICAL.read_text(encoding="utf-8")
    direct = DIRECT.read_text(encoding="utf-8")
    ffi = FFI.read_text(encoding="utf-8")
    canonical_class = canonical.split(
        "pub(crate) fn alloc_itertools_class(", maxsplit=1
    )[1].split("#[cfg(test)]", maxsplit=1)[0]
    direct_class = direct.split("pub fn alloc_itertools_class(", maxsplit=1)[1].split(
        "pub fn class_set_iter_next(", maxsplit=1
    )[0]
    ffi_class = ffi.split(
        'pub extern "C" fn molt_itertools_alloc_class(', maxsplit=1
    )[1].split("#[unsafe(no_mangle)]", maxsplit=1)[0]

    assert canonical.count("pub(crate) fn alloc_itertools_class(") == 1
    assert "class_set_instance_shape_id(class_ptr, shape)" in canonical_class
    assert direct_class.count("crate::itertools_class::alloc_itertools_class(") == 1
    assert ffi_class.count("alloc_itertools_class(_py, name, layout_size, shape)") == 1
    for duplicate_authority in (
        "alloc_class_obj(",
        "class_set_instance_shape_id(",
        "object_init_class_edge_unpublished(",
        "__molt_layout_size__",
    ):
        assert duplicate_authority not in direct_class
        assert duplicate_authority not in ffi_class
