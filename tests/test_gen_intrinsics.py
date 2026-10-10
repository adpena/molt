from __future__ import annotations

import importlib.util
from pathlib import Path
from types import SimpleNamespace

import pytest


ROOT = Path(__file__).resolve().parents[1]
GEN_INTRINSICS = ROOT / "tools" / "gen_intrinsics.py"


def _load_gen_intrinsics_module():
    spec = importlib.util.spec_from_file_location(
        "molt_test_gen_intrinsics", GEN_INTRINSICS
    )
    assert spec is not None
    assert spec.loader is not None
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


def test_backend_symbol_overrides_file_is_removed() -> None:
    module = _load_gen_intrinsics_module()
    overrides = ROOT / "runtime/molt-backend/src/intrinsic_symbol_overrides.rs"
    assert overrides in module.RETIRED_OUTPUTS
    for retired in module.RETIRED_OUTPUTS:
        assert not retired.exists()


def test_async_sleep_intrinsic_symbol_matches_public_name() -> None:
    module = _load_gen_intrinsics_module()
    _raw, entries = module._load_manifest()
    symbols = {name: symbol for name, symbol, _arity in entries}
    assert symbols["molt_async_sleep"] == "molt_async_sleep"


def test_all_stdlib_literal_intrinsic_requests_are_manifested() -> None:
    from molt.stdlib_intrinsic_policy import intrinsic_names_from_source

    module = _load_gen_intrinsics_module()
    _raw, entries = module._load_manifest()
    declared = {entry.name for entry in entries}
    stdlib = ROOT / "src" / "molt" / "stdlib"
    missing = {
        path.relative_to(stdlib).as_posix(): sorted(required - declared)
        for path in sorted(stdlib.rglob("*.py"))
        if (required := intrinsic_names_from_source(path.read_text(encoding="utf-8")))
        - declared
    }
    assert missing == {}, (
        f"stdlib intrinsic requests lack canonical declarations: {missing}"
    )


def test_manifest_literal_defaults_feed_generated_intrinsic_metadata() -> None:
    module = _load_gen_intrinsics_module()
    _raw, entries = module._load_manifest()

    by_name = {entry.name: entry for entry in entries}
    length_hint = by_name["molt_operator_length_hint"]

    assert length_hint.arity == 2
    assert length_hint.defaults == ("IntrinsicDefaultValue::Int(0)",)
    assert by_name["molt_getframe"].arity == 1
    assert by_name["molt_getframe"].defaults == ("IntrinsicDefaultValue::Int(0)",)
    for name in ("molt_require_intrinsic_runtime", "molt_load_intrinsic_runtime"):
        assert by_name[name].arity == 2
        assert by_name[name].defaults == ("IntrinsicDefaultValue::None",)
    assert by_name["molt_runtime_active_runtime"].arity == 0
    assert by_name["molt_runtime_active_runtime"].defaults == ()

    generated = (ROOT / "runtime/molt-runtime/src/intrinsics/generated.rs").read_text(
        encoding="utf-8"
    )
    assert "pub(crate) enum IntrinsicDefaultValue" in generated
    assert "molt_operator_length_hint" in generated
    assert "defaults: &[IntrinsicDefaultValue::Int(0)]," in generated

    registry = (ROOT / "runtime/molt-runtime/src/intrinsics/registry.rs").read_text(
        encoding="utf-8"
    )
    assert (
        "let defaults = materialize_intrinsic_defaults(_py, default_values)?;"
        in registry
    )
    assert "build_runtime_function(_py, fn_ptr, arity, &defaults)" in registry
    assert "crate::builtins::methods::alloc_builtin_function_with_defaults(" in registry
    assert "fn attach_function_defaults" not in registry
    assert (
        registry.count("build_intrinsic_func(_py, fn_ptr, spec.arity, spec.defaults)")
        == 2
    ), "eager and lazy intrinsic registration must attach manifest defaults"


