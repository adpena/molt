#!/usr/bin/env python3
"""Generate Python, Rust, and browser native-callable ABI projections.

Usage::

    python tools/gen_native_callable_abi.py
    python tools/gen_native_callable_abi.py --check
"""

from __future__ import annotations

import argparse
import json
import re
import shutil
import sys
import tomllib
from dataclasses import dataclass
from pathlib import Path
from typing import cast

from generator_io import generated_file_matches, write_generated_text

try:
    from tools.command_execution import CommandExecutor
except ModuleNotFoundError:  # pragma: no cover - direct tools/ execution
    from command_execution import CommandExecutor  # type: ignore

_COMMANDS = CommandExecutor.for_file(__file__)

ROOT = Path(__file__).resolve().parents[1]
SOURCE = ROOT / "runtime" / "native_callable_abi.toml"
OUT_PYTHON = ROOT / "src" / "molt" / "native_callable_abi.py"
OUT_RUST = ROOT / "runtime" / "molt-ir" / "src" / "native_callable_abi.rs"
OUT_JAVASCRIPT = ROOT / "wasm" / "native_callable_abi_generated.js"
OUTPUTS = (OUT_PYTHON, OUT_RUST, OUT_JAVASCRIPT)

_NAME = re.compile(r"^[a-z][a-z0-9_]*$")
_TOKEN = re.compile(r"^molt\.[a-z][a-z0-9_]*$")
_WASM_TYPES = frozenset({"i32", "i64", "f32", "f64"})
_NATIVE_TYPES = frozenset({"molt_value", "pointer", "u64", "i32"})
_LOWERINGS = frozenset(
    {"object_values", "object_callargs", "forward_f32", "pyinit_module"}
)
_REQUIRED_ABI_KEYS = frozenset(
    {
        "name",
        "token",
        "lowering",
        "browser_params",
        "browser_result",
        "wasm_params",
        "wasm_results",
        "native_params",
        "native_results",
    }
)
_LOWERING_CONTRACTS = {
    "object_values": (
        None,
        ("molt.value...",),
        "molt.value",
        ("i64...",),
        ("i64",),
        ("molt_value...",),
        ("molt_value",),
    ),
    "object_callargs": (
        1,
        ("molt.callargs",),
        "molt.value",
        ("i64",),
        ("i64",),
        ("molt_value",),
        ("molt_value",),
    ),
    "forward_f32": (
        1,
        ("bytes.float32",),
        "bytes.float32",
        ("i32", "i64", "i32"),
        ("i32",),
        ("pointer", "u64", "pointer"),
        ("i32",),
    ),
    "pyinit_module": (
        0,
        (),
        "molt.pyobject_ptr",
        (),
        ("i32",),
        (),
        ("pointer",),
    ),
}


class SchemaError(ValueError):
    """The checked-in native-callable ABI authority is inconsistent."""


@dataclass(frozen=True)
class NativeCallableAbi:
    name: str
    token: str
    lowering: str
    fixed_arity: int | None
    browser_params: tuple[str, ...]
    browser_result: str
    wasm_params: tuple[str, ...]
    wasm_results: tuple[str, ...]
    native_params: tuple[str, ...]
    native_results: tuple[str, ...]

    @property
    def constant(self) -> str:
        return f"NATIVE_CALLABLE_ABI_{self.name.upper()}"

    @property
    def rust_variant(self) -> str:
        return "".join(part.capitalize() for part in self.name.split("_"))

    @property
    def rust_lowering(self) -> str:
        return "".join(part.capitalize() for part in self.lowering.split("_"))

    @property
    def uses_callargs(self) -> bool:
        return self.lowering == "object_callargs"

    @property
    def requires_direct_symbol_binding(self) -> bool:
        return self.lowering in {"forward_f32", "pyinit_module"}


@dataclass(frozen=True)
class Schema:
    abis: tuple[NativeCallableAbi, ...]


def _string_list(value: object, context: str) -> tuple[str, ...]:
    if not isinstance(value, list) or not all(
        isinstance(item, str) and item for item in value
    ):
        raise SchemaError(f"{context} must be a list of non-empty strings")
    return tuple(cast(list[str], value))


