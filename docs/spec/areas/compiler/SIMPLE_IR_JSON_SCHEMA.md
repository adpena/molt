# SimpleIR JSON Schema Reference

Formal schema for the SimpleIR interchange format between the Python frontend
(`src/molt/frontend/__init__.py`) and the Rust backend
(`runtime/molt-backend/src/ir.rs`).

> Scope note: this document describes the current transport format only. The
> canonical IR architecture and representation contract live in
> `docs/spec/areas/compiler/0100_MOLT_IR.md`. Fields such as `fast_int`,
> `fast_float`, `raw_int`, and `type_hint` are transitional compatibility
> hints for transport consumers; they are not a substitute for
> representation-aware SSA.

## Transport Formats

### Batch JSON

Single JSON object containing all functions:

```json
{
  "functions": [ ... ],
  "profile": { ... }
}
```

### NDJSON Streaming

One JSON object per line. The backend accepts both formats via
`SimpleIR::from_json_str` (batch) and `SimpleIR::from_ndjson_reader` (stream).

```
{"kind":"ir_stream_start","profile":null}
{"kind":"function","name":"molt_main","params":[],"ops":[...]}
{"kind":"function","name":"helper","params":["a"],"ops":[...]}
{"kind":"ir_stream_end"}
```

Stream envelope kinds: `ir_stream_start`, `function`, `ir_stream_end`.
Unknown kinds are silently skipped for forward compatibility.

---

## Top-Level: SimpleIR

| Field       | Type              | Required | Description                          |
|-------------|-------------------|----------|--------------------------------------|
| `functions` | `FunctionIR[]`    | yes      | Ordered list of function definitions |
| `profile`   | `PgoProfileIR?`   | no       | Optional PGO profile data            |

### PgoProfileIR

| Field           | Type       | Required | Description                     |
|-----------------|------------|----------|---------------------------------|
| `version`       | `string?`  | no       | Profile format version          |
| `hash`          | `string?`  | no       | Source hash for staleness check |
| `hot_functions` | `string[]` | no       | Defaults to `[]` if absent      |

---

## FunctionIR

| Field         | Type       | Required | Description                                           |
|---------------|------------|----------|-------------------------------------------------------|
| `name`        | `string`   | yes      | Mangled function name (e.g. `molt_main`, `__main__`)  |
| `params`      | `string[]` | yes      | Parameter names (SSA value names)                     |
| `ops`         | `OpIR[]`   | yes      | Ordered instruction sequence                          |
| `param_types` | `string[]?`| no       | Optional type annotations parallel to `params`        |

The frontend may also emit `borrowed_params` (list of param names eligible for
Perceus-style borrow elision), but the backend does not require it.

---

## OpIR

All fields except `kind` are optional. The Rust struct uses `#[serde(default)]`
so absent fields deserialize to `None`/default.

| Field             | Type       | Default | Description                                                    |
|-------------------|------------|---------|----------------------------------------------------------------|
| `kind`            | `string`   | --      | **Required.** Operation discriminator (see below)              |
| `value`           | `i64?`     | `null`  | Integer immediate (const value, label target, field offset, code_id) |
| `f_value`         | `f64?`     | `null`  | Float immediate. Non-finite encoded as strings: `"NaN"`, `"Infinity"`, `"-Infinity"` |
| `s_value`         | `string?`  | `null`  | String immediate (string literal, call target name, attribute name) |
| `bytes`           | `u8[]?`    | `null`  | Raw byte array (for `const_bytes`, surrogate-containing `const_str`) |
| `var`             | `string?`  | `null`  | Variable name reference (used by store/load patterns)          |
| `args`            | `string[]?`| `null`  | Operand value names (SSA references)                           |
| `out`             | `string?`  | `null`  | Result value name (SSA definition)                             |
| `fast_int`        | `bool?`    | `null`  | Hint: operands are known integers, use unboxed fast path       |
| `fast_float`      | `bool?`    | `null`  | Hint: operands are known floats, use unboxed fast path         |
| `raw_int`         | `bool?`    | `null`  | Operands are raw (unboxed) i64. Mutually exclusive with `fast_int`. Allowed only on: `add`, `box_from_raw_int`, `const`, `loop_index_next`, `loop_index_start`, `lt`, `unbox_to_raw_int` |
| `stack_eligible`  | `bool?`    | `null`  | Hint: result can be stack-allocated                            |
| `task_kind`       | `string?`  | `null`  | Async task classification                                      |
| `container_type`  | `string?`  | `null`  | For `contains`: known container type (`set`, `frozenset`, `dict`, `list`, `str`) |
| `type_hint`       | `string?`  | `null`  | Type annotation from source                                    |
| `ic_index`        | `i64?`     | `null`  | Inline cache site index for `get_attr_generic_ptr`. Transmitted inside a nested `metadata` object in JSON: `{"metadata": {"ic_index": N}}` |

