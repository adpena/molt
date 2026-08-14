"""Drift and consumer teeth for the cross-language native-callable ABI registry."""

from __future__ import annotations

import importlib
import importlib.util
from pathlib import Path
from types import ModuleType

import pytest

ROOT = Path(__file__).resolve().parents[1]


def _gen():
    return importlib.import_module("tools.gen_native_callable_abi")


def _generated_python() -> ModuleType:
    path = _gen().OUT_PYTHON
    spec = importlib.util.spec_from_file_location(
        "_molt_native_callable_abi_test", path
    )
    assert spec is not None and spec.loader is not None
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


def test_generated_outputs_are_byte_exact() -> None:
    gen = _gen()
    for path, expected in gen.render_all(gen.load_schema()).items():
        assert path.read_bytes() == expected.encode("utf-8"), (
            f"{path.relative_to(ROOT)} is stale; run "
            "`python tools/gen_native_callable_abi.py`"
        )


def test_python_and_rust_projections_cover_every_registry_row() -> None:
    gen = _gen()
    schema = gen.load_schema()
    generated = _generated_python()
    rust = gen.OUT_RUST.read_text(encoding="utf-8")
    javascript = gen.OUT_JAVASCRIPT.read_text(encoding="utf-8")

    assert generated.NATIVE_CALLABLE_ABIS == tuple(abi.token for abi in schema.abis)
    for abi in schema.abis:
        assert generated.native_callable_fixed_arity(abi.token) == abi.fixed_arity
        assert generated.native_callable_uses_callargs(abi.token) is abi.uses_callargs
        assert (
            generated.native_callable_requires_direct_symbol_binding(abi.token)
            is abi.requires_direct_symbol_binding
        )
        assert generated.native_callable_browser_signature(abi.token) == {
            "params": list(abi.browser_params),
            "result": abi.browser_result,
        }
        assert f'pub const {abi.constant}: &str = "{abi.token}";' in rust
        assert f'"{abi.token}":' in javascript
        assert f'"params":{list(abi.browser_params)!r}'.replace("'", '"') in javascript
        assert f'"result":"{abi.browser_result}"' in javascript
        assert (
            f"Self::{abi.rust_variant} => NativeCallableLowering::{abi.rust_lowering},"
            in rust
        )
        expected_arity = (
            "None" if abi.fixed_arity is None else f"Some({abi.fixed_arity})"
        )
        assert f"Self::{abi.rust_variant} => {expected_arity}," in rust


def test_schema_rejects_identity_arity_and_machine_signature_drift(
    tmp_path: Path,
) -> None:
    gen = _gen()
    source = gen.SOURCE.read_text(encoding="utf-8")

    duplicate = tmp_path / "duplicate.toml"
    duplicate.write_text(
        source.replace('name = "object_callargs_v1"', 'name = "object_call_v1"', 1),
        encoding="utf-8",
    )
    with pytest.raises(gen.SchemaError, match="token must be exactly|duplicate"):
        gen.load_schema(duplicate)

    arity = tmp_path / "arity.toml"
    arity.write_text(
        source.replace(
            'fixed_arity = 1\nbrowser_params = ["molt.callargs"]',
            'fixed_arity = 2\nbrowser_params = ["molt.callargs"]',
            1,
        ),
        encoding="utf-8",
    )
    with pytest.raises(gen.SchemaError, match="fixed_arity must equal"):
        gen.load_schema(arity)

    machine = tmp_path / "machine.toml"
    machine.write_text(
        source.replace('native_results = ["pointer"]', 'native_results = ["usize"]', 1),
        encoding="utf-8",
    )
    with pytest.raises(gen.SchemaError, match="unsupported native machine types"):
        gen.load_schema(machine)

    variadic = tmp_path / "variadic.toml"
    variadic.write_text(
        source.replace(
            'native_params = ["molt_value..."]', 'native_params = ["usize..."]', 1
        ),
        encoding="utf-8",
    )
    with pytest.raises(gen.SchemaError, match="unsupported native machine types"):
        gen.load_schema(variadic)

    valid_but_wrong = tmp_path / "valid_but_wrong.toml"
    valid_but_wrong.write_text(
        source.replace('wasm_results = ["i32"]', 'wasm_results = ["i64"]', 1),
        encoding="utf-8",
    )
    with pytest.raises(gen.SchemaError, match="signatures and arity must match"):
        gen.load_schema(valid_but_wrong)


def test_consumers_delegate_abi_classification_to_generated_projections() -> None:
    frontend = (ROOT / "src/molt/frontend/visitors/call_module_dispatch.py").read_text(
        encoding="utf-8"
    )
    simple_ir = (ROOT / "runtime/molt-ir/src/ir_schema.rs").read_text(encoding="utf-8")
    wasm_imports = (
        ROOT / "runtime/molt-backend-wasm/src/wasm/module_abi/native_callables.rs"
    ).read_text(encoding="utf-8")
    wasm_calls = (
        ROOT / "runtime/molt-backend-wasm/src/wasm/op_loop/call_ops/dynamic.rs"
    ).read_text(encoding="utf-8")
    native_calls = (
        ROOT
        / "runtime/molt-backend-native/src/native_backend/function_compiler/fc/calls.rs"
    ).read_text(encoding="utf-8")
    frame_requirements = (
        ROOT
        / "runtime/molt-backend-wasm/src/wasm/function_frame/planning/requirements.rs"
    ).read_text(encoding="utf-8")
    browser_embed = (ROOT / "wasm/browser_embed.js").read_text(encoding="utf-8")

    assert "native_callable_requires_direct_symbol_binding" in frontend
    assert "NATIVE_CALLABLE_ABI_FORWARD_F32_V1" not in frontend
    assert "NATIVE_CALLABLE_ABI_PYINIT_MODULE_V1" not in frontend
    assert "parsed_abi.fixed_arity()" in simple_ir
    assert "parsed.requires_direct_symbol_binding()" in wasm_imports
    assert ".wasm_machine_signature(arity)" in wasm_imports
    assert "abi_contract.requires_direct_symbol_binding()" in wasm_calls
    assert "abi_contract.uses_callargs()" in wasm_calls
    assert "abi_contract.lowering()" in wasm_calls
    assert "abi.uses_callargs()" in native_calls
    assert "abi_contract.lowering()" in native_calls
    assert "abi.lowering() == NativeCallableLowering::ForwardF32" in frame_requirements
    assert "native_callable_abi_generated.js" in browser_embed
    for abi in _gen().load_schema().abis:
        assert abi.token not in browser_embed
    for consumer in (wasm_imports, wasm_calls, native_calls, frame_requirements):
        assert "NativeCallableAbi::ForwardF32V1" not in consumer
        assert "NativeCallableAbi::PyinitModuleV1" not in consumer
        assert "NativeCallableAbi::ObjectCallargsV1" not in consumer