def load_schema(path: Path = SOURCE) -> Schema:
    data = tomllib.loads(path.read_text(encoding="utf-8"))
    if set(data) != {"schema_version", "abi"}:
        raise SchemaError("top-level keys must be exactly schema_version and abi")
    if data["schema_version"] != 1:
        raise SchemaError("native-callable ABI schema_version must be 1")
    rows = data["abi"]
    if (
        not isinstance(rows, list)
        or not rows
        or not all(isinstance(row, dict) for row in rows)
    ):
        raise SchemaError("abi must be a non-empty array of tables")

    abis: list[NativeCallableAbi] = []
    names: set[str] = set()
    tokens: set[str] = set()
    variants: set[str] = set()
    for index, raw_row in enumerate(rows):
        row = cast(dict[str, object], raw_row)
        keys = set(row)
        allowed = _REQUIRED_ABI_KEYS | {"fixed_arity"}
        if not _REQUIRED_ABI_KEYS <= keys or not keys <= allowed:
            missing = sorted(_REQUIRED_ABI_KEYS - keys)
            unknown = sorted(keys - allowed)
            raise SchemaError(
                f"abi[{index}] keys are invalid: missing={missing!r}, unknown={unknown!r}"
            )
        name = row["name"]
        token = row["token"]
        lowering = row["lowering"]
        if not isinstance(name, str) or _NAME.fullmatch(name) is None:
            raise SchemaError(f"abi[{index}].name must be a snake_case identifier")
        if not isinstance(token, str) or _TOKEN.fullmatch(token) is None:
            raise SchemaError(f"abi {name!r} token must be a canonical molt.* token")
        if token != f"molt.{name}":
            raise SchemaError(f"abi {name!r} token must be exactly 'molt.{name}'")
        if not isinstance(lowering, str) or lowering not in _LOWERINGS:
            raise SchemaError(
                f"abi {name!r} lowering must be one of {sorted(_LOWERINGS)!r}"
            )
        variant = "".join(part.capitalize() for part in name.split("_"))
        if name in names or token in tokens or variant in variants:
            raise SchemaError(f"duplicate native-callable ABI identity {name!r}")
        names.add(name)
        tokens.add(token)
        variants.add(variant)

        fixed_arity_value = row.get("fixed_arity")
        if fixed_arity_value is None:
            fixed_arity = None
        elif (
            not isinstance(fixed_arity_value, int)
            or isinstance(fixed_arity_value, bool)
            or fixed_arity_value < 0
        ):
            raise SchemaError(
                f"abi {name!r} fixed_arity must be a non-negative integer"
            )
        else:
            fixed_arity = fixed_arity_value

        browser_params = _string_list(
            row["browser_params"], f"abi {name}.browser_params"
        )
        browser_result = row["browser_result"]
        if not isinstance(browser_result, str) or not browser_result:
            raise SchemaError(f"abi {name}.browser_result must be a non-empty string")
        wasm_params = _string_list(row["wasm_params"], f"abi {name}.wasm_params")
        wasm_results = _string_list(row["wasm_results"], f"abi {name}.wasm_results")
        native_params = _string_list(row["native_params"], f"abi {name}.native_params")
        native_results = _string_list(
            row["native_results"], f"abi {name}.native_results"
        )

        variadic_wasm = tuple(value for value in wasm_params if value.endswith("..."))
        variadic_native = tuple(
            value for value in native_params if value.endswith("...")
        )
        if fixed_arity is None:
            if (
                len(wasm_params) != 1
                or len(variadic_wasm) != 1
                or len(native_params) != 1
                or len(variadic_native) != 1
            ):
                raise SchemaError(
                    f"variadic ABI {name!r} must declare one variadic WASM and native payload type"
                )
        elif variadic_wasm or variadic_native:
            raise SchemaError(
                f"fixed-arity ABI {name!r} cannot declare variadic machine types"
            )
        if any(value.endswith("...") for value in (*wasm_results, *native_results)):
            raise SchemaError(f"abi {name!r} result machine types cannot be variadic")

        plain_wasm_types = {
            value.removesuffix("...") for value in (*wasm_params, *wasm_results)
        }
        if unknown_wasm := plain_wasm_types - _WASM_TYPES:
            raise SchemaError(
                f"abi {name!r} has unsupported WASM types {sorted(unknown_wasm)!r}"
            )
        plain_native_types = {
            value.removesuffix("...") for value in (*native_params, *native_results)
        }
        if unknown_native := plain_native_types - _NATIVE_TYPES:
            raise SchemaError(
                f"abi {name!r} has unsupported native machine types {sorted(unknown_native)!r}"
            )
        if fixed_arity is not None and fixed_arity != len(browser_params):
            raise SchemaError(
                f"abi {name!r} fixed_arity must equal its browser payload parameter count"
            )
        if lowering == "object_callargs" and (
            fixed_arity != 1 or browser_params != ("molt.callargs",)
        ):
            raise SchemaError(
                f"callargs ABI {name!r} must have one molt.callargs payload parameter"
            )

        actual_contract = (
            fixed_arity,
            browser_params,
            browser_result,
            wasm_params,
            wasm_results,
            native_params,
            native_results,
        )
        if actual_contract != _LOWERING_CONTRACTS[lowering]:
            raise SchemaError(
                f"abi {name!r} signatures and arity must match lowering {lowering!r}"
            )

        abis.append(
            NativeCallableAbi(
                name=name,
                token=token,
                lowering=lowering,
                fixed_arity=fixed_arity,
                browser_params=browser_params,
                browser_result=browser_result,
                wasm_params=wasm_params,
                wasm_results=wasm_results,
                native_params=native_params,
                native_results=native_results,
            )
        )
    return Schema(tuple(abis))


