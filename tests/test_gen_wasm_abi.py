from __future__ import annotations

import copy
import importlib
import json
import re
import sys
import tomllib
from pathlib import Path

import pytest

from tools import gen_wasm_abi
from tools.generator_io import generated_file_matches, stale_outputs
from tools.wasm_abi_gen import manifest
from tools.wasm_abi_gen.paths import (
    OUT_NATIVE_EXCEPTION_OBSERVER_ABI_RS,
    OUT_RUNTIME_BOXED_ABI_RS,
    OUT_RUNTIME_CALLABLE_ABI_RS,
    OUT_RUNTIME_CALLABLES_RS,
)
from tests.process_guard_common import run_guarded_test_process

ROOT = Path(__file__).resolve().parents[1]
_MANIFEST_CACHE: dict | None = None
_MANIFEST_CACHE_BASELINE: dict | None = None
_RENDERED_RS_MODULES_CACHE: dict[str, str] | None = None
_RENDERED_RUNTIME_CALLABLES_CACHE: str | None = None
_RENDERED_PY_CACHE: str | None = None
_RENDERED_TABLE_LAYOUT_INC_CACHE: str | None = None
_RENDERED_ALLOWED_IMPORTS_CACHE: str | None = None


@pytest.mark.parametrize("projection_state", ["absent", "invalid"])
def test_fresh_wasm_abi_generation_does_not_consume_its_python_projection(
    tmp_path: Path, projection_state: str
) -> None:
    # Give a fresh import process an actually absent or invalid output at the
    # generated module boundary. A source declaration must never import it.
    projection = tmp_path / "_wasm_abi_generated.py"
    if projection_state == "invalid":
        projection.write_text("<<<<<<< invalid generated output\n", encoding="utf-8")
    result = run_guarded_test_process(
        [
            sys.executable,
            "-c",
            """
import ast
import importlib.abc
import importlib.util
import json
import sys
from pathlib import Path

projection = Path(sys.argv[1])
class GeneratedProjection(importlib.abc.MetaPathFinder):
    def find_spec(self, fullname, path=None, target=None):
        if fullname == 'molt._wasm_abi_generated':
            if not projection.exists():
                raise ModuleNotFoundError(fullname)
            return importlib.util.spec_from_file_location(fullname, projection)
        return None
sys.meta_path.insert(0, GeneratedProjection())
from tools import gen_wasm_abi as gen
gen.WASM_ABI_GEN_CACHE = projection.parent / 'empty-render-cache'
assert not gen.WASM_ABI_GEN_CACHE.exists()
outputs = gen.generated_outputs()
ast.parse(outputs[gen.OUT_PY])
assert not any(name == 'molt.frontend' or name.startswith('molt.frontend.') for name in sys.modules)
assert 'molt._wasm_abi_generated' not in sys.modules
print(json.dumps({'outputs': len(outputs), 'python_bytes': len(outputs[gen.OUT_PY])}))
""",
            str(projection),
        ],
        cwd=ROOT,
        timeout=60,
    )
    assert result.returncode == 0, result.stderr
    assert result.descendants_closed
    rendered = json.loads(result.stdout)
    assert rendered["outputs"] == len(gen_wasm_abi.OUTPUT_PATHS)
    assert rendered["python_bytes"] > 0


def test_builtin_callable_defaults_share_one_declarative_missing_value() -> None:
    from molt.builtin_callables import (
        BUILTIN_FUNC_SPECS,
        _MOLT_MISSING,
        builtin_func_declared_arity,
    )

    for name in ("enumerate", "round", "dir", "next", "anext", "getattr", "vars"):
        assert BUILTIN_FUNC_SPECS[name].defaults[-1] is _MOLT_MISSING
    assert BUILTIN_FUNC_SPECS["__import__"].defaults[0] is _MOLT_MISSING
    for name in ("min", "max"):
        assert BUILTIN_FUNC_SPECS[name].kw_defaults[1] is _MOLT_MISSING
    for name, arity in (("len", 1), ("pow", 3), ("next", 2), ("print", 5)):
        assert builtin_func_declared_arity(BUILTIN_FUNC_SPECS[name]) == arity


def test_builtin_callable_arity_is_checked_against_the_current_raw_manifest() -> None:
    data = copy.deepcopy(gen_wasm_abi.load_manifest())
    entry = next(row for row in data["import"] if row.get("runtime_name") == "molt_len")
    entry["callable_arity"] = 2
    with pytest.raises(ValueError, match="builtin function 'len' arity drift"):
        gen_wasm_abi._python_builtin_global_callables(data)


@pytest.fixture(scope="module", autouse=True)
def _gen_render_cache():
    """Memoize the generator's manifest load and renders for this module only.

    The wrappers replace ``tools.gen_wasm_abi`` globals so the generator's own
    internal calls hit the cache too; the context restores the originals when
    this module's tests finish.
    """
    with pytest.MonkeyPatch.context() as patch:
        _install_gen_cache(gen_wasm_abi, patch)
        yield


def test_wasm_abi_generator_cache_identity_uses_runtime_abi_surface() -> None:
    input_files = {path.resolve() for path in manifest.generator_input_files()}
    assert (ROOT / "src/molt/rust_source_scan.py").resolve() in input_files
    assert (ROOT / "tools/intrinsic_manifest.py").resolve() in input_files
    assert (ROOT / "src/molt/_host_capabilities_generated.py").resolve() in input_files
    assert OUT_RUNTIME_CALLABLES_RS.resolve() not in input_files
    assert OUT_RUNTIME_BOXED_ABI_RS.resolve() not in input_files
    assert OUT_RUNTIME_CALLABLE_ABI_RS.resolve() not in input_files
    assert (ROOT / "src/molt/builtin_callables.py").resolve() in input_files
    assert (ROOT / "src/molt/frontend/_types.py").resolve() not in input_files
    assert not any(
        path.match("runtime/molt-runtime/src/call/function.rs") for path in input_files
    )
    assert manifest.CPYTHON_ABI_VARIADIC_SHIM.resolve() in input_files
    assert (
        manifest.CPYTHON_ABI_SOURCE_ROOT.parent / "include" / "Python.h"
    ).resolve() in input_files
    assert (
        ROOT / "include/molt/shared/_module_callable_exports.h"
    ).resolve() in input_files
    assert (
        ROOT / "runtime/molt-cpython-abi/include/traceback.h"
    ).resolve() not in input_files
    assert {
        path.resolve() for path in manifest.CPYTHON_ABI_SOURCE_ROOT.rglob("*.rs")
    } <= input_files

    runtime_exports = manifest.generator_runtime_export_signature_rows()
    assert runtime_exports == tuple(sorted(runtime_exports))
    assert any(
        name == "molt_importlib_import_transaction"
        and params == ("i64", "i64", "i64", "i64", "i64")
        and result == "i64"
        for name, params, result in runtime_exports
    )
    cpython_abi_imports = manifest.generator_cpython_abi_link_import_names()
    assert cpython_abi_imports == tuple(sorted(cpython_abi_imports))
    cpython_abi_import_kinds = manifest.generator_cpython_abi_link_import_kinds()
    assert cpython_abi_import_kinds == tuple(sorted(cpython_abi_import_kinds))
    assert all(kind in {"data", "function"} for _, kind in cpython_abi_import_kinds)
    assert {
        "PyArg_ParseTuple",
        "PyFloat_Check",
        "PyLong_Check",
        "PyBool_Check",
        "PyExc_TypeError",
        "PyObject_Init",
        "PyMemoryView_FromMemory",
        "Py_None",
        "molt_cpython_abi_date_from_date",
        "PyLong_FromString",
        "PyLong_AsSize_t",
        "PyUnstable_Long_IsCompact",
        "PyUnstable_Long_CompactValue",
        "_PyLong_NumBits",
        "_PyLong_Size_t_Converter",
        "_PyLong_UnsignedShort_Converter",
        "_PyLong_UnsignedInt_Converter",
        "_PyLong_UnsignedLong_Converter",
        "_PyLong_UnsignedLongLong_Converter",
        "PyLong_GetInfo",
    } <= set(cpython_abi_imports)


def test_intrinsic_aliases_share_one_wasm_executable_import() -> None:
    data = gen_wasm_abi.load_manifest()
    runtime_names = [entry.get("runtime_name") for entry in data["import"]]
    declarations = manifest._intrinsic_manifest_names()
    for alias, symbol in (
        ("molt_operator_inv", "molt_operator_invert"),
        ("molt_binascii_hexlify", "molt_binascii_b2a_hex"),
        ("molt_binascii_unhexlify", "molt_binascii_a2b_hex"),
    ):
        assert alias in declarations
        assert runtime_names.count(symbol) == 1
        assert alias not in runtime_names


def test_runtime_callable_abi_preserves_target_neutral_callable_contracts() -> None:
    gen = gen_wasm_abi
    data = gen.load_manifest()
    specs = data["runtime_callable_abi"]
    symbols = {spec["runtime_name"]: spec for spec in specs}
    assert len(symbols) == len(specs)
    assert list(symbols) == sorted(symbols)
    for symbol, arity, trampoline in (
        ("molt_abs_builtin", 1, "unpack_args"),
        ("molt_importlib_import_transaction", 5, "unpack_args"),
        ("molt_cpython_abi_cext_call_trampoline", 3, "call_frame"),
        ("molt_sqlite3_connect", 1, "unpack_args"),
        ("molt_locale_getdefaultlocale", 0, "unpack_args"),
    ):
        assert symbols[symbol] == {
            "runtime_name": symbol,
            "arity": arity,
            "trampoline_abi": trampoline,
        }
    # A native provider's ABI survives the separate WASM availability projection.
    assert "sqlite3_connect" not in {entry["name"] for entry in data["import"]}
    assert "locale_getdefaultlocale" not in {entry["name"] for entry in data["import"]}
    assert (
        not {
            "molt_int_from_i64",
            "molt_int_as_i64",
            "molt_json_parse_scalar",
            "molt_dict_getitem_borrowed",
            "molt_list_getitem_borrowed",
            "molt_tuple_getitem_borrowed",
        }
        & symbols.keys()
    )
    # Check against actual Rust declarations, independently of manifest arities.
    exports = manifest._rust_export_signatures(include_native=True)
    for symbol, spec in symbols.items():
        assert exports[symbol] == {(("i64",) * spec["arity"], "i64")}, symbol
    rendered = gen.render_runtime_callable_abi_rs(data)
    assert "pub fn runtime_callable_abi(symbol: &str)" in rendered
    assert "RuntimeCallableTrampolineAbi::CallFrame" in rendered
    wasm_rendered = gen.render_rs_modules(data)["runtime_callables.rs"]
    assert "fn runtime_callable_arity" not in wasm_rendered
    assert "enum ReservedRuntimeCallableTrampolineAbi" not in wasm_rendered


def test_runtime_raw_abi_projects_all_word_raw_returns_only() -> None:
    gen = gen_wasm_abi
    data = gen.load_manifest()
    specs = data["runtime_raw_abi"]
    symbols = {spec["runtime_name"]: spec["arity"] for spec in specs}
    assert len(symbols) == len(specs)
    assert list(symbols) == sorted(symbols)
    # The host init entry the CLI calls directly is a raw-bits word source.
    assert symbols["molt_runtime_init"] == 0
    # Raw machine helpers never become generic call targets, even with an
    # all-word raw_bits signature; only an explicit raw_call row is projected.
    assert "molt_int_as_i64" not in symbols
    assert "molt_exception_pending" not in symbols
    contracts = manifest.runtime_import_return_specs(data)
    static_types = data["static_type"]
    for entry in data["import"]:
        symbol = manifest.runtime_export_name(entry)
        signature = static_types[entry["type"]]
        projected = symbol in symbols
        assert projected == entry.get("raw_call", False), symbol
        if projected:
            assert contracts[entry["name"]] == "raw_bits", symbol
            assert all(param == "i64" for param in signature["params"]), symbol
            assert signature["results"] == ["i64"], symbol
            assert symbols[symbol] == len(signature["params"]), symbol
    boxed = {spec["runtime_name"] for spec in manifest.runtime_boxed_call_specs(data)}
    callable_rows = {spec["runtime_name"] for spec in data["runtime_callable_abi"]}
    assert not (symbols.keys() & boxed) and not (symbols.keys() & callable_rows)
    rendered = gen.render_runtime_raw_abi_rs(data)
    assert "pub fn runtime_raw_abi(symbol: &str, arity: usize)" in rendered
    assert 'RuntimeRawAbi { symbol: "molt_runtime_init", arity: 0 }' in rendered
    # The opt-in is checked, not trusted: a boxed-object row cannot claim it.
    drifted = copy.deepcopy(data)
    next(row for row in drifted["import"] if row["name"] == "cell_new")["raw_call"] = (
        True
    )
    with pytest.raises(manifest.WasmAbiManifestError, match="raw_call requires"):
        manifest.runtime_raw_abi_specs(drifted)


def test_errno_protocols_are_separate_and_preserve_alias_order() -> None:
    data = copy.deepcopy(gen_wasm_abi.load_manifest())
    raw = manifest.wasi_errno_table(data)
    guest = manifest.synthetic_errno_table(data)
    resolver = manifest.resolver_error_table(data)
    assert raw["EAGAIN"] == 6 and raw["EINPROGRESS"] == 26
    assert guest["EAGAIN"] == guest["EWOULDBLOCK"] == 11
    assert guest["EINPROGRESS"] == 115
    assert guest["ECONNREFUSED"] == 111
    assert resolver["EAI_NONAME"] == 8
    assert not any(name.startswith("EAI_") for name in guest)
    assert list(guest).index("EWOULDBLOCK") < list(guest).index("EAGAIN")
    rust = gen_wasm_abi.render_synthetic_errno_rs(data)
    assert "pub enum GuestErrno" in rust and "pub enum GuestResolverError" in rust
    duplicate = copy.deepcopy(data)
    duplicate["synthetic_errno"].append({"name": "EFAKE", "value": 115})
    with pytest.raises(manifest.WasmAbiManifestError, match="explicit alias"):
        manifest.synthetic_errno_table(duplicate)
    dangling = copy.deepcopy(data)
    dangling["synthetic_errno"].append({"name": "EFAKE", "alias": "EMISSING"})
    with pytest.raises(manifest.WasmAbiManifestError, match="cyclic or undeclared"):
        manifest.synthetic_errno_table(dangling)


@pytest.mark.parametrize(
    "mutate",
    [
        lambda data: data["resolver_error"].append(
            {"name": "EAI_BAD-NAME", "value": 99, "role": "resolver"}
        ),
        lambda data: data["resolver_error"].append(
            {"name": "EAI_FAKE", "value": 2**31, "role": "resolver"}
        ),
        lambda data: data["synthetic_errno"][0].update(
            windows_socket_name="WSA_BAD-NAME"
        ),
        lambda data: data["resolver_error"][0].update(windows_code=2**31),
        lambda data: data["errno_adoption"].update(unknown="EAI_NONAME"),
        lambda data: data["wasi_errno_disposition"].clear(),
        lambda data: data["wasi_errno_disposition"].update(ENOTCAPABLE="ENOTCAPABLE"),
        lambda data: next(
            row for row in data["synthetic_errno"] if row["name"] == "ESHUTDOWN"
        ).pop("wasi_sdk"),
        lambda data: next(
            row for row in data["synthetic_errno"] if row["name"] == "E2BIG"
        ).update(wasi_sdk=False),
        lambda data: data["wasm_socket_constant"].append(
            {"name": "AF_FAKE", "kind": "address_family", "value": 2}
        ),
        lambda data: data["wasm_socket_constant"].append(
            {
                "name": "SO_FAKE",
                "kind": "option",
                "option_level": "MSG_PEEK",
                "value": 33,
            }
        ),
        lambda data: data["wasm_socket_constant"].append(
            {
                "name": "SO_FAKE",
                "kind": "option",
                "option_level": "SOL_SOCKET",
                "value": 4,
            }
        ),
        lambda data: data["wasm_socket_constant"].append(
            {"name": "MSG_FAKE", "kind": "message_flag", "value": 3}
        ),
        lambda data: data["wasm_socket_constant"].append(
            {"kind": "option", "value": 99}
        ),
    ],
)
def test_error_domain_declarations_reject_invalid_wire_identity(mutate) -> None:
    data = copy.deepcopy(gen_wasm_abi.load_manifest())
    mutate(data)
    with pytest.raises(manifest.WasmAbiManifestError):
        manifest.validate_loaded_manifest(data)


def test_runtime_callable_abi_rejects_transport_arity_drift() -> None:
    data = copy.deepcopy(gen_wasm_abi.load_manifest())
    entry = next(
        row for row in data["import"] if row.get("runtime_name") == "molt_abs_builtin"
    )
    entry["callable_arity"] = 2
    with pytest.raises(
        manifest.WasmAbiManifestError, match="molt_abs_builtin.*import ABI"
    ):
        manifest.runtime_callable_abi_specs(data)


def test_target_neutral_declarations_own_native_and_wasm_abi_views() -> None:
    data = gen_wasm_abi.load_manifest()
    declarations = {row["name"]: row for row in data["runtime_import"]}
    providers = {row["name"]: row for row in data["import"]}
    assert providers.keys() < declarations.keys()
    assert all(row is declarations[name] for name, row in providers.items())
    assert "locale_getdefaultlocale" not in providers
    assert "sqlite3_connect" not in providers
    for name in ("locale_getencoding", "locale_setlocale", "locale_constants"):
        assert name in providers
    boxed = {
        row["runtime_name"]: row for row in manifest.runtime_boxed_call_specs(data)
    }
    assert boxed["molt_locale_getdefaultlocale"] == {
        "runtime_name": "molt_locale_getdefaultlocale",
        "arity": 0,
        "result": "i64",
    }
    assert boxed["molt_sqlite3_connect"] == {
        "runtime_name": "molt_sqlite3_connect",
        "arity": 1,
        "result": "i64",
    }
    assert "molt_locale_getdefaultlocale" in gen_wasm_abi.render_runtime_boxed_abi_rs(
        data
    )


@pytest.mark.parametrize("projection", ["callable", "boxed"])
def test_native_only_abi_mutation_is_validated_from_canonical_declarations(
    projection: str,
) -> None:
    data = copy.deepcopy(gen_wasm_abi.load_manifest())
    declaration = next(
        row
        for row in data["runtime_import"]
        if row["name"] == "locale_getdefaultlocale"
    )
    declaration["callable_arity"] = 1
    with pytest.raises(manifest.WasmAbiManifestError, match="shape.*import ABI"):
        if projection == "callable":
            manifest.runtime_callable_abi_specs(data)
        else:
            manifest.runtime_boxed_call_specs(data)


def test_runtime_boxed_abi_projects_semantics_not_integer_carriers() -> None:
    gen = gen_wasm_abi
    data = gen.load_manifest()
    specs = manifest.runtime_boxed_call_specs(data)
    symbols = {spec["runtime_name"]: spec for spec in specs}
    assert list(symbols) == sorted(symbols)
    for symbol, arity, result in (
        ("molt_cell_new", 1, "i64"),
        ("molt_cell_get", 1, "i64"),
        ("molt_cell_set", 2, "i64"),
        ("molt_abs_builtin", 1, "i64"),
        ("molt_chan_new", 1, "i64"),
        ("molt_eq", 2, "i64"),
        ("molt_module_import_star", 2, "i64"),
        ("molt_set_update", 2, "i64"),
        ("molt_string_split_field_len_from_bounds", 4, "i64"),
        ("molt_print_newline", 0, "void"),
        ("molt_spawn", 1, "i64"),
        ("molt_gpu_thread_id", 0, "i64"),
        ("molt_gpu_block_id", 0, "i64"),
        ("molt_gpu_block_dim", 0, "i64"),
        ("molt_gpu_grid_dim", 0, "i64"),
        ("molt_gpu_barrier", 0, "i64"),
    ):
        assert symbols[symbol] == {
            "runtime_name": symbol,
            "arity": arity,
            "result": result,
        }
    assert (
        not {
            "molt_int_from_i64",
            "molt_int_as_i64",
            "molt_is_truthy",
            "molt_obj_get_state",
            "molt_object_field_get_ptr",
            "molt_task_new",
            "molt_json_parse_scalar",
            "molt_function_closure_bits",
            "molt_dict_getitem_borrowed",
            "molt_list_getitem_borrowed",
            "molt_tuple_getitem_borrowed",
        }
        & symbols.keys()
    )
    rendered = gen.render_runtime_boxed_abi_rs(data)
    assert "pub enum RuntimeBoxedReturn" in rendered
    assert "RuntimeBoxedReturn::OwnedValue" in rendered
    assert "RuntimeBoxedReturn::Void" in rendered
    assert "binary_search_by_key" in rendered
    assert "(abi.arity == arity).then_some(abi)" in rendered
    assert OUT_RUNTIME_BOXED_ABI_RS in gen.OUTPUT_PATHS
    assert (
        OUT_RUNTIME_BOXED_ABI_RS
        == ROOT / "runtime/molt-ir/src/runtime_boxed_abi_generated.rs"
    )


