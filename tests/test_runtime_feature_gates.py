"""Guards for the generated runtime intrinsic availability table.

``src/molt/_runtime_feature_gates.py`` projects each intrinsic's ``# cfg:``
declaration in ``runtime/molt-runtime/src/intrinsics/manifest.pyi``. The
generated resolvers prove at compile time that each definition's cfg equals its
declaration, so every declared feature removes its symbols from a runtime built
without it, and reachability selects or refuses that feature (HF-487).

These tests cover the projection's semantics and the source rule the compile
check cannot see: no runtime export may stand in for a disabled feature.
"""

from __future__ import annotations

import re
import tomllib
from pathlib import Path

import pytest

from molt._runtime_feature_gates import (
    RUNTIME_FEATURES,
    feature_for_symbol,
    features_unsupported_on_target,
    runtime_symbol_available_on_target,
)

ROOT = Path(__file__).resolve().parents[1]
RUNTIME_CRATE = ROOT / "runtime" / "molt-runtime"


def test_declared_features_are_runtime_cargo_features() -> None:
    cargo = tomllib.loads((RUNTIME_CRATE / "Cargo.toml").read_text(encoding="utf-8"))
    assert RUNTIME_FEATURES <= set(cargo["features"])


def test_symbol_features_are_exact_not_prefix_attributed() -> None:
    # One spelling family, two facts: networking and pure helpers. The core
    # compile() builtin needs no feature; the ast module does.
    assert feature_for_symbol("molt_socket_new") == "stdlib_net"
    assert feature_for_symbol("molt_socket_htons") is None
    assert feature_for_symbol("molt_compile_builtin") is None
    assert feature_for_symbol("molt_ast_parse") == "stdlib_ast"
    assert feature_for_symbol("molt_signal_default_int_handler") is None
    assert feature_for_symbol("molt_hash_builtin") is None
    for symbol in (
        "molt_hash_new",
        "molt_hash_update",
        "molt_hash_copy",
        "molt_hash_digest",
    ):
        assert feature_for_symbol(symbol) == "stdlib_crypto"


def test_sqlite_target_availability_is_generated_for_profiles_and_admission() -> None:
    assert runtime_symbol_available_on_target(
        "molt_sqlite3_connect",
        target_triple="x86_64-unknown-linux-gnu",
        target_os="linux",
    )
    assert not runtime_symbol_available_on_target(
        "molt_sqlite3_connect", target_triple="wasm32-wasip1", target_os="wasi"
    )
    assert not runtime_symbol_available_on_target(
        "molt_sqlite3_connect", target_triple="wasm32-unknown-unknown", target_os="none"
    )
    assert features_unsupported_on_target(
        target_arch="wasm32", target_os="wasi"
    ) == frozenset({"sqlite"})


@pytest.mark.parametrize(
    ("triple", "target_os", "available"),
    [
        ("aarch64-apple-darwin", "macos", True),
        ("x86_64-unknown-linux-gnu", "linux", True),
        ("wasm32-wasip1", "wasi", True),
        ("wasm32-unknown-unknown", "none", True),
        ("x86_64-pc-windows-msvc", "windows", False),
    ],
)
def test_networking_has_no_windows_provider(
    triple: str, target_os: str, available: bool
) -> None:
    for symbol in ("molt_socketpair", "molt_socket_new", "molt_ssl_wrap_socket"):
        assert (
            runtime_symbol_available_on_target(
                symbol, target_triple=triple, target_os=target_os
            )
            is available
        )
    unsupported = features_unsupported_on_target(
        target_arch=triple.split("-", 1)[0], target_os=target_os
    )
    assert ("stdlib_net" in unsupported) is not available