def _rust_string(value: str) -> str:
    return '"' + value.replace("\\", "\\\\").replace('"', '\\"') + '"'


def _rust_string_slice(values: tuple[str, ...]) -> str:
    return "&[" + ", ".join(_rust_string(value) for value in values) + "]"


def _rust_bool_match(schema: Schema, attribute: str) -> list[str]:
    truthy = [abi for abi in schema.abis if getattr(abi, attribute)]
    if not truthy:
        return ["        false\n"]
    patterns = " | ".join(f"Self::{abi.rust_variant}" for abi in truthy)
    return [f"        matches!(self, {patterns})\n"]


def render_python(schema: Schema) -> str:
    lines = [
        "# @generated by tools/gen_native_callable_abi.py from\n",
        "# runtime/native_callable_abi.toml. DO NOT EDIT.\n",
        '"""Native-callable ABI facts shared by package admission and lowering."""\n\n',
        "from __future__ import annotations\n\n",
        "from typing import Any, Final, TypedDict\n\n",
    ]
    for abi in schema.abis:
        lines.append(f"{abi.constant}: Final = {abi.token!r}\n")
    lines.extend(["\nNATIVE_CALLABLE_ABIS: Final[tuple[str, ...]] = (\n"])
    for abi in schema.abis:
        lines.append(f"    {abi.constant},\n")
    lines.extend(
        [
            ")\n",
            "KNOWN_NATIVE_CALLABLE_ABIS: Final[frozenset[str]] = frozenset(NATIVE_CALLABLE_ABIS)\n",
            'NATIVE_CALLABLE_ABI_CHOICES: Final = ", ".join(NATIVE_CALLABLE_ABIS)\n\n\n',
            "class _NativeCallableBrowserSignature(TypedDict):\n",
            "    params: list[str]\n",
            "    result: str\n\n\n",
            "_NATIVE_CALLABLE_BROWSER_SIGNATURES: Final[\n",
            "    dict[str, _NativeCallableBrowserSignature]\n",
            "] = {\n",
        ]
    )
    for abi in schema.abis:
        lines.extend(
            [
                f"    {abi.constant}: {{\n",
                f'        "params": {list(abi.browser_params)!r},\n',
                f'        "result": {abi.browser_result!r},\n',
                "    },\n",
            ]
        )
    lines.extend(
        ["}\n\n", "_NATIVE_CALLABLE_FIXED_ARITY: Final[dict[str, int | None]] = {\n"]
    )
    for abi in schema.abis:
        lines.append(f"    {abi.constant}: {abi.fixed_arity!r},\n")
    lines.extend(
        [
            "}\n\n",
            "_NATIVE_CALLABLE_CALLARGS_ABIS: Final[frozenset[str]] = frozenset(\n",
            "    {\n",
        ]
    )
    for abi in schema.abis:
        if abi.uses_callargs:
            lines.append(f"        {abi.constant},\n")
    lines.extend(
        [
            "    }\n",
            ")\n\n",
            "_NATIVE_CALLABLE_DIRECT_SYMBOL_ABIS: Final[frozenset[str]] = frozenset(\n",
            "    {\n",
        ]
    )
    for abi in schema.abis:
        if abi.requires_direct_symbol_binding:
            lines.append(f"        {abi.constant},\n")
    lines.extend(
        [
            "    }\n",
            ")\n\n\n",
            "def normalize_native_callable_abi(value: Any) -> str | None:\n",
            "    if not isinstance(value, str):\n",
            "        return None\n",
            "    abi = value.strip()\n",
            "    if abi not in KNOWN_NATIVE_CALLABLE_ABIS:\n",
            "        return None\n",
            "    return abi\n\n\n",
            "def native_callable_abi_choices() -> str:\n",
            "    return NATIVE_CALLABLE_ABI_CHOICES\n\n\n",
            "def native_callable_browser_signature(abi: str) -> dict[str, object]:\n",
            "    signature = _NATIVE_CALLABLE_BROWSER_SIGNATURES[abi]\n",
            '    return {"params": list(signature["params"]), "result": signature["result"]}\n\n\n',
            "def native_callable_fixed_arity(abi: str) -> int | None:\n",
            "    return _NATIVE_CALLABLE_FIXED_ARITY[abi]\n\n\n",
            "def native_callable_uses_callargs(abi: str) -> bool:\n",
            "    return abi in _NATIVE_CALLABLE_CALLARGS_ABIS\n\n\n",
            "def native_callable_requires_direct_symbol_binding(abi: str) -> bool:\n",
            "    return abi in _NATIVE_CALLABLE_DIRECT_SYMBOL_ABIS\n",
        ]
    )
    return "".join(lines)