`fast_int`, `fast_float`, `raw_int`, and `type_hint` exist to describe
compatibility metadata on the current backend transport. New lowering work
should prefer preserving explicit representation in typed IR over introducing
additional transport-only hint fields.

### Validation Constraints

- `fast_int` and `raw_int` cannot both be `true` on the same op.
- `raw_int` is only valid on ops listed above.
- Every value name in `args` / `var` must be defined by a prior op's `out` or
  by the function's `params`, with limited exceptions (`dict_set`, `index` at
  position 0).

---

## Operation Kind Catalog

The `kind` field is a lowercase string. The backend maps many aliases to the
same TIR opcode (see `kind_to_opcode` in `tir/ssa.rs`). Below is the
canonical set grouped by category.

### Constants

| kind               | Fields used                        | Description                          |
|--------------------|------------------------------------|--------------------------------------|
| `const`            | `value`, `out`                     | Integer constant (47-bit signed inline range) |
| `const_bigint`     | `s_value`, `out`                   | Arbitrary-precision integer as decimal string |
| `const_bool`       | `value` (0 or 1), `out`           | Boolean constant                     |
| `const_float`      | `f_value`, `out`                   | Float constant (non-finite as string) |
| `const_str`        | `s_value` or `bytes`, `out`        | String constant. Uses `bytes` for surrogate content |
| `const_bytes`      | `bytes`, `out`                     | Bytes literal                        |
| `const_none`       | `out`                              | `None` singleton                     |
| `const_ellipsis`   | `out`                              | `Ellipsis` singleton                 |
| `const_not_implemented` | `out`                         | `NotImplemented` singleton           |

**Example:**
```json
{"kind": "const", "value": 42, "out": "v0"}
{"kind": "const_str", "s_value": "hello", "out": "v1"}
{"kind": "const_float", "f_value": 3.14, "out": "v2"}
{"kind": "const_none", "out": "v3"}
```

### Arithmetic and Unary

| kind          | Fields used               | Description              |
|---------------|---------------------------|--------------------------|
| `add`         | `args` [lhs, rhs], `out`  | Addition                 |
| `sub`         | `args` [lhs, rhs], `out`  | Subtraction              |
| `mul`         | `args` [lhs, rhs], `out`  | Multiplication           |
| `div`         | `args` [lhs, rhs], `out`  | True division            |
| `floor_div`   | `args` [lhs, rhs], `out`  | Floor division           |
| `mod`         | `args` [lhs, rhs], `out`  | Modulo                   |
| `pow`         | `args` [lhs, rhs], `out`  | Exponentiation           |
| `neg`         | `args` [operand], `out`   | Unary negation           |
| `pos`         | `args` [operand], `out`   | Unary positive           |
| `matmul`      | `args` [lhs, rhs], `out`  | Matrix multiplication    |
| `pow_mod`     | `args`, `out`             | Three-arg pow(b, e, m)   |
| `round`       | `args`, `out`             | round()                  |
| `trunc`       | `args`, `out`             | math.trunc()             |
| `abs`         | `args` [operand], `out`   | Absolute value           |
| `invert`      | `args` [operand], `out`   | Bitwise invert (~x)      |

All binary arithmetic ops accept optional `fast_int` / `fast_float` hints.

Inplace variants: `inplace_add`, `inplace_sub`, `inplace_mul`,
`inplace_bit_or`, `inplace_bit_and`, `inplace_bit_xor`. Same schema as their
non-inplace counterparts.

**Example:**
```json
{"kind": "add", "args": ["v0", "v1"], "out": "v2", "fast_int": true}
```

### Comparison

