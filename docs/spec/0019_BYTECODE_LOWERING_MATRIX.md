# Bytecode/Opcode Lowering Matrix
**Spec ID:** 0019
**Status:** Draft
**Owner:** compiler
**Goal:** Track coverage of CPython 3.13+ opcodes mapped to Molt IR.

## 1. Principles
- **Semantics, not VM:** Molt is an AST-based compiler, but we use CPython opcodes as the canonical checklist for language semantic coverage. "Supporting `BINARY_ADD`" means Molt lowers AST `BinOp(Add)` to equivalent IR with correct semantics.
- **Tiers:**
    - **T0 (Static):** Lowered to direct LIR/native code (e.g., `i64.add`).
    - **T1 (Guarded):** Lowered to guarded code with deopt info.
    - **T2 (Dynamic):** Lowered to a runtime call (`molt_runtime::binary_op`) or fallback interpreter.
- **Deoptimization:** Any T1 opcode must have a defined deopt mapping back to a Python frame state.

## 2. Matrix (CPython 3.13)

### 2.1 General & Stack
| Opcode | Semantics | Status | Tier | Notes |
| --- | --- | --- | --- | --- |
| `NOP` | No-op | Supported | T0 | Eliminated in IR. |
| `POP_TOP` | Discard TOS | Supported | T0 | Side-effect check only. |
| `COPY` | Duplicate TOS | Supported | T0 | SSA value use. |
| `SWAP` | Swap TOS | Supported | T0 | SSA variable remapping. |
| `LOAD_CONST` | Load literal | Supported | T0 | `Const` in IR. |

### 2.2 Names & Variables
| Opcode | Semantics | Status | Tier | Notes |
| --- | --- | --- | --- | --- |
| `LOAD_FAST` | Local var load | Supported | T0 | SSA use. |
| `STORE_FAST` | Local var store | Supported | T0 | SSA def. |
| `DELETE_FAST` | Local var del | Supported | T1 | Guard "is defined". |
| `LOAD_GLOBAL` | Global load | Partial | T1 | Guarded global lookup. |
| `STORE_GLOBAL` | Global store | Partial | T2 | Runtime call (side effect). |
| `LOAD_NAME` | REPL/script load | Partial | T2 | Dictionary lookup. |

### 2.3 Math & Logic
| Opcode | Semantics | Status | Tier | Notes |
| --- | --- | --- | --- | --- |
| `BINARY_OP` | +, -, *, etc. | Supported | T0/T1 | `Add/Sub` IR + overflow guards. |
| `UNARY_NEGATIVE` | -x | Supported | T0/T1 | `Neg` IR. |
| `UNARY_NOT` | not x | Supported | T0 | `Not` IR (bool). |
| `UNARY_INVERT` | ~x | Supported | T0/T1 | `BitNot` IR. |
| `COMPARE_OP` | ==, !=, <, etc. | Supported | T0/T1 | `Cmp` IR. |
| `CONTAINS_OP` | in / not in | Partial | T1 | `Call(contains)` or specialized. |
| `IS_OP` | is / is not | Supported | T0 | `PtrEq` IR. |

### 2.4 Attributes & Items
| Opcode | Semantics | Status | Tier | Notes |
| --- | --- | --- | --- | --- |
| `LOAD_ATTR` | obj.attr | Supported | T0/T1 | Struct load or `Call(getattr)`. |
| `STORE_ATTR` | obj.attr = v | Supported | T0/T1 | Struct store or `Call(setattr)`. |
| `DELETE_ATTR` | del obj.attr | Partial | T2 | `Call(delattr)`. |
| `BINARY_SUBSCR` | obj[key] | Supported | T0/T1 | `VecGet`/`DictGet` or runtime. |
| `STORE_SUBSCR` | obj[key] = v | Supported | T0/T1 | `VecSet`/`DictSet` or runtime. |
| `DELETE_SUBSCR` | del obj[key] | Partial | T2 | `Call(delitem)`. |

### 2.5 Control Flow
| Opcode | Semantics | Status | Tier | Notes |
| --- | --- | --- | --- | --- |
| `JUMP_FORWARD` | Unconditional jump | Supported | T0 | CFG Edge. |
| `POP_JUMP_IF_TRUE` | Branch | Supported | T0 | `If(cond)`. |
| `POP_JUMP_IF_FALSE` | Branch | Supported | T0 | `If(!cond)`. |
| `FOR_ITER` | Iterator next | Supported | T0/T1 | `Loop` + `Call(next)`. |
| `GET_ITER` | Get iter | Supported | T0/T1 | `Call(iter)`. |
| `RETURN_VALUE` | Return | Supported | T0 | `Return(val)`. |
| `YIELD_VALUE` | Generator yield | Supported | T2 | State machine lowering; async generator coverage still pending (TODO(opcode-matrix, owner:frontend, milestone:TC2, priority:P2, status:partial): async generator coverage). |

