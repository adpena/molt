# `@molt.proto` Decorator Design -- Protobuf Message Mapping

## Goal

Provide a first-class Python decorator that maps dataclass-like Python classes
to protobuf messages at compile time, using buffa 0.2 for wire encoding and
zero-copy decoding.  The decorator eliminates the need for `.proto` files or
code-generation steps for simple message types while retaining full wire
compatibility with standard protobuf toolchains.

## Current Grounded Status (2026-03-28)

- `molt-runtime-protobuf` crate exists with `buffa = "0.2"` dependency.
- The crate exports `MessageSchema`, `FieldDef`, `WireType`, and convenience
  wrappers around buffa's varint codec and field-level encoding primitives.
- buffa re-exports are available: `Message` trait (owned encode/decode),
  `MessageView` trait (zero-copy views), `Tag`, `WireType`, `DecodeError`,
  `EncodeError`.
- SimpleTIRGenerator already recognizes class-level decorators (`@dataclass`,
  `@gpu.kernel`, `@contextmanager`) and lowers them to typed IR with
  compile-time metadata extraction.
- WASM host import infrastructure is mature: the backend emits typed import
  calls and the host import registry in `wasm_imports.rs` dispatches them to
  Rust implementations.

## Problem Statement

Python applications targeting Cloudflare Workers and other edge runtimes need
efficient protobuf serialization for gRPC and wire-format APIs.  Today, this
requires either:

1. Bundling a full Python protobuf runtime (google.protobuf) -- which is large,
   slow, and relies on runtime reflection that Molt does not support.
2. Hand-writing encode/decode logic -- which is error-prone and defeats the
   purpose of a schema-driven format.

Molt can do better: the compiler sees the full type structure at compile time,
so it can generate direct calls to buffa's encode/decode functions with no
runtime reflection, no descriptor pool, and no allocations on the decode path
(via zero-copy views).

## Non-Negotiable Requirements

1. Wire-compatible with proto3 encoding.  Messages encoded by Molt must be
   decodable by any standard protobuf decoder, and vice versa.
2. Field numbers are compile-time constants.  No runtime descriptor reflection.
3. Zero-copy `decode_view` path must work on WASM linear memory without
   copying the input buffer.
4. Type errors (wrong field type, missing required field number) are
   compile-time errors, not runtime panics.
5. No implicit field numbering.  Every field must have an explicit field number
   via `field(N)`.

## Non-Goals

- **No `.proto` file parsing.**  Use `buffa-build` for that workflow.  This
  decorator is for Python-first schema definition.
- **No runtime reflection or descriptor access.**  No `DESCRIPTOR` attribute,
  no `ListFields()`, no `WhichOneof()`.
- **No service/RPC generation.**  gRPC service stubs are a separate feature
  that builds on top of message types.
- **No proto2 semantics.**  Required fields, default values, extensions, and
  groups are out of scope.  This is proto3-only.
- **No `oneof` in the initial implementation.**  Can be added later via
  `typing.Union` mapping.

---

## User-Facing API

### Message Declaration

```python
from molt.proto import message, field

@message("mypackage.UserProfile")
class UserProfile:
    name: str = field(1)         # field number 1, wire type length-delimited
    age: int = field(2)          # field number 2, wire type varint
    email: str = field(3)
    scores: list[float] = field(4, repeated=True)
```

The `@message` decorator takes a single positional argument: the fully
qualified protobuf message name.  This name is used for nested message type
URLs and debug output; it has no effect on wire encoding.

The `field()` sentinel takes a positional field number and optional keyword
arguments:

| Parameter  | Type   | Default | Description                          |
|------------|--------|---------|--------------------------------------|
| `number`   | int    | required| Protobuf field number (1-536870911)  |
| `repeated` | bool   | False   | Whether the field is a repeated field |
| `packed`   | bool   | True    | Pack repeated scalar fields (proto3 default) |

Fields without a `field()` assignment are compile-time errors.

### Encoding

```python
user = UserProfile(name="Alice", age=30, email="alice@example.com", scores=[9.5, 8.0])

# Encode to wire bytes
wire_bytes: bytes = user.encode()
```

`encode()` returns a `bytes` object containing the proto3 wire encoding.
Fields set to their default values (0, empty string, empty list) are omitted
per proto3 semantics.

