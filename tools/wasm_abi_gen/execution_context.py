"""The generated attached-execution calling convention.

The manifest owns classification. Logical callable identity and Python arity
remain independent of physical symbols and the hidden machine argument.
"""

from __future__ import annotations

import hashlib
import json


def context_schema(data: dict) -> dict:
    spec = data.get("execution_context_abi")
    if not isinstance(spec, dict) or set(spec) != {
        "version",
        "carrier",
        "position",
        "thread_record_offset",
        "section",
        "boundary_entries",
        "compiled_entries",
    }:
        raise ValueError(
            "execution_context_abi must declare the complete context protocol"
        )
    if any(
        type(spec[key]) is not int
        for key in ("version", "position", "thread_record_offset")
    ) or (
        spec["version"],
        spec["carrier"],
        spec["position"],
        spec["thread_record_offset"],
    ) != (1, "i64", 0, 0):
        raise ValueError("unsupported attached execution context ABI")
    if (
        not isinstance(spec["section"], str)
        or not spec["section"]
        or "\0" in spec["section"]
    ):
        raise ValueError("attached context ABI needs its nonempty section identity")
    names = spec["boundary_entries"]
    if (
        not isinstance(names, list)
        or any(not isinstance(n, str) or not n for n in names)
        or len(set(names)) != len(names)
    ):
        raise ValueError("context boundary entries must be distinct declared symbols")
    compiled = spec["compiled_entries"]
    if (
        not isinstance(compiled, list)
        or not compiled
        or any(not isinstance(n, str) or not n for n in compiled)
        or len(set(compiled)) != len(compiled)
        or set(compiled) & set(names)
    ):
        raise ValueError("compiled entries must be distinct attached symbols")
    return spec


def context_fingerprint(data: dict) -> int:
    # Bind the transport and classification; the ordinary object ABI witness
    # separately binds every record offset generated code addresses.
    wire = json.dumps(
        context_schema(data), sort_keys=True, separators=(",", ":")
    ).encode()
    return int.from_bytes(hashlib.sha256(wire).digest()[:8], "big")


def context_suffix(data: dict) -> str:
    return f"__molt_context_{context_fingerprint(data):016x}"


def runtime_execution_specs(data: dict) -> list[dict]:
    boundary = set(context_schema(data)["boundary_entries"])
    symbols = set(boundary)
    symbols.update(context_schema(data)["compiled_entries"])
    # Actual Rust producers complete the raw native/core bridge closure. Their
    # typed signatures, not a handwritten backend list, own these entries.
    from tools.wasm_abi_gen.manifest import _rust_export_signatures

    symbols.update(_rust_export_signatures(include_native=True))
    # Macro-generated entries are real producers too. Read literal identities
    # only at invocations of source-declared C-export producer macros.
    from pathlib import Path
    from tools.wasm_abi_gen.attached_entries import exported_macro_symbols

    root = Path(__file__).resolve().parents[2]
    symbols.update(exported_macro_symbols(root))
    for entry in data["runtime_import"]:
        symbols.add(entry.get("runtime_name") or "molt_" + entry["name"])
    for entry in data.get("host_import", []):
        symbols.add(entry["name"])
    for entry in data.get("link_allowed_import", []):
        if entry["name"].startswith("molt_call_indirect"):
            symbols.add(entry["name"])
    for entry in data.get("reserved_runtime_callable", []):
        symbols.add(entry["runtime_name"])
    return [
        dict(
            symbol=s,
            context="Boundary" if s in boundary else "Attached",
            physical_symbol=s if s in boundary else s + context_suffix(data),
        )
        for s in sorted(symbols)
    ]


def render_rust_context_abi(data: dict) -> str:
    suffix = context_suffix(data)
    lines = [
        "\n/// Calling convention selected by the canonical runtime-call manifest.\n",
        "#[derive(Clone, Copy, Debug, Eq, PartialEq)]\n",
        "pub enum RuntimeExecutionContext { Attached, Boundary }\n\n",
        "#[derive(Clone, Copy, Debug, Eq, PartialEq)]\n",
        "pub struct RuntimeExecutionAbi {\n",
        "    pub context: RuntimeExecutionContext,\n    pub physical_symbol: &'static str,\n}\n\n",
        f"pub const ATTACHED_EXECUTION_CONTEXT_ABI_FINGERPRINT: u64 = 0x{context_fingerprint(data):016x};\n",
        f'pub const ATTACHED_EXECUTION_CONTEXT_ABI_SUFFIX: &str = "{suffix}";\n',
        "pub const ATTACHED_CONTEXT_THREAD_RECORD_OFFSET: i32 = 0;\n",
        f"pub const ATTACHED_EXECUTION_CONTEXT_ABI_SECTION: &str = {json.dumps(context_schema(data)['section'])};\n\n",
        "pub fn compiled_execution_symbol(logical: &str) -> String {\n",
        "    let mut physical = String::with_capacity(logical.len() + ATTACHED_EXECUTION_CONTEXT_ABI_SUFFIX.len());\n",
        "    physical.push_str(logical);\n    physical.push_str(ATTACHED_EXECUTION_CONTEXT_ABI_SUFFIX);\n    physical\n}\n\n",
        "pub fn runtime_execution_abi(logical: &str) -> Option<RuntimeExecutionAbi> {\n",
        "    Some(match logical {\n",
    ]
    for row in runtime_execution_specs(data):
        lines.append(
            f'        "{row["symbol"]}" => RuntimeExecutionAbi {{ context: RuntimeExecutionContext::{row["context"]}, physical_symbol: "{row["physical_symbol"]}" }},\n'
        )
    lines.extend(["        _ => return None,\n    })\n}\n"])
    return "".join(lines)