def render_javascript(schema: Schema) -> str:
    contracts = {
        abi.token: {
            "params": list(abi.browser_params),
            "result": abi.browser_result,
        }
        for abi in schema.abis
    }
    encoded = json.dumps(contracts, sort_keys=True, separators=(",", ":"))
    return "".join(
        [
            "// @generated by tools/gen_native_callable_abi.py from\n",
            "// runtime/native_callable_abi.toml. DO NOT EDIT.\n\n",
            f"const contracts = {encoded};\n",
            "for (const signature of Object.values(contracts)) {\n",
            "  Object.freeze(signature.params);\n",
            "  Object.freeze(signature);\n",
            "}\n",
            "export const NATIVE_CALLABLE_BROWSER_SIGNATURES = Object.freeze(contracts);\n\n",
            "export const nativeCallableBrowserSignature = (abi) => {\n",
            "  const signature = NATIVE_CALLABLE_BROWSER_SIGNATURES[abi];\n",
            "  if (signature === undefined) {\n",
            "    throw new Error(`unsupported browser native callable ABI: ${abi}`);\n",
            "  }\n",
            "  return { params: [...signature.params], result: signature.result };\n",
            "};\n",
        ]
    )


def _machine_vector_expr(
    values: tuple[str, ...],
    *,
    enum_name: str,
    variants: dict[str, str],
    payload_arity: bool,
) -> str:
    if len(values) == 1 and values[0].endswith("..."):
        if not payload_arity:
            raise AssertionError(
                "variadic machine signature is valid only for parameters"
            )
        value = values[0].removesuffix("...")
        return f"vec![{enum_name}::{variants[value]}; payload_arity]"
    entries = ", ".join(f"{enum_name}::{variants[value]}" for value in values)
    return f"vec![{entries}]"