| kind        | Fields used               | Description              |
|-------------|---------------------------|--------------------------|
| `eq`        | `args` [lhs, rhs], `out`  | `==`                     |
| `ne`        | `args` [lhs, rhs], `out`  | `!=`                     |
| `lt`        | `args` [lhs, rhs], `out`  | `<`                      |
| `le`        | `args` [lhs, rhs], `out`  | `<=`                     |
| `gt`        | `args` [lhs, rhs], `out`  | `>`                      |
| `ge`        | `args` [lhs, rhs], `out`  | `>=`                     |
| `is`        | `args` [lhs, rhs], `out`  | Identity check           |
| `is_not`    | `args` [lhs, rhs], `out`  | Negated identity         |
| `in`        | `args`, `out`             | Membership               |
| `not_in`    | `args`, `out`             | Negated membership       |
| `contains`  | `args`, `out`, optional `container_type` | `__contains__` with optional type hint |
| `string_eq` | `args` [lhs, rhs], `out`  | String-specialized `==`  |

Comparisons accept optional `fast_int` / `fast_float`.

### Logical and Bitwise

| kind        | Fields used               | Description              |
|-------------|---------------------------|--------------------------|
| `and`       | `args` [lhs, rhs], `out`  | Short-circuit and        |
| `or`        | `args` [lhs, rhs], `out`  | Short-circuit or         |
| `not`       | `args` [operand], `out`   | Boolean negation         |
| `bit_and`   | `args` [lhs, rhs], `out`  | `&`                      |
| `bit_or`    | `args` [lhs, rhs], `out`  | `\|`                     |
| `bit_xor`   | `args` [lhs, rhs], `out`  | `^`                      |
| `bit_not`   | `args` [operand], `out`   | `~` (alias of `invert`)  |
| `lshift`    | `args` [lhs, rhs], `out`  | `<<`                     |
| `rshift`    | `args` [lhs, rhs], `out`  | `>>`                     |

### Control Flow

| kind                  | Fields used           | Description                              |
|-----------------------|-----------------------|------------------------------------------|
| `if`                  | `args` [cond]         | Conditional branch start                 |
| `else`                | --                    | Else branch                              |
| `end_if`              | --                    | End of conditional                       |
| `label`               | `value` (label id)    | Branch target label                      |
| `state_label`         | `value` (label id)    | State machine label (generators)         |
| `jump`                | `value` (label id)    | Unconditional jump                       |
| `check_exception`     | `value` (label id)    | Branch to label if exception pending     |
| `phi`                 | `args`, `out`         | SSA phi node                             |
| `loop_start`          | --                    | Loop header                              |
| `loop_end`            | --                    | Loop footer                              |
| `loop_break`          | --                    | Break out of current loop                |
| `loop_continue`       | --                    | Continue to loop header                  |
| `loop_break_if_true`  | `args` [cond]         | Conditional break                        |
| `loop_break_if_false` | `args` [cond]         | Conditional break (inverted)             |
| `loop_index_start`    | `args`, `out`         | Initialize loop counter                  |
| `loop_index_next`     | `args`, `out`         | Increment loop counter                   |
| `ret_void`            | --                    | Return void (module-level)               |
| `return`              | `args` [value]        | Return value from function               |

**Example:**
```json
{"kind": "label", "value": 3}
{"kind": "if", "args": ["v5"]}
{"kind": "jump", "value": 3}
{"kind": "check_exception", "value": 7}
```

### Calls

| kind              | Fields used                              | Description                         |
|-------------------|------------------------------------------|-------------------------------------|
| `call`            | `s_value` (target), `args`, `value` (code_id), `out` | Direct function call     |
| `call_internal`   | `s_value` (target), `args`, `value` (code_id), `out` | Internal (non-exported) call |
| `call_indirect`   | `args` [callable, ...positional], `out`  | Indirect call via function pointer  |
| `call_func`       | `args` [callable, ...positional], `out`  | Generic function call               |
| `call_guarded`    | `s_value` (target), `args`, `out`        | Guarded call (deopt on type change) |
| `call_bind`       | `args`, `out`                            | Partial application / bind          |
| `call_method`     | `args`, `out`                            | Method call                         |
| `invoke_ffi`      | `args`, `out`, optional `s_value` (lane) | Foreign function invocation         |
| `print`           | `args`                                   | Built-in print                      |
| `print_newline`   | --                                       | Print bare newline                  |

**Example:**
```json
{"kind": "call", "s_value": "molt_init___main__", "args": ["v0"], "value": 1, "out": "v3"}
{"kind": "call_indirect", "args": ["v5", "v6", "v7"], "out": "v8"}
```

### Exception Handling