def test_ssl_intrinsic_abi_is_not_profile_gated() -> None:
    module = _load_gen_intrinsics_module()
    _raw, entries = module._load_manifest()

    ssl_symbols = sorted(
        {symbol for _name, symbol, _arity in entries if symbol.startswith("molt_ssl_")}
    )
    assert ssl_symbols
    for symbol in ssl_symbols:
        assert module._feature_gate_for_symbol(symbol) is None

    assert module._feature_gate_for_symbol("molt_http_client_execute") == "stdlib_http"
    assert module._feature_gate_for_symbol("molt_html_escape") == "stdlib_text"
    assert module._feature_gate_for_symbol("molt_unicodedata_category") == "stdlib_text"
    assert (
        module._feature_gate_for_symbol("molt_zoneinfo_available_timezones")
        == "stdlib_zoneinfo"
    )

    generated = (
        ROOT / "runtime/molt-runtime/src/intrinsics/generated_resolvers/ssl_resolver.rs"
    ).read_text(encoding="utf-8")
    ssl_block = generated.split("pub(super) fn resolve_symbol", 1)[1].split(
        "        _ => None,",
        1,
    )[0]
    assert '#[cfg(feature = "stdlib_net")]' not in ssl_block


def test_intrinsic_target_availability_is_toml_owned() -> None:
    module = _load_gen_intrinsics_module()

    assert module._target_arch_exclusions_for_symbol("molt_db_query_obj") == ()
    assert module._target_arch_exclusions_for_symbol("molt_sqlite3_connect") == (
        "wasm32",
    )
    assert module._cfg_gate_for_symbol("molt_sqlite3_connect") == (
        'all(feature = "sqlite", not(target_arch = "wasm32"))'
    )
    assert module._FEATURE_TARGET_ARCH_EXCLUSIONS == [("sqlite", ("wasm32",))]

    generated = (
        ROOT
        / "runtime/molt-runtime/src/intrinsics/generated_resolvers/sqlite_resolver.rs"
    ).read_text(encoding="utf-8")
    assert '#[cfg(all(feature = "sqlite", not(target_arch = "wasm32")))]' in generated


def test_exact_builtin_category_precedes_feature_and_target_prefixes(
    tmp_path: Path,
) -> None:
    module = _load_gen_intrinsics_module()
    categories = tmp_path / "categories.toml"
    categories.write_text(
        """
[builtin]
functions = ["molt_hash_builtin", "molt_hash_new_builtin"]
type_constructors = ["molt_hash_builtin"]

[stdlib.crypto]
prefixes = ["hash_"]
feature = "stdlib_crypto"
unsupported_target_arches = ["wasm32"]
""".strip(),
        encoding="utf-8",
    )

    availability = module.load_intrinsic_availability(categories)

    assert availability.builtin_symbols == (
        "molt_hash_builtin",
        "molt_hash_new_builtin",
    )
    for symbol in availability.builtin_symbols:
        assert availability.feature_gate_for_symbol(symbol) is None
        assert availability.target_arch_exclusions_for_symbol(symbol) == ()
        assert availability.symbol_available_on_target_arch(symbol, "wasm32")
    assert availability.feature_gate_for_symbol("molt_hash_new") == "stdlib_crypto"
    assert availability.target_arch_exclusions_for_symbol("molt_hash_new") == (
        "wasm32",
    )


def test_all_canonical_builtin_symbols_are_ungated() -> None:
    module = _load_gen_intrinsics_module()
    builtin_symbols, _internal_prefixes, _stdlib_modules = module._load_categories()

    assert builtin_symbols
    assert set(builtin_symbols) == set(module._RUNTIME_AVAILABILITY.builtin_symbols)
    for symbol in builtin_symbols:
        assert module._feature_gate_for_symbol(symbol) is None
        assert module._target_arch_exclusions_for_symbol(symbol) == ()

    assert module._feature_gate_for_symbol("molt_hash_builtin") is None
    for symbol in (
        "molt_hash_new",
        "molt_hash_update",
        "molt_hash_copy",
        "molt_hash_digest",
        "molt_hash_drop",
    ):
        assert module._feature_gate_for_symbol(symbol) == "stdlib_crypto"


