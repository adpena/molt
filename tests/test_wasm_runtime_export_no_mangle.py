"""Gate: every WASM-required runtime export must carry ``#[unsafe(no_mangle)]``.

The split-runtime WASM linker exports runtime functions by their exact C symbol
name via ``--export-if-defined=molt_<name>``. A function defined as
``pub extern "C" fn molt_<name>`` *without* ``#[no_mangle]`` gets a Rust-mangled
symbol, so ``--export-if-defined`` finds nothing and the symbol is silently
dropped from the runtime artifact. Because the runtime-export check only runs
after a program reaches the link stage, such a drop is invisible until an actual
``molt build --target wasm`` link — and it breaks linking for EVERY program that
needs the symbol (structural imports, host exports, reserved callables).

This is exactly how ``molt_exception_kind``, ``molt_exceptiongroup_init`` and
``molt_header_size`` regressed: the "Extract … authorities" refactors moved the
functions and dropped the attribute. This gate makes that bug class
unexpressible by asserting, statically, that every required export that is
defined as a ``pub extern "C"`` function also carries ``no_mangle``.

The required-export set is sourced from the SAME authorities the runtime build
and the export-link args use (`molt._wasm_runtime_exports` /
`molt._wasm_abi_generated`), so the gate cannot drift from the real contract.
"""

from __future__ import annotations

import re
from pathlib import Path

from molt._wasm_abi_generated import (
    WASM_IMPORT_REGISTRY,
    WASM_RESERVED_RUNTIME_CALLABLES,
)
from molt._wasm_runtime_exports import _HOST_RUNTIME_EXPORTS

REPO_ROOT = Path(__file__).resolve().parents[1]
RUNTIME_ROOT = REPO_ROOT / "runtime"

# `pub extern "C" fn molt_<name>` — the C-ABI export definition form whose symbol
# name is governed by the presence/absence of `#[no_mangle]`.
_EXTERN_C_FN_RE = re.compile(r'pub\s+(?:unsafe\s+)?extern\s+"C"\s+fn\s+(molt_\w+)')

# Lines that may legally sit between a `#[no_mangle]` attribute and the function
# signature: other attributes, doc/line comments, block-comment bodies, blanks.
_ATTR_OR_DOC_RE = re.compile(r"^\s*(#\[|///|//!|//|\*|/\*|\*/)|^\s*$")


def _required_export_symbols() -> set[str]:
    """The molt_-prefixed runtime symbols the WASM link contract requires."""

    required: set[str] = set(_HOST_RUNTIME_EXPORTS)
    required |= {f"molt_{name}" for name in WASM_IMPORT_REGISTRY}
    # WASM_RESERVED_RUNTIME_CALLABLES entries are (index, runtime_name, import_name, arity).
    required |= {entry[1] for entry in WASM_RESERVED_RUNTIME_CALLABLES}
    return required


def _has_no_mangle_above(lines: list[str], def_idx: int) -> bool:
    """True if a `no_mangle` attribute precedes the def through the contiguous
    attribute/doc/comment/blank block (robust to a doc comment sitting between
    the attribute and the signature)."""

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


def _extern_c_fn_no_mangle_map() -> dict[str, tuple[bool, Path, int]]:
    """Map every shipped `pub extern "C" fn molt_*` def to whether it has
    no_mangle, plus its location (first definition wins)."""

    found: dict[str, tuple[bool, Path, int]] = {}
    for path in sorted(RUNTIME_ROOT.rglob("*.rs")):
        posix = path.as_posix()
        if "/target/" in posix or not _is_shipped_runtime_source(path):
            continue
        lines = path.read_text(encoding="utf-8", errors="replace").splitlines()
        for line_idx, line in enumerate(lines):
            match = _EXTERN_C_FN_RE.search(line)
            if match and match.group(1) not in found:
                found[match.group(1)] = (
                    _has_no_mangle_above(lines, line_idx),
                    path,
                    line_idx + 1,
                )
    return found


def test_required_wasm_runtime_exports_have_no_mangle() -> None:
    required = _required_export_symbols()
    defined = _extern_c_fn_no_mangle_map()

    missing = sorted(
        symbol
        for symbol in required
        if symbol in defined and not defined[symbol][0]
    )

    detail = "\n".join(
        f"  {symbol}: {defined[symbol][1].relative_to(REPO_ROOT).as_posix()}"
        f":{defined[symbol][2]}"
        for symbol in missing
    )
    assert not missing, (
        "WASM-required runtime exports are defined as `pub extern \"C\"` but lack "
        "`#[unsafe(no_mangle)]`, so the linker cannot export them and no program "
        f"can link to WASM:\n{detail}\n"
        "Add `#[unsafe(no_mangle)]` to each (the symbol name is the link contract)."
    )