def render_rust(schema: Schema) -> str:
    lines = [
        "// @generated by tools/gen_native_callable_abi.py from\n",
        "// runtime/native_callable_abi.toml. DO NOT EDIT.\n\n",
        "//! Native-callable ABI facts shared by SimpleIR, TIR, and all backends.\n\n",
    ]
    for abi in schema.abis:
        lines.append(f"pub const {abi.constant}: &str = {_rust_string(abi.token)};\n")
    lines.extend(["\npub const NATIVE_CALLABLE_ABIS: &[&str] = &[\n"])
    for abi in schema.abis:
        lines.append(f"    {abi.constant},\n")
    choices = ", ".join(abi.token for abi in schema.abis)
    lines.extend(
        [
            "];\n",
            f"pub const NATIVE_CALLABLE_ABI_CHOICES: &str = {_rust_string(choices)};\n\n",
            "#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]\n",
            "pub enum NativeCallableAbi {\n",
        ]
    )
    for abi in schema.abis:
        lines.append(f"    {abi.rust_variant},\n")
    lines.extend(
        [
            "}\n\n",
            "#[derive(Clone, Copy, Debug, Eq, PartialEq)]\n",
            "pub enum NativeCallableLowering {\n",
        ]
    )
    for lowering in sorted(_LOWERINGS):
        variant = "".join(part.capitalize() for part in lowering.split("_"))
        lines.append(f"    {variant},\n")
    lines.extend(
        [
            "}\n\n",
            "pub const NATIVE_CALLABLE_ABI_CONTRACTS: &[NativeCallableAbi] = &[\n",
        ]
    )
    for abi in schema.abis:
        lines.append(f"    NativeCallableAbi::{abi.rust_variant},\n")
    lines.extend(
        [
            "];\n\n",
            "#[derive(Clone, Copy, Debug, Eq, PartialEq)]\n",
            "pub struct NativeCallableBrowserSignature {\n",
            "    pub params: &'static [&'static str],\n",
            "    pub result: &'static str,\n",
            "}\n\n",
            "#[derive(Clone, Copy, Debug, Eq, PartialEq)]\n",
            "pub struct NativeCallableWasmSignature {\n",
            "    pub params: &'static [&'static str],\n",
            "    pub results: &'static [&'static str],\n",
            "}\n\n",
            "#[derive(Clone, Copy, Debug, Eq, PartialEq)]\n",
            "pub enum NativeCallableWasmType {\n",
            "    I32,\n",
            "    I64,\n",
            "    F32,\n",
            "    F64,\n",
            "}\n\n",
            "#[derive(Clone, Debug, Eq, PartialEq)]\n",
            "pub struct NativeCallableWasmMachineSignature {\n",
            "    pub params: Vec<NativeCallableWasmType>,\n",
            "    pub results: Vec<NativeCallableWasmType>,\n",
            "}\n\n",
            "#[derive(Clone, Copy, Debug, Eq, PartialEq)]\n",
            "pub enum NativeCallableMachineType {\n",
            "    MoltValue,\n",
            "    Pointer,\n",
            "    U64,\n",
            "    I32,\n",
            "}\n\n",
            "#[derive(Clone, Debug, Eq, PartialEq)]\n",
            "pub struct NativeCallableMachineSignature {\n",
            "    pub params: Vec<NativeCallableMachineType>,\n",
            "    pub results: Vec<NativeCallableMachineType>,\n",
            "}\n\n",
            "impl NativeCallableAbi {\n",
            "    pub const fn token(self) -> &'static str {\n",
            "        match self {\n",
        ]
    )
    for abi in schema.abis:
        lines.append(f"            Self::{abi.rust_variant} => {abi.constant},\n")
    lines.extend(
        [
            "        }\n",
            "    }\n\n",
            "    pub const fn lowering(self) -> NativeCallableLowering {\n",
            "        match self {\n",
        ]
    )
    for abi in schema.abis:
        lines.append(
            f"            Self::{abi.rust_variant} => NativeCallableLowering::{abi.rust_lowering},\n"
        )
    lines.extend(
        [
            "        }\n",
            "    }\n\n",
            "    pub const fn fixed_arity(self) -> Option<usize> {\n",
            "        match self {\n",
        ]
    )
    for abi in schema.abis:
        value = "None" if abi.fixed_arity is None else f"Some({abi.fixed_arity})"
        lines.append(f"            Self::{abi.rust_variant} => {value},\n")
    lines.extend(
        ["        }\n", "    }\n\n", "    pub const fn uses_callargs(self) -> bool {\n"]
    )
    lines.extend(_rust_bool_match(schema, "uses_callargs"))
    lines.extend(
        [
            "    }\n\n",
            "    pub const fn requires_direct_symbol_binding(self) -> bool {\n",
        ]
    )
    lines.extend(_rust_bool_match(schema, "requires_direct_symbol_binding"))
    lines.extend(
        [
            "    }\n\n",
            "    pub const fn browser_signature(self) -> NativeCallableBrowserSignature {\n",
            "        match self {\n",
        ]
    )
    for abi in schema.abis:
        lines.extend(
            [
                f"            Self::{abi.rust_variant} => NativeCallableBrowserSignature {{\n",
                f"                params: {_rust_string_slice(abi.browser_params)},\n",
                f"                result: {_rust_string(abi.browser_result)},\n",
                "            },\n",
            ]
        )
    lines.extend(
        [
            "        }\n",
            "    }\n\n",
            "    pub const fn wasm_signature(self) -> NativeCallableWasmSignature {\n",
            "        match self {\n",
        ]
    )
    for abi in schema.abis:
        lines.extend(
            [
                f"            Self::{abi.rust_variant} => NativeCallableWasmSignature {{\n",
                f"                params: {_rust_string_slice(abi.wasm_params)},\n",
                f"                results: {_rust_string_slice(abi.wasm_results)},\n",
                "            },\n",
            ]
        )
    lines.extend(
        [
            "        }\n",
            "    }\n\n",
            "    pub fn wasm_machine_signature(\n",
            "        self,\n",
            "        payload_arity: usize,\n",
            "    ) -> Option<NativeCallableWasmMachineSignature> {\n",
            "        if self.fixed_arity().is_some_and(|expected| expected != payload_arity) {\n",
            "            return None;\n",
            "        }\n",
            "        let (params, results) = match self {\n",
        ]
    )
    wasm_variants = {"i32": "I32", "i64": "I64", "f32": "F32", "f64": "F64"}
    for abi in schema.abis:
        params = _machine_vector_expr(
            abi.wasm_params,
            enum_name="NativeCallableWasmType",
            variants=wasm_variants,
            payload_arity=True,
        )
        results = _machine_vector_expr(
            abi.wasm_results,
            enum_name="NativeCallableWasmType",
            variants=wasm_variants,
            payload_arity=False,
        )
        lines.append(
            f"            Self::{abi.rust_variant} => ({params}, {results}),\n"
        )
    lines.extend(
        [
            "        };\n",
            "        Some(NativeCallableWasmMachineSignature { params, results })\n",
            "    }\n\n",
            "    pub fn native_machine_signature(\n",
            "        self,\n",
            "        payload_arity: usize,\n",
            "    ) -> Option<NativeCallableMachineSignature> {\n",
            "        if self.fixed_arity().is_some_and(|expected| expected != payload_arity) {\n",
            "            return None;\n",
            "        }\n",
            "        let (params, results) = match self {\n",
        ]
    )
    native_variants = {
        "molt_value": "MoltValue",
        "pointer": "Pointer",
        "u64": "U64",
        "i32": "I32",
    }
    for abi in schema.abis:
        params = _machine_vector_expr(
            abi.native_params,
            enum_name="NativeCallableMachineType",
            variants=native_variants,
            payload_arity=True,
        )
        results = _machine_vector_expr(
            abi.native_results,
            enum_name="NativeCallableMachineType",
            variants=native_variants,
            payload_arity=False,
        )
        lines.append(
            f"            Self::{abi.rust_variant} => ({params}, {results}),\n"
        )
    lines.extend(
        [
            "        };\n",
            "        Some(NativeCallableMachineSignature { params, results })\n",
            "    }\n",
            "}\n\n",
            "pub fn parse_native_callable_abi(abi: &str) -> Option<NativeCallableAbi> {\n",
            "    match abi {\n",
        ]
    )
    for abi in schema.abis:
        lines.append(
            f"        {abi.constant} => Some(NativeCallableAbi::{abi.rust_variant}),\n"
        )
    lines.extend(
        [
            "        _ => None,\n",
            "    }\n",
            "}\n\n",
            "pub fn is_known_native_callable_abi(abi: &str) -> bool {\n",
            "    parse_native_callable_abi(abi).is_some()\n",
            "}\n\n",
            "#[cfg(test)]\n",
            "mod tests {\n",
            "    use super::*;\n\n",
            "    #[test]\n",
            "    fn every_native_callable_contract_round_trips() {\n",
            "        assert_eq!(NATIVE_CALLABLE_ABIS.len(), NATIVE_CALLABLE_ABI_CONTRACTS.len());\n",
            "        for (&token, &contract) in NATIVE_CALLABLE_ABIS.iter().zip(NATIVE_CALLABLE_ABI_CONTRACTS) {\n",
            "            assert_eq!(parse_native_callable_abi(token), Some(contract));\n",
            "            assert_eq!(contract.token(), token);\n",
            "            assert!(!contract.browser_signature().result.is_empty());\n",
            "            assert!(!contract.wasm_signature().results.is_empty());\n",
            "            let arity = contract.fixed_arity().unwrap_or(3);\n",
            "            assert!(contract.wasm_machine_signature(arity).is_some());\n",
            "            assert!(contract.native_machine_signature(arity).is_some());\n",
            "            if let Some(fixed_arity) = contract.fixed_arity() {\n",
            "                assert!(contract.wasm_machine_signature(fixed_arity + 1).is_none());\n",
            "                assert!(contract.native_machine_signature(fixed_arity + 1).is_none());\n",
            "            }\n",
            "        }\n",
            '        assert!(parse_native_callable_abi("molt.unknown_v1").is_none());\n',
            "    }\n",
            "}\n",
        ]
    )
    return "".join(lines)