@pytest.mark.parametrize(
    "triple,target_os,expected",
    [
        ("x86_64-pc-windows-msvc", "windows", True),
        ("x86_64-pc-windows-gnu", "windows", True),
        ("aarch64-pc-windows-msvc", "windows", True),
        ("x86_64-unknown-linux-gnu", "linux", False),
        ("aarch64-unknown-linux-musl", "linux", False),
        ("aarch64-apple-darwin", "macos", False),
        ("wasm32-wasip1", "wasi", False),
        ("wasm32-unknown-unknown", "none", False),
        (None, "windows", True),
        (None, "linux", False),
        (None, None, False),
        ("x86_64-pc-windows-msvc", None, False),
    ],
)
def test_windows_locale_provider_requires_actual_target_os(
    triple: str | None, target_os: str | None, expected: bool
) -> None:
    assert (
        runtime_symbol_available_on_target(
            "molt_locale_getdefaultlocale", target_triple=triple, target_os=target_os
        )
        is expected
    )
    for symbol in (
        "molt_locale_getencoding",
        "molt_locale_setlocale",
        "molt_locale_constants",
    ):
        assert runtime_symbol_available_on_target(
            symbol, target_triple=triple, target_os=target_os
        )


def test_high_level_gpu_intrinsics_require_gpu_primitives() -> None:
    for symbol in (
        "molt_gpu_linear_contiguous",
        "molt_gpu_tensor__zeros",
        "molt_gpu_interop_decode_f16_bytes_to_f32",
        "molt_gpu_prim_create_tensor",
    ):
        assert feature_for_symbol(symbol) == "molt_gpu_primitives"


# --- No runtime export stands in for a disabled feature ---------------------

# A cfg predicate that holds only when a feature, or the networking provider a
# feature selects, is off. Target-only alternatives (`not(target_arch = ...)`)
# are separate providers, not stand-ins.
_FEATURE_FACT = re.compile(
    r"feature\s*=|\bmolt_has_net_io\b|\bmolt_wasm_net\b|\bmolt_net\b"
)


def _negates_feature(cfg: str) -> bool:
    """Whether some `not(...)` in *cfg* negates a feature-derived fact."""
    start = cfg.find("not(")
    while start != -1:
        depth, index = 0, start + len("not")
        for index in range(start + len("not"), len(cfg)):
            depth += {"(": 1, ")": -1}.get(cfg[index], 0)
            if depth == 0:
                break
        if _FEATURE_FACT.search(cfg[start:index]):
            return True
        start = cfg.find("not(", start + 1)
    return False


_ATTRIBUTE = re.compile(r"^\s*#\[(.*)\]\s*$")
_MOD_DECL = re.compile(r"^\s*(?:pub(?:\([^)]*\))?\s+)?mod\s+(\w+)\s*;")
_EXPORT = re.compile(r"^\s*pub\s+(?:unsafe\s+)?extern\s+\"C\"\s+fn\s+(\w+)")
_GENERATED_RESOLVERS = RUNTIME_CRATE / "src" / "intrinsics" / "generated_resolvers"


def _cfgs(attributes: list[str]) -> list[str]:
    return [attribute for attribute in attributes if attribute.startswith("cfg(")]


def _feature_off_exports(
    path: Path, inherited: list[str], found: list[str], root: Path, crate_src: Path
) -> None:
    """Record exports in *path* (and its modules) defined only when a feature is off.

    A satellite crate's own source compiled inline (``#[path]`` into another
    crate) while its crate feature is off is the same implementation linked
    from a different crate, not a stand-in, so files outside *crate_src* are
    walked for nested modules only.
    """
    shared_source = not path.resolve().is_relative_to(crate_src.resolve())
    lines = path.read_text(encoding="utf-8").splitlines()
    attributes: list[str] = []
    for line in lines:
        if match := _ATTRIBUTE.match(line):
            attributes.append(match.group(1))
            continue
        if line.strip().startswith("//") or not line.strip():
            continue
        cfgs = inherited + _cfgs(attributes)
        if (decl := _MOD_DECL.match(line)) is not None:
            explicit = next(
                (
                    attribute[len('path = "') : -1]
                    for attribute in attributes
                    if attribute.startswith('path = "')
                ),
                None,
            )
            base = (
                path.parent
                if path.name in {"lib.rs", "mod.rs"}
                else path.with_suffix("")
            )
            candidates = (
                [path.parent / explicit]
                if explicit is not None
                else [base / f"{decl.group(1)}.rs", base / decl.group(1) / "mod.rs"]
            )
            for child in candidates:
                if (
                    child.is_file()
                    and child.resolve().parent != _GENERATED_RESOLVERS.resolve()
                ):
                    _feature_off_exports(child, cfgs, found, root, crate_src)
                    break
        elif (
            export := _EXPORT.match(line)
        ) is not None and "unsafe(no_mangle)" in " ".join(attributes):
            if not shared_source and any(_negates_feature(cfg) for cfg in cfgs):
                found.append(f"{path.relative_to(root)}: {export.group(1)} {cfgs}")
        attributes = []


