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
duplicate native symbols during Rust tests. Ownership is per feature and target:
a disabled-feature fallback or WASM-only diagnostic provider cannot supply the
native feature-enabled symbol.
A wrapper and extracted implementation must not both export in the same cell.

The required-export set is sourced from the same authorities the runtime build
and the export-link args use (`molt._wasm_runtime_exports` /
`molt._wasm_abi_generated`), so the gate cannot drift from the real contract.
"""

from __future__ import annotations

from dataclasses import dataclass
import re
from pathlib import Path

import pytest

from molt._runtime_feature_gates import feature_gate_for_symbol
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


def _attributes_above(lines: list[str], def_idx: int) -> str:
    idx = def_idx - 1
    attributes = []
    while idx >= 0 and _ATTR_OR_DOC_RE.match(lines[idx]):
        if lines[idx].strip().startswith("#["):
            attributes.append(lines[idx].strip())
        idx -= 1
    return "\n".join(reversed(attributes))


@dataclass(frozen=True)
class ExportDefinition:
    no_mangle: bool
    path: Path
    line: int
    attributes: str

    def exports_in(self, cell: str) -> bool:
        if self.no_mangle:
            return True
        # This current source form supplies an exact C symbol only on WASM.
        # A Rust-callable native definition is not a native C-export promise.
        return cell.endswith("/wasm32") and (
            '#[cfg_attr(target_arch = "wasm32", unsafe(no_mangle))]'
            in self.attributes.splitlines()
        )

    @property
    def disabled_feature(self) -> str | None:
        match = re.search(r'#\[cfg\(not\(feature = "([^\"]+)"\)\)\]', self.attributes)
        return match[1] if match else None


def _lzma_target_owners() -> dict[Path, str]:
    """Recognize the two source-declared LZMA modules, failing on gate drift.

    This is a bounded projection of the current module gates, not a Rust cfg
    evaluator. Keep the exact gates checked before treating owners as disjoint.
    """
    owners = {}
    for parent, expected_cfg, expected_path, target in (
        (
            RUNTIME_ROOT / "molt-runtime-compression/src/lib.rs",
            '#[cfg(not(target_arch = "wasm32"))]',
            "lzma.rs",
            "native",
        ),
        (
            RUNTIME_ROOT / "molt-runtime/src/builtins/mod.rs",
            '#[cfg(all(feature = "stdlib_compression", target_arch = "wasm32"))]',
            "lzma_wasm.rs",
            "wasm32",
        ),
    ):
        lines = parent.read_text(encoding="utf-8").splitlines()
        declarations = [
            idx
            for idx, line in enumerate(lines)
            if re.fullmatch(r"pub(?:\(crate\))? mod lzma;", line.strip())
        ]
        assert len(declarations) == 1, f"LZMA module owner changed: {parent}"
        attributes = _attributes_above(lines, declarations[0])
        cfgs = [line for line in attributes.splitlines() if line.startswith("#[cfg")]
        assert cfgs == [expected_cfg], f"LZMA target gate changed: {parent}: {cfgs}"
        path_attribute = re.search(r'#\[path = "([^\"]+)"\]', attributes)
        filename = path_attribute[1] if path_attribute else "lzma.rs"
        assert filename == expected_path, f"LZMA module path changed: {parent}"
        owners[parent.parent / filename] = target
    return owners


def _owner_cells(
    symbol: str, definitions: list[ExportDefinition], targets: dict[Path, str]
) -> dict[str, list[ExportDefinition]]:
    enabled = [
        definition for definition in definitions if not definition.disabled_feature
    ]
    # The required set is a WASM ABI authority. The explicitly target-split
    # LZMA modules additionally own the native C surface checked above; do not
    # infer a native C-export requirement for every Rust callable in the set.
    target_cells = (
        ("native", "wasm32")
        if any(definition.path in targets for definition in definitions)
        else ("wasm32",)
    )
    cells = {
        f"feature-enabled/{target}": [
            definition
            for definition in enabled
            if targets.get(definition.path, target) == target
        ]
        for target in target_cells
    }
    for definition in definitions:
        feature = definition.disabled_feature
        if feature is None:
            continue
        # The canonical symbol gate or a direct positive cfg must establish the
        # counterpart. An arbitrary negative annotation is not an exemption.
        assert feature_gate_for_symbol(symbol) == feature or any(
            f'#[cfg(feature = "{feature}")]' in owner.attributes for owner in enabled
        ), f"unmatched disabled-feature owner: {symbol}: {definition}"
        cells.setdefault(f"{feature}-disabled/wasm32", []).append(definition)
    return cells


def _is_shipped_runtime_source(path: Path) -> bool:
    if "tests" in path.parts:
        return False
    return path.name not in {"test_host.rs", "bridge_test_stubs.rs"}


def _extern_c_fn_definitions() -> dict[str, list[ExportDefinition]]:
    """Map every shipped `pub extern "C" fn molt_*` definition to its export
    ownership bit and source location."""

    found: dict[str, list[ExportDefinition]] = {}
    for path in sorted(RUNTIME_ROOT.rglob("*.rs")):
        posix = path.as_posix()
        if "/target/" in posix or not _is_shipped_runtime_source(path):
            continue
        lines = path.read_text(encoding="utf-8", errors="replace").splitlines()
        for line_idx, line in enumerate(lines):
            match = _EXTERN_C_FN_RE.search(line)
            if match:
                found.setdefault(match.group(1), []).append(
                    ExportDefinition(
                        bool(
                            re.search(
                                r"#\[(?:unsafe\()?no_mangle\)?\]",
                                _attributes_above(lines, line_idx),
                            )
                        ),
                        path,
                        line_idx + 1,
                        _attributes_above(lines, line_idx),
                    )
                )
    return found


def _is_molt_runtime_crate_source(path: Path) -> bool:
    return (RUNTIME_ROOT / "molt-runtime" / "src") in path.parents


def test_required_wasm_runtime_exports_have_no_mangle() -> None:
    defined = _extern_c_fn_definitions()
    targets = _lzma_target_owners()
    missing = []
    for symbol in sorted(_required_export_symbols() & defined.keys()):
        for cell, definitions in _owner_cells(symbol, defined[symbol], targets).items():
            if not any(definition.exports_in(cell) for definition in definitions):
                locations = ", ".join(
                    f"{definition.path.relative_to(REPO_ROOT)}:{definition.line}"
                    for definition in definitions
                )
                missing.append(f"  {symbol} [{cell}]: {locations or 'no definition'}")
    assert not missing, (
        'Required runtime `pub extern "C"` definitions lack an unmangled owner '
        "in their feature/target cell, so the linker cannot export them:\n"
        + "\n".join(missing)
    )


def test_extracted_runtime_crates_do_not_duplicate_wrapper_exports() -> None:
    defined = _extern_c_fn_definitions()
    targets = _lzma_target_owners()
    duplicates = []
    for symbol in sorted(_required_export_symbols() & defined.keys()):
        for cell, definitions in _owner_cells(symbol, defined[symbol], targets).items():
            runtime = [
                definition
                for definition in definitions
                if definition.exports_in(cell)
                and _is_molt_runtime_crate_source(definition.path)
            ]
            extracted = [
                definition
                for definition in definitions
                if definition.exports_in(cell)
                and not _is_molt_runtime_crate_source(definition.path)
            ]
            if runtime and extracted:
                locations = ", ".join(
                    f"{definition.path.relative_to(REPO_ROOT)}:{definition.line}"
                    for definition in runtime + extracted
                )
                duplicates.append(f"  {symbol} [{cell}]: {locations}")
    assert not duplicates, (
        "Runtime wrappers and extracted implementations both own no_mangle "
        "in the same feature/target cell, creating duplicate symbols:\n"
        + "\n".join(duplicates)
    )


@pytest.fixture
def export_owner_sources(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> dict[str, Path]:
    # Independent Rust fragments exercise the actual scanner and assertions.
    # These are source-guard controls; they are never compiled as runtime code.
    runtime = tmp_path / "runtime"
    files = {
        "native-module": (
            "molt-runtime-compression/src/lib.rs",
            '#[cfg(not(target_arch = "wasm32"))]\npub mod lzma;\n',
        ),
        "wasm-module": (
            "molt-runtime/src/builtins/mod.rs",
            '#[cfg(all(feature = "stdlib_compression", target_arch = "wasm32"))]\n'
            '#[path = "lzma_wasm.rs"]\npub(crate) mod lzma;\n',
        ),
        "raw": (
            "molt-runtime-compression/src/zlib.rs",
            '#[unsafe(no_mangle)]\npub extern "C" fn molt_deflate_raw(a: u64, b: u64) -> u64 { 0 }\n',
        ),
        "fallback": (
            "molt-runtime/src/builtins/micro_stubs.rs",
            '#[cfg(not(feature = "stdlib_compression"))]\n#[unsafe(no_mangle)]\n'
            'pub extern "C" fn molt_deflate_raw(a: u64, b: u64) -> u64 { 0 }\n',
        ),
        "native-lzma": (
            "molt-runtime-compression/src/lzma.rs",
            '#[unsafe(no_mangle)]\npub extern "C" fn molt_lzma_format_auto() -> u64 { 0 }\n',
        ),
        "wasm-lzma": (
            "molt-runtime/src/builtins/lzma_wasm.rs",
            '#[unsafe(no_mangle)]\npub extern "C" fn molt_lzma_format_auto() -> u64 { 0 }\n',
        ),
    }
    paths = {}
    for role, (relative, text) in files.items():
        path = runtime / relative
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_text(text, encoding="utf-8")
        paths[role] = path
    monkeypatch.setitem(globals(), "REPO_ROOT", tmp_path)
    monkeypatch.setitem(globals(), "RUNTIME_ROOT", runtime)
    return paths


@pytest.mark.parametrize(
    ("role", "symbol", "cell"),
    [
        ("raw", "molt_deflate_raw", "feature-enabled/wasm32"),
        ("native-lzma", "molt_lzma_format_auto", "feature-enabled/native"),
    ],
)
def test_export_owner_guard_does_not_borrow_incompatible_providers(
    export_owner_sources: dict[str, Path], role: str, symbol: str, cell: str
) -> None:
    test_required_wasm_runtime_exports_have_no_mangle()
    test_extracted_runtime_crates_do_not_duplicate_wrapper_exports()
    path = export_owner_sources[role]
    path.write_text(
        path.read_text(encoding="utf-8").replace("#[unsafe(no_mangle)]\n", ""),
        encoding="utf-8",
    )
    with pytest.raises(AssertionError) as missing:
        test_required_wasm_runtime_exports_have_no_mangle()
    assert f"{symbol} [{cell}]" in str(missing.value)
    assert "stdlib_compression-disabled" not in str(missing.value)
    if role == "native-lzma":
        assert "feature-enabled/wasm32" not in str(missing.value)


def test_export_owner_guard_rejects_unmatched_negative_feature(
    export_owner_sources: dict[str, Path],
) -> None:
    path = export_owner_sources["fallback"]
    path.write_text(
        path.read_text(encoding="utf-8").replace(
            "stdlib_compression", "unmatched_feature"
        ),
        encoding="utf-8",
    )
    with pytest.raises(AssertionError, match="unmatched disabled-feature owner"):
        test_required_wasm_runtime_exports_have_no_mangle()


@pytest.mark.parametrize("role", ["native-module", "wasm-module"])
def test_export_owner_guard_rejects_target_gate_drift(
    export_owner_sources: dict[str, Path], role: str
) -> None:
    path = export_owner_sources[role]
    path.write_text(
        path.read_text(encoding="utf-8").replace(
            'target_arch = "wasm32"', 'target_arch = "aarch64"'
        ),
        encoding="utf-8",
    )
    with pytest.raises(AssertionError, match="LZMA target gate changed"):
        test_required_wasm_runtime_exports_have_no_mangle()


def test_export_owner_guard_rejects_real_wrapper_duplicate(
    export_owner_sources: dict[str, Path],
) -> None:
    path = export_owner_sources["fallback"].with_name("wrapper.rs")
    path.write_text(
        '#[cfg(feature = "stdlib_compression")]\n#[unsafe(no_mangle)]\n'
        'pub extern "C" fn molt_deflate_raw(a: u64, b: u64) -> u64 { 0 }\n',
        encoding="utf-8",
    )
    with pytest.raises(AssertionError, match="creating duplicate symbols") as duplicate:
        test_extracted_runtime_crates_do_not_duplicate_wrapper_exports()
    assert "molt_deflate_raw [feature-enabled/wasm32]" in str(duplicate.value)
    assert "stdlib_compression-disabled" not in str(duplicate.value)


def test_export_owner_guard_honors_wasm_conditional_symbol_name(
    export_owner_sources: dict[str, Path],
) -> None:
    path = export_owner_sources["raw"]
    path.write_text(
        path.read_text(encoding="utf-8").replace(
            "#[unsafe(no_mangle)]",
            '#[cfg_attr(target_arch = "wasm32", unsafe(no_mangle))]',
        ),
        encoding="utf-8",
    )
    test_required_wasm_runtime_exports_have_no_mangle()
    test_extracted_runtime_crates_do_not_duplicate_wrapper_exports()
    path.write_text(
        path.read_text(encoding="utf-8").replace(
            'target_arch = "wasm32"', 'target_arch = "aarch64"'
        ),
        encoding="utf-8",
    )
    with pytest.raises(
        AssertionError, match=r"molt_deflate_raw \[feature-enabled/wasm32\]"
    ):
        test_required_wasm_runtime_exports_have_no_mangle()


def test_export_owner_guard_does_not_promote_conditional_name_to_native(
    export_owner_sources: dict[str, Path],
) -> None:
    path = export_owner_sources["native-lzma"]
    path.write_text(
        path.read_text(encoding="utf-8").replace(
            "#[unsafe(no_mangle)]",
            '#[cfg_attr(target_arch = "wasm32", unsafe(no_mangle))]',
        ),
        encoding="utf-8",
    )
    with pytest.raises(
        AssertionError, match=r"molt_lzma_format_auto \[feature-enabled/native\]"
    ):
        test_required_wasm_runtime_exports_have_no_mangle()