| kind                        | Fields used           | Description                          |
|-----------------------------|-----------------------|--------------------------------------|
| `try_start`                 | --                    | Enter try block                      |
| `try_end`                   | --                    | Exit try block                       |
| `raise`                     | `args` [exc], `out`   | Raise exception                      |
| `exception_push`            | `out`                 | Push exception handler               |
| `exception_pop`             | `out`                 | Pop exception handler                |
| `exception_new`             | `args` [kind, msg], `out` | Create exception from kind+message |
| `exception_new_from_class`  | `args` [cls, msg], `out` | Create exception from class+message |
| `exception_last`            | `out`                 | Get current exception                |
| `exception_clear`           | `out`                 | Clear current exception              |
| `exception_set_last`        | `args` [exc], `out`   | Set current exception                |
| `exception_set_cause`       | `args` [exc, cause], `out` | Set `__cause__` (raise from)    |
| `exception_context_set`     | `args` [exc], `out`   | Set `__context__`                    |
| `exception_kind`            | `args` [exc], `out`   | Get exception type tag               |
| `exception_class`           | `args` [exc], `out`   | Get exception class                  |
| `exception_message`         | `args` [exc], `out`   | Get exception message                |
| `exception_stack_enter`     | `out`                 | Enter exception stack frame          |
| `exception_stack_exit`      | `args`, `out`         | Exit exception stack frame           |
| `exception_stack_clear`     | `out`                 | Clear exception stack                |
| `exception_stack_depth`     | `out`                 | Get exception stack depth            |
| `exception_stack_set_depth` | `args`, `out`         | Restore exception stack depth        |
| `exceptiongroup_match`      | `args` [eg, filter], `out` | Match ExceptionGroup            |
| `exceptiongroup_combine`    | `args` [eg], `out`    | Combine ExceptionGroup               |

**Example:**
```json
{"kind": "try_start"}
{"kind": "raise", "args": ["v10"], "out": "v11"}
{"kind": "check_exception", "value": 5}
{"kind": "try_end"}
```

### Attribute Access

| kind                     | Fields used                                    | Description                        |
|--------------------------|------------------------------------------------|------------------------------------|
| `load`                   | `args` [obj], `value` (offset), `out`          | Load field at known offset         |
| `store`                  | `args` [obj, val], `value` (offset)            | Store field at known offset        |
| `store_init`             | `args` [obj, val], `value` (offset)            | Store field (init, no old decref)  |
| `get_attr_generic_ptr`   | `args` [obj], `s_value` (attr), `out`, `metadata.ic_index` | Generic attr get (ptr-based) |
| `get_attr_generic_obj`   | `args` [obj], `s_value` (attr), `out`          | Generic attr get (obj-based)       |
| `get_attr_name`          | `args`, `out`                                  | Get attr by runtime name value     |
| `get_attr_name_default`  | `args`, `out`                                  | Get attr with default              |
| `get_attr_special_obj`   | `args` [obj], `s_value` (attr), `out`          | Get special/dunder attr            |
| `set_attr_generic_ptr`   | `args` [obj, val], `s_value` (attr), `out`     | Generic attr set (ptr-based)       |
| `set_attr_generic_obj`   | `args` [obj, val], `s_value` (attr), `out`     | Generic attr set (obj-based)       |
| `set_attr_name`          | `args`, `out`                                  | Set attr by runtime name value     |
| `del_attr_generic_ptr`   | `args` [obj], `s_value` (attr), `out`          | Delete attr (ptr-based)            |
| `del_attr_generic_obj`   | `args` [obj], `s_value` (attr), `out`          | Delete attr (obj-based)            |
| `del_attr_name`          | `args`, `out`                                  | Delete attr by runtime name value  |
| `has_attr_name`          | `args`, `out`                                  | hasattr() by runtime name          |
| `guarded_field_get`      | `args` [obj, cls, ver], `s_value`, `value` (offset), `out` | Guarded field load  |
| `guarded_field_set`      | `args` [obj, cls, ver, val], `s_value`, `value` (offset), `out` | Guarded field store |
| `guarded_field_set_init` | `args` [obj, cls, ver, val], `s_value`, `value` (offset), `out` | Guarded field init store |

### Indexing

| kind           | Fields used          | Description              |
|----------------|----------------------|--------------------------|
| `index`        | `args`, `out`        | `obj[key]`               |
| `store_index`  | `args`, `out`        | `obj[key] = val`         |
| `del_index`    | `args`, `out`        | `del obj[key]`           |
| `slice`        | `args`, `out`        | Slice operation          |
| `slice_new`    | `args`, `out`        | Create slice object      |

### Collection Construction