### Decoding (Owned)

```python
decoded: UserProfile = UserProfile.decode(wire_bytes)
assert decoded.name == "Alice"
assert decoded.age == 30
```

`decode()` is a classmethod that parses wire bytes and returns a new owned
instance.  Unknown fields are silently skipped (proto3 forward compatibility).

### Decoding (Zero-Copy View)

```python
view: UserProfile.View = UserProfile.decode_view(wire_bytes)
print(view.name)   # zero-copy slice into wire_bytes
print(view.age)    # parsed on access from wire_bytes
```

`decode_view()` returns a lightweight view object that borrows from the input
buffer.  String and bytes fields return zero-copy slices.  Scalar fields are
decoded on access.  The view is valid only as long as the input buffer is alive.

This is the key performance feature for Cloudflare Workers: incoming request
bytes stay in WASM linear memory and the view reads directly from them with no
allocation.

### Nested Messages

```python
@message("mypackage.Address")
class Address:
    street: str = field(1)
    city: str = field(2)
    zip_code: str = field(3)

@message("mypackage.UserProfile")
class UserProfile:
    name: str = field(1)
    age: int = field(2)
    address: Address = field(3)              # nested message
    previous: list[Address] = field(4, repeated=True)  # repeated nested
```

Nested `@message` classes are encoded as length-delimited embedded messages.
The compiler resolves nesting at compile time; no forward references are
needed (all referenced message classes must be defined before use).

### Optional Fields

```python
from typing import Optional

@message("mypackage.SearchResult")
class SearchResult:
    title: str = field(1)
    snippet: Optional[str] = field(2)    # has presence tracking
    score: Optional[float] = field(3)
```

`Optional[T]` fields have explicit presence tracking.  They can be `None`
(absent from wire) or set to a value.  This maps to proto3's `optional` keyword.

---

## Type Mapping

| Python type       | Protobuf wire type   | buffa Rust type | Wire type ID |
|-------------------|---------------------|-----------------|--------------|
| `int`             | varint              | `i64`           | 0            |
| `float`           | fixed64             | `f64`           | 1            |
| `str`             | length-delimited    | `String`        | 2            |
| `bytes`           | length-delimited    | `Bytes`         | 2            |
| `bool`            | varint              | `bool`          | 0            |
| `list[T]`         | repeated            | `Vec<T>`        | varies       |
| `Optional[T]`     | optional            | `Option<T>`     | varies       |
| `@message` class  | length-delimited    | nested `Message`| 2            |

### Encoding Details

- **int**: Encoded as signed 64-bit varint using ZigZag encoding (`sint64`
  wire semantics).  This handles negative values correctly without the 10-byte
  penalty of raw negative varints.
- **float**: Encoded as 8-byte little-endian fixed64 (`double` wire semantics).
  This avoids precision loss from float32.
- **bool**: Encoded as single-byte varint (0 or 1).
- **str**: Encoded as UTF-8 bytes with varint length prefix.
- **bytes**: Encoded as raw bytes with varint length prefix.
- **list[scalar]**: Packed encoding by default (single length-delimited field
  containing concatenated scalar encodings).  Proto3 decoders accept both
  packed and unpacked; Molt always emits packed.
- **list[message]**: Each element is a separate length-delimited field entry
  (messages cannot be packed).

---

## Frontend Integration

### SimpleTIRGenerator Changes

SimpleTIRGenerator recognizes `@message` the same way it recognizes
`@dataclass`: during `visit_ClassDef`, decorator analysis checks for the
`molt.proto.message` decorator and extracts compile-time metadata.

The recognition flow:

1. **Decorator detection**: `_is_proto_message_decorator(deco)` checks for
   `message(...)` imported from `molt.proto`.
2. **Field extraction**: For each annotated class attribute with a `field(N)`
   default, extract:
   - Field name (from the attribute name)
   - Field number (from the `field()` positional argument)
   - Python type annotation (resolved to wire type via the type mapping table)
   - Repeated flag (from `field(..., repeated=True)`)
   - Optional flag (from `Optional[T]` annotation)