def test_runtime_feature_gates_are_generated_from_categories() -> None:
    module = _load_gen_intrinsics_module()
    gates_path = ROOT / "src/molt/_runtime_feature_gates.py"
    spec = importlib.util.spec_from_file_location(
        "molt_test_runtime_feature_gates", gates_path
    )
    assert spec is not None
    assert spec.loader is not None
    gates_module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(gates_module)

    expected_gates = module._load_runtime_feature_gates_from_categories()
    assert gates_module.RUNTIME_BUILTIN_SYMBOLS == frozenset(
        module._RUNTIME_AVAILABILITY.builtin_symbols
    )
    assert list(gates_module.RUNTIME_FEATURE_GATES) == expected_gates
    assert tuple(sorted(gates_module.LINK_AFFECTING_FEATURES)) == (
        module._mechanically_derived_link_affecting_features(expected_gates)
    )
    assert "# @generated by tools/gen_intrinsics.py. DO NOT EDIT." in (
        gates_path.read_text(encoding="utf-8")
    )


def test_intrinsic_symbol_table_is_generated_from_manifest() -> None:
    module = _load_gen_intrinsics_module()
    _raw, entries = module._load_manifest()
    symbols_path = ROOT / "src/molt/_intrinsic_symbols.py"
    spec = importlib.util.spec_from_file_location(
        "molt_test_intrinsic_symbols", symbols_path
    )
    assert spec is not None
    assert spec.loader is not None
    symbols_module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(symbols_module)

    expected = {entry.name: entry.symbol for entry in entries}
    assert symbols_module.INTRINSIC_SYMBOL_NAMES == expected
    assert (
        symbols_module.intrinsic_runtime_symbol_name("molt_ssl_context_new")
        == "molt_ssl_context_new"
    )
    assert symbols_module.intrinsic_runtime_symbol_name("not_manifested") == (
        "not_manifested"
    )
    assert "# @generated by tools/gen_intrinsics.py" in symbols_path.read_text(
        encoding="utf-8"
    )


def test_toml_prefixes_do_not_fall_back_to_extra_prefix_modules() -> None:
    module = _load_gen_intrinsics_module()
    _builtin_symbols, _internal_prefixes, stdlib_modules = module._load_categories()
    toml_prefixes = [
        (prefix, module_name)
        for module_name, prefixes in stdlib_modules.items()
        for prefix in prefixes
    ]
    shadowed = [
        (extra_prefix, extra_module, toml_prefix, toml_module)
        for extra_prefix, extra_module in module._EXTRA_PREFIX_MODULES
        for toml_prefix, toml_module in toml_prefixes
        if extra_prefix.startswith(toml_prefix)
    ]
    assert shadowed == []