| kind              | Fields used   | Description              |
|-------------------|---------------|--------------------------|
| `list_new`        | `args`, `out` | Create list              |
| `tuple_new`       | `args`, `out` | Create tuple             |
| `dict_new`        | `args`, `out` | Create dict              |
| `set_new`         | `args`, `out` | Create set               |
| `frozenset_new`   | `args`, `out` | Create frozenset         |
| `range_new`       | `args`, `out` | Create range object      |
| `list_from_range` | `args`, `out` | list(range(...))         |
| `tuple_from_list` | `args`, `out` | tuple(list)              |
| `dict_from_obj`   | `args`, `out` | dict(iterable)           |

### Collection Methods

List: `list_append`, `list_pop`, `list_extend`, `list_insert`, `list_remove`,
`list_clear`, `list_copy`, `list_reverse`, `list_count`, `list_index`,
`list_index_range`.

Dict: `dict_get`, `dict_set`, `dict_pop`, `dict_setdefault`,
`dict_setdefault_empty_list`, `dict_update`, `dict_update_missing`,
`dict_update_kwstar`, `dict_clear`, `dict_copy`, `dict_popitem`, `dict_keys`,
`dict_values`, `dict_items`, `dict_inc`, `dict_str_int_inc`.

Set: `set_add`, `set_discard`, `set_remove`, `set_pop`, `set_update`,
`set_intersection_update`, `set_difference_update`, `set_symdiff_update`,
`frozenset_add`.

Tuple: `tuple_count`, `tuple_index`.

All use the standard `args` + `out` pattern.

### String Operations

`string_find`, `string_find_slice`, `string_format`, `string_startswith`,
`string_startswith_slice`, `string_endswith`, `string_endswith_slice`,
`string_count`, `string_count_slice`, `string_join`, `string_split`,
`string_split_max`, `string_lower`, `string_upper`, `string_capitalize`,
`string_strip`, `string_lstrip`, `string_rstrip`, `string_replace`,
`string_split_ws_dict_inc`, `string_split_sep_dict_inc`.

Bytes/bytearray: `bytes_find`, `bytes_find_slice`, `bytearray_find`,
`bytearray_find_slice`, `bytes_startswith`, `bytes_startswith_slice`,
`bytearray_startswith`, `bytearray_startswith_slice`, `bytes_endswith`,
`bytes_endswith_slice`, `bytearray_endswith`, `bytearray_endswith_slice`,
`bytes_count`, `bytearray_count`, `bytes_count_slice`,
`bytearray_count_slice`, `bytes_split`, `bytes_split_max`.

All use the standard `args` + `out` pattern.

### Type and Conversion

| kind                  | Fields used                   | Description              |
|-----------------------|-------------------------------|--------------------------|
| `isinstance`          | `args` [obj, cls], `out`      | isinstance()             |
| `issubclass`          | `args` [cls, base], `out`     | issubclass()             |
| `type_of`             | `args` [obj], `out`           | type(obj)                |
| `builtin_type`        | `args`, `out`                 | Get built-in type object |
| `str_from_obj`        | `args`, `out`                 | str(obj)                 |
| `repr_from_obj`       | `args`, `out`                 | repr(obj)                |
| `ascii_from_obj`      | `args`, `out`                 | ascii(obj)               |
| `int_from_obj`        | `args`, `out`                 | int(obj)                 |
| `float_from_obj`      | `args`, `out`                 | float(obj)               |
| `complex_from_obj`    | `args`, `out`                 | complex(obj)             |
| `bytes_from_obj`      | `args`, `out`                 | bytes(obj)               |
| `bytes_from_str`      | `args`, `out`                 | str.encode()             |
| `bytearray_from_obj`  | `args`, `out`                 | bytearray(obj)           |
| `bytearray_from_str`  | `args`, `out`                 | bytearray from str       |
| `intarray_from_seq`   | `args`, `out`                 | Internal int array       |
| `len`                 | `args`, `out`                 | len(obj)                 |
| `id`                  | `args`, `out`                 | id(obj)                  |
| `ord`                 | `args`, `out`                 | ord(ch)                  |
| `chr`                 | `args`, `out`                 | chr(n)                   |
| `hash`                | `args`, `out`                 | hash(obj)                |
| `bool_from_obj`       | `args`, `out`                 | bool(obj)                |

### Box/Unbox and Memory