def test_no_runtime_export_stands_in_for_a_disabled_feature() -> None:
    """A feature-off build lacks the feature's symbols; nothing replaces them.

    The generated resolvers' absence probes prove declared intrinsics are
    absent outside their cfg. This walks every runtime crate's module tree for
    the remaining shape: an export compiled only when a feature (or the
    networking provider it selects) is off, which would move a missing
    feature from build time to run time.
    """
    found: list[str] = []
    for lib in sorted((ROOT / "runtime").glob("*/src/lib.rs")):
        _feature_off_exports(lib, [], found, ROOT, lib.parent)
    assert found == []


def test_feature_off_export_scan_detects_module_and_item_gates(tmp_path: Path) -> None:
    src = tmp_path / "src"
    src.mkdir()
    (src / "lib.rs").write_text(
        '#[cfg(not(any(molt_has_net_io, target_arch = "wasm32")))]\nmod stubs;\nmod real;\n',
        encoding="utf-8",
    )
    (src / "stubs.rs").write_text(
        '#[unsafe(no_mangle)]\npub extern "C" fn molt_socket_new() -> u64 { 0 }\n',
        encoding="utf-8",
    )
    (src / "real.rs").write_text(
        '#[cfg(not(feature = "stdlib_ast"))]\n#[unsafe(no_mangle)]\n'
        'pub extern "C" fn molt_compile_builtin() -> u64 { 0 }\n'
        '#[cfg(not(target_arch = "wasm32"))]\n#[unsafe(no_mangle)]\n'
        'pub extern "C" fn molt_native_only() -> u64 { 0 }\n',
        encoding="utf-8",
    )
    found: list[str] = []
    _feature_off_exports(src / "lib.rs", [], found, tmp_path, src)
    assert [entry.split(":")[1].split()[0] for entry in found] == [
        "molt_socket_new",
        "molt_compile_builtin",
    ]


def _mod_builtins_cfg_prefix(lib_rs: str) -> str | None:
    """Return the `#[cfg(...)]` attribute gating `mod builtins;`, or None.

    Matches an optional attribute line immediately preceding the module
    declaration. When `mod builtins;` is declared unconditionally (the correct
    core-authority shape) this returns None.
    """
    match = re.search(
        r"(?:#\[cfg\(([^\]]+)\)\]\s*\n)?[ \t]*mod builtins;",
        lib_rs,
    )
    assert match is not None, "lib.rs must declare `mod builtins;`"
    return match.group(1)


def _expand_cargo_feature_names(feature: str, features: dict) -> set[str]:
    """Transitively expand *feature* to the molt-runtime feature names reached.

    Skips ``dep:crate`` and ``crate/feat`` / ``crate?/feat`` cross-crate
    activations (they select dependency features, not molt-runtime's own).
    Includes the seed feature itself so a directly-triggering seed is caught.
    """
    reached: set[str] = set()
    stack = [feature]
    while stack:
        current = stack.pop()
        if current in reached:
            continue
        reached.add(current)
        for entry in features.get(current, []):
            if entry.startswith("dep:") or "/" in entry:
                continue
            stack.append(entry)
    return reached