def test_generated_resolvers_are_split_from_manifest_table() -> None:
    """Resolver address-taking is generated into per-module Rust files."""
    generated_path = ROOT / "runtime/molt-runtime/src/intrinsics/generated.rs"
    resolver_root = ROOT / "runtime/molt-runtime/src/intrinsics/generated_resolvers"
    generated = generated_path.read_text(encoding="utf-8")
    resolver_mod = (resolver_root / "mod.rs").read_text(encoding="utf-8")
    core_resolver = (resolver_root / "core_resolver.rs").read_text(encoding="utf-8")
    ssl_resolver = (resolver_root / "ssl_resolver.rs").read_text(encoding="utf-8")

    assert (
        '#[path = "generated_resolvers/mod.rs"]\nmod generated_resolvers;' in generated
    )
    assert "pub(crate) use generated_resolvers::resolve_symbol;" in generated
    assert "IntrinsicSpec {" in generated
    assert "fn resolve_core_symbol" not in generated
    assert "mod core_resolver;" in resolver_mod
    assert "pub(crate) fn resolve_symbol" in resolver_mod
    assert "molt_capabilities_trusted" in core_resolver
    assert "molt_ssl_context_new" not in core_resolver
    assert "molt_ssl_context_new" in ssl_resolver

    html_resolver = (resolver_root / "html_resolver.rs").read_text(encoding="utf-8")
    unicodedata_resolver = (resolver_root / "unicodedata_resolver.rs").read_text(
        encoding="utf-8"
    )
    zoneinfo_resolver = (resolver_root / "zoneinfo_resolver.rs").read_text(
        encoding="utf-8"
    )
    assert '#[cfg(feature = "stdlib_text")]' in html_resolver
    assert '#[cfg(feature = "stdlib_text")]' in unicodedata_resolver
    assert '#[cfg(feature = "stdlib_zoneinfo")]' in zoneinfo_resolver


def test_stringprep_category_is_toml_owned() -> None:
    module = _load_gen_intrinsics_module()
    builtin_symbols, internal_prefixes, stdlib_modules = module._load_categories()

    assert stdlib_modules["stringprep"] == ["molt_stringprep_"]
    assert ("molt_stringprep_", "stringprep") not in module._EXTRA_PREFIX_MODULES
    assert "feature" not in module.LEAF_RESOLVER_REGISTRIES["stringprep"]
    assert (
        module._classify_symbol(
            "molt_stringprep_in_table",
            builtin_symbols,
            internal_prefixes,
            stdlib_modules,
        )
        == "stringprep"
    )
    assert (
        module._leaf_resolver_feature_gate("stringprep", ["molt_stringprep_in_table"])
        == "stdlib_stringprep"
    )


def test_stringprep_resolver_is_leaf_owned() -> None:
    resolver_root = ROOT / "runtime/molt-runtime/src/intrinsics/generated_resolvers"
    facade_resolver = (resolver_root / "stringprep_resolver.rs").read_text(
        encoding="utf-8"
    )
    leaf_resolver = (
        ROOT / "runtime/molt-runtime-stringprep/src/intrinsics_generated.rs"
    ).read_text(encoding="utf-8")

    assert "molt_runtime_stringprep::intrinsics_generated::resolve_symbol_with" in (
        facade_resolver
    )
    assert "crate::builtins::functions::runtime_fn_addr" in facade_resolver
    assert "crate::molt_stringprep_in_table as *const ()" not in facade_resolver
    assert "pub fn resolve_symbol_with" in leaf_resolver
    assert "crate::stringprep::molt_stringprep_in_table as *const ()" in leaf_resolver
    assert "molt_runtime_stringprep::stringprep::molt_stringprep_in_table" in (
        leaf_resolver
    )


def test_collections_categories_are_toml_owned() -> None:
    module = _load_gen_intrinsics_module()
    builtin_symbols, internal_prefixes, stdlib_modules = module._load_categories()

    assert stdlib_modules["collections"] == [
        "molt_namedtuple_",
        "molt_ordereddict_",
        "molt_defaultdict_",
        "molt_deque_",
        "molt_chainmap_",
    ]
    for prefix in (
        "molt_namedtuple_",
        "molt_ordereddict_",
        "molt_defaultdict_",
        "molt_deque_",
        "molt_chainmap_",
    ):
        assert all(prefix != extra[0] for extra in module._EXTRA_PREFIX_MODULES)

    assert (
        module._classify_symbol(
            "molt_deque_append",
            builtin_symbols,
            internal_prefixes,
            stdlib_modules,
        )
        == "collections"
    )
    assert (
        module._classify_symbol(
            "molt_collections_abc_runtime_types",
            builtin_symbols,
            internal_prefixes,
            stdlib_modules,
        )
        == "core"
    )