3. **Validation** (compile-time errors):
   - Field numbers must be positive integers in range [1, 536870911].
   - Field numbers must be unique within a message.
   - Field numbers 19000-19999 are reserved (protobuf spec).
   - Type annotations must map to a known wire type.
   - Attributes without `field()` are rejected.
   - `repeated=True` must match a `list[T]` annotation.
4. **Schema IR emission**: The frontend emits a `ProtoMessageDef` IR node
   containing the message name, field definitions, and resolved wire types.
   This node is stored in the module's type metadata and referenced by
   encode/decode call sites.

### IR Representation

```
ProtoMessageDef {
    name: "mypackage.UserProfile",
    fields: [
        ProtoFieldDef { number: 1, name: "name",   wire_type: LengthDelimited, repeated: false, optional: false },
        ProtoFieldDef { number: 2, name: "age",    wire_type: Varint,          repeated: false, optional: false },
        ProtoFieldDef { number: 3, name: "email",  wire_type: LengthDelimited, repeated: false, optional: false },
        ProtoFieldDef { number: 4, name: "scores", wire_type: Fixed64,         repeated: true,  optional: false },
    ]
}
```

The `ProtoMessageDef` is lowered through TIR like any other class definition.
The class constructor is synthesized to accept keyword arguments matching the
field names.  The `encode`, `decode`, and `decode_view` methods are synthesized
as static-dispatch calls -- they do not go through Python's method resolution.

---

## Backend Integration

### WASM Codegen

The WASM backend emits host import calls for encode and decode operations.
These are registered in `wasm_imports.rs` alongside existing host imports.

#### Host Imports

| Import name                | Signature                              | Description                    |
|---------------------------|----------------------------------------|--------------------------------|
| `molt_proto_encode`       | `(schema_id: i32, obj_ptr: i32) -> i32`| Encode object to wire bytes    |
| `molt_proto_decode`       | `(schema_id: i32, buf_ptr: i32, buf_len: i32) -> i32` | Decode wire bytes to owned object |
| `molt_proto_decode_view`  | `(schema_id: i32, buf_ptr: i32, buf_len: i32) -> i32` | Decode wire bytes to zero-copy view |

- `schema_id` is a compile-time integer identifying which `MessageSchema` to
  use.  The schema table is embedded in the WASM module's data section.
- `obj_ptr` points to the Molt object in linear memory.
- Return value is a pointer to the result (wire bytes for encode, object for
  decode).

#### Encode Path

```
user.encode()
  --> WASM: call $molt_proto_encode(schema_id=0, obj_ptr)
  --> Host: read fields from linear memory using schema layout
  --> Host: call buffa field encoders (encode_varint, encode_bytes_field, etc.)
  --> Host: write result bytes to linear memory, return pointer
```

The host reads each field from the object's memory layout (which the compiler
controls), encodes it using the corresponding `molt-runtime-protobuf` wrapper
function, and writes the concatenated wire bytes back to linear memory.

#### Decode Path (Owned)

```
UserProfile.decode(wire_bytes)
  --> WASM: call $molt_proto_decode(schema_id=0, buf_ptr, buf_len)
  --> Host: iterate wire bytes, decode tags and values using buffa
  --> Host: allocate Molt object in linear memory
  --> Host: populate fields from decoded values
  --> Host: return object pointer
```

#### Decode Path (Zero-Copy View)

```
UserProfile.decode_view(wire_bytes)
  --> WASM: call $molt_proto_decode_view(schema_id=0, buf_ptr, buf_len)
  --> Host: parse wire bytes to locate field offsets (no data copying)
  --> Host: allocate thin view object with offset table
  --> Host: return view pointer
```

The view object stores `(offset, length)` pairs for each length-delimited
field and raw decoded values for scalar fields.  String access returns a slice
into the original buffer.  This avoids all allocation for the common case of
reading a few fields from a large message.

### Native Codegen

The native backend emits direct calls to `molt-runtime-protobuf` functions.
No host import indirection is needed; the linker resolves symbols directly.

```
user.encode()
  --> call molt_runtime_protobuf::encode_message(&schema, &fields)
  --> returns Vec<u8>
```

This is straightforward because the native backend has direct access to the
Rust runtime.  The schema is a `&'static MessageSchema` embedded in the
compiled binary's read-only data section.

---