def test_core_builtins_authority_compiles_unconditionally() -> None:
    """`builtins` is the crate-root CORE authority and must never be gated.

    `runtime/molt-runtime/src/builtins/mod.rs` re-exports the core runtime
    symbols (`raise_exception`, `to_i64`/`to_f64`, `molt_type_new`,
    `attr_lookup_ptr`, `ellipsis_bits`, the whole exception machinery) that
    ~230 files reference UNCONDITIONALLY via `crate::…`. Gating the module
    behind a derived `molt_runtime_builtins` cfg (or any other cfg) drops it for
    feature subsets that enable no `stdlib_*`/`builtin_*` feature — e.g. the
    wasm-browser split-runtime numpy/scipy compute plan — producing thousands of
    `E0433 cannot find 'builtins' in 'crate'` / `E0432 unresolved import`
    errors. Only the OPTIONAL stdlib submodules inside `builtins/mod.rs` may be
    feature-gated (they already are, internally); the core module is
    unconditional by construction.
    """

    lib_rs = (RUNTIME_CRATE / "src" / "lib.rs").read_text(encoding="utf-8")
    build_rs = (RUNTIME_CRATE / "build.rs").read_text(encoding="utf-8")

    cfg = _mod_builtins_cfg_prefix(lib_rs)
    assert cfg is None, (
        "`mod builtins;` must compile UNCONDITIONALLY (it is the crate-root "
        "core authority), but it is gated behind "
        f"`#[cfg({cfg})]`. Remove the outer gate; keep only the per-feature "
        "gates on the OPTIONAL stdlib submodules inside builtins/mod.rs."
    )

    # The vestigial derived cfg must be fully removed, not merely unreferenced,
    # so a future edit cannot silently re-gate the core module.
    assert "molt_runtime_builtins" not in lib_rs
    assert "molt_runtime_builtins" not in build_rs


def test_zero_stdlib_wasm_browser_profile_keeps_core_builtins() -> None:
    """Regression for the witness blocker: the MINIMAL zero-stdlib wasm-browser
    split-runtime feature set must still compile the core builtins authority.

    The browser split-runtime numpy/scipy compute runtime is built with
    ``cargo rustc -p molt-runtime --target wasm32-wasip1 --lib
    --no-default-features --features wasm_freestanding`` — a feature set that
    enables NONE of the ``stdlib_*``/``builtin_*``/``sqlite``/
    ``molt_gpu_primitives`` features. This test asserts that fact from
    Cargo.toml (so the reproduction stays honest), then asserts the core
    ``builtins`` module is not gated out of such a build. Under the previous
    ``#[cfg(molt_runtime_builtins)]`` gating this feature set dropped
    ``mod builtins`` and produced 2824 compile errors that reached the witness;
    the existing gate tests never exercised a zero-stdlib profile, which is the
    coverage gap this closes.
    """

    cargo = tomllib.loads((RUNTIME_CRATE / "Cargo.toml").read_text(encoding="utf-8"))
    features = cargo.get("features", {})

    def _triggers_builtins(feature: str) -> bool:
        upper = feature.upper()
        return (
            upper.startswith("STDLIB_")
            or upper.startswith("BUILTIN_")
            or upper in {"SQLITE", "MOLT_GPU_PRIMITIVES"}
        )

    reached = _expand_cargo_feature_names("wasm_freestanding", features)
    triggering = sorted(f for f in reached if _triggers_builtins(f))
    assert triggering == [], (
        "wasm_freestanding unexpectedly chains builtins-triggering features "
        f"{triggering}; the zero-stdlib reproduction is no longer minimal — "
        "update this test to a feature set that truly enables no stdlib_/"
        "builtin_ feature."
    )

    lib_rs = (RUNTIME_CRATE / "src" / "lib.rs").read_text(encoding="utf-8")
    assert _mod_builtins_cfg_prefix(lib_rs) is None, (
        "The zero-stdlib wasm-browser split-runtime feature set enables no "
        "builtins-triggering feature, so any cfg gating `mod builtins;` drops "
        "the crate-root core authority and reintroduces the witness blocker."
    )