def _format_python(source: str) -> str:
    completed = _COMMANDS.run(
        [
            sys.executable,
            "-m",
            "ruff",
            "format",
            "-",
            "--stdin-filename",
            str(OUT_PYTHON),
        ],
        cwd=ROOT,
        input=source,
        text=True,
        encoding="utf-8",
        errors="replace",
        capture_output=True,
        check=False,
    )
    if completed.returncode != 0:
        raise RuntimeError(f"ruff format failed:\n{completed.stderr}")
    return completed.stdout


def _format_rust(source: str) -> str:
    rustfmt = shutil.which("rustfmt")
    if rustfmt is None:
        raise RuntimeError("rustfmt is required to generate native-callable ABI Rust")
    completed = _COMMANDS.run(
        [rustfmt, "--edition", "2024", "--emit", "stdout"],
        cwd=ROOT,
        input=source,
        text=True,
        encoding="utf-8",
        errors="replace",
        capture_output=True,
        check=False,
    )
    if completed.returncode != 0:
        raise RuntimeError(f"rustfmt failed:\n{completed.stderr}")
    return completed.stdout


def render_all(schema: Schema) -> dict[Path, str]:
    return {
        OUT_PYTHON: _format_python(render_python(schema)),
        OUT_RUST: _format_rust(render_rust(schema)),
        OUT_JAVASCRIPT: render_javascript(schema),
    }


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument(
        "--check", action="store_true", help="fail if generated outputs are stale"
    )
    args = parser.parse_args(argv)
    stale = False
    for path, source in render_all(load_schema()).items():
        if args.check:
            if not generated_file_matches(path, source):
                print(
                    f"STALE generated file: {path.relative_to(ROOT)}", file=sys.stderr
                )
                stale = True
        else:
            write_generated_text(path, source)
    return int(stale)


if __name__ == "__main__":
    raise SystemExit(main())