## Zero-Copy Views for Workers

This section details the zero-copy decode path, which is the primary
performance motivation for this feature.

### The Problem

A typical Cloudflare Worker handling gRPC:

1. Receives protobuf bytes in the request body.
2. Decodes the message to read a few fields.
3. Performs business logic.
4. Encodes a response message.

With owned decoding, step 2 allocates and copies every string and bytes field.
For a 10KB request message where the handler only reads 2 fields, this wastes
both time and memory.

### The Solution

`decode_view` creates a view that borrows from the input buffer:

```python
# Request bytes arrive in WASM linear memory at (buf_ptr, buf_len).
# No copy has occurred yet.

view = UserProfile.decode_view(request_body)
# View object: ~48 bytes (field offset table only)
# Original bytes: untouched in linear memory

name = view.name
# Returns a str that points directly into request_body
# Zero allocation, zero copy
```

### Implementation Details

The view object layout in linear memory:

```
ViewObject {
    header: MoltObjectHeader,       // 16 bytes (standard Molt object header)
    source_ptr: i32,                // pointer to original wire bytes
    source_len: i32,                // length of original wire bytes
    field_offsets: [FieldSlot; N],  // one per schema field
}

FieldSlot (for length-delimited fields) {
    offset: u32,   // byte offset into source buffer
    length: u32,   // byte length of the field value
}

FieldSlot (for scalar fields) {
    value: u64,    // decoded scalar value (varint, fixed32, fixed64)
    _pad: u32,
}
```

The view is populated by a single pass over the wire bytes.  For each field
tag encountered:

1. Decode the tag to get field number and wire type.
2. Look up the field number in the schema.
3. For scalars: decode the value and store it in the `FieldSlot`.
4. For length-delimited: record the `(offset, length)` without copying.
5. Skip unknown fields.

### Lifetime Safety

The view borrows from the input buffer.  In WASM, this is safe because:

- The input buffer is in linear memory and its lifetime is controlled by the
  host.
- The view object is also in linear memory.
- The host ensures the input buffer is not freed while views exist (reference
  counting on the buffer).

In native mode, the view holds a `&[u8]` borrow.  Rust's borrow checker
enforces the lifetime constraint.  The Python-level API communicates this as:
"the view is valid as long as `wire_bytes` is alive."

---

## Schema Embedding

Each `@message` class produces a `MessageSchema` that must be available at
encode/decode time.  The compiler embeds schemas in the binary:

### WASM

Schemas are serialized into the WASM module's data section as a compact binary
table:

```
SchemaTable {
    count: u32,
    schemas: [SerializedSchema; count],
}

SerializedSchema {
    name_offset: u32,   // offset into string table
    name_len: u32,
    field_count: u32,
    fields: [SerializedField; field_count],
}

SerializedField {
    number: u32,
    name_offset: u32,
    name_len: u32,
    wire_type: u8,
    flags: u8,          // bit 0: repeated, bit 1: optional
}
```

The `schema_id` passed to host imports is an index into this table.

### Native

Schemas are `static` Rust values:

```rust
static SCHEMA_USER_PROFILE: MessageSchema = MessageSchema {
    name: "mypackage.UserProfile",
    fields: &[
        FieldDef { number: 1, name: "name",  wire_type: WireType::LengthDelimited, repeated: false, optional: false },
        FieldDef { number: 2, name: "age",   wire_type: WireType::Varint,          repeated: false, optional: false },
        // ...
    ],
};
```

---

## Error Handling

### Compile-Time Errors

| Condition                                    | Error message                                        |
|----------------------------------------------|------------------------------------------------------|
| Missing `field()` on annotated attribute     | `proto field 'X' must have a field() assignment`     |
| Duplicate field number                       | `proto field number N is used by both 'X' and 'Y'`  |
| Reserved field number (19000-19999)          | `proto field number N is reserved`                   |
| Unsupported type annotation                  | `type 'X' cannot be mapped to a protobuf wire type` |
| `repeated=True` without `list[T]` annotation | `repeated field 'X' must have list[T] annotation`   |
| Field number out of range                    | `proto field number must be in [1, 536870911]`       |
| Nested message class not decorated           | `type 'X' used as proto field must be a @message`    |

### Runtime Errors