def test_collections_resolvers_are_leaf_owned() -> None:
    resolver_root = ROOT / "runtime/molt-runtime/src/intrinsics/generated_resolvers"
    facade_collections = (resolver_root / "collections_resolver.rs").read_text(
        encoding="utf-8"
    )
    leaf_root = ROOT / "runtime/molt-runtime-collections/src/intrinsics_generated"
    leaf_index = (leaf_root / "mod.rs").read_text(encoding="utf-8")
    leaf_collections = (leaf_root / "collections_resolver.rs").read_text(
        encoding="utf-8"
    )
    assert "pub mod collections_resolver;" in leaf_index

    assert (
        "molt_runtime_collections::intrinsics_generated::collections_resolver"
        "::resolve_symbol_with" in facade_collections
    )
    assert "crate::molt_deque_append as *const ()" not in facade_collections
    assert "crate::builtins::functions::runtime_fn_addr" in facade_collections

    assert "crate::collections_ext::molt_deque_append as *const ()" in (
        leaf_collections
    )
    assert "molt_runtime_collections::collections_ext::molt_deque_append" in (
        leaf_collections
    )


def test_serial_resolvers_are_leaf_owned() -> None:
    resolver_root = ROOT / "runtime/molt-runtime/src/intrinsics/generated_resolvers"
    leaf_root = ROOT / "runtime/molt-runtime-serial/src/intrinsics_generated"
    leaf_index = (leaf_root / "mod.rs").read_text(encoding="utf-8")

    cases = {
        "base64": ("stdlib_serial", "base64_mod", "molt_base64_b64encode"),
        "binascii": ("stdlib_serial", "binascii", "molt_binascii_crc32"),
        "configparser": (
            "stdlib_serial",
            "configparser",
            "molt_configparser_new",
        ),
        "csv": ("stdlib_csv", "csv", "molt_csv_reader_new"),
        "datetime": ("stdlib_serial", "datetime", "molt_datetime_is_leap"),
        "decimal": ("stdlib_decimal", "decimal", "molt_decimal_context_new"),
        "email": ("stdlib_email", "email", "molt_email_message_new"),
        "quopri": ("stdlib_email", "email", "molt_quopri_encode"),
        "struct": ("stdlib_serial", "structs", "molt_struct_pack"),
        "uu": ("stdlib_serial", "binascii", "molt_uu_codec_encode"),
        "archive": ("stdlib_archive", "zipfile", "molt_zipfile_crc32"),
    }

    for module_name, (feature, rust_module, symbol) in cases.items():
        facade = (resolver_root / f"{module_name}_resolver.rs").read_text(
            encoding="utf-8"
        )
        leaf = (leaf_root / f"{module_name}_resolver.rs").read_text(encoding="utf-8")

        assert f"pub mod {module_name}_resolver;" in leaf_index
        assert f'#[cfg(feature = "{feature}")]' in facade
        assert (
            "molt_runtime_serial::intrinsics_generated"
            f"::{module_name}_resolver::resolve_symbol_with"
        ) in facade
        assert f"crate::{symbol} as *const ()" not in facade
        assert "crate::builtins::functions::runtime_fn_addr" in facade
        assert f"crate::{rust_module}::{symbol} as *const ()" in leaf
        assert f"molt_runtime_serial::{rust_module}::{symbol}" in leaf