| kind              | Fields used              | Description                          |
|-------------------|--------------------------|--------------------------------------|
| `box`             | `args` [raw], `out`      | Box a raw value into an object       |
| `box_from_raw_int`| `args` [raw], `out`      | Box a raw i64 into a Python int      |
| `unbox`           | `args` [obj], `out`      | Unbox object to raw value            |
| `unbox_to_raw_int`| `args` [obj], `out`      | Unbox Python int to raw i64          |
| `alloc`           | `value` (size), `out`    | Allocate object of known size        |
| `alloc_class`     | `args` [cls], `value` (size), `out` | Allocate instance of class |
| `alloc_class_trusted` | `args`, `value`, `out` | Trusted alloc (no guard)           |
| `alloc_class_static` | `args`, `value`, `out`  | Static alloc                       |
| `inc_ref`         | `args`, `out`            | Increment reference count            |
| `dec_ref`         | `args`, `out`            | Decrement reference count            |
| `borrow`          | `args`, `out`            | Perceus borrow                       |
| `release`         | `args`, `out`            | Perceus release                      |
| `free`            | `args`                   | Free allocation                      |
| `stack_alloc`     | --                       | Stack allocation marker              |

### Class and Object

| kind                      | Fields used                    | Description                    |
|---------------------------|--------------------------------|--------------------------------|
| `class_new`               | `args`, `out`                  | Create new class object        |
| `class_def`               | `args`, `s_value`, `out`       | Define class with body         |
| `class_set_base`          | `args`, `out`                  | Set class base                 |
| `class_apply_set_name`    | `args`, `out`                  | Apply `__set_name__`           |
| `class_layout_version`    | `args`, `out`                  | Get class layout version       |
| `class_set_layout_version`| `args`, `out`                  | Set class layout version       |
| `guard_layout`            | `args`, `out`                  | Guard on class layout          |
| `object_new`              | `out`                          | Create bare object             |
| `object_set_class`        | `args`, `out`                  | Set object's class ref         |
| `super_new`               | `args`, `out`                  | Create super() proxy           |
| `dataclass_new`           | `args`, `out`                  | Create dataclass instance      |
| `dataclass_get`           | `args`, `out`                  | Dataclass field get            |
| `dataclass_set`           | `args`, `out`                  | Dataclass field set            |
| `dataclass_set_class`     | `args`, `out`                  | Set dataclass's class          |
| `missing`                 | `out`                          | Sentinel missing value         |

### Function and Code Objects

| kind                      | Fields used                              | Description                    |
|---------------------------|------------------------------------------|--------------------------------|
| `func_new`                | `s_value` (name), `value` (arity), `out` | Create function object         |
| `func_new_closure`        | `s_value`, `value`, `args` [closure], `out` | Create closure              |
| `builtin_func`            | `s_value` (name), `value` (arity), `out` | Reference to built-in function |
| `code_new`                | `args`, `out`                            | Create code object             |
| `code_slot_set`           | `value` (code_id), `args` [code_obj]     | Register code in slot table    |
| `code_slots_init`         | `value` (count)                          | Initialize code slot table     |
| `fn_ptr_code_set`         | `s_value` (func), `args` [code]          | Associate code with fn ptr     |
| `classmethod_new`         | `args`, `out`                            | Wrap as classmethod            |
| `staticmethod_new`        | `args`, `out`                            | Wrap as staticmethod           |
| `property_new`            | `args`, `out`                            | Create property descriptor     |
| `bound_method_new`        | `args`, `out`                            | Create bound method            |
| `function_closure_bits`   | `args`, `out`                            | Closure capture bitmap         |

### Module

| kind                | Fields used          | Description              |
|---------------------|----------------------|--------------------------|
| `module_new`        | `args`, `out`        | Create module object     |
| `module_import`     | `args`, `out`        | Import module            |
| `module_cache_get`  | `args`, `out`        | Get from module cache    |
| `module_cache_set`  | `args`, `out`        | Set in module cache      |
| `module_cache_del`  | `args`, `out`        | Delete from module cache |
| `module_get_attr`   | `args`, `out`        | Get module attribute     |
| `module_get_global` | `args`, `out`        | Get module global        |
| `module_set_attr`   | `args`, `out`        | Set module attribute     |
| `module_del_global` | `args`, `out`        | Delete module global     |
| `module_import_star`| `args`, `out`        | `from module import *`   |
| `import`            | --                   | Import statement         |
| `import_from`       | --                   | `from X import Y`        |

### Iterator and Unpacking

