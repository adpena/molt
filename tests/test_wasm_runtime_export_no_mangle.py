"""Gate: every WASM-required runtime export must have one symbol owner.

The split-runtime WASM linker exports runtime functions by their exact C symbol
name via ``--export-if-defined=molt_<name>``. A function defined as
``pub extern "C" fn molt_<name>`` *without* ``#[no_mangle]`` gets a Rust-mangled
symbol, so ``--export-if-defined`` finds nothing and the symbol is silently
dropped from the runtime artifact. Because the runtime-export check only runs
after a program reaches the link stage, such a drop is invisible until an actual
``molt build --target wasm`` link, and it breaks linking for every program that
needs the symbol: structural imports, host exports, and reserved callables.

This is exactly how ``molt_exception_kind``, ``molt_exceptiongroup_init`` and
``molt_header_size`` regressed: extract-authority refactors moved the functions
and dropped the attribute. This gate makes that bug class unexpressible by
asserting, statically, that every required export has a real ``no_mangle`` owner.

The inverse bug class is equally structural: adding ``no_mangle`` to both a
``molt-runtime`` export wrapper and its extracted implementation crate creates
duplicate native symbols during Rust tests. The runtime wrapper owns the ABI
export when it exists; extracted crates own implementation, not duplicate export
symbols.

The required-export set is sourced from the same authorities the runtime build
and the export-link args use (`molt._wasm_runtime_exports` /
`molt._wasm_abi_generated`), so the gate cannot drift from the real contract.
"""

from __future__ import annotations

import re
from pathlib import Path

from molt._wasm_abi_generated import (
    WASM_IMPORT_REGISTRY,
    WASM_RUNTIME_HOST_EXPORTS,
    WASM_RESERVED_RUNTIME_CALLABLES,
)

REPO_ROOT = Path(__file__).resolve().parents[1]
RUNTIME_ROOT = REPO_ROOT / "runtime"

# `pub extern "C" fn molt_<name>`: the C-ABI export definition form whose symbol
# name is governed by the presence or absence of `#[no_mangle]`.
_EXTERN_C_FN_RE = re.compile(r'pub\s+(?:unsafe\s+)?extern\s+"C"\s+fn\s+(molt_\w+)')

# Lines that may legally sit between a `#[no_mangle]` attribute and the function
# signature: other attributes, doc/line comments, block-comment bodies, blanks.
_ATTR_OR_DOC_RE = re.compile(r"^\s*(#\[|///|//!|//|\*|/\*|\*/)|^\s*$")


def _required_export_symbols() -> set[str]:
    """The molt_-prefixed runtime symbols the WASM link contract requires."""

    required: set[str] = set(WASM_RUNTIME_HOST_EXPORTS)
    required |= {f"molt_{name}" for name in WASM_IMPORT_REGISTRY}
    # WASM_RESERVED_RUNTIME_CALLABLES entries include runtime export names.
    required |= {entry[1] for entry in WASM_RESERVED_RUNTIME_CALLABLES}
    return required


def _has_no_mangle_above(lines: list[str], def_idx: int) -> bool:
    """True if a `no_mangle` attribute precedes the def through the contiguous
    attribute/doc/comment/blank block."""

    idx = def_idx - 1
    while idx >= 0 and _ATTR_OR_DOC_RE.match(lines[idx]):
        if "no_mangle" in lines[idx]:
            return True
        idx -= 1
    return False


def _is_shipped_runtime_source(path: Path) -> bool:
    if "tests" in path.parts:
        return False
    return path.name not in {"test_host.rs", "bridge_test_stubs.rs"}


def _extern_c_fn_definitions() -> dict[str, list[tuple[bool, Path, int]]]:
    """Map every shipped `pub extern "C" fn molt_*` definition to its export
    ownership bit and source location."""

    found: dict[str, list[tuple[bool, Path, int]]] = {}
    for path in sorted(RUNTIME_ROOT.rglob("*.rs")):
        posix = path.as_posix()
        if "/target/" in posix or not _is_shipped_runtime_source(path):
            continue
        lines = path.read_text(encoding="utf-8", errors="replace").splitlines()
        for line_idx, line in enumerate(lines):
            match = _EXTERN_C_FN_RE.search(line)
            if match:
                found.setdefault(match.group(1), []).append(
                    (
                        _has_no_mangle_above(lines, line_idx),
                        path,
                        line_idx + 1,
                    )
                )
    return found


def _is_molt_runtime_crate_source(path: Path) -> bool:
    return (RUNTIME_ROOT / "molt-runtime" / "src") in path.parents


def test_required_wasm_runtime_exports_have_no_mangle() -> None:
    required = _required_export_symbols()
    defined = _extern_c_fn_definitions()

    missing = sorted(
        symbol
        for symbol in required
        if symbol in defined and not any(has_no_mangle for has_no_mangle, _, _ in defined[symbol])
    )

    detail = "\n".join(
        f"  {symbol}: "
        + ", ".join(
            f"{path.relative_to(REPO_ROOT).as_posix()}:{line}"
            for _, path, line in defined[symbol]
        )
        for symbol in missing
    )
    assert not missing, (
        "WASM-required runtime exports are defined as `pub extern \"C\"` but lack "
        "`#[unsafe(no_mangle)]`, so the linker cannot export them and no program "
        f"can link to WASM:\n{detail}\n"
        "Add `#[unsafe(no_mangle)]` to the export-owning definition; the symbol "
        "name is the link contract."
    )


def test_extracted_runtime_crates_do_not_duplicate_wrapper_exports() -> None:
    required = _required_export_symbols()
    defined = _extern_c_fn_definitions()

    duplicate_owned = []
    for symbol in sorted(required):
        defs = defined.get(symbol, [])
        runtime_owners = [
            (path, line)
            for has_no_mangle, path, line in defs
            if has_no_mangle and _is_molt_runtime_crate_source(path)
        ]
        extracted_owners = [
            (path, line)
            for has_no_mangle, path, line in defs
            if has_no_mangle and not _is_molt_runtime_crate_source(path)
        ]
        if runtime_owners and extracted_owners:
            duplicate_owned.append((symbol, runtime_owners, extracted_owners))

    detail = "\n".join(
        f"  {symbol}: runtime="
        + ", ".join(f"{path.relative_to(REPO_ROOT).as_posix()}:{line}" for path, line in runtime)
        + " extracted="
        + ", ".join(
            f"{path.relative_to(REPO_ROOT).as_posix()}:{line}" for path, line in extracted
        )
        for symbol, runtime, extracted in duplicate_owned
    )
    assert not duplicate_owned, (
        "`molt-runtime` wrapper exports and extracted implementation crates both "
        "own `#[unsafe(no_mangle)]` for the same WASM-required symbol, which "
        f"creates native duplicate symbols:\n{detail}\n"
        "Keep `no_mangle` on the ABI wrapper when one exists; leave extracted "
        "crates as implementation authority."
    )