def test_crypto_resolver_is_leaf_owned_with_per_symbol_paths() -> None:
    resolver_root = ROOT / "runtime/molt-runtime/src/intrinsics/generated_resolvers"
    leaf_root = ROOT / "runtime/molt-runtime-crypto/src/intrinsics_generated"
    facade = (resolver_root / "crypto_resolver.rs").read_text(encoding="utf-8")
    leaf_index = (leaf_root / "mod.rs").read_text(encoding="utf-8")
    leaf = (leaf_root / "crypto_resolver.rs").read_text(encoding="utf-8")

    assert "pub mod crypto_resolver;" in leaf_index
    assert '#[cfg(feature = "stdlib_crypto")]' in facade
    assert (
        "molt_runtime_crypto::intrinsics_generated"
        "::crypto_resolver::resolve_symbol_with"
    ) in facade
    assert "crate::molt_hash_new as *const ()" not in facade
    assert "crate::builtins::functions::runtime_fn_addr" in facade

    cases = {
        "hashlib": ("molt_hash_new", "molt_pbkdf2_hmac", "molt_scrypt"),
        "hmac": ("molt_hmac_new", "molt_compare_digest"),
        "secrets": ("molt_secrets_token_bytes",),
    }
    for rust_module, symbols in cases.items():
        for symbol in symbols:
            assert f"crate::{rust_module}::{symbol} as *const ()" in leaf
            assert f"molt_runtime_crypto::{rust_module}::{symbol}" in leaf


def test_rustfmt_uses_shared_memory_guard(monkeypatch, tmp_path: Path) -> None:
    module = _load_gen_intrinsics_module()
    calls: list[dict[str, object]] = []
    first = tmp_path / "a" / "generated.rs"
    second = tmp_path / "b" / "mod.rs"

    def fake_guarded_completed_process(cmd, **kwargs):
        calls.append({"cmd": list(cmd), **kwargs})
        return SimpleNamespace(
            returncode=0,
            stdout="",
            stderr="",
            check_returncode=lambda: None,
        )

    monkeypatch.setattr(
        module,
        "_HARNESS_MEMORY_GUARD",
        SimpleNamespace(guarded_completed_process=fake_guarded_completed_process),
    )

    module._rustfmt([first, second])

    assert calls == [
        {
            "cmd": [
                "rustfmt",
                "--config",
                "skip_children=true",
                str(first),
                str(second),
            ],
            "prefix": "MOLT_GENERATOR",
            "cwd": ROOT,
            "capture_output": True,
            "text": True,
            "timeout": 60.0,
        }
    ]


def test_rustfmt_failure_reports_guarded_output(monkeypatch, tmp_path: Path) -> None:
    module = _load_gen_intrinsics_module()
    target = tmp_path / "generated.rs"
    target.write_text("fn main( {\n", encoding="utf-8")

    def fake_guarded_completed_process(_cmd, **_kwargs):
        return SimpleNamespace(
            returncode=1,
            stdout="format stdout",
            stderr="format stderr",
        )

    monkeypatch.setattr(
        module,
        "_HARNESS_MEMORY_GUARD",
        SimpleNamespace(guarded_completed_process=fake_guarded_completed_process),
    )

    with pytest.raises(RuntimeError) as raised:
        module._rustfmt([target])

    message = str(raised.value)
    assert f"rustfmt failed for {target}" in message
    assert "stdout:\nformat stdout" in message
    assert "stderr:\nformat stderr" in message


def test_rustfmt_batches_stay_inside_the_command_line_budget(monkeypatch) -> None:
    module = _load_gen_intrinsics_module()
    # Each path costs len("scratch/N/resolver.rs") + 3 == 24 characters, so a
    # 50-character budget holds exactly two paths per rustfmt invocation.
    monkeypatch.setattr(module, "_RUSTFMT_ARGV_CHAR_BUDGET", 50)
    paths = [Path(f"scratch/{index}/resolver.rs") for index in range(5)]

    batches = list(module._rustfmt_batches(paths))

    assert batches == [paths[0:2], paths[2:4], paths[4:5]]