def _contextvars_provider_arities() -> dict[str, int]:
    # These are the provider argument counts, including the receiver and the
    # tuple/dict transport for variadic native methods and constructors.
    names_by_arity = {
        0: ("copy_current",),
        1: (
            "types",
            "var_hash",
            "copy",
            "len",
            "keys",
            "values",
            "items",
            "iter_self",
            "iter_next",
            "token_enter",
            "var_repr",
            "token_repr",
            "missing_repr",
        ),
        2: ("var_set", "var_reset", "getitem", "contains", "eq", "ne", "property"),
        3: (
            "new",
            "token_new",
            "missing_new",
            "keys_new",
            "values_new",
            "items_new",
            "var_new",
            "var_get",
            "get",
            "run",
        ),
        4: ("token_exit",),
    }
    return {
        f"contextvars_{name}": arity
        for arity, names in names_by_arity.items()
        for name in names
    }


def test_contextvars_callable_family_has_one_boxed_return_authority() -> None:
    data = gen_wasm_abi.load_manifest()
    expected = _contextvars_provider_arities()
    entries = {
        entry["name"]: entry
        for entry in data["import"]
        if entry["name"].startswith("contextvars_")
    }
    assert entries.keys() == expected.keys()
    boxed = {
        spec["runtime_name"]: spec for spec in manifest.runtime_boxed_call_specs(data)
    }
    callables = {
        spec["runtime_name"]: spec for spec in manifest.runtime_callable_abi_specs(data)
    }
    contracts = manifest.runtime_import_return_specs(data)
    for name, arity in expected.items():
        entry = entries[name]
        symbol = f"molt_{name}"
        assert entry["runtime_name"] == symbol
        assert entry["callable_arity"] == arity
        assert entry["shared_runtime_callable"] is True
        assert "return_contract" not in entry
        assert data["static_type"][entry["type"]] == {
            "params": ["i64"] * arity,
            "results": ["i64"],
        }
        assert boxed[symbol] == {
            "runtime_name": symbol,
            "arity": arity,
            "result": "i64",
        }
        assert callables[symbol] == {
            "runtime_name": symbol,
            "arity": arity,
            "trampoline_abi": "unpack_args",
        }
        assert contracts[name] == "owned_object"


def test_compiler_only_boxed_abi_keeps_owned_results_private() -> None:
    data = gen_wasm_abi.load_manifest()
    entries = {entry["name"]: entry for entry in data["import"]}
    specs = {
        spec["runtime_name"]: spec for spec in manifest.runtime_boxed_call_specs(data)
    }
    contracts = manifest.runtime_import_return_specs(data)
    callables = {
        spec["runtime_name"]: spec for spec in manifest.runtime_callable_abi_specs(data)
    }
    for kind, arity in (
        ("vec_sum", 3),
        ("vec_prod", 3),
        ("vec_min", 3),
        ("vec_max", 3),
        ("gpu_kernel_descriptor_set", 2),
    ):
        entry = entries[kind]
        assert entry["boxed_call"] is True
        assert "callable_arity" not in entry
        assert "return_contract" not in entry
        symbol = manifest.runtime_export_name(entry)
        assert symbol not in callables
        assert specs[symbol] == {
            "runtime_name": symbol,
            "arity": arity,
            "result": "i64",
        }
        assert contracts[kind] == "owned_object"

        # The machine carrier alone must not restore a lost boxed contract.
        probe = {
            "static_type": data["static_type"],
            "import": [copy.deepcopy(entry)],
        }
        probe["runtime_import"] = probe["import"]
        probe["import"][0].pop("boxed_call")
        assert manifest.runtime_boxed_call_specs(probe) == []
        with pytest.raises(
            manifest.WasmAbiManifestError, match="unclassified i64 return"
        ):
            manifest.runtime_import_return_specs(probe)
        probe["import"][0]["boxed_call"] = True
        probe["import"][0]["return_contract"] = "owned_object"
        with pytest.raises(
            manifest.WasmAbiManifestError, match="boxed return authority"
        ):
            manifest.runtime_boxed_call_specs(probe)


def _boxed_abi_fixture() -> dict:
    data = {
        "static_type": [
            {"params": ["i64"], "results": ["i64"]},
            {"params": ["i32"], "results": ["i64"]},
            {"params": ["i64"], "results": []},
        ],
        "import": [{"name": "example", "type": 0}],
    }
    data["runtime_import"] = data["import"]
    return data


@pytest.mark.parametrize(
    "authority", ["callable", "reserved", "lir", "fixed_lir", "compiler", "op_loop"]
)
def test_runtime_boxed_abi_uses_existing_normalized_contracts(authority: str) -> None:
    data = _boxed_abi_fixture()
    entry = data["import"][0]
    if authority == "callable":
        entry.update(runtime_name="molt_example", callable_arity=1)
    elif authority == "reserved":
        data["reserved_runtime_callable"] = [
            {
                "import_name": "example",
                "runtime_name": "molt_example",
                "callable_arity": 1,
            }
        ]
    elif authority == "lir":
        data["lir_runtime_call"] = [
            {"import_name": "example", "boxed_operand_count": 1}
        ]
    elif authority == "fixed_lir":
        data["op_loop_runtime_call"] = [
            {
                "kind": "example",
                "import_name": "example",
                "lir_variant": "Example",
                "lir_operand_count": 1,
            }
        ]
    elif authority == "compiler":
        entry["boxed_call"] = True
    else:
        data["op_loop_runtime_call_group"] = [
            {
                "kinds": ["example"],
                "arg_count": 1,
                "boxed_call": True,
            }
        ]
        # This fixture's ABI contains only example, so its op-call authority
        # has no staged runtime-call rows. Expansion must not read another tree.
        data["op_loop_runtime_call"] = manifest._expand_op_loop_runtime_calls(data, [])
    before = copy.deepcopy(data)
    assert manifest.runtime_boxed_call_specs(data) == [
        {"runtime_name": "molt_example", "arity": 1, "result": "i64"}
    ]
    assert data == before
    if authority not in {"callable", "reserved"}:
        assert "callable_arity" not in entry
        assert "runtime_name" not in entry


def test_runtime_boxed_abi_does_not_promote_transport_only_contracts() -> None:
    data = _boxed_abi_fixture()
    data["op_loop_runtime_call"] = [
        {
            "kind": "example",
            "import_name": "example",
            "args": ["local:0"],
        }
    ]
    assert manifest.runtime_boxed_call_specs(data) == []
    data["import"][0].update(callable_arity=1, callable_dispatch="trampoline")
    assert manifest.runtime_boxed_call_specs(data) == []


@pytest.mark.parametrize("callable_authority", ["import", "reserved"])
def test_runtime_boxed_abi_admits_explicit_boxed_trampoline_transport(
    callable_authority: str,
) -> None:
    data = _boxed_abi_fixture()
    data["import"][0]["boxed_call"] = True
    callable_declaration = {
        "runtime_name": "molt_example",
        "callable_arity": 1,
        "callable_dispatch": "trampoline",
    }
    if callable_authority == "import":
        data["import"][0].update(callable_declaration)
    else:
        data["reserved_runtime_callable"] = [
            {"import_name": "example", **callable_declaration}
        ]
    before = copy.deepcopy(data)
    assert manifest.runtime_boxed_call_specs(data) == [
        {"runtime_name": "molt_example", "arity": 1, "result": "i64"}
    ]
    assert manifest.runtime_import_return_specs(data) == {"example": "owned_object"}
    assert data == before


@pytest.mark.parametrize(
    "type_index, arity, result", [(1, 1, "i64"), (0, 2, "i64"), (2, 1, "i64")]
)
def test_runtime_boxed_abi_rejects_inconsistent_boxed_trampoline_shapes(
    type_index: int, arity: int, result: str
) -> None:
    data = _boxed_abi_fixture()
    data["import"][0].update(
        type=type_index,
        boxed_call=True,
        callable_arity=arity,
        callable_result=result,
        callable_dispatch="trampoline",
    )
    with pytest.raises(manifest.WasmAbiManifestError, match="boxed call"):
        manifest.runtime_boxed_call_specs(data)


@pytest.mark.parametrize("contract", ["owned_object", "raw_bits", "unpublished_object"])
def test_runtime_boxed_abi_rejects_conflicting_boxed_trampoline_ownership(
    contract: str,
) -> None:
    data = _boxed_abi_fixture()
    data["import"][0].update(
        boxed_call=True,
        callable_arity=1,
        callable_dispatch="trampoline",
        return_contract=contract,
    )
    with pytest.raises(manifest.WasmAbiManifestError, match="boxed return authority"):
        manifest.runtime_boxed_call_specs(data)


def test_import_transaction_direct_boxed_entry_preserves_callable_trampoline() -> None:
    data = gen_wasm_abi.load_manifest()
    name = "importlib_import_transaction"
    symbol = "molt_importlib_import_transaction"
    entry = next(row for row in data["runtime_import"] if row["name"] == name)
    assert entry["boxed_call"] is True
    assert "return_contract" not in entry
    assert data["static_type"][entry["type"]] == {
        "params": ["i64", "i64", "i64", "i64", "i64"],
        "results": ["i64"],
    }
    assert next(
        row
        for row in data["reserved_runtime_callable"]
        if row["runtime_name"] == symbol
    ) == {
        "index": 23,
        "runtime_name": symbol,
        "import_name": name,
        "callable_arity": 5,
        "callable_dispatch": "trampoline",
    }
    assert next(
        row for row in data["runtime_callable_abi"] if row["runtime_name"] == symbol
    ) == {"runtime_name": symbol, "arity": 5, "trampoline_abi": "unpack_args"}
    assert next(
        row
        for row in manifest.runtime_boxed_call_specs(data)
        if row["runtime_name"] == symbol
    ) == {"runtime_name": symbol, "arity": 5, "result": "i64"}
    assert manifest.runtime_import_return_specs(data)[name] == "owned_object"


@pytest.mark.parametrize(
    "type_index, arity, result", [(1, 1, "i64"), (0, 2, "i64"), (2, 1, "i64")]
)
def test_runtime_boxed_abi_rejects_inconsistent_callable_shapes(
    type_index: int, arity: int, result: str
) -> None:
    data = _boxed_abi_fixture()
    data["import"][0].update(
        type=type_index, callable_arity=arity, callable_result=result
    )
    with pytest.raises(manifest.WasmAbiManifestError, match="boxed call"):
        manifest.runtime_boxed_call_specs(data)


@pytest.mark.parametrize(
    "args",
    [
        ["op_value_i64:raw"],
        ["local:1"],
        ["local:0", "local:1"],
    ],
)
def test_runtime_boxed_abi_rejects_inconsistent_op_loop_shapes(args: list[str]) -> None:
    data = _boxed_abi_fixture()
    data["op_loop_runtime_call"] = [
        {
            "kind": "example",
            "import_name": "example",
            "args": args,
            "boxed_call": True,
        }
    ]
    with pytest.raises(manifest.WasmAbiManifestError, match="boxed"):
        manifest.runtime_boxed_call_specs(data)


def test_runtime_boxed_abi_keeps_real_void_distinct_from_boxed_none() -> None:
    data = _boxed_abi_fixture()
    data["import"][0].update(type=2, boxed_call=True)
    assert manifest.runtime_boxed_call_specs(data)[0]["result"] == "void"
    data["lir_runtime_call"] = [{"import_name": "example", "boxed_operand_count": 1}]
    with pytest.raises(manifest.WasmAbiManifestError, match="boxed LIR shape"):
        manifest.runtime_boxed_call_specs(data)


def test_runtime_return_contract_requires_semantics_for_every_i64_import() -> None:
    data = _boxed_abi_fixture()
    with pytest.raises(manifest.WasmAbiManifestError, match="unclassified i64 return"):
        manifest.runtime_import_return_specs(data)
    for contract in (
        "owned_object",
        "borrowed_object",
        "raw_bits",
        "unpublished_object",
        "scratch_allocation",
        "execution_token",
        "poll_result",
    ):
        data["import"][0]["return_contract"] = contract
        assert manifest.runtime_import_return_specs(data) == {"example": contract}


@pytest.mark.parametrize("contract", ["void", "unknown", [], True])
def test_runtime_return_contract_rejects_invalid_or_contradictory_i64_declarations(
    contract,
) -> None:
    data = _boxed_abi_fixture()
    data["import"][0]["return_contract"] = contract
    with pytest.raises(manifest.WasmAbiManifestError, match="return_contract"):
        manifest.runtime_import_return_specs(data)


@pytest.mark.parametrize(
    "results, expected",
    [([], "void"), (["i32"], "raw_bits"), (["f32"], "raw_bits"), (["f64"], "raw_bits")],
)
def test_runtime_return_contract_derives_unambiguous_machine_results(
    results, expected
) -> None:
    data = _boxed_abi_fixture()
    data["static_type"][0]["results"] = results
    assert manifest.runtime_import_return_specs(data) == {"example": expected}
    data["import"][0]["return_contract"] = expected
    with pytest.raises(
        manifest.WasmAbiManifestError, match="duplicates or contradicts"
    ):
        manifest.runtime_import_return_specs(data)


@pytest.mark.parametrize("contract", ["borrowed_object", "poll_result"])
def test_runtime_return_contract_refines_one_shared_boxed_authority(
    contract: str,
) -> None:
    data = _boxed_abi_fixture()
    data["import"][0]["boxed_call"] = True
    assert manifest.runtime_import_return_specs(data) == {"example": "owned_object"}
    data["import"][0]["return_contract"] = contract
    assert manifest.runtime_import_return_specs(data) == {"example": contract}
    assert manifest.runtime_boxed_call_specs(data)[0]["ownership"] == contract
    for contract in ("owned_object", "raw_bits", "void", "unpublished_object"):
        data["import"][0]["return_contract"] = contract
        with pytest.raises(
            manifest.WasmAbiManifestError, match="boxed return authority"
        ):
            manifest.runtime_import_return_specs(data)


def test_runtime_return_contract_uses_poll_membership_without_another_list() -> None:
    data = _boxed_abi_fixture()
    data["import"][0]["poll_table_slot"] = 1
    assert manifest.runtime_import_return_specs(data) == {"example": "poll_result"}
    data["import"][0]["boxed_call"] = True
    assert manifest.runtime_boxed_call_specs(data)[0]["ownership"] == "poll_result"
    data["import"][0]["return_contract"] = "poll_result"
    with pytest.raises(manifest.WasmAbiManifestError, match="boxed return authority"):
        manifest.runtime_import_return_specs(data)
    data["import"][0].pop("return_contract")
    data["import"][0]["type"] = 1
    with pytest.raises(manifest.WasmAbiManifestError, match="boxed call|poll table"):
        manifest.runtime_import_return_specs(data)
    data["import"][0].pop("boxed_call")
    with pytest.raises(manifest.WasmAbiManifestError, match="poll table"):
        manifest.runtime_import_return_specs(data)


def test_runtime_return_contract_rejects_multiple_results() -> None:
    data = _boxed_abi_fixture()
    data["static_type"][0]["results"] = ["i64", "i64"]
    with pytest.raises(manifest.WasmAbiManifestError, match="multiple return values"):
        manifest.runtime_import_return_specs(data)


@pytest.mark.parametrize(
    "section", ["op_loop_runtime_call", "op_loop_runtime_call_group"]
)
def test_op_loop_cannot_redeclare_return_sink_policy(section: str) -> None:
    row = {"kind": "example", "sink": "owned_result_or_release"}
    if section.endswith("group"):
        row.update(kinds=["example"], arg_count=1)
    with pytest.raises(manifest.WasmAbiManifestError, match="sink policy is derived"):
        manifest._expand_op_loop_runtime_calls({section: [row]}, [])


def test_runtime_return_contract_generated_rust_agrees_with_shared_boxed_projection() -> (
    None
):
    gen = gen_wasm_abi
    data = gen.load_manifest()
    contracts = manifest.runtime_import_return_specs(data)
    assert len(contracts) == len(data["import"])
    assert contracts["alloc_class"] == "unpublished_object"
    assert contracts["scratch_alloc"] == "scratch_allocation"
    assert contracts["runtime_execution_enter"] == "execution_token"
    assert contracts["int_from_i64"] == "owned_object"
    assert contracts["int_as_i64"] == "raw_bits"
    assert contracts["guard_type"] == "borrowed_object"
    assert contracts["dict_set"] == "borrowed_object"
    assert contracts["store_index"] == "borrowed_object"
    for name in ("dict_setitem", "list_int_setitem", "del_index"):
        assert contracts[name] == "borrowed_object", name
    assert contracts["io_wait"] == "poll_result"
    assert contracts["future_poll"] == "poll_result"
    for name in (
        "gpu_thread_id",
        "gpu_block_id",
        "gpu_block_dim",
        "gpu_grid_dim",
        "gpu_barrier",
    ):
        entry = next(entry for entry in data["import"] if entry["name"] == name)
        assert contracts[name] == "owned_object"
        assert entry["runtime_name"] == f"molt_{name}"
        assert entry["callable_arity"] == 0
        assert "return_contract" not in entry
    for name in (
        "exception_pending",
        "async_work_poll_and_exception_pending",
        "thread_record",
        "recursion_enter_fast",
    ):
        assert contracts[name] == "raw_bits"
        entry = next(entry for entry in data["import"] if entry["name"] == name)
        assert "callable_arity" not in entry
    for kind in (
        "trace_enter_slot",
        "trace_exit",
        "trace_set_line",
    ):
        assert contracts[kind] == "borrowed_object"
    boxed_names = {
        row["runtime_name"] for row in manifest.runtime_boxed_call_specs(data)
    }
    # A direct call hands its callee over through the thread record; the
    # runtime keeps no invocation token protocol.
    import_names = {entry["name"] for entry in data["import"]}
    assert not {"frame_invocation_enter", "frame_invocation_exit"} & import_names
    assert "molt_thread_record" not in boxed_names
    for name in ("dtype", "nbytes", "free", "numel", "realize", "contiguous"):
        runtime_name = f"molt_gpu_prim_{name}"
        assert runtime_name in data["non_runtime_callable_intrinsic"]
        assert runtime_name not in boxed_names
    for name in (
        "chan_recv",
        "chan_recv_blocking",
        "chan_send",
        "chan_try_recv",
        "chan_try_send",
        "stream_recv",
        "ws_recv",
        "stream_reader_read",
        "stream_reader_readline",
        "stream_send_obj",
        "ws_send_obj",
    ):
        assert contracts[name] == "poll_result"
    assert contracts["chan_send_blocking"] == "owned_object"
    modules = gen.render_rs_modules(data)
    variants = gen._runtime_import_variants(data)
    return_body = modules["import_metadata.rs"].split(
        "pub(crate) const fn return_contract(self) -> WasmRuntimeReturn {", 1
    )[1]
    # rustfmt may turn a long expression arm into a braced block. Parse both
    # shapes, then require exactly one matching return fact for every import.
    arms = re.findall(
        r"Self::(\w+)\s*=>\s*(?:WasmRuntimeReturn::(\w+)\s*,"
        r"|\{\s*WasmRuntimeReturn::(\w+)\s*\}\s*,?)",
        return_body,
    )
    expected = {
        variants[name]: manifest.RUNTIME_RETURN_CONTRACTS[contract]
        for name, contract in contracts.items()
    }
    assert len(arms) == len(expected)
    assert {variant: inline or block for variant, inline, block in arms} == expected
    assert "OpLoopRuntimeSinkSpec" not in modules["lir_runtime_calls.rs"]
    shared = gen.render_runtime_boxed_abi_rs(data)
    assert "RuntimeBoxedReturn::BorrowedValue" in shared
    assert "RuntimeBoxedReturn::PollValue" in shared


def test_fixed_arity_constructors_take_raw_word_ranges_not_boxed_operands() -> None:
    data = gen_wasm_abi.load_manifest()
    imports = {entry["name"]: entry for entry in data["import"]}
    contracts = manifest.runtime_import_return_specs(data)
    boxed = {row["runtime_name"] for row in manifest.runtime_boxed_call_specs(data)}
    lir_calls = {row["import_name"]: row for row in data["lir_runtime_call"]}
    # Tuple construction has one constructor; the builder finish is retired.
    assert "tuple_builder_finish" not in imports
    assert "tuple_builder_finish" not in lir_calls
    for name, arity in (
        ("tuple_from_values", 2),
        ("list_from_values", 2),
        ("dataclass_new_from_values", 5),
    ):
        entry = imports[name]
        signature = data["static_type"][entry["type"]]
        assert signature["params"] == ["i64"] * arity, name
        assert signature["results"] == ["i64"], name
        assert contracts[name] == "owned_object", name
        # The range address and length are raw machine operands: generic boxed
        # lowering and callable admission must never box or release them.
        assert f"molt_{name}" not in boxed, name
        assert "callable_arity" not in entry, name
    assert "boxed_operand_count" not in lir_calls["tuple_from_values"]


@pytest.mark.parametrize(
    "import_name", ["alloc", "alloc_class", "scratch_alloc", "runtime_execution_enter"]
)
def test_op_loop_rejects_returns_that_require_a_dedicated_lifetime_protocol(
    import_name: str,
) -> None:
    data = copy.deepcopy(gen_wasm_abi.load_manifest())
    data["op_loop_runtime_call"].append(
        {"kind": "synthetic_forbidden_sink", "import_name": import_name, "args": []}
    )
    with pytest.raises(manifest.WasmAbiManifestError, match="cannot sink"):
        manifest.validate_loaded_manifest(data)