### 2.6 Calls & Functions
| Opcode | Semantics | Status | Tier | Notes |
| --- | --- | --- | --- | --- |
| `CALL` | Function call | Supported | T0/T1 | `CallKnown` or `CallDyn`. |
| `KW_NAMES` | Keyword args | Partial | T1 | Argument reordering. |
| `MAKE_FUNCTION` | Create func | Supported | T0 | Closure creation IR. |

### 2.7 Data Structures
| Opcode | Semantics | Status | Tier | Notes |
| --- | --- | --- | --- | --- |
| `BUILD_LIST` | [a, b] | Supported | T0 | `AllocVec`. |
| `BUILD_TUPLE` | (a, b) | Supported | T0 | `AllocTuple`. |
| `BUILD_SET` | {a, b} | Partial | T1 | `AllocSet`. |
| `BUILD_MAP` | {k: v} | Supported | T0 | `AllocDict`. |
| `BUILD_CONST_KEY_MAP`| {k: v} const | Supported | T0 | `AllocDict` optimized. |
| `BUILD_STRING` | f-string/concat | Partial | T1 | `StrConcat`. |
| `LIST_EXTEND` | list.extend | Supported | T1 | Loop lowering. |
| `SET_UPDATE` | set.update | Partial | T1 | Loop lowering. |
| `DICT_UPDATE` | dict.update | Supported | T1 | Loop lowering. |
| `DICT_MERGE` | dict(**kw) | Supported | T1 | Loop lowering. |

### 2.8 Exceptions & Context
| Opcode | Semantics | Status | Tier | Notes |
| --- | --- | --- | --- | --- |
| `RAISE_VARARGS` | raise | Supported | T0 | `Raise(exc)`. |
| `RERAISE` | raise (re) | Supported | T0 | Exception stack handling. |
| `PUSH_EXC_INFO` | except ... | Supported | T1 | Exception stack handling. |
| `POP_EXCEPT` | Exit except | Supported | T1 | Cleanup. |
| `SETUP_WITH` | with ... | Supported | T2 | `__enter__`/`__exit__` calls (contextlib.contextmanager still unsupported; TODO(stdlib-compat, owner:stdlib, milestone:SL2, priority:P2, status:missing): contextmanager lowering). |
| `WITH_EXCEPT_START` | with err | Supported | T2 | `__exit__` with exc (contextlib.contextmanager still unsupported; TODO(stdlib-compat, owner:stdlib, milestone:SL2, priority:P2, status:missing): contextmanager lowering). |

### 2.9 Async
| Opcode | Semantics | Status | Tier | Notes |
| --- | --- | --- | --- | --- |
| `GET_AITER` | async for | Partial | T2 | `__aiter__` (awaitable `__aiter__` pending; TODO(opcode-matrix, owner:frontend, milestone:TC2, priority:P2, status:missing): awaitable `__aiter__` support). |
| `GET_ANEXT` | async next | Partial | T2 | `__anext__` and `anext` lowering. |
| `END_ASYNC_FOR` | end loop | Partial | T2 | StopAsyncIteration handling. |
| `SEND` | await/gen send | Partial | T2 | Coroutine lowering; async generators pending (TODO(opcode-matrix, owner:frontend, milestone:TC2, priority:P2, status:partial): async generator op coverage). |

### 2.10 Pattern Matching (3.10+)
| Opcode | Semantics | Status | Tier | Notes |
| --- | --- | --- | --- | --- |
| `MATCH_CLASS` | case C() | Missing | - | Structural matching. |
| `MATCH_MAPPING` | case {} | Missing | - | Mapping check. |
| `MATCH_SEQUENCE` | case [] | Missing | - | Sequence check. |
| `MATCH_KEYS` | case {k:v} | Missing | - | Key subset check. |

## 3. Optimization Intrinsics (Superinstructions)
Molt may fuse ops.
- `LOAD_ATTR_METHOD_WITH_VALUES`: Specialized method call.
- `LOAD_GLOBAL_MODULE`: Global from module dict (immutable).

## 4. TODOs
- TODO(opcode-matrix, owner:frontend, milestone:M3, priority:P2, status:missing): Complete `MATCH_*` lowering (Milestone 3).
- TODO(opcode-matrix, owner:frontend, milestone:M2, priority:P3, status:planned): Optimize `SETUP_WITH` to inline `__enter__` (Milestone 2).
- TODO(opcode-matrix, owner:frontend, milestone:TC2, priority:P2, status:partial): Add async generator op coverage (e.g., `ASYNC_GEN_WRAP`) and confirm lowering gaps.