| kind               | Fields used          | Description              |
|--------------------|----------------------|--------------------------|
| `iter`             | `args`, `out`        | Get iterator             |
| `iter_next`        | `args`, `out`        | Get next from iterator   |
| `enumerate`        | `args`, `out`        | enumerate()              |
| `aiter`            | `args`, `out`        | Async iterator           |
| `anext`            | `args`, `out`        | Async next               |
| `unpack_sequence`  | `args`, `value` (expected_count) | Unpack sequence into N values |

### Context Manager

| kind                | Fields used                   | Description              |
|---------------------|-------------------------------|--------------------------|
| `context_null`      | `args`, `out`                 | Null context manager     |
| `context_enter`     | `args` [ctx], `out`           | `__enter__`              |
| `context_exit`      | `args` [ctx, exc], `out`      | `__exit__`               |
| `context_unwind`    | `args` [ctx], `out`           | Unwind context           |
| `context_depth`     | `out`                         | Get context depth        |
| `context_unwind_to` | `args` [depth, exc], `out`    | Unwind to depth          |
| `context_closing`   | `args`, `out`                 | contextlib.closing        |

### Async

| kind               | Fields used          | Description              |
|--------------------|----------------------|--------------------------|
| `yield`            | --                   | Generator yield          |
| `yield_from`       | --                   | Generator yield from     |
| `is_native_awaitable` | `args`, `out`     | Check if native awaitable |
| `gen_locals_register` | `s_value`, `args`  | Register generator locals |
| `asyncgen_locals_register` | `s_value`, `args` | Register async gen locals |

### Guard and Deopt

| kind             | Fields used   | Description                     |
|------------------|---------------|---------------------------------|
| `guard_type`     | `args`        | Deopt if type mismatch          |
| `guard_tag`      | `args`        | Deopt if NaN-box tag mismatch   |
| `guard_dict_shape` | `args`, `out` | Deopt if dict shape changed   |
| `type_guard`     | --            | Generic type guard              |

### Call Arguments Protocol

| kind                    | Fields used   | Description              |
|-------------------------|---------------|--------------------------|
| `callargs_new`          | `out`         | Create CallArgs builder  |
| `callargs_push_pos`     | `args`, `out` | Push positional arg      |
| `callargs_push_kw`      | `args`, `out` | Push keyword arg         |
| `callargs_expand_star`  | `args`, `out` | Expand *args             |
| `callargs_expand_kwstar`| `args`, `out` | Expand **kwargs          |

### Vectorized Intrinsics

Fused loop intrinsics for common patterns:

`vec_sum_int`, `vec_sum_int_trusted`, `vec_sum_int_range`,
`vec_sum_int_range_trusted`, `vec_sum_int_range_iter`,
`vec_sum_int_range_iter_trusted`, `vec_sum_float`, `vec_sum_float_trusted`,
`vec_sum_float_range`, `vec_sum_float_range_trusted`,
`vec_sum_float_range_iter`, `vec_sum_float_range_iter_trusted`,
`vec_prod_int`, `vec_prod_int_trusted`, `vec_prod_int_range`,
`vec_prod_int_range_trusted`, `vec_min_int`, `vec_min_int_trusted`,
`vec_min_int_range`, `vec_min_int_range_trusted`, `vec_max_int`,
`vec_max_int_trusted`, `vec_max_int_range`, `vec_max_int_range_trusted`.

All use the standard `args` + `out` pattern.

### Miscellaneous

| kind                | Fields used           | Description                      |
|---------------------|-----------------------|----------------------------------|
| `nop`               | --                    | No operation                     |
| `line`              | `value` (line number) | Source line mapping               |
| `trace_enter_slot`  | `value` (slot)        | Enter trace slot                 |
| `trace_exit`        | --                    | Exit trace                       |
| `frame_locals_set`  | `args`                | Set frame locals dict            |
| `copy`              | --                    | SSA copy / variable alias        |
| `identity_alias`    | `args`, `out`         | Identity alias (no-op copy)      |
| `cast`              | `args`, `out`         | Type cast                        |
| `widen`             | `args`, `out`         | Widen representation             |
| `json_parse`        | `args`, `out`         | Parse JSON string                |
| `msgpack_parse`     | `args`, `out`         | Parse msgpack bytes              |
| `cbor_parse`        | `args`, `out`         | Parse CBOR bytes                 |
| `file_open`         | `args`, `out`         | Open file                        |
| `file_read`         | `args`, `out`         | Read file                        |
| `file_write`        | `args`, `out`         | Write file                       |
| `file_close`        | `args`, `out`         | Close file                       |
| `file_flush`        | `args`, `out`         | Flush file                       |
| `env_get`           | `args`, `out`         | Get environment variable         |
| `memoryview_new`    | `args`, `out`         | Create memoryview                |
| `memoryview_tobytes`| `args`, `out`         | memoryview.tobytes()             |
| `buffer2d_new`      | `args`, `out`         | Create 2D buffer                 |
| `buffer2d_get`      | `args`, `out`         | Get from 2D buffer               |
| `buffer2d_set`      | `args`, `out`         | Set in 2D buffer                 |
| `buffer2d_matmul`   | `args`, `out`         | 2D buffer matrix multiply        |
| `taq_ingest_line`   | `args`, `out`         | TAQ data ingestion               |
| `state_block_start` | --                    | State machine block start        |
| `state_block_end`   | --                    | State machine block end          |