| Condition                | Behavior                                        |
|--------------------------|-------------------------------------------------|
| Decode: truncated input  | Raises `ValueError("protobuf decode: unexpected end of input")` |
| Decode: invalid varint   | Raises `ValueError("protobuf decode: invalid varint")`          |
| Decode: invalid UTF-8    | Raises `ValueError("protobuf decode: invalid UTF-8 in string field 'X'")` |
| Decode: unknown field    | Silently skipped (proto3 forward compatibility)                  |
| View: source buffer freed| Undefined behavior in WASM; compile-time borrow error in native |

---

## Testing Strategy

### Unit Tests (Rust)

Extend `molt-runtime-protobuf/src/lib.rs` tests:

- Roundtrip encode/decode for every Python type in the type mapping table.
- Edge cases: empty strings, empty lists, i64::MIN, i64::MAX, NaN, Inf.
- Packed repeated field encoding correctness.
- Nested message encode/decode.
- Unknown field skipping.
- Truncated input error handling.

### Integration Tests (Python)

- Compile a `@message` class and verify `encode()`/`decode()` roundtrip.
- Cross-validate with `google.protobuf` Python library (encode with Molt,
  decode with google.protobuf, and vice versa).
- Zero-copy view: verify that `decode_view` does not allocate (measure via
  WASM memory growth counter).
- Nested messages: encode/decode with 3+ levels of nesting.
- Repeated fields: encode/decode with 0, 1, 1000 elements.

### WASM-Specific Tests

- Host import registration: verify `molt_proto_encode`, `molt_proto_decode`,
  `molt_proto_decode_view` are present in the import table.
- Schema table embedding: verify the data section contains the schema table.
- Linear memory safety: decode_view followed by buffer operations to verify
  no corruption.

---

## Implementation Plan

### Phase 1: Frontend Recognition

1. Add `_is_proto_message_decorator` to SimpleTIRGenerator.
2. Add `ProtoMessageDef` and `ProtoFieldDef` to the IR.
3. Implement field extraction and compile-time validation.
4. Synthesize constructor, `encode`, `decode`, `decode_view` method stubs.

### Phase 2: Owned Encode/Decode

1. Implement `molt_proto_encode` host import using existing
   `molt-runtime-protobuf` wrappers.
2. Implement `molt_proto_decode` host import with full field-by-field decoding.
3. Register host imports in `wasm_imports.rs`.
4. Implement native backend direct calls.
5. End-to-end test: roundtrip a simple message through WASM.

### Phase 3: Zero-Copy Views

1. Implement view object layout and `decode_view` host import.
2. Implement lazy field access on view objects.
3. Verify zero-allocation property on WASM.
4. Benchmark against owned decode on realistic message sizes.

### Phase 4: Nested Messages and Repeated Fields

1. Add nested message encode/decode support.
2. Add packed repeated scalar encoding.
3. Add repeated message encoding.
4. Cross-validation tests with google.protobuf.

---

## Open Questions

1. **ZigZag vs. raw varint for `int`?**  ZigZag is more efficient for negative
   values but differs from proto3's default `int64` encoding.  Recommendation:
   use ZigZag (`sint64` semantics) since Python ints are commonly negative.
   Document that this means Molt `int` fields are wire-compatible with proto3
   `sint64`, not `int64`.

2. **`float` as fixed64 vs. fixed32?**  Python's `float` is 64-bit, so
   `fixed64` (`double`) preserves precision.  But proto3's `float` type is
   32-bit.  Recommendation: map to `double` by default; add `field(N, f32=True)`
   option for explicit 32-bit.

3. **Map fields?**  `dict[K, V]` could map to proto3 map fields.  Deferred to
   a future iteration due to the complexity of the synthetic map entry message.

4. **Enum fields?**  Python `enum.IntEnum` could map to proto3 enums.  Deferred
   to a future iteration.

5. **View lifetime in Python?**  Should `decode_view` return a context manager
   to make the borrowing relationship explicit?

   ```python
   with UserProfile.decode_view(wire_bytes) as view:
       name = view.name
   # view invalidated here
   ```

   This is safer but more verbose.  Recommendation: start with the simple API
   and add the context manager variant if lifetime bugs become a problem in
   practice.