def test_op_loop_rejects_a_marked_import_with_a_different_return_contract() -> None:
    data = copy.deepcopy(gen_wasm_abi.load_manifest())
    observer = next(
        row
        for row in data["op_loop_runtime_call"]
        if row["kind"] == "exception_finally_pending_observer"
    )
    observer["marked_import_name"] = "runtime_init"
    with pytest.raises(
        manifest.WasmAbiManifestError, match="marked import return contract"
    ):
        manifest.validate_loaded_manifest(data)


def _install_gen_cache(gen, patch: pytest.MonkeyPatch) -> None:
    uncached_load_manifest = gen.load_manifest
    uncached_render_rs_modules = gen.render_rs_modules
    uncached_render_runtime_callables_rs = gen.render_runtime_callables_rs
    uncached_render_py = gen.render_py
    uncached_render_table_layout_inc = gen.render_table_layout_inc
    uncached_render_allowed_imports = gen.render_allowed_imports

    def load_manifest_cached():
        global _MANIFEST_CACHE, _MANIFEST_CACHE_BASELINE
        if _MANIFEST_CACHE is None:
            _MANIFEST_CACHE = uncached_load_manifest()
            _MANIFEST_CACHE_BASELINE = copy.deepcopy(_MANIFEST_CACHE)
        return _MANIFEST_CACHE

    def render_rs_modules_cached(data: dict) -> dict[str, str]:
        global _RENDERED_RS_MODULES_CACHE
        if data is _MANIFEST_CACHE:
            if _RENDERED_RS_MODULES_CACHE is None:
                _RENDERED_RS_MODULES_CACHE = uncached_render_rs_modules(data)
            return _RENDERED_RS_MODULES_CACHE
        return uncached_render_rs_modules(data)

    def render_runtime_callables_rs_cached(data: dict) -> str:
        global _RENDERED_RUNTIME_CALLABLES_CACHE
        if data is _MANIFEST_CACHE:
            if _RENDERED_RUNTIME_CALLABLES_CACHE is None:
                _RENDERED_RUNTIME_CALLABLES_CACHE = (
                    uncached_render_runtime_callables_rs(data)
                )
            return _RENDERED_RUNTIME_CALLABLES_CACHE
        return uncached_render_runtime_callables_rs(data)

    def render_py_cached(data: dict) -> str:
        global _RENDERED_PY_CACHE
        if data is _MANIFEST_CACHE:
            if _RENDERED_PY_CACHE is None:
                _RENDERED_PY_CACHE = uncached_render_py(data)
            return _RENDERED_PY_CACHE
        return uncached_render_py(data)

    def render_table_layout_inc_cached(data: dict) -> str:
        global _RENDERED_TABLE_LAYOUT_INC_CACHE
        if data is _MANIFEST_CACHE:
            if _RENDERED_TABLE_LAYOUT_INC_CACHE is None:
                _RENDERED_TABLE_LAYOUT_INC_CACHE = uncached_render_table_layout_inc(
                    data
                )
            return _RENDERED_TABLE_LAYOUT_INC_CACHE
        return uncached_render_table_layout_inc(data)

    def render_allowed_imports_cached(data: dict) -> str:
        global _RENDERED_ALLOWED_IMPORTS_CACHE
        if data is _MANIFEST_CACHE:
            if _RENDERED_ALLOWED_IMPORTS_CACHE is None:
                _RENDERED_ALLOWED_IMPORTS_CACHE = uncached_render_allowed_imports(data)
            return _RENDERED_ALLOWED_IMPORTS_CACHE
        return uncached_render_allowed_imports(data)

    patch.setattr(gen, "load_manifest", load_manifest_cached)
    patch.setattr(gen, "render_rs_modules", render_rs_modules_cached)
    patch.setattr(
        gen, "render_runtime_callables_rs", render_runtime_callables_rs_cached
    )
    patch.setattr(gen, "render_py", render_py_cached)
    patch.setattr(gen, "render_table_layout_inc", render_table_layout_inc_cached)
    patch.setattr(gen, "render_allowed_imports", render_allowed_imports_cached)


def _reset_render_caches() -> None:
    global _RENDERED_RS_MODULES_CACHE
    global _RENDERED_RUNTIME_CALLABLES_CACHE
    global _RENDERED_PY_CACHE
    global _RENDERED_TABLE_LAYOUT_INC_CACHE
    global _RENDERED_ALLOWED_IMPORTS_CACHE

    _RENDERED_RS_MODULES_CACHE = None
    _RENDERED_RUNTIME_CALLABLES_CACHE = None
    _RENDERED_PY_CACHE = None
    _RENDERED_TABLE_LAYOUT_INC_CACHE = None
    _RENDERED_ALLOWED_IMPORTS_CACHE = None


@pytest.fixture(autouse=True)
def _manifest_cache_is_read_only():
    yield
    global _MANIFEST_CACHE
    if (
        _MANIFEST_CACHE is not None
        and _MANIFEST_CACHE_BASELINE is not None
        and _MANIFEST_CACHE != _MANIFEST_CACHE_BASELINE
    ):
        _MANIFEST_CACHE = copy.deepcopy(_MANIFEST_CACHE_BASELINE)
        _reset_render_caches()
        raise AssertionError(
            "tests must copy the cached WASM ABI manifest before mutating it"
        )


def test_runtime_import_variants_are_fresh_immutable_render_inputs() -> None:
    gen = gen_wasm_abi
    data = {"import": [{"name": "first_import"}]}
    first = gen._runtime_import_variants(data)
    assert dict(first) == {"first_import": "FirstImport"}
    with pytest.raises(TypeError):
        first["first_import"] = "Wrong"
    data["import"][0]["name"] = "second_import"
    second = gen._runtime_import_variants(data)
    assert dict(first) == {"first_import": "FirstImport"}
    assert dict(second) == {"second_import": "SecondImport"}
    assert gen._rust_runtime_import(second, "second_import") == (
        "WasmRuntimeImport::SecondImport"
    )
    with pytest.raises(KeyError):
        gen._rust_runtime_import(second, "first_import")


@pytest.mark.parametrize("names", [("test_name", "test__name"), ("same", "same")])
def test_runtime_import_variant_collisions_keep_exact_diagnostics(names) -> None:
    gen = gen_wasm_abi
    data = {"import": [{"name": names[0]}]}
    gen._runtime_import_variants(data)
    data["import"].append({"name": names[1]})
    variant = gen._rust_pascal_variant(names[0])
    with pytest.raises(manifest.WasmAbiManifestError) as raised:
        gen._runtime_import_variants(data)
    assert str(raised.value) == (
        "runtime import enum variant collision: "
        f"{names[0]!r} and {names[1]!r} both map to {variant!r}"
    )