---

## Backend Kind Aliases

The Rust backend TIR lowering (`tir/ssa.rs`) maps multiple kind strings to the
same internal opcode. These aliases are accepted in the JSON:

| Aliases                                                                  | Canonical TIR OpCode |
|--------------------------------------------------------------------------|----------------------|
| `const`, `const_int`, `load_const`                                       | ConstInt             |
| `copy`, `store_var`, `load_var`                                          | Copy                 |
| `call`, `call_func`, `call_internal`, `call_indirect`, `call_bind`, `call_function`, `call_guarded`, `invoke_ffi` | Call |
| `call_builtin`, `builtin_print`, `print`                                | CallBuiltin          |
| `get_attr`, `get_attr_generic_ptr`, `get_attr_generic_obj`, `get_attr_name`, `guarded_field_get`, `load`, `load_attr` | LoadAttr |
| `set_attr`, `store_attr`, `set_attr_name`, `set_attr_generic_ptr`, `set_attr_generic_obj`, `guarded_field_set`, `guarded_field_set_init`, `store`, `store_init` | StoreAttr |
| `del_attr`, `del_attr_generic_ptr`, `del_attr_generic_obj`              | DelAttr              |
| `store_index`, `index_set`                                              | StoreIndex           |
| `box`, `box_from_raw_int`                                               | BoxVal               |
| `unbox`, `unbox_to_raw_int`                                             | UnboxVal             |
| `floor_div` (lowercase)                                                  | FloorDiv             |

**Known TIR alias gaps:** The frontend emits `lshift` and `rshift` for shift
operations. The TIR `kind_to_opcode` function maps only `shl` and `shr` to
`OpCode::Shl` / `OpCode::Shr`. Because `lshift` / `rshift` are not in the
mapping, they fall through to `Copy` in the TIR pipeline. This does not affect
correctness for backends that dispatch on the original `op.kind` string (WASM,
Luau, Rust, native), but means TIR optimization passes treat shifts as no-ops.
Similarly, `del_attr_name`, `has_attr_name`, `get_attr_name_default`,
`get_attr_special_obj`, `set_attr_name`, and `del_attr_name` are not mapped
and fall through to `Copy` in TIR. The same applies to many collection method
ops, string ops, and type/conversion ops -- they are handled directly by the
backends and not by the TIR pipeline.

Unknown kind strings fall through to `Copy` (no-op).

---

## Full Example

```json
{
  "functions": [
    {
      "name": "molt_main",
      "params": [],
      "ops": [
        {"kind": "call", "s_value": "molt_init___main__", "args": [], "value": 0, "out": "v0"},
        {"kind": "ret_void"}
      ]
    },
    {
      "name": "molt_init___main__",
      "params": [],
      "ops": [
        {"kind": "const", "value": 10, "out": "v0"},
        {"kind": "const", "value": 20, "out": "v1"},
        {"kind": "add", "args": ["v0", "v1"], "out": "v2", "fast_int": true},
        {"kind": "const_str", "s_value": "result", "out": "v3"},
        {"kind": "print", "args": ["v3", "v2"]},
        {"kind": "try_start"},
        {"kind": "const", "value": 0, "out": "v4"},
        {"kind": "div", "args": ["v2", "v4"], "out": "v5"},
        {"kind": "check_exception", "value": 1},
        {"kind": "jump", "value": 2},
        {"kind": "label", "value": 1},
        {"kind": "exception_last", "out": "v6"},
        {"kind": "exception_clear", "out": "v7"},
        {"kind": "label", "value": 2},
        {"kind": "try_end"},
        {"kind": "ret_void"}
      ]
    }
  ]
}
```
