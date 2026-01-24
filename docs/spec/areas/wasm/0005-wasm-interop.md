# Molt WASM & Interop

## 1. Molt Packages & WASM
Molt Packages are the replacement for C-extensions. They are preferred to be compiled to WASM for:
- **Portability**: Same binary runs on macOS/Linux.
- **Security**: Strict capability-based sandboxing.
- **Stability**: Fixed ABI independent of Molt's internal changes.

## 2. WASM Component Model & WIT
Molt uses the WASM Component Model. Interfaces are defined using `wit` files.

### 2.1 Example: `molt-json.wit` (compatibility)
```wit
package molt:json;

interface parser {
    record json-value {
        kind: string,
        data: list<u8>
    }
    parse: func(input: string) -> result<json-value, string>;
}

world json-service {
    import parser;
}
```

### 2.2 Runtime intrinsics WIT
Core runtime imports are defined in `wit/molt-runtime.wit` and consumed by the WASM backend. This is the stable contract for NaN-boxed values, async/task hooks, and scalar parsing.

## 3. Calling Convention (Molt-to-WASM)
1.  **Serialization**: Molt serializes Python objects to a linear memory buffer (for complex types) or passes primitives directly.
2.  **Lifting/Lowering**: Uses the WASM Canonical ABI to lift Python types into WASM types.
3.  **Zero-copy**: For `bytes` and `str`, Molt passes a pointer/length into the WASM instance's memory to avoid copying when possible.

**Codec preference:** MsgPack/CBOR for production payloads, Arrow IPC for tabular data. JSON remains for compatibility/debug workflows.

## 4. FFI: The `@molt.ffi` Attribute
Molt allows direct binding to Rust-defined functions in a Molt Package.
```python
# molt_json is a compatibility package (compiled Rust or WASM).
# Use `--codec json` when the compiler should lower parse calls to JSON.
@molt.ffi("molt_json::parse")
def parse_json(data: str) -> dict:
    """Compiled by Molt into a direct call to the WASM export."""
    ...
```

## 5. Sandboxing & Capabilities
Molt uses a "Principle of Least Privilege".
- **Default**: No access to FS, Network, or Env.
- **Explicit**: Capabilities are granted in `molt.toml` or `pyproject.toml`.
  ```toml
  [molt.packages.molt_http]
  capabilities = ["net"]
  ```
