from __future__ import annotations

import re
from pathlib import Path


ROOT = Path(__file__).resolve().parents[1]
RUST_AUTHORITIES = (
    ROOT / "runtime" / "molt-runtime" / "src",
    ROOT / "runtime" / "molt-runtime-core" / "src",
    ROOT / "runtime" / "molt-obj-model" / "src",
    ROOT / "runtime" / "molt-cpython-abi" / "src",
)


def _production_rust() -> str:
    chunks: list[str] = []
    for authority in RUST_AUTHORITIES:
        for path in sorted(authority.rglob("*.rs")):
            chunks.append(f"\n// FILE: {path.relative_to(ROOT).as_posix()}\n")
            chunks.extend(
                line.split("//", 1)[0] + "\n" for line in path.read_text().splitlines()
            )
    return "".join(chunks)


def test_integer_carried_abi_values_never_use_truncating_pointer_conversions() -> None:
    source = _production_rust()
    forbidden = {
        "integer-pointer round trip": re.compile(r"as\s+usize\s+as\s+\*(?:const|mut)"),
        "integer transmute target": re.compile(r"transmute\([^\n)]*\bas\s+usize\b"),
        "ABI bits to usize": re.compile(r"\b[A-Za-z_]\w*_bits\s+as\s+usize\b"),
        "u32 pointer reconstruction": re.compile(r"as\s+u32\s+as\s+\*(?:const|mut)"),
    }
    failures = [name for name, pattern in forbidden.items() if pattern.search(source)]
    assert not failures, (
        "integer-carried ABI values must decode through the checked target-width and "
        f"strict-provenance authorities; forbidden lanes: {failures}"
    )


def test_checked_width_authorities_remain_fail_closed_and_inlined() -> None:
    platform = (ROOT / "runtime/molt-runtime-platform/src/utils.rs").read_text()
    provenance = (ROOT / "runtime/molt-runtime/src/provenance/abi.rs").read_text()
    layout = (ROOT / "runtime/molt-runtime/src/object/layout.rs").read_text()
    assert "#[inline(always)]\npub fn usize_from_bits" in platform
    assert "usize::try_from(bits).ok()" in platform
    assert "with_exposed_provenance::<T>" in provenance
    assert "with_exposed_provenance_mut::<T>" in provenance
    assert "ptr_addr % alignment != 0" in provenance
    assert "len.checked_mul(core::mem::size_of::<T>())?" in provenance
    assert "byte_len > isize::MAX as usize" in provenance
    assert "ptr_addr.checked_add(byte_len)?" in provenance
    assert "pub(crate) unsafe fn function_arity_usize" in layout
    assert "usize::try_from(function_arity(ptr)).ok()" in layout