def render_context_storage(data: dict) -> str:
    return f'''// @generated by tools/gen_wasm_abi.py. DO NOT EDIT.
use core::cell::Cell;

pub const ATTACHED_EXECUTION_CONTEXT_ABI_FINGERPRINT: u64 = 0x{context_fingerprint(data):016x};
pub const ATTACHED_EXECUTION_CONTEXT_ABI_SUFFIX: &str = "{context_suffix(data)}";
pub const ATTACHED_CONTEXT_THREAD_RECORD_OFFSET: i32 = 0;
pub const ATTACHED_CONTEXT_ARGUMENT_POSITION: u32 = {context_schema(data)["position"]};
pub const ATTACHED_CONTEXT_ARGUMENT_CARRIER_BYTES: u32 = 8;
pub const ATTACHED_EXECUTION_CONTEXT_ABI_SECTION: &str = {json.dumps(context_schema(data)["section"])};

/// Permanent storage. Only admitted custody lends it as a typed Python token.
#[repr(C)]
pub struct AttachedExecutionContextStorage {{
    thread_record: Cell<u64>,
}}

impl AttachedExecutionContextStorage {{
    pub const fn new() -> Self {{ Self {{ thread_record: Cell::new(0) }} }}
    pub fn bind_thread_record(&self, address: u64) {{
        assert_ne!(address, 0, "attached context requires its actual thread record");
        let previous = self.thread_record.get();
        if previous == 0 {{ self.thread_record.set(address); }}
        else {{ assert_eq!(previous, address, "attached context changed native thread"); }}
    }}
    #[inline(always)]
    pub fn thread_record(&self) -> u64 {{ self.thread_record.get() }}
}}
impl Default for AttachedExecutionContextStorage {{ fn default() -> Self {{ Self::new() }} }}
const _: () = {{
    assert!(core::mem::offset_of!(AttachedExecutionContextStorage, thread_record) == ATTACHED_CONTEXT_THREAD_RECORD_OFFSET as usize);
    assert!(!core::mem::needs_drop::<AttachedExecutionContextStorage>());
}};
'''


def render_python_context_abi(data: dict) -> str:
    rows = runtime_execution_specs(data)
    lines = [
        "\nfrom typing import NamedTuple\n\n",
        "class RuntimeExecutionAbi(NamedTuple):\n    context: Literal['Attached', 'Boundary']\n    physical_symbol: str\n\n",
        f"ATTACHED_EXECUTION_CONTEXT_ABI_FINGERPRINT: int = {context_fingerprint(data)}\n",
        f"ATTACHED_EXECUTION_CONTEXT_ABI_SUFFIX: str = {context_suffix(data)!r}\n",
        "ATTACHED_CONTEXT_THREAD_RECORD_OFFSET: int = 0\n",
        f"ATTACHED_EXECUTION_CONTEXT_ABI_SECTION: str = {context_schema(data)['section']!r}\n",
        "RUNTIME_EXECUTION_ABIS: dict[str, RuntimeExecutionAbi] = {\n",
    ]
    lines.extend(
        f"    {r['symbol']!r}: RuntimeExecutionAbi({r['context']!r}, {r['physical_symbol']!r}),\n"
        for r in rows
    )
    lines.extend(
        [
            "}\n\ndef runtime_execution_abi(logical: str) -> RuntimeExecutionAbi | None:\n",
            "    return RUNTIME_EXECUTION_ABIS.get(logical)\n\n",
            "def compiled_execution_symbol(logical: str) -> str:\n",
            "    return logical + ATTACHED_EXECUTION_CONTEXT_ABI_SUFFIX\n",
        ]
    )
    return "".join(lines)


def render_physical_symbol_macro(data: dict) -> str:
    lines = [
        "\n/// Closed physical link identities for actual attached producers.\n",
        "#[macro_export]\nmacro_rules! attached_symbol {\n",
    ]
    for row in runtime_execution_specs(data):
        if row["context"] == "Attached":
            lines.append(
                f'    ("{row["symbol"]}") => {{ "{row["physical_symbol"]}" }};\n'
            )
    lines.append(
        '    ($unknown:literal) => { compile_error!(concat!("symbol has no canonical attached execution producer: ", $unknown)) };\n}\n'
    )
    return "".join(lines)


def context_json(data: dict) -> dict:
    return dict(
        fingerprint=str(context_fingerprint(data)),
        suffix=context_suffix(data),
        thread_record_offset=0,
        section=context_schema(data)["section"],
    )