def test_rust_render_derives_import_variants_once_per_transaction(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    gen = gen_wasm_abi
    data = copy.deepcopy(gen.load_manifest())
    calls = 0
    derive = gen._runtime_import_variants

    def tracked(inputs):
        nonlocal calls
        calls += 1
        assert inputs is data
        return derive(inputs)

    monkeypatch.setattr(gen, "_runtime_import_variants", tracked)
    monkeypatch.setattr(gen, "rustfmt_sources", lambda modules, label: modules)
    first = gen.render_rs_modules(data)
    assert calls == 1
    assert "WasmRuntimeImport" in first["import_tokens.rs"]
    # A mutable manifest must not acquire a stale identity-keyed derived cache.
    data["import"].append(
        {
            "name": "render_transaction_extra_import",
            "type": _import_by_name(data, "types_bootstrap")["type"],
            "return_contract": "raw_bits",
        }
    )
    second = gen.render_rs_modules(data)
    assert calls == 2
    assert "RenderTransactionExtraImport" not in first["import_tokens.rs"]
    assert "RenderTransactionExtraImport" in second["import_tokens.rs"]
    assert "RenderTransactionExtraImport" in second["import_metadata.rs"]


def _raw_manifest() -> dict:
    return tomllib.loads(manifest.MANIFEST.read_text(encoding="utf-8"))


def _import_by_name(data: dict, name: str, *, section: str = "import") -> dict:
    return next(entry for entry in data[section] if entry["name"] == name)


def _import_signature(
    data: dict, name: str, *, section: str = "import"
) -> tuple[tuple[str, ...], tuple[str, ...]]:
    signature = data["static_type"][
        _import_by_name(data, name, section=section)["type"]
    ]
    return tuple(signature["params"]), tuple(signature["results"])


def _rendered_rs(gen, data) -> str:
    return "".join(gen.render_rs_modules(data).values())


def _exec_rendered_py(rendered_py: str) -> dict[str, object]:
    namespace: dict[str, object] = {}
    exec(rendered_py, namespace)
    return namespace


def test_wasm_abi_generated_files_are_in_sync() -> None:
    gen = gen_wasm_abi
    assert gen._stale_generated_files() == []
    # Render from source (bypassing the render cache) so the renderer itself is
    # proven, then compare paths rather than text: pytest's text diff can take
    # minutes on these megabyte-sized, highly repetitive projections.
    outputs = gen.render_outputs(gen.load_manifest())
    assert set(outputs) == set(gen.OUTPUT_PATHS)
    assert stale_outputs(outputs) == []


@pytest.mark.parametrize("state", ["missing", "stale", "current-crlf"])
def test_generated_projection_diagnostic_is_bounded(
    tmp_path, capsys, monkeypatch, state
) -> None:
    gen = gen_wasm_abi
    path = tmp_path / "large_generated.py"
    expected = 'callable = ("i64", "i64")\n' * 60_000
    if state == "stale":
        path.write_text(expected + "drift = True\n", encoding="utf-8")
    elif state == "current-crlf":
        path.write_bytes(expected.replace("\n", "\r\n").encode("utf-8"))
    monkeypatch.setattr(gen, "generated_outputs", lambda: {path: expected})

    assert gen.main(["--check"]) == (0 if state == "current-crlf" else 1)
    diagnostic = capsys.readouterr().err
    if state == "current-crlf":
        assert diagnostic == ""
    else:
        assert str(path) in diagnostic
        assert "stale:" in diagnostic
        assert "--write" in diagnostic
        assert len(diagnostic) < len(str(path)) + 160
        assert "callable =" not in diagnostic


@pytest.mark.parametrize("retired", ["legacy", "removed", "unexpected_module"])
def test_retired_outputs_fail_closed_without_deletion(
    tmp_path, monkeypatch, retired
) -> None:
    gen = gen_wasm_abi
    out_dir = tmp_path / "wasm_abi_generated"
    out_dir.mkdir()
    legacy = tmp_path / "wasm_abi_generated.rs"
    removed = tmp_path / "wasm_runtime_callables.inc"
    monkeypatch.setattr(gen, "OUT_RS_DIR", out_dir)
    monkeypatch.setattr(gen, "OUT_RS_FILES", {"mod.rs": out_dir / "mod.rs"})
    monkeypatch.setattr(gen, "LEGACY_OUT_RS", legacy)
    monkeypatch.setattr(gen, "REMOVED_GENERATED_FILES", (removed,))
    (out_dir / "mod.rs").write_text("// current\n", encoding="utf-8")
    assert gen._stale_generated_files() == []

    stale = {
        "legacy": legacy,
        "removed": removed,
        "unexpected_module": out_dir / "retired_module.rs",
    }[retired]
    stale.write_text("// stale\n", encoding="utf-8")

    def unreachable(*_args, **_kwargs):
        raise AssertionError("retired outputs must fail before rendering")

    monkeypatch.setattr(gen, "rustfmt_version", unreachable)
    with pytest.raises(RuntimeError, match="no longer renders") as raised:
        gen.generated_outputs()
    assert str(stale) in str(raised.value)
    assert stale.read_text(encoding="utf-8") == "// stale\n"


def test_render_cache_round_trips_exactly_the_generated_outputs(
    tmp_path, monkeypatch
) -> None:
    gen = gen_wasm_abi
    monkeypatch.setattr(gen, "WASM_ABI_GEN_CACHE", tmp_path / "cache")
    outputs = {path: f"// {path.name}\n" for path in gen.OUTPUT_PATHS}

    assert gen._load_render_cache("key") is None
    gen._store_render_cache("key", outputs)
    assert gen._load_render_cache("key") == outputs
    assert gen._load_render_cache("other-key") is None

    partial = dict(outputs)
    partial.pop(gen.OUT_PY)
    gen._store_render_cache("partial", partial)
    assert gen._load_render_cache("partial") is None


def test_cpython_abi_link_import_discovery_covers_the_complete_crate() -> None:
    data = manifest.load_manifest()
    generated = dict(manifest.generator_cpython_abi_link_import_kinds())
    bridge_and_hook_exports = {
        "Py_DecRef",
        "Py_IncRef",
        "molt_capi_any_decref",
        "molt_capi_any_incref",
        "molt_capi_handle_to_borrowed_pyobj",
        "molt_capi_handle_to_pyobj",
        "molt_capi_pyobj_is_bridge_managed",
        "molt_capi_pyobj_to_handle",
        "molt_capi_result_to_pyobj",
        "molt_capi_semantic_type",
        "molt_capi_set_semantic_type",
        "molt_cpython_abi_init",
        "molt_cpython_abi_register_hooks",
    }

    assert bridge_and_hook_exports <= generated.keys()
    assert {generated[name] for name in bridge_and_hook_exports} == {"function"}
    link_imports = {
        entry["name"]: entry
        for entry in data["external_native_link_import"]
        if entry["name"] in bridge_and_hook_exports
    }
    assert set(link_imports) == bridge_and_hook_exports
    assert {entry["primitive_class"] for entry in link_imports.values()} == {
        "molt_cpython_abi_link_import"
    }


def test_linked_shared_header_declaration_changes_signatures_and_cache_key(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    shared_header = (ROOT / "include/molt/shared/_module_callable_exports.h").resolve()
    assert shared_header in {
        path.resolve() for path in manifest.generator_input_files()
    }
    gen = gen_wasm_abi
    monkeypatch.setattr(gen, "generator_runtime_export_signature_rows", lambda: ())
    monkeypatch.setattr(gen, "generator_cpython_abi_link_import_kinds", lambda: ())
    original_read_bytes = Path.read_bytes
    original_read_text = Path.read_text
    original = shared_header.read_bytes()
    changed = original.replace(
        b"extern PyObject *PyModule_New(",
        b"extern double PyModule_New(",
        1,
    )
    assert changed != original
    manifest.generator_cpython_abi_link_import_signatures.cache_clear()
    try:
        baseline = dict(
            (name, (params, results))
            for name, params, results in (
                manifest.generator_cpython_abi_link_import_signatures()
            )
        )
        assert baseline["PyModule_New"] == (("i32",), ("i32",))
        baseline_key = gen._render_cache_key("test-rustfmt")

        def changed_read_bytes(path: Path) -> bytes:
            if path.resolve() == shared_header:
                return changed
            return original_read_bytes(path)

        def changed_read_text(path: Path, *args: object, **kwargs: object) -> str:
            if path.resolve() == shared_header:
                return changed.decode("utf-8")
            return original_read_text(path, *args, **kwargs)

        monkeypatch.setattr(Path, "read_bytes", changed_read_bytes)
        monkeypatch.setattr(Path, "read_text", changed_read_text)
        manifest.generator_cpython_abi_link_import_signatures.cache_clear()
        changed_signatures = dict(
            (name, (params, results))
            for name, params, results in (
                manifest.generator_cpython_abi_link_import_signatures()
            )
        )
        assert changed_signatures["PyModule_New"] == (("i32",), ("f64",))
        assert gen._render_cache_key("test-rustfmt") != baseline_key
    finally:
        manifest.generator_cpython_abi_link_import_signatures.cache_clear()


def test_linked_header_closure_rejects_missing_local_dependency(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    shared_header = (ROOT / "include/molt/shared/_module_callable_exports.h").resolve()
    original_is_file = Path.is_file

    def missing_shared_header(path: Path) -> bool:
        if path.resolve() == shared_header:
            return False
        return original_is_file(path)

    monkeypatch.setattr(Path, "is_file", missing_shared_header)
    with pytest.raises(
        manifest.WasmAbiManifestError,
        match="missing local header '_module_callable_exports.h'",
    ):
        manifest.generator_input_files()


def test_external_native_function_signature_authority_is_complete() -> None:
    gen = gen_wasm_abi
    data = gen.load_manifest()
    cpython_signatures = {
        name: (params, results)
        for name, params, results in (
            manifest.generator_cpython_abi_link_import_signatures()
        )
    }
    assert cpython_signatures["PyObject_Init"] == (("i32", "i32"), ("i32",))
    assert cpython_signatures["PyLong_FromLongLong"] == (("i64",), ("i32",))
    assert cpython_signatures["PyFloat_FromDouble"] == (("f64",), ("i32",))
    assert cpython_signatures["PyBuffer_Release"] == (("i32",), ())
    assert cpython_signatures["PyComplex_AsCComplex"] == (
        ("i32", "i32"),
        (),
    )
    assert cpython_signatures["PyCapsule_New"] == (
        ("i32", "i32", "i32"),
        ("i32",),
    )
    assert cpython_signatures["PyArg_ParseTuple"] == (
        ("i32", "i32", "i32"),
        ("i32",),
    )

    namespace = _exec_rendered_py(gen.render_py(data))
    shapes = namespace["WASM_EXTERNAL_NATIVE_ARTIFACT_IMPORT_SHAPES"]
    signatures = namespace["WASM_EXTERNAL_NATIVE_ARTIFACT_FUNCTION_SIGNATURES"]
    wasm_import_signature = namespace["wasm_import_signature"]
    assert signatures[("wasi_snapshot_preview1", "fd_write")] == {
        "params": ["i32", "i32", "i32", "i32"],
        "result": "i32",
    }
    assert signatures[("wasi_snapshot_preview1", "clock_time_get")] == {
        "params": ["i32", "i64", "i32"],
        "result": "i32",
    }
    assert signatures[("wasi_snapshot_preview1", "path_readlink")] == {
        "params": ["i32", "i32", "i32", "i32", "i32", "i32"],
        "result": "i32",
    }
    assert wasm_import_signature("path_readlink") == (("i64",), ("i64",))
    assert signatures[("env", "molt_call_indirect3")] == {
        "params": ["i64", "i64", "i64", "i64"],
        "result": "i64",
    }
    assert wasm_import_signature("molt_cbor_parse_scalar_obj") is not None
    signatureless = sorted(
        name
        for name, (module, kind) in shapes.items()
        if kind == "function"
        and (module, name) not in signatures
        and wasm_import_signature(name) is None
    )
    assert signatureless == []


def test_wasm_abi_manifest_owns_import_signature_catalog() -> None:
    gen = gen_wasm_abi
    data = gen.load_manifest()
    static_types = data["static_type"]
    static_type_count = len(static_types)

    assert _import_signature(data, "types_bootstrap") == ((), ("i64",))
    assert _import_signature(data, "inc_ref_obj") == (("i64",), ())
    assert {"params": [], "results": ["i32"]} in static_types
    # A generic write site passes receiver, name pointer and length, value
    # and its site id.
    write_site = next(
        entry for entry in data["import"] if entry["name"] == "set_attr_object_ic"
    )
    assert static_types[write_site["type"]] == {
        "params": ["i64", "i32", "i64", "i64", "i64"],
        "results": ["i64"],
    }
    assert not any(
        entry["name"].startswith("list_builder_") for entry in data["import"]
    )
    assert _import_signature(data, "code_new") == (("i64",) * 9, ("i64",))
    signatures = {
        (tuple(entry["params"]), tuple(entry["results"])) for entry in static_types
    }
    assert len(signatures) == len(static_types)
    assert {entry["type"] for entry in data["runtime_import"]} == set(
        range(static_type_count)
    )
    assert all(len(signature["results"]) <= 1 for signature in static_types)
    assert all(entry["type"] < len(static_types) for entry in data["import"])

    rendered_rs = _rendered_rs(gen, data)
    rendered_py = gen.render_py(data)
    assert "STATIC_FUNC_TYPES" in rendered_rs
    modules = gen.render_rs_modules(data)
    assert "fn machine_signature(" in modules["import_metadata.rs"]
    assert "fn type_idx(" not in modules["import_metadata.rs"]
    assert "type_idx:" not in modules["import_registry.rs"]
    generated = _exec_rendered_py(rendered_py)
    assert {
        (params, results) for _, params, results in generated["WASM_IMPORT_SIGNATURES"]
    } == signatures

    wasm_abi = (ROOT / "runtime/molt-backend-wasm/src/wasm_abi.rs").read_text(
        encoding="utf-8"
    )
    assert "static_func_type(" not in wasm_abi
    assert "const STATIC_FUNC_TYPES" not in wasm_abi


def test_wasm_abi_catalog_keeps_signatures_produced_by_synthesized_imports() -> None:
    raw = _raw_manifest()
    loaded = gen_wasm_abi.load_manifest()
    for name, arity in (
        ("datetime_format_isodatetime", 10),
        ("gpu_rope_apply_contiguous", 11),
    ):
        assert name not in {entry["name"] for entry in raw["import"]}
        assert _import_signature(loaded, name) == (("i64",) * arity, ("i64",))


def test_wasm_abi_catalog_rejects_duplicate_signatures() -> None:
    raw = _raw_manifest()
    raw["static_type"].append(copy.deepcopy(raw["static_type"][0]))
    with pytest.raises(
        manifest.WasmAbiManifestError, match="duplicate static type signature"
    ):
        manifest.validate_loaded_manifest(raw)


def test_wasm_abi_catalog_rejects_signatures_without_import_producers() -> None:
    raw = _raw_manifest()
    extra = {"params": ["f64"], "results": ["f64"]}
    assert extra not in raw["static_type"]
    raw["static_type"].append(extra)
    with pytest.raises(
        manifest.WasmAbiManifestError, match="no materialized import producer"
    ):
        manifest.validate_loaded_manifest(raw)


def test_wasm_abi_catalog_order_does_not_change_import_contracts() -> None:
    raw = _raw_manifest()
    last = len(raw["static_type"]) - 1
    raw["static_type"].reverse()
    for entry in raw["import"]:
        entry["type"] = last - entry["type"]
    reordered = manifest.validate_loaded_manifest(raw)
    expected = gen_wasm_abi.load_manifest()
    for section in ("import", "runtime_import"):
        before = {
            entry["name"]: (
                expected["static_type"][entry["type"]],
                {key: value for key, value in entry.items() if key != "type"},
            )
            for entry in expected[section]
        }
        after = {
            entry["name"]: (
                reordered["static_type"][entry["type"]],
                {key: value for key, value in entry.items() if key != "type"},
            )
            for entry in reordered[section]
        }
        assert before == after


@pytest.mark.parametrize("invalid_index", [False, True])
@pytest.mark.parametrize("import_kind", ["ordinary", "intrinsic", "reserved"])
def test_wasm_abi_import_type_indices_require_exact_integers(
    import_kind: str, invalid_index: bool
) -> None:
    raw = _raw_manifest()
    name = {
        "ordinary": "inc_ref_obj",
        "intrinsic": "abs_builtin",
        "reserved": "object_new_bound",
    }[import_kind]
    if import_kind == "intrinsic":
        # Exercise intrinsic synthesis's explicit transport validator before
        # the synthesized callable metadata could bypass that boundary.
        imported = copy.deepcopy(_import_by_name(gen_wasm_abi.load_manifest(), name))
        imported.pop("runtime_name", None)
        imported.pop("callable_arity", None)
        raw["import"] = [entry for entry in raw["import"] if entry["name"] != name]
        raw["import"].append(imported)
    imported = _import_by_name(raw, name)
    old = imported["type"]
    position = int(invalid_index)
    raw["static_type"][old], raw["static_type"][position] = (
        raw["static_type"][position],
        raw["static_type"][old],
    )
    for entry in raw["import"]:
        if entry["type"] == old:
            entry["type"] = position
        elif entry["type"] == position:
            entry["type"] = old
    # The selected signature is correct even at index 0 or 1; only the index's
    # boolean identity makes this import invalid.
    imported["type"] = invalid_index
    with pytest.raises(manifest.WasmAbiManifestError, match="invalid .*type"):
        manifest.validate_loaded_manifest(raw)


def test_wasm_abi_manifest_owns_runtime_export_policy() -> None:
    gen = gen_wasm_abi
    data = gen.load_manifest()
    manifest_names = {entry["name"] for entry in data["import"]}
    imports_by_name = {entry["name"]: entry for entry in data["import"]}
    host_exports = set(data["runtime_export_policy"]["host_exports"])
    host_export_signatures = {
        entry["name"]: entry for entry in data["runtime_host_export_signature"]
    }

    runtime_exports_path = ROOT / "src/molt/_wasm_runtime_exports.py"
    text = runtime_exports_path.read_text(encoding="utf-8")
    assert "wasm_imports.rs" not in text
    assert "WASM_IMPORT_REGISTRY" in text
    assert "_normalize_runtime_export_name" not in text
    assert "_HOST_RUNTIME_EXPORTS" not in text
    assert "_BROWSER_RUNTIME_IMPORT_FALLBACK_EXPORTS" not in text
    assert {"alloc", "runtime_init", "socket_connect", "task_new"} <= manifest_names
    assert imports_by_name["runtime_init"]["runtime_name"] == "molt_runtime_init"
    assert "callable_arity" not in imports_by_name["runtime_init"]
    assert (
        imports_by_name["runtime_shutdown"]["runtime_name"] == "molt_runtime_shutdown"
    )
    assert "callable_arity" not in imports_by_name["runtime_shutdown"]
    assert {
        "molt_runtime_shutdown",
        "molt_set_wasm_table_base",
        "molt_gpu_matmul_contiguous",
    } <= host_exports
    assert host_export_signatures["molt_bool_from_i32"] == {
        "name": "molt_bool_from_i32",
        "params": ["i32"],
        "results": ["i64"],
    }
    assert host_export_signatures["molt_buffer_acquire"] == {
        "name": "molt_buffer_acquire",
        "params": ["i64", "i32"],
        "results": ["i32"],
    }
    assert host_export_signatures["molt_handle_resolve"] == {
        "name": "molt_handle_resolve",
        "params": ["i64"],
        "results": ["i32"],
    }
    assert {"molt_gpu_matmul_contiguous", "molt_gpu_tensor__zeros"} <= host_exports
    assert "gpu_intrinsic_manifest_name" not in data

    rendered_py = gen.render_py(data)
    rendered_js_abi = json.loads(gen.render_js_abi(data))
    rendered_rs = _rendered_rs(gen, data)
    assert "GPU_INTRINSIC_MANIFEST_NAMES" not in rendered_rs
    assert "WASM_GPU_INTRINSIC_MANIFEST_NAMES" not in rendered_py
    assert "WASM_RUNTIME_HOST_EXPORTS" in rendered_py
    assert "WASM_RUNTIME_HOST_EXPORT_SIGNATURES" in rendered_py
    assert "WASM_RUNTIME_IMPORT_FALLBACK_EXPORTS" not in rendered_py
    assert "WASM_RUNTIME_IMPORT_FALLBACK_SPECS" not in rendered_py
    assert "WASM_RUNTIME_IMPORT_EXPORT_NAMES" in rendered_py
    assert "WASM_RUNTIME_EXPORT_BY_IMPORT" in rendered_py
    assert "WASM_RUNTIME_IMPORT_BY_EXPORT" in rendered_py
    assert "WASM_EXTERNAL_NATIVE_LINK_IMPORT_SPLIT_EXPORT_NAMES" in rendered_py
    assert "WASM_EXTERNAL_NATIVE_LINK_IMPORT_BY_SPLIT_EXPORT_NAME" in rendered_py
    assert "WASM_EXTERNAL_NATIVE_LINK_IMPORT_SYMBOL_KINDS" in rendered_py
    assert "def wasm_runtime_import_name" in rendered_py
    assert "def wasm_runtime_export_name" in rendered_py
    assert '("alloc", "molt_alloc")' in rendered_py
    assert "if name in WASM_RUNTIME_HOST_EXPORTS" in rendered_py
    assert '("runtime_init", "molt_runtime_init")' in rendered_py
    assert '("runtime_execution_enter", "molt_runtime_execution_enter")' in rendered_py
    assert '("runtime_execution_leave", "molt_runtime_execution_leave")' in rendered_py
    assert '("runtime_shutdown", "molt_runtime_shutdown")' in rendered_py
    assert '("socket_drop", "molt_socket_drop")' in rendered_py
    assert '"PyArg_ParseTuple": "molt_PyArg_ParseTuple"' in rendered_py
    assert '"molt_PyArg_ParseTuple": "PyArg_ParseTuple"' in rendered_py
    assert '"Py_EllipsisObject": "molt_Py_EllipsisObject"' in rendered_py
    assert '"Py_EllipsisObject": "data"' in rendered_py
    assert '"PyEllipsis_Type": "molt_PyEllipsis_Type"' in rendered_py
    assert '"molt_PyEllipsis_Type": "PyEllipsis_Type"' in rendered_py
    assert '"PyEllipsis_Type": "data"' in rendered_py
    assert '"Py_GenericAliasType": "data"' in rendered_py
    assert '"Py_OptimizeFlag": "data"' in rendered_py
    assert '"PyType_Ready": "function"' in rendered_py
    assert rendered_js_abi["runtime_export_by_import"]["socket_drop"] == (
        "molt_socket_drop"
    )
    assert rendered_js_abi["runtime_export_by_import"]["runtime_execution_enter"] == (
        "molt_runtime_execution_enter"
    )
    assert rendered_js_abi["runtime_export_by_import"]["runtime_execution_leave"] == (
        "molt_runtime_execution_leave"
    )
    assert rendered_js_abi["runtime_export_by_import"]["PyArg_ParseTuple"] == (
        "molt_PyArg_ParseTuple"
    )
    assert rendered_js_abi["runtime_export_by_import"]["molt_PyArg_ParseTuple"] == (
        "molt_PyArg_ParseTuple"
    )
    assert "runtime_import_canonical_names" not in rendered_js_abi
    assert rendered_js_abi["runtime_export_by_import"]["PyType_Ready"] == (
        "molt_PyType_Ready"
    )
    assert "runtime_import_fallbacks" not in rendered_js_abi
    assert "runtime_export_name" in rendered_rs
    assert "RUNTIME_HOST_EXPORTS" in rendered_rs
    # The Python projection carries the table; Rust selects through a match.
    assert "RUNTIME_HOST_EXPORT_SIGNATURES" not in rendered_rs
    assert ".find(|export| *export == name)" in rendered_rs
    assert 'Self::Alloc => "molt_alloc"' in rendered_rs
    assert 'Self::RuntimeInit => "molt_runtime_init"' in rendered_rs
    assert '"molt_runtime_init" => Some(WasmRuntimeImport::RuntimeInit)' in rendered_rs
    assert (
        '"molt_runtime_shutdown" => Some(WasmRuntimeImport::RuntimeShutdown)'
        in rendered_rs
    )
    assert 'Self::SocketDrop => "molt_socket_drop"' in rendered_rs
    rendered_namespace: dict[str, object] = {}
    exec(rendered_py, rendered_namespace)
    # These independent public signatures must select the real producers, not
    # call-bind builders or borrowed getters with a compatible machine type.
    for name, arity in (
        ("fast_list_append", 2),
        ("fast_str_join", 2),
        ("fast_dict_get", 3),
        ("dict_setitem", 3),
        ("dict_getitem", 2),
        ("tuple_getitem", 2),
    ):
        export_name = f"molt_{name}"
        assert rendered_namespace["wasm_runtime_export_name"](name) == export_name
        assert rendered_namespace["wasm_runtime_import_name"](export_name) == name
        assert rendered_namespace["wasm_import_signature"](name) == (
            ("i64",) * arity,
            ("i64",),
        )
        assert rendered_js_abi["runtime_export_by_import"][name] == export_name
    assert rendered_namespace["wasm_runtime_import_name"]("molt_none") == "molt_none"
    assert rendered_namespace["wasm_runtime_export_name"]("molt_none") == "molt_none"
    assert rendered_namespace["wasm_import_signature"]("molt_none") == (
        (),
        ("i64",),
    )
    assert rendered_namespace["WASM_EXTERNAL_NATIVE_ARTIFACT_FUNCTION_SIGNATURES"][
        ("env", "molt_none")
    ] == {"params": [], "result": "i64"}
    assert rendered_namespace["wasm_import_signature"]("molt_bool_from_i32") == (
        ("i32",),
        ("i64",),
    )
    assert rendered_namespace["WASM_EXTERNAL_NATIVE_ARTIFACT_FUNCTION_SIGNATURES"][
        ("env", "molt_bool_from_i32")
    ] == {"params": ["i32"], "result": "i64"}


def test_runtime_export_sources_follow_cargo_dependency_authority() -> None:
    gen = gen_wasm_abi
    manifest = importlib.import_module("tools.wasm_abi_gen.manifest")

    roots = set(manifest._runtime_crate_dependency_roots())
    assert ROOT / "runtime/molt-runtime" in roots
    assert ROOT / "runtime/molt-gpu" in roots
    assert ROOT / "runtime/molt-gpu-runtime" in roots
    assert ROOT / "runtime/molt-runtime-core" in roots
    assert ROOT / "runtime/molt-cpython-abi" not in roots
    assert ROOT / "runtime/molt-backend-wasm" not in roots

    sources = set(manifest._runtime_rust_files())
    assert ROOT / "runtime/molt-gpu-runtime/src/bridge.rs" in sources
    assert not any(
        path.is_relative_to(ROOT / "runtime/molt-cpython-abi") for path in sources
    )

    data = gen.load_manifest()
    host_exports = set(data["runtime_export_policy"]["host_exports"])
    assert {
        "molt_gpu_matmul_contiguous",
        "molt_gpu_tensor__zeros",
    } <= host_exports


def test_wasm_abi_manifest_owns_pure_profile_prefixes() -> None:
    gen = gen_wasm_abi
    data = gen.load_manifest()
    prefixes = {entry["prefix"] for entry in data["pure_skip_prefix"]}
    assert {"process_", "socket", "db_", "ws_", "time_"} <= prefixes
    rendered_rs = _rendered_rs(gen, data)
    assert "pure_profile_skips_import" in rendered_rs
    assert "PURE_PROFILE_SKIP_PREFIXES" in rendered_rs


def test_wasm_abi_manifest_owns_runtime_callable_registry() -> None:
    gen = gen_wasm_abi
    data = gen.load_manifest()
    assert "runtime_feature" not in manifest.MANIFEST.read_text(encoding="utf-8")
    imports = {entry["name"]: entry for entry in data["import"]}

    assert _import_signature(data, "importlib_import_transaction") == (
        ("i64",) * 5,
        ("i64",),
    )
    assert {
        key: value
        for key, value in imports["importlib_import_transaction"].items()
        if key != "type"
    } == {
        "name": "importlib_import_transaction",
        "boxed_call": True,
    }
    assert (
        manifest.runtime_import_return_specs(data)["importlib_import_transaction"]
        == "owned_object"
    )
    assert imports["types_bootstrap"]["runtime_name"] == "molt_types_bootstrap"
    assert imports["types_bootstrap"]["callable_arity"] == 0
    assert imports["types_bootstrap"].get("shared_runtime_callable") is None

    assert imports["socket_drop"].get("callable_result", "i64") == "i64"
    assert imports["stream_close"].get("callable_result", "i64") == "i64"
    assert imports["future_features"]["runtime_name"] == "molt_future_features"
    assert imports["site_help0"]["callable_arity"] == 0
    assert imports["site_help1"]["callable_arity"] == 1
    assert imports["load_intrinsic_runtime"]["runtime_name"] == (
        "molt_load_intrinsic_runtime"
    )
    assert imports["load_intrinsic_runtime"]["callable_arity"] == 2
    assert imports["set_app_callable_resolver"]["runtime_name"] == (
        "molt_set_app_callable_resolver"
    )
    assert _import_signature(data, "set_app_callable_resolver") == (("i64",), ("i64",))
    assert "callable_arity" not in imports["set_app_callable_resolver"]
    assert imports["function_init_metadata_packed"]["runtime_name"] == (
        "molt_function_init_metadata_packed"
    )
    assert imports["function_init_metadata_packed"]["callable_arity"] == 4
    assert imports["file_open_ex"]["runtime_name"] == "molt_file_open_ex"
    assert imports["file_open_ex"]["callable_arity"] == 8
    assert imports["file_exit_method"]["runtime_name"] == "molt_file_exit_method"
    assert imports["file_exit_method"]["callable_arity"] == 4
    assert imports["env_mapping_keys"]["runtime_name"] == "molt_env_mapping_keys"
    assert imports["env_mapping_keys"]["callable_arity"] == 0
    assert imports["env_mapping_keys"].get("callable_result", "i64") == "i64"
    assert imports["typing_type_param"]["runtime_name"] == "molt_typing_type_param"
    assert imports["typing_type_param"]["callable_arity"] == 2

    shared_callables = gen._shared_runtime_callables(data)
    reserved_callables = data["reserved_runtime_callable"]
    assert [entry["index"] for entry in shared_callables] == list(
        range(len(shared_callables))
    )
    assert [
        {
            "index": entry["index"],
            "runtime_name": entry["runtime_name"],
            "import_name": entry["import_name"],
            "callable_arity": entry["callable_arity"],
            "callable_dispatch": entry["callable_dispatch"],
            "trampoline_abi": entry["trampoline_abi"],
        }
        for entry in shared_callables[: len(reserved_callables)]
    ] == [
        {
            **entry,
            "callable_dispatch": entry.get("callable_dispatch", "direct"),
            "trampoline_abi": entry.get("trampoline_abi", "unpack_args"),
        }
        for entry in reserved_callables
    ]
    assert {
        entry["runtime_name"]: entry["callable_arity"]
        for entry in shared_callables[len(reserved_callables) :]
    } == {
        "molt_coroutine_send_method": 2,
        "molt_coroutine_throw_method": 2,
        "molt_coroutine_close_method": 1,
        "molt_awaitable_await": 1,
        "molt_coroutine_wrapper_iter": 1,
        "molt_coroutine_wrapper_next": 1,
        "molt_asyncgen_awaitable_self": 1,
        "molt_asyncgen_awaitable_next": 1,
        "molt_asyncgen_awaitable_send": 2,
        "molt_asyncgen_awaitable_throw": 2,
        "molt_asyncgen_awaitable_close": 1,
        "molt_generator_throw_method": 2,
        **{
            f"molt_{name}": arity
            for name, arity in _contextvars_provider_arities().items()
        },
    }
    assert shared_callables[23] == {
        "index": 23,
        "runtime_name": "molt_importlib_import_transaction",
        "import_name": "importlib_import_transaction",
        "callable_arity": 5,
        "callable_dispatch": "trampoline",
        "trampoline_abi": "unpack_args",
        "runtime_cfg": None,
        "symbol_path": "molt_importlib_import_transaction",
    }
    assert shared_callables[24] == {
        "index": 24,
        "runtime_name": "molt_importlib_module_spec_init",
        "import_name": "importlib_module_spec_init",
        "callable_arity": 5,
        "callable_dispatch": "direct",
        "trampoline_abi": "unpack_args",
        "runtime_cfg": None,
        "symbol_path": "molt_importlib_module_spec_init",
    }
    assert all(
        entry["runtime_name"] != "molt_types_bootstrap" for entry in shared_callables
    )

    rendered_runtime = gen.render_runtime_callables_rs(data)
    assert "molt_importlib_import_transaction" in rendered_runtime
    assert "molt_cpython_abi_cext_call_trampoline" in rendered_runtime
    assert "molt_types_bootstrap" not in rendered_runtime
    assert (
        '(23, "molt_importlib_import_transaction", '
        '"importlib_import_transaction", 5, "trampoline")' in gen.render_py(data)
    )
    assert (
        '(24, "molt_importlib_module_spec_init", '
        '"importlib_module_spec_init", 5, "direct")' in gen.render_py(data)
    )

    broken_marker = copy.deepcopy(data)
    broken_marker["import"][0]["shared_runtime_callable"] = "yes"
    with pytest.raises(
        manifest.WasmAbiManifestError, match="invalid shared_runtime_callable"
    ):
        manifest.validate_loaded_manifest(broken_marker)

    broken_dispatch = copy.deepcopy(data)
    broken_dispatch["import"][0]["callable_dispatch"] = "sideways"
    with pytest.raises(
        manifest.WasmAbiManifestError, match="invalid callable_dispatch"
    ):
        manifest.validate_loaded_manifest(broken_dispatch)

    broken_missing_arity = copy.deepcopy(data)
    broken_missing_arity["import"][0]["shared_runtime_callable"] = True
    broken_missing_arity["import"][0].pop("callable_arity", None)
    with pytest.raises(
        manifest.WasmAbiManifestError,
        match="cannot set shared_runtime_callable without callable_arity",
    ):
        manifest.validate_loaded_manifest(broken_missing_arity)
    assert imports["logging_get_level_name"]["callable_arity"] == 1
    for retired in (
        "logging_stream_handler_new",
        "logging_stream_handler_emit",
        "logging_file_handler_emit",
        "logging_basic_config",
    ):
        assert retired not in imports
    for name, arity in {
        "abc_bootstrap": 0,
        "collections_abc_runtime_types": 0,
        "abc_get_cache_token": 0,
        "abc_init": 1,
        "abc_register": 2,
        "abc_instancecheck": 2,
        "abc_subclasscheck": 2,
        "abc_get_dump": 1,
        "abc_reset_registry": 1,
        "abc_reset_caches": 1,
        "abc_update_abstractmethods": 1,
    }.items():
        assert imports[name]["runtime_name"] == f"molt_{name}"
        assert imports[name]["callable_arity"] == arity
    for name, arity in {
        "array_new": 1,
        "array_from_list": 2,
        "array_append": 2,
        "array_buffer_info": 1,
        "array_count": 2,
        "array_delitem": 2,
        "array_extend": 2,
        "array_frombytes": 2,
        "array_getitem": 2,
        "array_index": 2,
        "array_insert": 3,
        "array_itemsize": 1,
        "array_len": 1,
        "array_pop": 2,
        "array_repeat": 2,
        "array_repeat_in_place": 2,
        "array_remove": 2,
        "array_reverse": 1,
        "array_setitem": 3,
        "array_tobytes": 1,
        "array_tolist": 1,
        "array_typecode": 1,
    }.items():
        assert imports[name]["runtime_name"] == f"molt_{name}"
        assert imports[name]["callable_arity"] == arity
    assert imports["runtime_active_runtime"]["callable_arity"] == 0
    assert imports["codecs_decode"]["runtime_name"] == "molt_codecs_decode"
    assert imports["codecs_decode"]["callable_arity"] == 4
    assert imports["codecs_encode"]["runtime_name"] == "molt_codecs_encode"
    assert imports["codecs_encode"]["callable_arity"] == 4
    assert imports["cbor_parse_scalar_obj"]["runtime_name"] == (
        "molt_cbor_parse_scalar_obj"
    )
    assert imports["cbor_parse_scalar_obj"]["callable_arity"] == 1
    assert imports["msgpack_parse_scalar_obj"]["runtime_name"] == (
        "molt_msgpack_parse_scalar_obj"
    )
    assert imports["msgpack_parse_scalar_obj"]["callable_arity"] == 1
    assert imports["thread_current_native_id"]["runtime_name"] == (
        "molt_thread_current_native_id"
    )
    assert imports["thread_current_native_id"]["callable_arity"] == 0
    assert imports["stream_drop"].get("runtime_feature") is None
    assert imports["asyncio_future_drop"]["runtime_feature"] == "stdlib_asyncio"
    # Pipe transports live in the core runtime, not the asyncio crate; a
    # zero-stdlib build defines them, so their declaration names no feature.
    assert imports["pipe_transport_drop"].get("runtime_feature") is None
    assert imports["email_message_drop"]["runtime_feature"] == "stdlib_email"
    assert imports["xml_element_drop"]["runtime_feature"] == "stdlib_xml"
    assert imports["statistics_mean"]["runtime_feature"] == "stdlib_math"
    assert imports["statistics_stdev"]["runtime_feature"] == "stdlib_math"
    assert not any(name.startswith("sqlite3_") for name in imports)
    for reserved in data["reserved_runtime_callable"]:
        reserved_import = imports[reserved["import_name"]]
        reserved_type = data["static_type"][reserved_import["type"]]
        assert reserved_type == {
            "params": ["i64"] * reserved["callable_arity"],
            "results": ["i64"],
        }
        assert "runtime_name" not in reserved_import
        assert "callable_arity" not in reserved_import
        assert "callable_result" not in reserved_import

    rendered_rs = _rendered_rs(gen, data)
    rendered_reserved_rs = gen.render_rs_modules(data)["runtime_callables.rs"]
    rendered_runtime_rs = gen.render_runtime_callables_rs(data)
    rendered_py = gen.render_py(data)
    assert "RUNTIME_CALLABLE_IMPORTS" in rendered_rs
    assert "use super::import_tokens::WasmRuntimeImport;" in rendered_rs
    assert "pub(crate) import: WasmRuntimeImport" in rendered_reserved_rs
    assert "import: None" not in rendered_reserved_rs
    assert "import: Some(" not in rendered_reserved_rs
    assert (
        "import: WasmRuntimeImport::ImportlibImportTransaction" in rendered_reserved_rs
    )
    assert "ReservedRuntimeCallableDispatch::Trampoline" in rendered_rs
    assert "pub(crate) fn runtime_callable_import" in rendered_rs
    callable_import_match = rendered_reserved_rs.split(
        "pub(crate) fn runtime_callable_import", 1
    )[1]
    for entry in gen._shared_runtime_callables(data):
        assert callable_import_match.count(f'"{entry["runtime_name"]}" =>') == 1
    assert '"molt_type_call" => Some(WasmRuntimeImport::TypeCall)' in rendered_rs
    assert (
        '"molt_importlib_import_transaction" => '
        "Some(WasmRuntimeImport::ImportlibImportTransaction)" in rendered_rs
    )
    assert (
        '"socket_drop" => Some(WasmRuntimeImport::SocketDrop),\n'
        '        "molt_socket_drop" => Some(WasmRuntimeImport::SocketDrop),'
    ) in rendered_rs
    assert "runtime_callable_import_name" not in rendered_rs
    assert "WASM_RUNTIME_CALLABLE_ARITY_BY_IMPORT" in rendered_py
    assert "WASM_RUNTIME_CALLABLE_IMPORTS" not in rendered_py
    assert "WASM_RUNTIME_CALLABLE_IMPORT_BY_RUNTIME" not in rendered_py
    assert "WASM_RUNTIME_CALLABLE_IMPORT_BY_IMPORT" not in rendered_py
    assert "WASM_RESERVED_RUNTIME_CALLABLE_SPEC_BY_RUNTIME" not in rendered_py
    assert "WASM_RESERVED_RUNTIME_CALLABLE_SPEC_BY_IMPORT" not in rendered_py
    assert "WASM_RESERVED_RUNTIME_CALLABLE_IMPORTS" not in rendered_py
    assert "WASM_RUNTIME_CALLABLE_LOOKUP_ROWS" not in rendered_py
    assert "def wasm_runtime_callable_spec(name: str)" not in rendered_py
    assert "def wasm_runtime_callable_import_name" not in rendered_py
    assert "def wasm_runtime_callable_arity" in rendered_py
    assert "def wasm_runtime_callable_result" not in rendered_py
    assert "molt_sqlite3_" not in rendered_rs
    assert "molt_sqlite3_" not in rendered_py
    assert "locale_getdefaultlocale" not in rendered_rs
    assert "locale_getdefaultlocale" not in rendered_py
    assert "ReservedRuntimeCallableSpec" in rendered_rs
    assert "RUNTIME_CALLABLE_IMPORTS" in rendered_rs
    assert "ReservedRuntimeCallableDispatch" in rendered_rs
    assert "RuntimeCallableTrampolineAbi" in rendered_rs
    assert "RuntimeCallableResult" not in rendered_rs
    assert "RESERVED_RUNTIME_CALLABLE_SPECS" in rendered_rs
    assert "RESERVED_RUNTIME_CALLABLE_COUNT" in rendered_rs
    assert "runtime_callable_key_from_symbol_name" in rendered_runtime_rs
    assert "runtime_callable_target_ptr" in rendered_runtime_rs
    assert "runtime_callable_returns_void_from_target_ptr" not in rendered_runtime_rs
    assert "pub(crate) fn python_builtin_function_info" in rendered_runtime_rs
    rendered_builtin_rs = gen.render_python_builtin_callables_rs(data)
    assert "pub fn python_builtin_callable(name: &str)" in rendered_builtin_rs
    assert "PYTHON_BUILTIN_CALLABLES" not in rendered_rs
    builtin_entries = sorted(
        gen._python_builtin_global_callables(data),
        key=lambda entry: entry["python_name"],
    )
    offsets = []
    for entry in builtin_entries:
        marker = f'python_name: "{entry["python_name"]}",'
        offsets.append(rendered_builtin_rs.index(marker))
        assert (
            marker
            + f'\n        python_module: "{entry["python_module"]}",'
            + f'\n        runtime_name: "{entry["runtime_name"]}",'
            + f"\n        arity: {entry['arity']},"
        ) in rendered_builtin_rs
    assert offsets == sorted(offsets), "binary-search projection must be name-sorted"
    runtime_offsets = [
        rendered_runtime_rs.index(f'python_name: "{entry["python_name"]}",')
        for entry in builtin_entries
    ]
    assert runtime_offsets == sorted(runtime_offsets), (
        "runtime metadata shares sorted lookup and enumeration"
    )
    assert '            python_name: "len",' in rendered_runtime_rs
    builtin_info_fields = rendered_runtime_rs.split(
        "pub(crate) struct PythonBuiltinFunctionInfo {", 1
    )[1].split("}", 1)[0]
    assert "pub(crate) index:" not in builtin_info_fields
    assert "pub(crate) python_module: &'static str" in builtin_info_fields
    assert '            runtime_name: "molt_len",' in rendered_runtime_rs
    assert "python_builtin_function_target_ptr" not in rendered_runtime_rs
    assert (
        "#[cfg(test)]\n#[rustfmt::skip]\n"
        "pub(crate) fn resolve_test_python_builtin_symbol(symbol: &str) -> Option<u64>"
    ) in rendered_runtime_rs
    for runtime_name in {entry["runtime_name"] for entry in builtin_entries}:
        assert (
            f'"{runtime_name}" => crate::{runtime_name} as *const (),'
            in rendered_runtime_rs
        )
    assert "pub(crate) fn python_builtin_function_info(" in rendered_runtime_rs
    assert "            fn_ptr:" not in rendered_runtime_rs
    assert '#[cfg(not(target_arch = "wasm32"))]' not in rendered_runtime_rs
    assert '            posonly_params: &["obj"],' in rendered_runtime_rs
    assert "            defaults: &[]," in rendered_runtime_rs
    assert '            python_name: "print",' in rendered_runtime_rs
    assert '            runtime_name: "molt_print_builtin",' in rendered_runtime_rs
    assert '            vararg: Some("args"),' in rendered_runtime_rs
    assert (
        '            kwonly_params: &["sep", "end", "file", "flush"],'
        in rendered_runtime_rs
    )
    assert (
        '            kw_defaults: &[("sep", GeneratedBuiltinDefaultValue::Str(" ")), '
        r'("end", GeneratedBuiltinDefaultValue::Str("\n")), '
        '("file", GeneratedBuiltinDefaultValue::None), '
        '("flush", GeneratedBuiltinDefaultValue::Bool(false))],'
    ) in rendered_runtime_rs
    assert "PYTHON_BUILTIN_FUNCTION_COUNT" not in rendered_runtime_rs
    assert "pub(crate) text_signature: Option<&'static str>" in rendered_runtime_rs
    assert "RUNTIME_VOID_CALLABLE_NAMES" not in rendered_runtime_rs
    assert "VOID_CALLABLE_TARGETS" not in rendered_runtime_rs
    assert "crate::intrinsics::resolve_symbol" not in rendered_runtime_rs
    void_runtime_names = {
        entry["runtime_name"]
        for entry in data["import"]
        if entry.get("callable_result") == "void"
    }
    assert not void_runtime_names
    assert "VOID_RESERVED_RUNTIME_CALLABLE_INDICES" not in rendered_runtime_rs
    for entry in gen._shared_runtime_callables(data):
        runtime_name = entry["runtime_name"]
        symbol_path = entry["symbol_path"]
        if symbol_path == runtime_name:
            assert (
                f"Some(crate::{runtime_name} as *const ())" not in rendered_runtime_rs
            )
            assert f"fn_key!(crate::{runtime_name})" not in rendered_runtime_rs
        assert f"Some({symbol_path} as *const ())" in rendered_runtime_rs
        assert (
            f"assert_runtime_symbol_key(fn_key!({symbol_path}), "
            f"RUNTIME_CALLABLE_KEY_BASE, {entry['index']}, {symbol_path} as *const ());"
        ) in rendered_runtime_rs
    assert "fn_addr!(crate::" not in rendered_runtime_rs
    assert "fn_addr!(molt_xml_element_drop)" not in rendered_runtime_rs
    assert "RUNTIME_POLL_CALLABLE_KEY_BASE" in rendered_runtime_rs
    # fn_key! and fn_addr! fold names at compile time through this const fn.
    assert (
        "pub(crate) const fn runtime_callable_key_from_symbol_name("
        "symbol_name: &[u8]) -> Option<u64> {"
    ) in rendered_runtime_rs
    assert (
        "return Some(RUNTIME_CALLABLE_KEY_BASE + entry.index);" in rendered_runtime_rs
    )
    assert (
        "assert_runtime_symbol_key(fn_key!(crate::molt_async_sleep_poll), "
        "RUNTIME_POLL_CALLABLE_KEY_BASE, 1, crate::molt_async_sleep_poll as *const ());"
    ) in rendered_runtime_rs
    assert 'runtime_name: "molt_type_call"' in rendered_runtime_rs
    assert '"type_call" => Some(WasmRuntimeImport::TypeCall)' in rendered_rs
    assert (
        '"object_new_bound" => Some(WasmRuntimeImport::ObjectNewBound)' in rendered_rs
    )
    assert "1 => Some(crate::molt_async_sleep_poll as *const ())" in rendered_runtime_rs

    wasm_abi = (ROOT / "runtime/molt-backend-wasm/src/wasm_abi.rs").read_text(
        encoding="utf-8"
    )
    assert "wasm_runtime_callables.inc" not in wasm_abi
    assert "macro_rules! entry_list" not in wasm_abi
    assert "runtime_callable_import_name" not in wasm_abi

    function_abi = (
        ROOT / "runtime/molt-runtime/src/builtins/functions/function_abi.rs"
    ).read_text(encoding="utf-8")
    call_function = (ROOT / "runtime/molt-runtime/src/call/function.rs").read_text(
        encoding="utf-8"
    )
    assert "wasm_runtime_callables.inc" not in function_abi
    assert "wasm_poll_callables.inc" not in function_abi
    assert '"molt_type_call" => Some' not in function_abi
    assert "molt_async_sleep_poll as *const" not in function_abi
    assert "runtime_callable_returns_void" not in function_abi
    assert "VOID_INTRINSICS" not in call_function
    assert "runtime_callable_returns_void(fn_ptr)" not in call_function


def test_native_poll_keys_follow_manifest_slots_in_runtime_and_compiler() -> None:
    gen = gen_wasm_abi
    data = copy.deepcopy(gen.load_manifest())
    poll = next(entry for entry in data["import"] if "poll_table_slot" in entry)
    added = copy.deepcopy(poll)
    added["name"] = "test_added_poll"
    added["poll_table_slot"] = 1 + max(
        entry.get("poll_table_slot", 0) for entry in data["import"]
    )
    data["import"].append(added)
    rendered = gen.render_runtime_callables_rs(data)
    compiler = gen.render_runtime_callable_abi_rs(data)
    keys = compiler.split("pub fn runtime_poll_native_key(symbol: &str)", 1)[1]
    assert "const BASE: u64 = 0xFFFFFF0000000100;" in keys
    assert "RUNTIME_CALLABLE_KEY_BASE: u64 = 0xFFFFFF0000000000;" in rendered
    assert "RUNTIME_CALLABLE_KEY_BASE + 0x100;" in rendered
    expected = {
        f"molt_{entry['name']}": entry["poll_table_slot"]
        for entry in data["import"]
        if "poll_table_slot" in entry
    }
    projected = dict(re.findall(r'"([^"]+)" => Some\(BASE \+ (\d+)\)', keys))
    assert {name: int(slot) for name, slot in projected.items()} == expected
    poll_table = rendered.split("pub(crate) const RUNTIME_POLL_CALLABLES", 1)[1]
    poll_table = poll_table.split("];", 1)[0]
    runtime_projected = {
        name: int(slot)
        for slot, name in re.findall(r'\((\d+), "([^"]+)"\)', poll_table)
    }
    assert runtime_projected == expected
    assert "_ => None" in keys
    assert "ordinary_poll" not in keys
    assert f"WASM_POLL_SLOT_MAX_OFFSET: u64 = {added['poll_table_slot']};" in rendered
    data["import"].pop()
    without = gen.render_runtime_callables_rs(data)
    assert "crate::molt_test_added_poll as *const ()" not in without
    assert "molt_test_added_poll" not in gen.render_runtime_callable_abi_rs(data)


def test_python_builtin_module_metadata_matches_cpython() -> None:
    import builtins

    gen = gen_wasm_abi
    entries = gen._python_builtin_global_callables(gen.load_manifest())
    assert entries
    for entry in entries:
        assert (
            entry["python_module"] == getattr(builtins, entry["python_name"]).__module__
        ), entry["python_name"]


def test_wasm_runtime_callable_resolver_is_app_local_in_production() -> None:
    registry = (ROOT / "runtime/molt-runtime/src/intrinsics/registry.rs").read_text(
        encoding="utf-8"
    )
    intrinsics_mod = (ROOT / "runtime/molt-runtime/src/intrinsics/mod.rs").read_text(
        encoding="utf-8"
    )

    assert "#[cfg(test)]\nuse crate::intrinsics::generated::resolve_symbol;" in registry
    assert '#[cfg(any(target_arch = "wasm32", test))]' not in registry
    assert '#[cfg(not(any(target_arch = "wasm32", test)))]' not in registry
    assert 'pub extern "C" fn molt_set_app_callable_resolver' in registry
    assert "pub(crate) fn try_app_resolve_runtime_callable" in registry
    assert "try_app_resolve_symbol(spec.symbol)" in registry
    assert "pub(crate) use registry::try_app_resolve_symbol;" in intrinsics_mod
    function_abi = (
        ROOT / "runtime/molt-runtime/src/builtins/functions/function_abi.rs"
    ).read_text(encoding="utf-8")
    assert "try_app_resolve_symbol(info.runtime_name)" in function_abi
    assert "python_builtin_function_target_ptr" not in function_abi


def test_wasm_trampoline_keeps_the_cross_target_owned_result_contract() -> None:
    source = (ROOT / "runtime/molt-runtime/src/call/function.rs").read_text(
        encoding="utf-8"
    )
    body = source.split("pub(crate) unsafe fn call_function_obj_trampoline", 1)[
        1
    ].split("pub(crate) unsafe fn call_function_obj_vec", 1)[0]

    assert "fresh results and argument aliases both arrive" in body
    assert "inc_ref_bits(_py, res)" not in body


def test_runtime_features_are_derived_not_manifest_owned() -> None:
    gen = gen_wasm_abi
    loaded = gen.load_manifest()
    manifest.validate_loaded_manifest(copy.deepcopy(loaded))
    loaded_imports = {entry["name"]: entry for entry in loaded["import"]}

    assert loaded_imports["hash_builtin"].get("runtime_feature") is None
    assert not {
        "bz2_compressor_drop",
        "bz2_decompressor_drop",
        "bz2_file_drop",
        "gzip_decompress",
        "gzip_drop",
        "hash_drop",
        "hmac_drop",
        "lzma_compressor_drop",
        "lzma_decompressor_drop",
        "lzma_file_drop",
        "secrets_compare_digest",
        "zlib_compressobj_drop",
        "zlib_decompressobj_drop",
    }.intersection(loaded_imports)

    for name in (
        "hash_new",
        "hash_update",
        "hash_copy",
        "hash_digest",
    ):
        assert loaded_imports[name]["runtime_feature"] == "stdlib_crypto"
        assert loaded_imports[name]["runtime_cfg"] == 'feature = "stdlib_crypto"'
    # A non-intrinsic runtime export declares its own cfg on its import entry.
    assert loaded_imports["cbor_parse_scalar"]["runtime_feature"] == (
        "stdlib_serialization"
    )
    assert loaded_imports["socketpair"]["runtime_cfg"] == (
        'all(feature = "stdlib_net", not(target_os = "windows"))'
    )

    runtime_callables = gen.render_runtime_callables_rs(loaded)
    hash_builtin_projection = runtime_callables.split(
        '            python_name: "hash",', 1
    )[1].split("        },", 1)[0]
    assert 'runtime_name: "molt_hash_builtin"' in hash_builtin_projection
    assert '#[cfg(feature = "stdlib_crypto")]' not in hash_builtin_projection

    raw = _raw_manifest()
    for entry in raw["import"]:
        if entry["name"] == "hash_builtin":
            entry["runtime_feature"] = "stdlib_crypto"
            break
    else:  # pragma: no cover - fixture corruption
        raise AssertionError("hash_builtin import missing")

    with pytest.raises(
        manifest.WasmAbiManifestError,
        match="runtime_cfg and runtime_feature are generated from declared cfgs",
    ):
        manifest.validate_loaded_manifest(raw, reject_manual_runtime_cfgs=True)

    raw = _raw_manifest()
    for entry in raw["import"]:
        if entry["name"] == "cbor_parse_scalar_obj":
            entry["cfg"] = 'feature = "stdlib_serialization"'
            break
    else:  # pragma: no cover - fixture corruption
        raise AssertionError("cbor_parse_scalar_obj import missing")
    with pytest.raises(manifest.WasmAbiManifestError, match="is an intrinsic"):
        manifest.validate_loaded_manifest(raw)


def test_python_builtins_must_be_defined_in_every_runtime() -> None:
    # A builtin is reached by a global name lookup, which reachability does
    # not map to a feature, so a gated builtin would fail at run time.
    gen = gen_wasm_abi
    data = copy.deepcopy(gen.load_manifest())
    gen._python_builtin_global_callables(data)
    for entry in data["import"]:
        if entry.get("runtime_name") == "molt_compile_builtin":
            assert entry.get("runtime_cfg") is None
            entry["runtime_cfg"] = 'feature = "stdlib_ast"'
            break
    else:  # pragma: no cover - fixture corruption
        raise AssertionError("compile_builtin import missing")
    with pytest.raises(ValueError, match="must be defined in every runtime"):
        gen._python_builtin_global_callables(data)


def test_wasm_abi_reserved_runtime_callable_import_names_are_fail_closed() -> None:
    gen = gen_wasm_abi
    data = gen.load_manifest()

    rematerialized = copy.deepcopy(data)
    rematerialized["import"] = [
        entry for entry in rematerialized["import"] if entry["name"] != "type_call"
    ]
    manifest.validate_loaded_manifest(rematerialized)
    assert {
        key: value
        for key, value in _import_by_name(rematerialized, "type_call").items()
        if key != "type"
    } == {"name": "type_call"}
    assert _import_signature(rematerialized, "type_call") == (("i64",), ("i64",))

    broken = copy.deepcopy(data)
    broken["import"].append(copy.deepcopy(_import_by_name(data, "type_call")))
    with pytest.raises(
        manifest.WasmAbiManifestError, match="duplicate import name 'type_call'"
    ):
        manifest.validate_loaded_manifest(broken)
    broken = copy.deepcopy(data)
    for entry in broken["import"]:
        if entry["name"] == "object_new_bound":
            entry["runtime_name"] = "molt_object_new_bound"
            entry["callable_arity"] = 1
            break
    with pytest.raises(
        manifest.WasmAbiManifestError,
        match="transport import must not duplicate reserved callable metadata",
    ):
        manifest.validate_loaded_manifest(broken)

    broken = copy.deepcopy(data)
    for entry in broken["import"]:
        if entry["name"] == "object_new_bound":
            entry["type"] = _import_by_name(data, "importlib_import_transaction")[
                "type"
            ]
            break
    with pytest.raises(
        manifest.WasmAbiManifestError,
        match="reserved runtime callable 'molt_object_new_bound'",
    ):
        manifest.validate_loaded_manifest(broken)

    broken = copy.deepcopy(data)
    broken["import"].append(
        {
            "name": "shadow_type_call",
            "type": _import_by_name(data, "type_call")["type"],
            "runtime_name": "molt_type_call",
            "callable_arity": 1,
        }
    )
    with pytest.raises(
        manifest.WasmAbiManifestError,
        match="reserved runtime callable 'molt_type_call'",
    ):
        manifest.validate_loaded_manifest(broken)


def test_witness_frontier_runtime_callables_must_be_reserved() -> None:
    gen = gen_wasm_abi
    data = gen.load_manifest()

    broken = copy.deepcopy(data)
    broken["reserved_runtime_callable"] = [
        entry
        for entry in broken["reserved_runtime_callable"]
        if entry["runtime_name"] != "molt_type_new"
    ]
    for index, entry in enumerate(broken["reserved_runtime_callable"]):
        entry["index"] = index

    with pytest.raises(
        manifest.WasmAbiManifestError,
        match="witness-frontier runtime callables are not reserved: molt_type_new",
    ):
        manifest.validate_loaded_manifest(broken)


def test_dynamic_builtin_family_uses_shared_python_specs_and_reserved_abi() -> None:
    gen = gen_wasm_abi
    rows = {
        entry["python_name"]: entry
        for entry in gen._python_builtin_global_callables(gen.load_manifest())
    }
    for name, symbol, arity in (
        ("globals", "molt_globals_builtin", 0),
        ("locals", "molt_locals_builtin", 0),
        ("vars", "molt_vars_builtin", 1),
        ("__import__", "molt_importlib_import_transaction", 5),
    ):
        assert rows[name]["runtime_name"] == symbol
        assert rows[name]["arity"] == arity
    assert (
        gen._rust_builtin_default_value(rows["__import__"]["defaults"][0])
        == "GeneratedBuiltinDefaultValue::Missing"
    )
    assert (
        gen._rust_builtin_default_value(rows["__import__"]["defaults"][2])
        == "GeneratedBuiltinDefaultValue::EmptyTuple"
    )
    assert rows["__import__"]["pos_or_kw_params"] == [
        "name",
        "globals",
        "locals",
        "fromlist",
        "level",
    ]


def test_wasm_abi_runtime_import_aliases_are_unambiguous() -> None:
    gen = gen_wasm_abi
    data = gen.load_manifest()

    broken = copy.deepcopy(data)
    broken["import"].append(
        {
            "name": "molt_socket_drop",
            "type": _import_by_name(data, "socket_drop")["type"],
        }
    )
    with pytest.raises(
        manifest.WasmAbiManifestError,
        match="runtime import aliases collide with canonical import names",
    ):
        manifest.validate_loaded_manifest(broken)


def test_wasm_abi_manifest_classifies_raw_intrinsics_fail_closed() -> None:
    gen = gen_wasm_abi
    data = gen.load_manifest()
    imports = {entry["name"]: entry for entry in data["import"]}
    runtime_callables = {
        entry["runtime_name"] for entry in data["import"] if "runtime_name" in entry
    }

    assert "molt_json_parse_scalar" in data["non_runtime_callable_intrinsic"]
    assert "molt_gpu_prim_create_tensor" in data["non_runtime_callable_intrinsic"]
    assert {
        "molt_dict_getitem_borrowed",
        "molt_list_getitem_borrowed",
        "molt_tuple_getitem_borrowed",
    } <= set(data["non_runtime_callable_intrinsic"])
    assert "runtime_name" not in imports["json_parse_scalar"]
    assert "molt_json_parse_scalar" not in runtime_callables
    assert "molt_gpu_prim_create_tensor" not in runtime_callables
    assert "molt_dict_getitem_borrowed" not in runtime_callables
    assert "molt_list_getitem_borrowed" not in runtime_callables
    assert "molt_tuple_getitem_borrowed" not in runtime_callables
    for import_name in (
        "dict_getitem_borrowed",
        "list_getitem_borrowed",
        "tuple_getitem_borrowed",
    ):
        assert "runtime_name" not in imports[import_name]
        assert "callable_arity" not in imports[import_name]
    assert "WASM_NON_RUNTIME_CALLABLE_INTRINSICS" in gen.render_py(data)
    assert "runtime_callable_symbol_is_non_callable" in gen.render_runtime_callables_rs(
        data
    )
    assert imports["json_parse_scalar_obj"]["runtime_name"] == (
        "molt_json_parse_scalar_obj"
    )

    broken = _raw_manifest()
    broken["non_runtime_callable_intrinsic"] = []
    with pytest.raises(
        manifest.WasmAbiManifestError,
        match="molt_json_parse_scalar.*non_runtime_callable_intrinsic",
    ):
        manifest.validate_loaded_manifest(broken)


def test_wasm_abi_runtime_callable_intrinsics_match_rust_exports() -> None:
    gen = gen_wasm_abi
    data = gen.load_manifest()

    # load_manifest performs the full Rust-export ABI validation; keep readable
    # sentinels for explicit manifest-owned callables and the old broad
    # synthesis class.
    imports = {
        entry["runtime_name"]: entry
        for entry in data["import"]
        if "runtime_name" in entry
    }
    reserved = {
        entry["runtime_name"]: entry for entry in data["reserved_runtime_callable"]
    }
    assert reserved["molt_importlib_import_transaction"]["callable_arity"] == 5
    assert "molt_importlib_import_transaction" not in imports
    assert imports["molt_load_intrinsic_runtime"]["callable_arity"] == 2
    assert imports["molt_types_bootstrap"]["callable_arity"] == 0
    assert imports["molt_is_bound_method"]["callable_arity"] == 1
    assert imports["molt_file_exit_method"]["callable_arity"] == 4
    assert imports["molt_logging_get_level_name"]["callable_arity"] == 1
    for retired in (
        "molt_logging_formatter_drop",
        "molt_logging_formatter_format",
        "molt_logging_formatter_format_time",
        "molt_logging_formatter_new",
        "molt_logging_handler_emit",
        "molt_logging_handler_set_formatter",
        "molt_logging_percent_style_format",
        "molt_logging_record_drop",
        "molt_logging_record_get_message",
        "molt_logging_record_new",
        "molt_logging_stream_handler_new",
        "molt_logging_stream_handler_emit",
        "molt_logging_file_handler_emit",
        "molt_logging_basic_config",
    ):
        assert retired not in imports


@pytest.mark.parametrize("symbol", ["molt_socket_drop", "molt_asyncio_future_drop"])
@pytest.mark.parametrize("mixed_target_signatures", [False, True])
def test_python_callable_provider_rejects_void_on_any_target(
    monkeypatch: pytest.MonkeyPatch, symbol: str, mixed_target_signatures: bool
) -> None:
    # Both explicit and synthesized imports must fail closed. A correct native
    # definition must not conceal a void cfg-gated WASM definition (or vice versa).
    exports = dict(manifest._rust_export_signatures(include_native=True))
    exports[symbol] = {(("i64",), "void")}
    if mixed_target_signatures:
        exports[symbol].add((("i64",), "i64"))
    monkeypatch.setattr(manifest, "_rust_export_signatures", lambda **kwargs: exports)
    with pytest.raises(
        manifest.WasmAbiManifestError, match=rf"callable ABI mismatches:.*{symbol}"
    ):
        manifest.validate_loaded_manifest(_raw_manifest())


@pytest.mark.parametrize("section", ["import", "reserved_runtime_callable"])
def test_python_callable_manifest_cannot_opt_into_void(section: str) -> None:
    data = _raw_manifest()
    entry = next(row for row in data[section] if "callable_arity" in row)
    entry["callable_result"] = "void"
    with pytest.raises(manifest.WasmAbiManifestError, match="must be i64"):
        manifest.validate_loaded_manifest(data)


def test_reserved_python_callable_rejects_native_only_void(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    scan = manifest._rust_export_signatures

    def signatures(*, include_native: bool = False):
        exports = dict(scan(include_native=include_native))
        if include_native:
            original = exports["molt_type_new"]
            exports["molt_type_new"] = original | {
                (params, "void") for params, _ in original
            }
        return exports

    monkeypatch.setattr(manifest, "_rust_export_signatures", signatures)
    with pytest.raises(
        manifest.WasmAbiManifestError, match="reserved runtime callable 'molt_type_new'"
    ):
        manifest.validate_loaded_manifest(_raw_manifest())


def test_python_callable_abi_and_cache_include_native_only_providers(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    provider = tmp_path / "providers.rs"
    provider.write_text(
        '#[cfg(not(target_arch = "wasm32"))]\n'
        "#[unsafe(no_mangle)]\n"
        'pub extern "C" fn molt_process_drop(handle: u64) {}\n'
        '#[cfg(target_arch = "wasm32")]\n'
        "#[unsafe(no_mangle)]\n"
        'pub extern "C" fn molt_process_drop(handle: u64) -> u64 { 0 }\n',
        encoding="utf-8",
    )
    monkeypatch.setattr(manifest, "_runtime_rust_files", lambda: [provider])
    monkeypatch.setattr(manifest, "_rust_type_aliases", lambda: {})
    manifest._rust_export_signatures.cache_clear()
    try:
        assert manifest._rust_export_signatures()["molt_process_drop"] == {
            (("i64",), "i64")
        }
        assert ("molt_process_drop", ("i64",), "void") in (
            manifest.generator_runtime_export_signature_rows()
        )
        with pytest.raises(manifest.WasmAbiManifestError, match="ABI mismatches"):
            manifest._validate_runtime_callable_export_abi(
                [{"runtime_name": "molt_process_drop", "callable_arity": 1}]
            )
    finally:
        manifest._rust_export_signatures.cache_clear()


def test_wasm_abi_manifest_owns_lir_runtime_calls() -> None:
    gen = gen_wasm_abi
    data = gen.load_manifest()
    calls = {entry["variant"]: entry for entry in data["lir_runtime_call"]}
    op_loop_calls = {entry["kind"]: entry for entry in data["op_loop_runtime_call"]}

    assert calls["Add"]["import_name"] == "add"
    assert calls["Add"]["boxed_operand_count"] == 2
    assert calls["FloorDiv"]["import_name"] == "floordiv"
    assert calls["Neg"]["boxed_operand_count"] == 1
    assert calls["StoreIndex"]["boxed_operand_count"] == 3
    assert calls["GetAttrName"]["boxed_operand_count"] == 2
    assert calls["Iter"]["boxed_operand_count"] == 1
    assert calls["ModuleImportStar"] == {
        "variant": "ModuleImportStar",
        "import_name": "module_import_star",
    }
    assert calls["ContextExit"] == {
        "variant": "ContextExit",
        "import_name": "context_exit",
    }
    assert calls["IntFromI64"]["import_name"] == "int_from_i64"
    assert calls["IntAsI64"]["import_name"] == "int_as_i64"
    assert op_loop_calls["module_import_star"]["lir_variant"] == "ModuleImportStar"
    assert op_loop_calls["module_import_star"]["lir_operand_count"] == 2
    assert op_loop_calls["context_exit"]["lir_variant"] == "ContextExit"
    assert op_loop_calls["context_exit"]["lir_operand_count"] == 2
    # The scalar parse kinds lost their last producer (HF-197).
    assert not {"json_parse", "msgpack_parse", "cbor_parse"} & op_loop_calls.keys()
    assert "sink" not in op_loop_calls["asyncgen_new"]
    assert "dec_ref_obj" not in op_loop_calls["asyncgen_new"]["required_imports"]
    assert "chan_drop" not in op_loop_calls
    finally_observer = op_loop_calls["exception_finally_pending_observer"]
    assert finally_observer["import_name"] == "exception_last_pending"
    assert (
        finally_observer["marked_import_name"]
        == "async_work_poll_and_exception_last_pending"
    )
    assert finally_observer["required_imports"] == ["exception_last_pending"]
    assert op_loop_calls["gpu_thread_id"] == {
        "kind": "gpu_thread_id",
        "import_name": "gpu_thread_id",
        "args": [],
        "required_imports": ["gpu_thread_id"],
    }
    assert op_loop_calls["gpu_barrier"] == {
        "kind": "gpu_barrier",
        "import_name": "gpu_barrier",
        "args": [],
        "required_imports": ["gpu_barrier"],
    }

    rendered_rs_modules = gen.render_rs_modules(data)
    rendered_lir_rs = rendered_rs_modules["lir_runtime_calls.rs"]
    assert "enum LirRuntimeCall" in rendered_lir_rs
    assert "OpLoopRuntimeSinkSpec" not in rendered_lir_rs
    assert "discard_result: bool" in rendered_lir_rs
    assert "use super::import_tokens::WasmRuntimeImport;" in rendered_lir_rs
    assert "pub(crate) const fn import(self) -> WasmRuntimeImport" in rendered_lir_rs
    assert "Self::FloorDiv => WasmRuntimeImport::Floordiv" in rendered_lir_rs
    assert "pub(crate) const fn boxed_operand_count" in rendered_lir_rs
    assert "Self::StoreIndex => Some(3)" in rendered_lir_rs
    assert "Self::ModuleImport => WasmRuntimeImport::ModuleImport" in rendered_lir_rs
    assert 'Self::ModuleImport => "module_import"' not in rendered_lir_rs
    assert "Self::ModuleImport => Some" not in rendered_lir_rs
    assert "pub(crate) const fn import_name" not in rendered_lir_rs
    assert "self.import().name()" not in rendered_lir_rs
    assert "lir_fixed_runtime_call" in rendered_lir_rs
    assert "molt_ir::OpKind::ContextExit => Some(LirFixedRuntimeCall" in rendered_lir_rs
    assert "pub(crate) fn op_loop_runtime_call(" in rendered_lir_rs
    assert "marked: bool" in rendered_lir_rs
    assert (
        "molt_ir::OpKind::ExceptionFinallyPendingObserver if marked => "
        "Some(OpLoopRuntimeCallSpec {" in rendered_lir_rs
    )
    assert (
        "import: WasmRuntimeImport::AsyncWorkPollAndExceptionLastPending"
        in rendered_lir_rs
    )
    native_rows = gen._native_marked_op_loop_symbol_rows(data)
    assert native_rows == (
        (
            "exception_finally_pending_observer",
            "molt_exception_last_pending",
            "molt_async_work_poll_and_exception_last_pending",
        ),
        ("exception_last_pending", "molt_exception_last_pending", None),
    )
    rendered_native_rs = gen.render_native_exception_observer_abi_rs(data)
    assert "@generated by tools/gen_wasm_abi.py" in rendered_native_rs
    assert (
        "molt_ir::OpKind::ExceptionFinallyPendingObserver if marked"
        in rendered_native_rs
    )
    assert (
        'Some("molt_async_work_poll_and_exception_last_pending")' in rendered_native_rs
    )
    assert (
        "molt_ir::OpKind::ExceptionFinallyPendingObserver | molt_ir::OpKind::ExceptionLastPending"
        in re.sub(r"\s+", " ", rendered_native_rs)
    )
    assert 'Some("molt_exception_last_pending")' in rendered_native_rs
    assert generated_file_matches(
        OUT_NATIVE_EXCEPTION_OBSERVER_ABI_RS, rendered_native_rs
    )

    second_marked = copy.deepcopy(data)
    synthetic = copy.deepcopy(finally_observer)
    synthetic["kind"] = "exception_current"
    second_marked["op_loop_runtime_call"].append(synthetic)
    assert (
        "exception_current",
        "molt_exception_last_pending",
        "molt_async_work_poll_and_exception_last_pending",
    ) in gen._native_marked_op_loop_symbol_rows(second_marked)
    assert "OpKind::ExceptionCurrent" in (
        gen.render_native_exception_observer_abi_rs(second_marked)
    )

    broken = copy.deepcopy(data)
    broken["lir_runtime_call"][0]["import_name"] = "not_a_real_import"
    with pytest.raises(
        manifest.WasmAbiManifestError, match="references unknown import"
    ):
        manifest.validate_loaded_manifest(broken)
    broken_count = copy.deepcopy(data)
    broken_count["lir_runtime_call"][0]["boxed_operand_count"] = -1
    with pytest.raises(manifest.WasmAbiManifestError, match="boxed_operand_count"):
        manifest.validate_loaded_manifest(broken_count)

    for helper in ("dec_ref_obj", "inc_ref_obj"):
        missing_refcount_import = copy.deepcopy(data)
        missing_refcount_import["import"] = [
            entry
            for entry in missing_refcount_import["import"]
            if entry["name"] != helper
        ]
        missing_refcount_import["lir_runtime_call"] = [
            entry
            for entry in missing_refcount_import["lir_runtime_call"]
            if entry["import_name"] != helper
        ]
        with pytest.raises(
            manifest.WasmAbiManifestError,
            match=f"runtime return contracts require {helper}",
        ):
            manifest.validate_loaded_manifest(missing_refcount_import)

    broken_marked_import = copy.deepcopy(data)
    next(
        entry
        for entry in broken_marked_import["op_loop_runtime_call"]
        if entry["kind"] == "exception_finally_pending_observer"
    )["marked_import_name"] = "not_a_real_import"
    with pytest.raises(manifest.WasmAbiManifestError, match="unknown marked import"):
        manifest.validate_loaded_manifest(broken_marked_import)

    duplicate_marked_import = copy.deepcopy(data)
    observer = next(
        entry
        for entry in duplicate_marked_import["op_loop_runtime_call"]
        if entry["kind"] == "exception_finally_pending_observer"
    )
    observer["marked_import_name"] = observer["import_name"]
    with pytest.raises(manifest.WasmAbiManifestError, match="must differ"):
        manifest.validate_loaded_manifest(duplicate_marked_import)

    local_facade = (
        ROOT / "runtime/molt-backend-wasm/src/wasm/lir_fast/runtime_calls.rs"
    ).read_text(encoding="utf-8")
    assert "enum LirRuntimeCall" not in local_facade
    assert "match kind" not in local_facade
    assert "lir_fixed_runtime_call" in local_facade
    assert "LirFixedRuntimeCall" in local_facade
    assert "crate::wasm_abi_generated::LirRuntimeCall" in local_facade

    call_abi = (
        ROOT / "runtime/molt-backend-wasm/src/wasm/lir_fast/lir_runtime_ops/call_abi.rs"
    ).read_text(encoding="utf-8")
    assert "boxed_operand_count().unwrap_or_else" in call_abi
    assert "runtime_call, 2" not in call_abi


def test_wasm_abi_manifest_owns_container_runtime_selector() -> None:
    gen = gen_wasm_abi
    data = gen.load_manifest()
    selectors = {
        (entry["op"], entry["fact"]): (
            entry["import_name"],
            entry.get("lir_variant"),
        )
        for entry in data["container_runtime_selector"]
    }

    assert selectors == {
        ("index", "flat_list_int"): ("list_int_getitem", "ListIntGetitem"),
        ("store_index", "flat_list_int"): (
            "list_int_setitem",
            "ListIntSetitem",
        ),
        ("index", "dict"): ("dict_getitem", "DictGetitem"),
        ("index", "tuple"): ("tuple_getitem", "TupleGetitem"),
        ("store_index", "dict"): ("dict_setitem", "DictSetitem"),
        ("contains", "set"): ("set_contains", "SetContains"),
        ("contains", "dict"): ("dict_contains", "DictContains"),
        ("contains", "list"): ("list_contains", "ListContains"),
        ("contains", "str"): ("str_contains", "StrContains"),
        ("len", "list"): ("len_list", None),
        ("len", "str"): ("len_str", None),
        ("len", "dict"): ("len_dict", None),
        ("len", "tuple"): ("len_tuple", None),
        ("len", "set"): ("len_set", None),
    }

    rendered_rs_modules = gen.render_rs_modules(data)
    rendered_selector_rs = rendered_rs_modules["container_runtime_selector.rs"]
    rendered_mod_rs = rendered_rs_modules["mod.rs"]
    rendered_py = gen.render_py(data)
    # The Python projection carries the table; Rust selects through a match.
    assert "WASM_CONTAINER_RUNTIME_SELECTORS" not in rendered_selector_rs
    assert "WasmContainerRuntimeFact::FlatListInt" in rendered_selector_rs
    assert "import: WasmRuntimeImport::ListIntGetitem" in rendered_selector_rs
    assert "LirRuntimeCall::ListIntGetitem" in rendered_selector_rs
    assert "wasm_container_runtime_selection" in rendered_selector_rs
    assert "mod container_runtime_selector;" in rendered_mod_rs
    assert "WasmContainerRuntimeOp" in rendered_mod_rs
    assert "WASM_CONTAINER_RUNTIME_SELECTORS" in rendered_py
    assert (
        '("index", "flat_list_int", "list_int_getitem", "ListIntGetitem")'
        in rendered_py
    )

    broken_duplicate = copy.deepcopy(data)
    broken_duplicate["container_runtime_selector"].append(
        copy.deepcopy(broken_duplicate["container_runtime_selector"][0])
    )
    with pytest.raises(
        manifest.WasmAbiManifestError, match="duplicate container_runtime_selector"
    ):
        manifest.validate_loaded_manifest(broken_duplicate)

    broken_import = copy.deepcopy(data)
    broken_import["container_runtime_selector"][0]["import_name"] = "not_a_real_import"
    with pytest.raises(
        manifest.WasmAbiManifestError, match="references unknown import"
    ):
        manifest.validate_loaded_manifest(broken_import)

    broken_lir = copy.deepcopy(data)
    broken_lir["container_runtime_selector"][0]["lir_variant"] = "DictGetitem"
    with pytest.raises(
        manifest.WasmAbiManifestError, match="does not match lir_variant"
    ):
        manifest.validate_loaded_manifest(broken_lir)


def test_wasm_abi_manifest_owns_numeric_runtime_selector() -> None:
    gen = gen_wasm_abi
    data = gen.load_manifest()
    selectors = {
        entry["kind"]: (
            entry["import_name"],
            entry["op_loop_variant"],
            entry.get("lir_variant"),
            entry.get("lir_operand_count"),
            tuple(entry["deps"]),
        )
        for entry in data["numeric_runtime_selector"]
    }

    assert selectors["add"] == ("add", "Add", "Add", 2, ("add", "str_concat"))
    assert selectors["inplace_add"] == (
        "inplace_add",
        "Add",
        "InplaceAdd",
        2,
        ("inplace_add", "str_concat"),
    )
    assert selectors["shl"] == ("lshift", "LShift", "LShift", 2, ("lshift",))
    assert selectors["bit_not"] == ("invert", "Invert", "Invert", 1, ("invert",))
    assert selectors["pow_mod"] == ("pow_mod", "PowMod", "PowMod", 3, ("pow_mod",))
    assert selectors["vec_sum"] == (
        "vec_sum",
        "VectorReduction",
        None,
        None,
        ("vec_sum",),
    )
    assert not any(
        kind.startswith("vec_") and kind.count("_") > 1 for kind in selectors
    )

    rendered_rs_modules = gen.render_rs_modules(data)
    rendered_selector_rs = rendered_rs_modules["numeric_runtime_selector.rs"]
    rendered_lir_rs = rendered_rs_modules["lir_runtime_calls.rs"]
    rendered_mod_rs = rendered_rs_modules["mod.rs"]
    rendered_py = gen.render_py(data)
    assert "WASM_NUMERIC_RUNTIME_SELECTORS" in rendered_selector_rs
    assert "WasmNumericOpLoopKind::VectorReduction" in rendered_selector_rs
    assert "import: WasmRuntimeImport::InplaceAdd" in rendered_selector_rs
    assert "LirRuntimeCall::InplaceAdd" in rendered_selector_rs
    assert (
        "deps: &[WasmRuntimeImport::InplaceAdd, WasmRuntimeImport::StrConcat]"
        in rendered_selector_rs
    )
    assert (
        "molt_ir::OpKind::Shl => Some(WasmNumericRuntimeSelection"
        in rendered_selector_rs
    )
    assert "call: LirRuntimeCall::PowMod" in rendered_lir_rs
    assert "mod numeric_runtime_selector;" in rendered_mod_rs
    assert "WasmNumericOpLoopKind" in rendered_mod_rs
    assert "WASM_NUMERIC_RUNTIME_SELECTORS" in rendered_py
    assert '("bit_not", "invert", "Invert", "Invert", 1, ("invert",))' in rendered_py

    broken_duplicate = copy.deepcopy(data)
    broken_duplicate["numeric_runtime_selector"].append(
        copy.deepcopy(broken_duplicate["numeric_runtime_selector"][0])
    )
    with pytest.raises(
        manifest.WasmAbiManifestError, match="duplicate numeric_runtime_selector"
    ):
        manifest.validate_loaded_manifest(broken_duplicate)

    broken_import = copy.deepcopy(data)
    broken_import["numeric_runtime_selector"][0]["import_name"] = "not_a_real_import"
    with pytest.raises(
        manifest.WasmAbiManifestError, match="references unknown import"
    ):
        manifest.validate_loaded_manifest(broken_import)

    broken_lir = copy.deepcopy(data)
    broken_lir["numeric_runtime_selector"][0]["lir_variant"] = "Sub"
    with pytest.raises(
        manifest.WasmAbiManifestError, match="does not match lir_variant"
    ):
        manifest.validate_loaded_manifest(broken_lir)

    broken_deps = copy.deepcopy(data)
    broken_deps["numeric_runtime_selector"][0]["deps"] = ["str_concat"]
    with pytest.raises(manifest.WasmAbiManifestError, match="deps must include"):
        manifest.validate_loaded_manifest(broken_deps)

    broken_count = copy.deepcopy(data)
    broken_count["numeric_runtime_selector"][0]["lir_operand_count"] = -1
    with pytest.raises(
        manifest.WasmAbiManifestError, match="invalid lir_operand_count"
    ):
        manifest.validate_loaded_manifest(broken_count)


def test_wasm_abi_manifest_owns_python_runtime_import_signatures() -> None:
    gen = gen_wasm_abi
    data = gen.load_manifest()
    static_types = data["static_type"]
    imports = {entry["name"]: entry for entry in data["import"]}

    def signature(import_name: str) -> tuple[tuple[str, ...], tuple[str, ...]]:
        import_entry = imports[import_name]
        static_type = static_types[import_entry["type"]]
        return tuple(static_type["params"]), tuple(static_type["results"])

    assert signature("function_set_builtin") == (("i64",), ("i64",))
    assert signature("string_from_bytes") == (("i32", "i64", "i32"), ("i32",))
    assert signature("codecs_decode") == (("i64", "i64", "i64", "i64"), ("i64",))
    assert signature("codecs_encode") == (("i64", "i64", "i64", "i64"), ("i64",))

    rendered_py = gen.render_py(data)
    assert "WASM_IMPORT_SIGNATURES" in rendered_py
    assert "WASM_IMPORT_SIGNATURE_BY_NAME" in rendered_py
    assert "WASM_IMPORT_NAME_BY_LOOKUP" in rendered_py
    rendered_ns = _exec_rendered_py(rendered_py)
    assert rendered_ns["wasm_import_name"]("molt_socket_drop") == "socket_drop"
    assert rendered_ns["wasm_runtime_import_name"]("molt_socket_drop") == "socket_drop"
    assert rendered_ns["wasm_import_result_kind"]("molt_socket_drop") == "i64"
    assert "def wasm_import_name" in rendered_py
    assert "def wasm_import_signature" in rendered_py
    assert "def wasm_import_result_kind" in rendered_py
    assert "WASM_RUNTIME_CALLABLE_ARITY_BY_IMPORT" in rendered_py
    assert "WASM_RESERVED_RUNTIME_CALLABLE_IMPORTS" not in rendered_py
    assert "WASM_RUNTIME_CALLABLE_LOOKUP_ROWS" not in rendered_py
    arity = rendered_ns["wasm_runtime_callable_arity"]
    assert arity("molt_importlib_import_transaction") == 5
    assert arity("importlib_import_transaction") == 5
    assert arity("molt_socket_drop") == arity("socket_drop") == 1
    assert arity("molt_print_newline") is None
    assert arity("unknown_callable") is None
    for entry in data["import"]:
        if "callable_arity" in entry:
            assert (
                arity(entry["name"])
                == arity(entry["runtime_name"])
                == entry["callable_arity"]
            )
    for entry in data["reserved_runtime_callable"]:
        assert (
            arity(entry["import_name"])
            == arity(entry["runtime_name"])
            == entry["callable_arity"]
        )


def test_wasm_abi_deletes_pre_emission_import_dependency_table() -> None:
    gen = gen_wasm_abi
    data = gen.load_manifest()
    op_loop_calls = {entry["kind"]: entry for entry in data["op_loop_runtime_call"]}

    rendered_rs = _rendered_rs(gen, data)
    assert "op_import_dep" not in data
    assert "OP_IMPORT_DEPS" not in rendered_rs
    assert op_loop_calls["module_cache_del"]["required_imports"] == ["module_cache_del"]
    assert op_loop_calls["context_enter"]["required_imports"] == [
        "context_enter",
        "context_exit",
        "context_depth",
        "context_closing",
        "context_null",
        "context_unwind",
        "context_unwind_to",
    ]
    assert "thread_submit" not in op_loop_calls
    assert op_loop_calls["gpu_thread_id"]["required_imports"] == ["gpu_thread_id"]
    assert op_loop_calls["gpu_barrier"]["required_imports"] == ["gpu_barrier"]
    assert not (
        ROOT / "runtime/molt-backend-wasm/src/wasm/module_abi/runtime_import_demand.rs"
    ).exists()


def test_retired_bulk_memory_wire_aliases_have_no_generated_selector() -> None:
    gen = gen_wasm_abi
    data = gen.load_manifest()
    # These backend-only spellings had no registered producer. Their WASM
    # instructions remain independent of the admitted SimpleIR wire vocabulary.
    assert data.get("wasm_bulk_memory_op", []) == []
    modules = gen.render_rs_modules(data)
    assert "bulk_memory_ops.rs" not in modules
    assert "bulk_memory_ops" not in modules["mod.rs"]
    assert "WASM_BULK_MEMORY_OPS" in gen.render_py(data)

    broken = copy.deepcopy(data)
    broken["wasm_bulk_memory_op"] = "not a list"
    with pytest.raises(manifest.WasmAbiManifestError, match="must be a list"):
        manifest.validate_loaded_manifest(broken)

    populated = copy.deepcopy(data)
    populated["wasm_bulk_memory_op"] = [
        {"kind": "memory_copy", "instruction": "memory_copy", "arg_count": 3}
    ]
    with pytest.raises(manifest.WasmAbiManifestError, match="retired"):
        manifest.validate_loaded_manifest(populated)


def test_wasm_abi_policy_projects_ir_owned_literals() -> None:
    gen = gen_wasm_abi
    data = gen.load_manifest()
    policies = {entry["kind"]: entry for entry in data["const_op_policy"]}

    assert policies["const"] == {
        "kind": "const",
        "inline_seed": "int",
        "literal_payload": "none",
        "scalar_payload": "int",
        "raw_int_effect": "set_int",
        "lir_fast": "lower",
        "materializer_import": "int_from_i64",
        "dispatch_runtime_seed": True,
    }
    assert policies["const_bool"]["scalar_payload"] == "bool"
    assert policies["const_float"]["scalar_payload"] == "float"
    assert policies["const_none"]["scalar_payload"] == "none"
    assert policies["const_str"]["materializer_import"] == "string_from_bytes"
    assert policies["const_str"]["literal_payload"] == "string"
    assert policies["const_str"]["scalar_payload"] == "none"
    assert policies["const_str"]["lir_fast"] == "materialize"
    assert policies["const_bytes"]["materializer_import"] == "bytes_from_bytes"
    assert policies["const_bytes"]["literal_payload"] == "bytes"
    assert policies["const_bytes"]["lir_fast"] == "materialize"
    assert policies["const_bigint"]["materializer_import"] == "bigint_from_bytes"
    assert policies["const_bigint"]["literal_payload"] == "bigint_bytes"
    assert policies["const_bigint"]["lir_fast"] == "materialize"
    assert all("parse_scalar_literal" not in policy for policy in policies.values())

    rendered_rs = _rendered_rs(gen, data)
    rendered_py = gen.render_py(data)
    assert "WASM_CONST_OP_POLICIES" in rendered_rs
    assert "WasmConstScalarPayload::Int" in rendered_rs
    assert "required_tir_scalar_value" in rendered_rs
    assert "Some(OwnedLiteralPayloadKind::BigintBytes)" in rendered_rs
    assert "enum WasmConstLiteralPayload" not in rendered_rs
    assert "wasm_const_op_policy" in rendered_rs
    assert "wasm_const_op_policy_for_opcode" in rendered_rs
    assert "opcode_canonical_kind_table(opcode)" in rendered_rs
    assert "PlaceholderZero" not in rendered_rs
    assert "WASM_CONST_OP_POLICIES" in rendered_py
    assert "parse_scalar_literal" not in rendered_rs
    assert "parse_scalar_literal" not in rendered_py


def test_wasm_abi_manifest_rejects_retired_parse_scalar_literal_policy() -> None:
    gen = gen_wasm_abi
    data = gen.load_manifest()
    broken = copy.deepcopy(data)
    broken["const_op_policy"][0]["parse_scalar_literal"] = False

    with pytest.raises(
        manifest.WasmAbiManifestError, match="parse_scalar_literal is retired"
    ):
        manifest.validate_loaded_manifest(broken)


def test_wasm_abi_manifest_keeps_runtime_surface_metadata_without_import_matchers() -> (
    None
):
    gen = gen_wasm_abi
    data = gen.load_manifest()

    rendered_rs = _rendered_rs(gen, data)
    rendered_py = gen.render_py(data)
    assert "runtime_required_import_prefix" not in data
    assert "runtime_required_import_singleton" not in data
    assert "runtime_surface_requires_direct_import" not in rendered_rs
    assert "WASM_REQUIRED_RUNTIME_IMPORT_PREFIXES" not in rendered_py
    assert "runtime_surface_requires_direct_import" not in rendered_py

    host_surface = (
        ROOT / "runtime/molt-backend-wasm/src/wasm/module_abi/host_surface.rs"
    ).read_text(encoding="utf-8")
    assert "GPU_INTRINSIC_MANIFEST_NAMES" not in host_surface
    assert "DEFAULT_GPU_INTRINSIC_MANIFEST_NAMES" not in host_surface


def test_wasm_abi_manifest_owns_split_runtime_table_prefix() -> None:
    gen = gen_wasm_abi
    data = gen.load_manifest()

    poll_slots = {
        entry["name"]: entry["poll_table_slot"]
        for entry in data["import"]
        if "poll_table_slot" in entry
    }
    assert poll_slots["async_sleep_poll"] == 1
    assert poll_slots["contextlib_async_exitstack_enter_context_poll"] == 27
    assert sorted(poll_slots.values()) == list(range(1, len(poll_slots) + 1))
    broken = copy.deepcopy(data)
    for entry in broken["import"]:
        if entry.get("name") == "contextlib_async_exitstack_enter_context_poll":
            entry["poll_table_slot"] = 34
            break
    with pytest.raises(manifest.WasmAbiManifestError, match="poll_table_slot values"):
        manifest.validate_loaded_manifest(broken)

    reserved = data["reserved_runtime_callable"]
    assert reserved[0] == {
        "index": 0,
        "runtime_name": "molt_type_call",
        "import_name": "type_call",
        "callable_arity": 1,
    }
    assert reserved[22] == {
        "index": 22,
        "runtime_name": "molt_cpython_abi_cext_call_trampoline",
        "import_name": "cpython_abi_cext_call_trampoline",
        "callable_arity": 3,
        "trampoline_abi": "call_frame",
    }
    assert reserved[23] == {
        "index": 23,
        "runtime_name": "molt_importlib_import_transaction",
        "import_name": "importlib_import_transaction",
        "callable_arity": 5,
        "callable_dispatch": "trampoline",
    }
    assert reserved[24] == {
        "index": 24,
        "runtime_name": "molt_importlib_module_spec_init",
        "import_name": "importlib_module_spec_init",
        "callable_arity": 5,
    }
    assert [entry["index"] for entry in reserved] == list(range(len(reserved)))

    rendered_rs = _rendered_rs(gen, data)
    rendered_py = gen.render_py(data)
    rendered_table_layout = gen.render_table_layout_inc(data)
    assert "PollTableImportSpec" in rendered_rs
    assert "POLL_TABLE_IMPORTS" in rendered_rs
    assert "import: WasmRuntimeImport::AsyncSleepPoll" in rendered_rs
    assert "pub(crate) const fn poll_table_import_slot" in rendered_rs
    assert (
        "WasmRuntimeImport::ContextlibAsyncExitstackEnterContextPoll => Some(27)"
        in rendered_rs
    )
    assert "POLL_TABLE_FUNCS" not in rendered_rs
    assert "WASM_POLL_TABLE_IMPORTS: tuple[tuple[int, str], ...]" in rendered_py
    assert '(27, "contextlib_async_exitstack_enter_context_poll")' in rendered_py
    assert "WASM_DEFAULT_APP_TABLE_BASE" in rendered_py
    assert data["callable_table_publication"] == {
        "section_name": "molt.callable_table",
        "version": 1,
        "layout_section_name": "molt.callable_table.layout",
        "layout_version": 1,
        "active_element_role": 0,
        "value_type_format": 1,
    }
    for name in (
        "WASM_CALLABLE_TABLE_SECTION_NAME",
        "WASM_CALLABLE_TABLE_SECTION_VERSION",
        "WASM_CALLABLE_TABLE_LAYOUT_SECTION_NAME",
        "WASM_CALLABLE_TABLE_LAYOUT_VERSION",
        "WASM_CALLABLE_TABLE_ACTIVE_ELEMENT_ROLE",
        "WASM_CALLABLE_TABLE_VALUE_TYPE_FORMAT",
    ):
        assert name in rendered_py
    assert "WASM_TABLE_REF_EXPORT_PREFIX" not in rendered_py
    assert "WASM_TABLE_REF_EXPORT_PREFIX" not in rendered_table_layout
    assert "WASM_RESERVED_RUNTIME_CALLABLE_BASE" in rendered_py

    callable_layout = (
        ROOT / "runtime/molt-backend-wasm/src/wasm/module_abi/callable_table/layout.rs"
    ).read_text(encoding="utf-8")
    assert (
        "poll_table.seed_function_table_slots(&mut func_to_table_idx)"
        in callable_layout
    )
    assert "for spec in RESERVED_RUNTIME_CALLABLE_SPECS" in callable_layout
    assert (
        "table_base\n                .checked_add(slot - fixed_prefix_len)"
        in callable_layout
    )
    assert "WasmCallableTableAddress::FixedSharedRuntimeAbi" in callable_layout


def test_wasm_abi_manifest_owns_host_import_policy() -> None:
    gen = gen_wasm_abi
    data = gen.load_manifest()

    allowed = [entry["name"] for entry in data["link_allowed_import"]]
    allowed_classes = {
        entry["name"]: entry["primitive_class"] for entry in data["link_allowed_import"]
    }
    external_native_classes = {
        entry["name"]: entry["primitive_class"]
        for entry in data["external_native_link_import"]
    }
    call_indirect = [name for name in allowed if name.startswith("molt_call_indirect")]
    assert "fd_write" in allowed
    assert "__indirect_function_table" in allowed
    assert allowed_classes["fd_write"] == "wasi_link_import"
    assert allowed_classes["__indirect_function_table"] == (
        "wasm_toolchain_link_import"
    )
    assert call_indirect == [f"molt_call_indirect{arity}" for arity in range(14)]
    assert all(
        allowed_classes[name] == "molt_indirect_call_import" for name in call_indirect
    )
    assert "molt_cbor_parse_scalar" in allowed
    assert allowed_classes["molt_cbor_parse_scalar"] == "molt_runtime_host_import"
    assert external_native_classes["__cpp_exception"] == ("wasm_toolchain_link_import")
    assert external_native_classes["molt_cpython_abi_date_from_date"] == (
        "molt_cpython_abi_link_import"
    )
    generated_cpython_abi_imports = set(
        manifest.generator_cpython_abi_link_import_names()
    )
    assert {
        "PyArg_ParseTuple",
        "PyFloat_Check",
        "PyLong_Check",
        "PyBool_Check",
        "PyObject_Init",
        "PyObject_InitVar",
        "_PyObject_New",
        "PyMemoryView_FromMemory",
        "PyBuffer_FillInfo",
        "PyErr_SetString",
        "PyExc_ValueError",
        "PyExc_TypeError",
        "Py_None",
        "molt_cpython_abi_date_from_date",
    } <= generated_cpython_abi_imports
    assert {
        name
        for name, primitive_class in external_native_classes.items()
        if primitive_class == "molt_cpython_abi_link_import"
    } == generated_cpython_abi_imports
    provider_classes = {
        "wasm_libc_link_import",
        "wasm_compiler_rt_link_import",
        "wasm_libcxx_link_import",
    }
    assert not (provider_classes & set(external_native_classes.values()))
    assert len(allowed) == len(set(allowed))
    broken = copy.deepcopy(data)
    broken["link_allowed_import"] = [
        entry
        for entry in broken["link_allowed_import"]
        if entry.get("name") != "molt_call_indirect7"
    ]
    with pytest.raises(manifest.WasmAbiManifestError, match="call_indirect import"):
        manifest.validate_loaded_manifest(broken)

    strip_rules = {
        (entry["module"], entry["name"]): entry for entry in data["strip_import_rule"]
    }
    assert strip_rules[("wasi_snapshot_preview1", "fd_write")]["category"] == (
        "io_stdout"
    )
    assert strip_rules[("env", "molt_socket_connect_host")]["category"] == "socket"
    assert strip_rules[("env", "__indirect_function_table")]["category"] == "table"

    prefix_rules = {
        (entry["module"], entry["prefix"]): entry
        for entry in data["strip_import_prefix_rule"]
    }
    assert prefix_rules[("env", "molt_call_indirect")]["category"] == ("indirect_call")

    rendered_py = gen.render_py(data)
    rendered_js_abi = json.loads(gen.render_js_abi(data))
    rendered_rs = _rendered_rs(gen, data)
    assert "CallIndirectImportSpec" in rendered_rs
    assert "CALL_INDIRECT_IMPORTS" in rendered_rs
    assert "CALL_INDIRECT_MAX_ARITY" not in rendered_rs
    assert "WASM_LINK_ALLOWED_IMPORTS" in rendered_py
    assert "WASM_LINK_ALLOWED_IMPORT_PRIMITIVE_CLASSES" in rendered_py
    assert "WASM_EXTERNAL_NATIVE_LINK_IMPORTS" in rendered_py
    assert "WASM_EXTERNAL_NATIVE_LINK_IMPORT_PRIMITIVE_CLASSES" in rendered_py
    assert "WASM_CALL_INDIRECT_IMPORTS" in rendered_py
    assert "WASM_STRIP_IMPORT_RULES" in rendered_py
    assert "WASM_STRIP_IMPORT_PREFIX_RULES" in rendered_py
    js_link_imports = rendered_js_abi["external_native_link_imports"]
    assert not (provider_classes & set(js_link_imports["primitive_classes"].values()))

    allowlist = gen.OUT_ALLOWED_IMPORTS.read_text(encoding="utf-8")
    assert allowlist == gen.render_allowed_imports(data)
    assert "# DO NOT EDIT BY HAND." in allowlist

    strip_tool = (ROOT / "tools/wasm_strip_unused.py").read_text(encoding="utf-8")
    assert "WASM_STRIP_IMPORT_RULES" in strip_tool
    assert "WASM_STRIP_IMPORT_PREFIX_RULES" in strip_tool
    assert "molt_process_write_host" not in strip_tool


def test_wasm_abi_manifest_owns_link_export_policy() -> None:
    gen = gen_wasm_abi
    data = gen.load_manifest()
    policy = data["output_export_policy"]

    assert policy["alias_prefix"] == "__molt_export_alias__"
    assert {
        "memory",
        "molt_memory",
        "molt_main",
        "molt_len",
        "molt_index",
        "molt_profile_dump",
        "molt_runtime_execution_enter",
        "molt_runtime_execution_leave",
        "molt_runtime_shutdown",
        "molt_table",
        "__indirect_function_table",
    } <= set(policy["essential_exports"])
    assert policy["runtime_export_aliases"] == [
        "molt_isolate_bootstrap",
        "molt_isolate_import",
    ]
    assert {"genexpr_", "listcomp_", "lambda_"} <= set(
        policy["internal_output_export_prefixes"]
    )

    rendered_py = gen.render_py(data)
    assert "WASM_OUTPUT_EXPORT_ALIAS_PREFIX" in rendered_py
    assert "WASM_OUTPUT_RUNTIME_EXPORT_ALIASES" in rendered_py
    assert "WASM_INTERNAL_OUTPUT_EXPORT_PREFIXES" in rendered_py
    assert "WASM_ESSENTIAL_EXPORTS" in rendered_py

    # Host construction and rollback are one contract in both the Cargo
    # export surface and final-output retention, including minimal runtimes.
    stream_host_exports = {
        "molt_stream_new",
        "molt_stream_send",
        "molt_stream_close",
        "molt_stream_drop",
        "molt_int_from_i64",
        "molt_exception_pending_fast",
        "molt_dec_ref_obj",
    }
    assert stream_host_exports <= set(policy["essential_exports"])
    assert stream_host_exports <= set(data["runtime_export_policy"]["host_exports"])

    link_format = (ROOT / "tools/wasm_link_format.py").read_text(encoding="utf-8")
    assert "_WASM_ABI.WASM_OUTPUT_EXPORT_ALIAS_PREFIX" in link_format
    assert "_WASM_ABI.WASM_OUTPUT_RUNTIME_EXPORT_ALIASES" in link_format
    assert "_WASM_ABI.WASM_INTERNAL_OUTPUT_EXPORT_PREFIXES" in link_format
    assert "_WASM_ABI.WASM_ESSENTIAL_EXPORTS" in link_format
    assert "_WASM_ABI.wasm_runtime_export_name" in link_format
    assert '"molt_alloc"' not in link_format
    assert '"molt_isolate_import"' not in link_format


def test_runtime_execution_token_exports_are_wasm_only() -> None:
    execution = (ROOT / "runtime/molt-runtime/src/concurrency/execution.rs").read_text(
        encoding="utf-8"
    )
    for name in ("molt_runtime_execution_enter", "molt_runtime_execution_leave"):
        declaration = (
            '#[cfg(target_arch = "wasm32")]\n'
            "#[unsafe(no_mangle)]\n"
            f'pub extern "C" fn {name}'
        )
        assert declaration in execution
    assert execution.count('pub extern "C" fn molt_runtime_execution_') == 2


def test_wasm_literal_projection_includes_aliases_and_rejects_local_drift() -> None:
    from molt.opcode_literal_payloads import owned_literal_payloads_by_kind

    table = {
        "opcode": [{"name": "Literal"}],
        "literal_payload_opcodes": [{"opcode": "Literal", "literal": "bytes"}],
        "kind": [
            {"canonical": "literal", "aliases": ["alias"], "mapper_opcode": "Literal"}
        ],
    }
    assert owned_literal_payloads_by_kind(table) == {
        "literal": "bytes",
        "alias": "bytes",
    }
    gen = gen_wasm_abi
    data = copy.deepcopy(gen.load_manifest())
    next(row for row in data["const_op_policy"] if row["kind"] == "const_str")[
        "literal_payload"
    ] = "bytes"
    with pytest.raises(
        manifest.WasmAbiManifestError, match="disagrees with op_kinds.toml"
    ):
        manifest.validate_loaded_manifest(data)


def test_pure_profile_keeps_every_intrinsic_core_stdlib_modules_require() -> None:
    """Pure omits IO/async/time families, never what every program imports.

    The core closure (``builtins``, ``sys``, ``__future__`` and their support
    modules) is compiled into every guest. ``asyncgen_`` and ``future_`` used
    to strip ``sys``'s asyncgen hook accessors and ``__future__``'s feature
    table, so every ``--wasm-profile pure`` (and Cloudflare) program died in
    ``import sys`` with "intrinsic unavailable: molt_asyncgen_hooks_get".
    """
    from molt._wasm_abi_generated import (
        WASM_RUNTIME_IMPORT_EXPORT_NAMES,
        pure_profile_skips_import,
    )

    import_by_runtime_name = {
        export: import_name for import_name, export in WASM_RUNTIME_IMPORT_EXPORT_NAMES
    }

    stdlib = ROOT / "src" / "molt" / "stdlib"
    core_modules = ("builtins", "sys", "__future__", "_sitebuiltins", "_intrinsics")
    required = set()
    for module in core_modules:
        source = (stdlib / f"{module}.py").read_text(encoding="utf-8")
        required.update(
            re.findall(r"""_require_intrinsic\(\s*["'](molt_\w+)["']""", source)
        )
    assert {"molt_asyncgen_hooks_get", "molt_future_features"} <= required
    assert required <= import_by_runtime_name.keys()
    stripped = sorted(
        name
        for name in required
        if pure_profile_skips_import(import_by_runtime_name[name])
    )
    assert stripped == []
    # The async execution families themselves stay out of Pure guests.
    for name in ("asyncgen_new", "asyncgen_poll", "future_poll", "future_cancel"):
        assert pure_profile_skips_import(name)


def test_every_frontend_direct_runtime_call_has_a_wasm_import() -> None:
    """Direct runtime calls must exist in the WASM ABI.

    The WASM backend panics on a direct runtime call without a manifest import
    ("direct runtime call missing WASM ABI manifest import"). Include literal
    calls emitted through the common helper as well as directly constructed IR.
    Exercise GPU publication too: its symbol is a shared constant, so a literal
    source census cannot prove that the actual producer has an ABI import.
    """
    import ast

    from molt.frontend import compile_to_tir
    from molt._wasm_abi_generated import WASM_RUNTIME_IMPORT_EXPORT_NAMES

    runtime_exports = {
        export for _import_name, export in WASM_RUNTIME_IMPORT_EXPORT_NAMES
    }
    frontend = ROOT / "src" / "molt" / "frontend"
    direct_calls: set[str] = set()
    for path in frontend.rglob("*.py"):
        for node in ast.walk(ast.parse(path.read_text(encoding="utf-8"))):
            if (
                isinstance(node, ast.Call)
                and isinstance(node.func, ast.Attribute)
                and node.func.attr == "_emit_runtime_call"
                and node.args
                and isinstance(node.args[0], ast.Constant)
                and isinstance(node.args[0].value, str)
                and node.args[0].value.startswith("molt_")
            ):
                direct_calls.add(node.args[0].value)
            if not (
                isinstance(node, ast.Call)
                and isinstance(node.func, ast.Name)
                and node.func.id == "MoltOp"
            ):
                continue
            keywords = {keyword.arg: keyword.value for keyword in node.keywords}
            kind = keywords.get("kind")
            args = keywords.get("args")
            if (
                isinstance(kind, ast.Constant)
                and kind.value == "CALL"
                and isinstance(args, ast.List)
                and args.elts
                and isinstance(args.elts[0], ast.Constant)
                and isinstance(args.elts[0].value, str)
                and args.elts[0].value.startswith("molt_")
            ):
                direct_calls.add(args.elts[0].value)
    ir = compile_to_tir(
        "import molt.gpu as gpu\n"
        "@gpu.kernel\n"
        "def kernel():\n"
        "    return gpu.thread_id()\n"
    )
    emitted_calls = {
        op["s_value"]
        for function in ir["functions"]
        for op in function["ops"]
        if op.get("kind") == "call"
        and isinstance(op.get("s_value"), str)
        and op["s_value"].startswith("molt_")
    }
    assert "molt_gpu_kernel_descriptor_set" in emitted_calls
    direct_calls.update(emitted_calls)
    assert sorted(direct_calls - runtime_exports) == []


def test_shared_boxed_ops_admit_only_complete_text_predicate_family() -> None:
    data = manifest.load_manifest()
    rows = manifest.runtime_boxed_op_specs(data)
    expected = {
        f"{receiver}_{method}{suffix}": 6 if suffix else 2
        for receiver in ("string", "bytes", "bytearray")
        for method in ("find", "startswith", "endswith", "count")
        for suffix in ("", "_slice")
    }
    assert {row["kind"]: len(row["args"]) for row in rows} == expected
    abi = {row["runtime_name"]: row for row in manifest.runtime_boxed_call_specs(data)}
    for row in rows:
        assert abi[row["runtime_name"]] == {
            "runtime_name": f"molt_{row['kind']}",
            "arity": expected[row["kind"]],
            "result": "i64",
        }


def _shared_boxed_op_fixture() -> dict:
    data = _boxed_abi_fixture()
    data["import"][0]["runtime_name"] = "molt_different_export"
    data["op_loop_runtime_call"] = [
        {
            "kind": "len",
            "import_name": "example",
            "args": ["local:0"],
            "boxed_call": True,
            "shared_boxed_call": True,
        }
    ]
    return data


def test_shared_boxed_op_mapping_uses_explicit_symbol_not_kind_spelling() -> None:
    data = _shared_boxed_op_fixture()
    rows = manifest.runtime_boxed_op_specs(data)
    assert rows[0]["kind"] == "len"
    assert rows[0]["runtime_name"] == "molt_different_export"
    gen = gen_wasm_abi
    source = gen.render_runtime_boxed_abi_rs(data)
    assert "crate::OpKind::Len => Some(&RUNTIME_BOXED_ABIS[0])" in source
    assert 'symbol: "molt_different_export"' in source
    assert "molt_len" not in source


@pytest.mark.parametrize(
    "mutation",
    [
        "transport_only",
        "borrowed",
        "poll",
        "void",
        "raw",
        "reordered",
        "wrong_arity",
        "marked",
        "discard",
        "dependencies",
        "invalid_opt_in",
    ],
)
def test_shared_boxed_ops_refuse_heterogeneous_protocols(mutation: str) -> None:
    data = _shared_boxed_op_fixture()
    entry = data["op_loop_runtime_call"][0]
    imported = data["import"][0]
    if mutation == "transport_only":
        entry["boxed_call"] = False
    elif mutation in ("borrowed", "poll"):
        imported["return_contract"] = (
            "borrowed_object" if mutation == "borrowed" else "poll_result"
        )
    elif mutation == "void":
        imported["type"] = 2
    elif mutation == "raw":
        imported["type"] = 1
    elif mutation == "reordered":
        entry["args"] = ["local:1"]
    elif mutation == "wrong_arity":
        entry["args"] = ["local:0", "local:1"]
    elif mutation == "marked":
        entry["marked_import_name"] = "example_marked"
    elif mutation == "discard":
        entry["discard_result"] = True
    elif mutation == "dependencies":
        entry["required_imports"] = ["example", "another"]
    else:
        entry["shared_boxed_call"] = "yes"
    with pytest.raises(manifest.WasmAbiManifestError, match="boxed"):
        manifest.runtime_boxed_op_specs(data)


@pytest.mark.parametrize("field", ["runtime_import_fallback", "unrecognized_section"])
def test_manifest_rejects_unknown_top_level_fields(field: str) -> None:
    data = copy.deepcopy(gen_wasm_abi.load_manifest())
    data[field] = []
    with pytest.raises(
        manifest.WasmAbiManifestError, match="unknown manifest fields"
    ) as error:
        manifest.validate_loaded_manifest(data)
    assert field in str(error.value)


def test_manifest_field_authority_covers_sources_and_recomputes_projections() -> None:
    raw = tomllib.loads(manifest.MANIFEST.read_text(encoding="utf-8"))
    # Allowed source categories may be absent when their last wire producer is
    # retired. Every populated category must still belong to the authority.
    assert set(raw) <= manifest.MANIFEST_FIELDS
    assert manifest.MANIFEST_FIELDS - set(raw) == {"wasm_bulk_memory_op"}
    assert raw.get("wasm_bulk_memory_op", []) == []
    loaded = copy.deepcopy(gen_wasm_abi.load_manifest())
    expected = manifest.validate_loaded_manifest(copy.deepcopy(loaded))
    assert set(expected) <= manifest.MANIFEST_FIELDS | manifest.DERIVED_MANIFEST_FIELDS
    assert set(expected) - set(raw) == manifest.DERIVED_MANIFEST_FIELDS
    for field in manifest.DERIVED_MANIFEST_FIELDS:
        loaded[field] = {"untrusted_projection": True}
    actual = manifest.validate_loaded_manifest(loaded)
    for field in manifest.DERIVED_MANIFEST_FIELDS:
        assert actual[field] == expected[field], field


@pytest.mark.parametrize("invalid", ["unknown", "named", "variadic", "defaults"])
def test_builtin_positional_policy_rejects_invalid_declarations(
    monkeypatch, invalid: str
) -> None:
    import ast
    from dataclasses import replace
    from molt.builtin_callables import BUILTIN_FUNC_SPECS, BuiltinBindingPolicy

    spec = BUILTIN_FUNC_SPECS["iter"]
    changes = {
        "unknown": {"bind_kind": 7},
        "named": {"pos_or_kw_params": ("extra",)},
        "variadic": {"vararg": "rest"},
        "defaults": {"defaults": (ast.Constant(None),) * 3},
    }[invalid]
    assert spec.bind_kind is BuiltinBindingPolicy.POSITIONAL_RANGE
    monkeypatch.setitem(BUILTIN_FUNC_SPECS, "iter", replace(spec, **changes))
    with pytest.raises(ValueError, match="binding policy|fixed positional metadata"):
        gen_wasm_abi._python_builtin_global_callables(gen_wasm_abi.load_manifest())


def test_builtin_binding_policy_projects_typed_identity_and_hot_path() -> None:
    from molt.builtin_callables import BUILTIN_FUNC_SPECS, BuiltinBindingPolicy

    data = gen_wasm_abi.load_manifest()
    rendered = gen_wasm_abi.render_runtime_callables_rs(data)
    assert (
        "BuiltinBindingPolicy"
        not in gen_wasm_abi.render_python_builtin_callables_rs(data)
    )
    assert "molt_ir::" not in rendered
    assert "bind_kind: Option<BuiltinBindingPolicy>" in rendered
    for policy in BuiltinBindingPolicy:
        variant = policy.name.title().replace("_", "")
        assert f"{variant} = {int(policy)}" in rendered
        assert f"{int(policy)} => Some(Self::{variant})" in rendered
        assert (
            f"Self::{variant} => {str(policy.requires_custom_binding).lower()}"
            in rendered
        )
    assert BuiltinBindingPolicy.CLINIC_NAMED.requires_custom_binding
    assert not BuiltinBindingPolicy.POSITIONAL_RANGE.requires_custom_binding
    assert {
        name
        for name, spec in BUILTIN_FUNC_SPECS.items()
        if spec.bind_kind is BuiltinBindingPolicy.POSITIONAL_RANGE
    } == {
        "iter",
        "next",
        "getattr",
        "dir",
        "vars",
        "format",
        "divmod",
        "hasattr",
        "setattr",
        "delattr",
        "anext",
        "isinstance",
        "issubclass",
    }


def test_native_resolver_alias_preference_is_explicit_and_order_independent() -> None:
    data = copy.deepcopy(gen_wasm_abi.load_manifest())
    nodata = next(row for row in data["resolver_error"] if row["name"] == "EAI_NODATA")
    assert nodata["native_alias_prefer"] == "EAI_NONAME"
    # Both declaration orders must emit the same guarded native branch. A
    # first-match alias implementation fails this independent SDK policy oracle.
    branch = (
        "if crate::native_error_codes::EAI_NODATA == Some(code.code())"
        " && crate::native_error_codes::EAI_NONAME != Some(code.code())"
    )
    for rows in (data["resolver_error"], list(reversed(data["resolver_error"]))):
        variant = copy.deepcopy(data)
        variant["resolver_error"] = rows
        manifest.resolver_error_table(variant)
        rendered = gen_wasm_abi.render_synthetic_errno_rs(variant)
        assert branch in rendered
        assert "if code == ::libc::EAI_" not in rendered
    nodata["native_alias_prefer"] = "EAI_SYSTEM"
    with pytest.raises(manifest.WasmAbiManifestError, match="alias preference"):
        manifest.resolver_error_table(data)


def test_raw_wasi_ingress_has_complete_typed_coverage_and_explicit_dispositions() -> (
    None
):
    data = copy.deepcopy(gen_wasm_abi.load_manifest())
    guest = manifest.synthetic_errno_table(data)
    assert guest["EMFILE"] == 24 and guest["ENOTTY"] == 25
    assert guest["ERANGE"] == 34 and guest["ECANCELED"] == 125
    assert guest["EOWNERDEAD"] == 130
    assert "ENOTCAPABLE" not in guest
    assert data["wasi_errno_disposition"] == {"ENOTCAPABLE": "EACCES"}
    assert data["errno_adoption"] == {"unknown": "EIO"}
    assert set(data["wasi_errno"]) - set(guest) == {"ENOTCAPABLE"}
    assert set(guest) - set(manifest.wasi_errno_table(data)) == {"ESHUTDOWN"}
    rendered = gen_wasm_abi.render_synthetic_errno_rs(data)
    raw_enum = rendered.split("pub enum RawWasiErrno {", 1)[1].split("}\n", 1)[0]
    assert len([line for line in raw_enum.splitlines() if "=" in line]) == 76
    assert "Self::ENOTCAPABLE => GuestErrno::EACCES" in rendered
    assert "pub fn from_sdk(code: i32) -> Option<Self>" in rendered
    assert "if code == ::libc::ENOENT { return Some(Self::ENOENT); }" in rendered
    assert "if code == 10035" not in rendered
    assert all(
        "windows_socket_code" not in row and "windows_crt" not in row
        for row in data["synthetic_errno"]
    )


def test_deployment_launch_required_names_have_one_generated_authority():
    from molt._wasm_abi_generated import WASM_EXECUTION_MANIFEST_REQUIRED_ENVIRONMENT

    data = gen_wasm_abi.load_manifest()
    expected = (
        "MOLT_CAPABILITIES",
        "MOLT_CAPABILITY_TIER",
        "MOLT_CAPABILITY_POLICY_DIGEST",
    )
    assert (
        tuple(data["deployment_manifest"]["launch_environment_required_names"])
        == expected
    )
    assert WASM_EXECUTION_MANIFEST_REQUIRED_ENVIRONMENT == expected
    projected = json.loads(gen_wasm_abi.render_js_abi(data))
    assert tuple(projected["execution_manifest_required_environment"]) == expected


@pytest.mark.parametrize(
    "names", [None, [], [False], [""], ["BAD=NAME"], ["BAD\0NAME"], ["SAME", "SAME"]]
)
def test_deployment_launch_declaration_rejects_invalid_required_names(names):
    raw = _raw_manifest()
    raw["deployment_manifest"]["launch_environment_required_names"] = names
    with pytest.raises(manifest.WasmAbiManifestError, match="launch environment names"):
        manifest.validate_loaded_manifest(raw)