def test_format_rust_sources_isolates_each_file_outside_the_output_tree(
    monkeypatch, tmp_path: Path
) -> None:
    module = _load_gen_intrinsics_module()
    output_root = tmp_path / "tracked-output"
    output_root.mkdir()
    sources = {
        output_root / "mod.rs": "mod child;\n",
        output_root / "child.rs": "fn child(){}\n",
        output_root / "nested" / "mod.rs": "pub mod other;\n",
    }
    observed: list[list[Path]] = []

    def fake_rustfmt(paths: list[Path]) -> None:
        observed.append(list(paths))
        for path in paths:
            path.write_text(f"// formatted {path.name}\n", encoding="utf-8")

    monkeypatch.setattr(module, "_rustfmt", fake_rustfmt)

    formatted = module._format_rust_sources(sources)

    assert len(observed) == 1, "every source formats in one guarded rustfmt call"
    scratch = observed[0]
    assert len(scratch) == len(sources)
    # One scratch directory per source: a `mod` declaration can never resolve
    # to a sibling output, so each file formats exactly as it would alone.
    assert len({path.parent for path in scratch}) == len(sources)
    assert all(output_root not in path.parents for path in scratch)
    assert not any(path.exists() for path in scratch)
    assert sorted(output_root.iterdir()) == []
    assert formatted == {output: f"// formatted {output.name}\n" for output in sources}


def test_orphaned_resolvers_are_reported_not_deleted(tmp_path: Path) -> None:
    module = _load_gen_intrinsics_module()
    resolver_root = tmp_path / "generated_resolvers"
    resolver_root.mkdir()
    hidden_temp = resolver_root / ".ssl_resolver.rs.12345.tmp.rs"
    hidden_temp.write_text("temp", encoding="utf-8")
    stale_resolver = resolver_root / "removed_resolver.rs"
    stale_resolver.write_text("stale", encoding="utf-8")
    current = resolver_root / "core_resolver.rs"
    current.write_text("current", encoding="utf-8")

    stale = module._stale_generated_files({current: "rendered"}, (resolver_root,))

    assert stale == [stale_resolver]
    assert stale_resolver.read_text(encoding="utf-8") == "stale"
    assert hidden_temp.read_text(encoding="utf-8") == "temp"


def test_generated_outputs_fail_closed_on_orphaned_resolver(
    monkeypatch, tmp_path: Path
) -> None:
    module = _load_gen_intrinsics_module()
    resolver_root = tmp_path / "generated_resolvers"
    resolver_root.mkdir()
    stale_resolver = resolver_root / "removed_resolver.rs"
    stale_resolver.write_text("stale", encoding="utf-8")
    monkeypatch.setattr(module, "OUT_RS_RESOLVERS_DIR", resolver_root)

    def unreachable_rustfmt(_paths: list[Path]) -> None:
        raise AssertionError("orphaned outputs must fail before formatting")

    monkeypatch.setattr(module, "_rustfmt", unreachable_rustfmt)

    with pytest.raises(RuntimeError, match="no longer renders") as raised:
        module.generated_outputs()

    assert str(stale_resolver) in str(raised.value)
    assert stale_resolver.read_text(encoding="utf-8") == "stale"


def test_retired_outputs_fail_closed(monkeypatch, tmp_path: Path) -> None:
    module = _load_gen_intrinsics_module()
    retired = tmp_path / "intrinsic_symbol_overrides.rs"
    monkeypatch.setattr(module, "RETIRED_OUTPUTS", (retired,))
    assert module._stale_generated_files({}, ()) == []

    retired.write_text("// stale\n", encoding="utf-8")

    assert module._stale_generated_files({}, ()) == [retired]
    assert retired.exists()


def test_resolver_render_covers_every_owned_directory() -> None:
    module = _load_gen_intrinsics_module()
    _raw, entries = module._load_manifest()

    sources, owned_dirs = module._render_generated_rs(entries)

    assert module.OUT_RS in sources
    assert module.OUT_RS_RESOLVERS_DIR / "mod.rs" in sources
    assert owned_dirs[0] == module.OUT_RS_RESOLVERS_DIR
    for directory in owned_dirs:
        assert any(path.parent == directory for path in sources), directory
    assert module._stale_generated_files(sources, owned_dirs) == []
