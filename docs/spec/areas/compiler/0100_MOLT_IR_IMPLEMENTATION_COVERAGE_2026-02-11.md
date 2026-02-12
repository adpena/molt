# 0100 Molt IR Implementation Coverage (2026-02-11)

Status: Audit snapshot
Owner: compiler/frontend
Scope: `docs/spec/areas/compiler/0100_MOLT_IR.md` instruction list vs repository implementation.

## Snapshot Summary
- Historical baseline (before 2026-02-11 closure work): 109 implemented, 13 partial, 12 missing.
- Current snapshot in this document: 109 implemented, 25 partial, 0 missing.
- Gate alignment:
  - `tools/check_molt_ir_ops.py` is green (`missing=0`) for spec-op inventory,
    frontend lowering branches, required native/wasm dedicated lanes, and
    behavior-level semantic assertions (including dedicated call-site labels for
    `invoke_ffi_bridge`/`invoke_ffi_deopt` vs `call_func` and `call_indirect`
    vs `call_bind`), required differential probe presence for dedicated-lane
    coverage, and CI-enforced probe execution/failure-queue linkage in
    `--require-probe-execution` mode.
  - Remaining work is semantic hardening (dedicated backend lanes + broader
    differential behavior evidence), not inventory presence.
  - Dedicated-lane differential probes now exist in
    `tests/differential/basic/call_indirect_dynamic_callable.py`,
    `tests/differential/basic/call_indirect_noncallable_deopt.py`,
    `tests/differential/basic/invoke_ffi_os_getcwd.py`,
    `tests/differential/basic/invoke_ffi_bridge_capability_enabled.py`,
    `tests/differential/basic/invoke_ffi_bridge_capability_denied.py`,
    `tests/differential/basic/guard_tag_type_hint_fail.py`, and
    `tests/differential/basic/guard_dict_shape_mutation.py`.

## Method
- Source scanned: `/Users/adpena/PycharmProjects/molt/src/molt/frontend/__init__.py`.
- Evidence types:
  - direct emitted op literals (`kind="..."`),
  - lowering op matches (`op.kind == "..."`).
- Status labels:
  - `implemented`: direct op mapping found,
  - `partial`: represented via alias/composite form or lowering-only evidence,
  - `missing`: no frontend emit/lowering match found.

| Category | Spec Op | Status | Repo Mapping | Evidence | Notes |
| --- | --- | --- | --- | --- | --- |
| Constants | ConstInt | partial | `CONST` | `/Users/adpena/PycharmProjects/molt/src/molt/frontend/__init__.py:1119` | Lowered as generic `CONST` (plus `CONST_BIGINT` path) rather than dedicated `CONST_INT`. |
| Constants | ConstFloat | implemented | `CONST_FLOAT` | `/Users/adpena/PycharmProjects/molt/src/molt/frontend/__init__.py:1639` |  |
| Constants | ConstBool | implemented | `CONST_BOOL` | `/Users/adpena/PycharmProjects/molt/src/molt/frontend/__init__.py:1078` |  |
| Constants | ConstNone | implemented | `CONST_NONE` | `/Users/adpena/PycharmProjects/molt/src/molt/frontend/__init__.py:1062` |  |
| Constants | ConstStr | implemented | `CONST_STR` | `/Users/adpena/PycharmProjects/molt/src/molt/frontend/__init__.py:947` |  |
| Constants | ConstBytes | implemented | `CONST_BYTES` | `/Users/adpena/PycharmProjects/molt/src/molt/frontend/__init__.py:4883` |  |
| Arithmetic/Logic | Add | implemented | `ADD` | `/Users/adpena/PycharmProjects/molt/src/molt/frontend/__init__.py:7590` |  |
| Arithmetic/Logic | Sub | implemented | `SUB` | `/Users/adpena/PycharmProjects/molt/src/molt/frontend/__init__.py:19049` |  |
| Arithmetic/Logic | Mul | implemented | `MUL` | `/Users/adpena/PycharmProjects/molt/src/molt/frontend/__init__.py:20199` |  |
| Arithmetic/Logic | Div | implemented | `DIV` | `/Users/adpena/PycharmProjects/molt/src/molt/frontend/__init__.py:25959` |  |
| Arithmetic/Logic | Eq | implemented | `EQ` | `/Users/adpena/PycharmProjects/molt/src/molt/frontend/__init__.py:3642` |  |
| Arithmetic/Logic | Lt | implemented | `LT` | `/Users/adpena/PycharmProjects/molt/src/molt/frontend/__init__.py:7565` |  |
| Arithmetic/Logic | Gt | implemented | `GT` | `/Users/adpena/PycharmProjects/molt/src/molt/frontend/__init__.py:9539` |  |
| Arithmetic/Logic | Is | implemented | `IS` | `/Users/adpena/PycharmProjects/molt/src/molt/frontend/__init__.py:3443` |  |
| Arithmetic/Logic | Contains | implemented | `CONTAINS` | `/Users/adpena/PycharmProjects/molt/src/molt/frontend/__init__.py:9519` |  |
| Arithmetic/Logic | And | implemented | `AND` | `/Users/adpena/PycharmProjects/molt/src/molt/frontend/__init__.py:8698` |  |
| Arithmetic/Logic | Or | implemented | `OR` | `/Users/adpena/PycharmProjects/molt/src/molt/frontend/__init__.py:21990` |  |
| Arithmetic/Logic | Not | implemented | `NOT` | `/Users/adpena/PycharmProjects/molt/src/molt/frontend/__init__.py:988` |  |
| Control | Phi | implemented | `PHI` | `/Users/adpena/PycharmProjects/molt/src/molt/frontend/__init__.py:3459` |  |
| Control | Jump | implemented | `JUMP` | `/Users/adpena/PycharmProjects/molt/src/molt/frontend/__init__.py:2845` |  |
| Control | Branch | partial | `IF`, `ELSE`, `END_IF` | `/Users/adpena/PycharmProjects/molt/src/molt/frontend/__init__.py:989`; `/Users/adpena/PycharmProjects/molt/src/molt/frontend/__init__.py:3456`; `/Users/adpena/PycharmProjects/molt/src/molt/frontend/__init__.py:1028` | Represented as structured IF/ELSE/END_IF sequence, not a single `BRANCH` op. |
| Control | Return | partial | `ret` | `/Users/adpena/PycharmProjects/molt/src/molt/frontend/__init__.py:2830` | Represented as lowercase `ret` op, not uppercase `RETURN`. |
| Control | Throw | partial | `RAISE` | `/Users/adpena/PycharmProjects/molt/src/molt/frontend/__init__.py:3703` | Represented as `RAISE`; no distinct `THROW` op name. |
| Control | TryStart | implemented | `TRY_START` | `/Users/adpena/PycharmProjects/molt/src/molt/frontend/__init__.py:21140` |  |
| Control | TryEnd | implemented | `TRY_END` | `/Users/adpena/PycharmProjects/molt/src/molt/frontend/__init__.py:21152` |  |
| Control | CheckException | implemented | `CHECK_EXCEPTION` | `/Users/adpena/PycharmProjects/molt/src/molt/frontend/__init__.py:1417` |  |
| Control | LoopStart | implemented | `LOOP_START` | `/Users/adpena/PycharmProjects/molt/src/molt/frontend/__init__.py:7427` |  |
| Control | LoopIndexStart | implemented | `LOOP_INDEX_START` | `/Users/adpena/PycharmProjects/molt/src/molt/frontend/__init__.py:7612` |  |
| Control | LoopIndexNext | implemented | `LOOP_INDEX_NEXT` | `/Users/adpena/PycharmProjects/molt/src/molt/frontend/__init__.py:7630` |  |
| Control | LoopBreakIfTrue | implemented | `LOOP_BREAK_IF_TRUE` | `/Users/adpena/PycharmProjects/molt/src/molt/frontend/__init__.py:7445` |  |
| Control | LoopBreakIfFalse | implemented | `LOOP_BREAK_IF_FALSE` | `/Users/adpena/PycharmProjects/molt/src/molt/frontend/__init__.py:7568` |  |
| Control | LoopBreak | implemented | `LOOP_BREAK` | `/Users/adpena/PycharmProjects/molt/src/molt/frontend/__init__.py:23304` |  |
| Control | LoopContinue | implemented | `LOOP_CONTINUE` | `/Users/adpena/PycharmProjects/molt/src/molt/frontend/__init__.py:7454` |  |
| Control | LoopEnd | implemented | `LOOP_END` | `/Users/adpena/PycharmProjects/molt/src/molt/frontend/__init__.py:7455` |  |
| Calls | Call | implemented | `CALL` | `/Users/adpena/PycharmProjects/molt/src/molt/frontend/__init__.py:4626` |  |
| Calls | CallIndirect | partial | `CALL_INDIRECT` -> `call_indirect` | `/Users/adpena/PycharmProjects/molt/src/molt/frontend/__init__.py` (`_emit_dynamic_call` + `map_ops_to_json`) | First-class dedicated lane is emitted/lowered; native/wasm route through dedicated runtime lanes (`molt_call_indirect_ic` / `call_indirect_ic`) with callable precheck, CI-enforced probe execution linkage, and runtime-feedback deopt counter `deopt_reasons.call_indirect_noncallable`. Broader deopt taxonomy remains. |
| Calls | InvokeFFI | partial | `INVOKE_FFI` -> `invoke_ffi` (`s_value="bridge"` lane marker when bridge policy is used) | `/Users/adpena/PycharmProjects/molt/src/molt/frontend/__init__.py` (`_emit_dynamic_call` + `map_ops_to_json`) | First-class dedicated lane is emitted/lowered; runtime bridge-capability ABI gate (`molt_invoke_ffi_ic`) plus positive/negative capability probes are landed with CI-enforced probe execution linkage and runtime-feedback deopt counter `deopt_reasons.invoke_ffi_bridge_capability_denied`. Broader deopt taxonomy remains. |
| Object/Layout | Alloc | implemented | `ALLOC` | `/Users/adpena/PycharmProjects/molt/src/molt/frontend/__init__.py:26843` |  |
| Object/Layout | LoadAttr | partial | `GETATTR` | `/Users/adpena/PycharmProjects/molt/src/molt/frontend/__init__.py:8538` | Mapped to GETATTR family rather than `LOADATTR` literal. |
| Object/Layout | StoreAttr | partial | `SETATTR` | `/Users/adpena/PycharmProjects/molt/src/molt/frontend/__init__.py:26897` | Mapped to SETATTR family rather than `STOREATTR` literal. |
| Object/Layout | GetAttrGenericPtr | implemented | `GETATTR_GENERIC_PTR` | `/Users/adpena/PycharmProjects/molt/src/molt/frontend/__init__.py:8508` |  |
| Object/Layout | SetAttrGenericPtr | implemented | `SETATTR_GENERIC_PTR` | `/Users/adpena/PycharmProjects/molt/src/molt/frontend/__init__.py:8380` |  |
| Object/Layout | GetAttrGenericObj | implemented | `GETATTR_GENERIC_OBJ` | `/Users/adpena/PycharmProjects/molt/src/molt/frontend/__init__.py:4692` |  |
| Object/Layout | SetAttrGenericObj | implemented | `SETATTR_GENERIC_OBJ` | `/Users/adpena/PycharmProjects/molt/src/molt/frontend/__init__.py:1091` |  |
| Object/Layout | LoadIndex | partial | `INDEX` | `/Users/adpena/PycharmProjects/molt/src/molt/frontend/__init__.py:2860` | Indexed load uses `INDEX` op; no distinct `LOAD_INDEX` literal. |
| Object/Layout | StoreIndex | implemented | `STORE_INDEX` | `/Users/adpena/PycharmProjects/molt/src/molt/frontend/__init__.py:2839` |  |
| Object/Layout | Index | implemented | `INDEX` | `/Users/adpena/PycharmProjects/molt/src/molt/frontend/__init__.py:2860` |  |
| Object/Layout | Iter | partial | `ITER_NEW` | `/Users/adpena/PycharmProjects/molt/src/molt/frontend/__init__.py:8332` | Iterator creation uses `ITER_NEW`; no literal `ITER` op. |
| Object/Layout | Enumerate | implemented | `ENUMERATE` | `/Users/adpena/PycharmProjects/molt/src/molt/frontend/__init__.py:17247` |  |
| Object/Layout | IterNext | implemented | `ITER_NEXT` | `/Users/adpena/PycharmProjects/molt/src/molt/frontend/__init__.py:8349` |  |
| Object/Layout | ListNew | implemented | `LIST_NEW` | `/Users/adpena/PycharmProjects/molt/src/molt/frontend/__init__.py:1057` |  |
| Object/Layout | DictNew | implemented | `DICT_NEW` | `/Users/adpena/PycharmProjects/molt/src/molt/frontend/__init__.py:3340` |  |
| Object/Layout | Len | implemented | `LEN` | `/Users/adpena/PycharmProjects/molt/src/molt/frontend/__init__.py:7517` |  |
| Object/Layout | Slice | implemented | `SLICE` | `/Users/adpena/PycharmProjects/molt/src/molt/frontend/__init__.py:18388` |  |
| Object/Layout | SliceNew | implemented | `SLICE_NEW` | `/Users/adpena/PycharmProjects/molt/src/molt/frontend/__init__.py:17273` |  |
| Object/Layout | BytearrayFromObj | implemented | `BYTEARRAY_FROM_OBJ` | `/Users/adpena/PycharmProjects/molt/src/molt/frontend/__init__.py:18147` |  |
| Object/Layout | IntArrayFromSeq | implemented | `INTARRAY_FROM_SEQ` | `/Users/adpena/PycharmProjects/molt/src/molt/frontend/__init__.py:8326` |  |
| Object/Layout | MemoryViewNew | implemented | `MEMORYVIEW_NEW` | `/Users/adpena/PycharmProjects/molt/src/molt/frontend/__init__.py:18197` |  |
| Object/Layout | MemoryViewToBytes | implemented | `MEMORYVIEW_TOBYTES` | `/Users/adpena/PycharmProjects/molt/src/molt/frontend/__init__.py:14274` |  |
| Object/Layout | RangeNew | implemented | `RANGE_NEW` | `/Users/adpena/PycharmProjects/molt/src/molt/frontend/__init__.py:7706` |  |
| Object/Layout | Buffer2DNew | implemented | `BUFFER2D_NEW` | `/Users/adpena/PycharmProjects/molt/src/molt/frontend/__init__.py:13425` |  |
| Object/Layout | Buffer2DGet | implemented | `BUFFER2D_GET` | `/Users/adpena/PycharmProjects/molt/src/molt/frontend/__init__.py:13436` |  |
| Object/Layout | Buffer2DSet | implemented | `BUFFER2D_SET` | `/Users/adpena/PycharmProjects/molt/src/molt/frontend/__init__.py:13449` |  |
| Object/Layout | Buffer2DMatmul | implemented | `BUFFER2D_MATMUL` | `/Users/adpena/PycharmProjects/molt/src/molt/frontend/__init__.py:13462` |  |
| Object/Layout | ClosureLoad | partial | `LOAD_CLOSURE` | `/Users/adpena/PycharmProjects/molt/src/molt/frontend/__init__.py:2804` | Alias form (`LOAD_CLOSURE`) rather than `CLOSURE_LOAD`. |
| Object/Layout | ClosureStore | partial | `STORE_CLOSURE` | `/Users/adpena/PycharmProjects/molt/src/molt/frontend/__init__.py:2791` | Alias form (`STORE_CLOSURE`) rather than `CLOSURE_STORE`. |
| Bytes/Bytearray/String | BytesFind | implemented | `BYTES_FIND` | `/Users/adpena/PycharmProjects/molt/src/molt/frontend/__init__.py:14874` |  |
| Bytes/Bytearray/String | BytesSplit | implemented | `BYTES_SPLIT` | `/Users/adpena/PycharmProjects/molt/src/molt/frontend/__init__.py:14718` |  |
| Bytes/Bytearray/String | BytesReplace | implemented | `BYTES_REPLACE` | `/Users/adpena/PycharmProjects/molt/src/molt/frontend/__init__.py:14850` |  |
| Bytes/Bytearray/String | BytearrayFind | implemented | `BYTEARRAY_FIND` | `/Users/adpena/PycharmProjects/molt/src/molt/frontend/__init__.py:14906` |  |
| Bytes/Bytearray/String | BytearraySplit | implemented | `BYTEARRAY_SPLIT` | `/Users/adpena/PycharmProjects/molt/src/molt/frontend/__init__.py:14734` |  |
| Bytes/Bytearray/String | BytearrayReplace | implemented | `BYTEARRAY_REPLACE` | `/Users/adpena/PycharmProjects/molt/src/molt/frontend/__init__.py:14859` |  |
| Bytes/Bytearray/String | StringFind | implemented | `STRING_FIND` | `/Users/adpena/PycharmProjects/molt/src/molt/frontend/__init__.py:14940` |  |
| Bytes/Bytearray/String | StringFormat | implemented | `STRING_FORMAT` | `/Users/adpena/PycharmProjects/molt/src/molt/frontend/__init__.py:9304` |  |
| Bytes/Bytearray/String | StringSplit | implemented | `STRING_SPLIT` | `/Users/adpena/PycharmProjects/molt/src/molt/frontend/__init__.py:14702` |  |
| Bytes/Bytearray/String | StringCapitalize | implemented | `STRING_CAPITALIZE` | `/Users/adpena/PycharmProjects/molt/src/molt/frontend/__init__.py:14760` |  |
| Bytes/Bytearray/String | StringStrip | implemented | `STRING_STRIP` | `/Users/adpena/PycharmProjects/molt/src/molt/frontend/__init__.py:14774` |  |
| Bytes/Bytearray/String | StringReplace | implemented | `STRING_REPLACE` | `/Users/adpena/PycharmProjects/molt/src/molt/frontend/__init__.py:14841` |  |
| Bytes/Bytearray/String | StringStartswith | implemented | `STRING_STARTSWITH` | `/Users/adpena/PycharmProjects/molt/src/molt/frontend/__init__.py:14463` |  |
| Bytes/Bytearray/String | StringEndswith | implemented | `STRING_ENDSWITH` | `/Users/adpena/PycharmProjects/molt/src/molt/frontend/__init__.py:14570` |  |
| Bytes/Bytearray/String | StringCount | implemented | `STRING_COUNT` | `/Users/adpena/PycharmProjects/molt/src/molt/frontend/__init__.py:14325` |  |
| Bytes/Bytearray/String | StringJoin | implemented | `STRING_JOIN` | `/Users/adpena/PycharmProjects/molt/src/molt/frontend/__init__.py:9299` |  |
| Exceptions | ExceptionNew | implemented | `EXCEPTION_NEW` | `/Users/adpena/PycharmProjects/molt/src/molt/frontend/__init__.py:5031` |  |
| Exceptions | ExceptionLast | implemented | `EXCEPTION_LAST` | `/Users/adpena/PycharmProjects/molt/src/molt/frontend/__init__.py:3863` |  |
| Exceptions | ExceptionClear | implemented | `EXCEPTION_CLEAR` | `/Users/adpena/PycharmProjects/molt/src/molt/frontend/__init__.py:3898` |  |
| Exceptions | ExceptionKind | implemented | `EXCEPTION_KIND` | `/Users/adpena/PycharmProjects/molt/src/molt/frontend/__init__.py:3889` |  |
| Exceptions | ExceptionMessage | partial | `EXCEPTION_MESSAGE` | `/Users/adpena/PycharmProjects/molt/src/molt/frontend/__init__.py:26747` | Lowering case exists; direct emitter usage is not obvious in current frontend paths. |
| Exceptions | ExceptionSetCause | implemented | `EXCEPTION_SET_CAUSE` | `/Users/adpena/PycharmProjects/molt/src/molt/frontend/__init__.py:23191` |  |
| Exceptions | ExceptionContextSet | implemented | `EXCEPTION_CONTEXT_SET` | `/Users/adpena/PycharmProjects/molt/src/molt/frontend/__init__.py:21181` |  |
| Exceptions | Raise | implemented | `RAISE` | `/Users/adpena/PycharmProjects/molt/src/molt/frontend/__init__.py:3703` |  |
| Generators/Async | AllocGenerator | partial | `ASYNCGEN_NEW` | `/Users/adpena/PycharmProjects/molt/src/molt/frontend/__init__.py:9978` | Generator allocation is represented via asyncgen/generator op family, not literal `ALLOC_GENERATOR`. |
| Generators/Async | GenSend | implemented | `GEN_SEND` | `/Users/adpena/PycharmProjects/molt/src/molt/frontend/__init__.py:13489` |  |
| Generators/Async | GenThrow | implemented | `GEN_THROW` | `/Users/adpena/PycharmProjects/molt/src/molt/frontend/__init__.py:25748` |  |
| Generators/Async | GenClose | implemented | `GEN_CLOSE` | `/Users/adpena/PycharmProjects/molt/src/molt/frontend/__init__.py:13559` |  |
| Generators/Async | IsGenerator | implemented | `IS_GENERATOR` | `/Users/adpena/PycharmProjects/molt/src/molt/frontend/__init__.py:25607` |  |
| Generators/Async | AIter | implemented | `AITER` | `/Users/adpena/PycharmProjects/molt/src/molt/frontend/__init__.py:8868` |  |
| Generators/Async | ANext | implemented | `ANEXT` | `/Users/adpena/PycharmProjects/molt/src/molt/frontend/__init__.py:17312` |  |
| Generators/Async | AllocFuture | partial | `PROMISE_NEW` | `/Users/adpena/PycharmProjects/molt/src/molt/frontend/__init__.py:15824` | Future allocation represented as promise op family, not literal `ALLOC_FUTURE`. |
| Generators/Async | CallAsync | implemented | `CALL_ASYNC` | `/Users/adpena/PycharmProjects/molt/src/molt/frontend/__init__.py:1652` |  |
| Generators/Async | StateSwitch | implemented | `STATE_SWITCH` | `/Users/adpena/PycharmProjects/molt/src/molt/frontend/__init__.py:9914` |  |
| Generators/Async | StateTransition | implemented | `STATE_TRANSITION` | `/Users/adpena/PycharmProjects/molt/src/molt/frontend/__init__.py:25366` |  |
| Generators/Async | StateYield | implemented | `STATE_YIELD` | `/Users/adpena/PycharmProjects/molt/src/molt/frontend/__init__.py:25552` |  |
| Generators/Async | ChanNew | implemented | `CHAN_NEW` | `/Users/adpena/PycharmProjects/molt/src/molt/frontend/__init__.py:15987` |  |
| Generators/Async | ChanSendYield | implemented | `CHAN_SEND_YIELD` | `/Users/adpena/PycharmProjects/molt/src/molt/frontend/__init__.py:16057` |  |
| Generators/Async | ChanRecvYield | implemented | `CHAN_RECV_YIELD` | `/Users/adpena/PycharmProjects/molt/src/molt/frontend/__init__.py:16132` |  |
| Vector | VecSumInt | implemented | `VEC_SUM_INT` | `/Users/adpena/PycharmProjects/molt/src/molt/frontend/__init__.py:27943` |  |
| Vector | VecProdInt | implemented | `VEC_PROD_INT` | `/Users/adpena/PycharmProjects/molt/src/molt/frontend/__init__.py:28039` |  |
| Vector | VecMinInt | implemented | `VEC_MIN_INT` | `/Users/adpena/PycharmProjects/molt/src/molt/frontend/__init__.py:28071` |  |
| Vector | VecMaxInt | implemented | `VEC_MAX_INT` | `/Users/adpena/PycharmProjects/molt/src/molt/frontend/__init__.py:28103` |  |
| Vector | VecSumIntTrusted | implemented | `VEC_SUM_INT_TRUSTED` | `/Users/adpena/PycharmProjects/molt/src/molt/frontend/__init__.py:27951` |  |
| Vector | VecProdIntTrusted | implemented | `VEC_PROD_INT_TRUSTED` | `/Users/adpena/PycharmProjects/molt/src/molt/frontend/__init__.py:28047` |  |
| Vector | VecMinIntTrusted | implemented | `VEC_MIN_INT_TRUSTED` | `/Users/adpena/PycharmProjects/molt/src/molt/frontend/__init__.py:28079` |  |
| Vector | VecMaxIntTrusted | implemented | `VEC_MAX_INT_TRUSTED` | `/Users/adpena/PycharmProjects/molt/src/molt/frontend/__init__.py:28111` |  |
| Vector | VecSumIntRange | implemented | `VEC_SUM_INT_RANGE` | `/Users/adpena/PycharmProjects/molt/src/molt/frontend/__init__.py:27959` |  |
| Vector | VecProdIntRange | implemented | `VEC_PROD_INT_RANGE` | `/Users/adpena/PycharmProjects/molt/src/molt/frontend/__init__.py:28055` |  |
| Vector | VecMinIntRange | implemented | `VEC_MIN_INT_RANGE` | `/Users/adpena/PycharmProjects/molt/src/molt/frontend/__init__.py:28087` |  |
| Vector | VecMaxIntRange | implemented | `VEC_MAX_INT_RANGE` | `/Users/adpena/PycharmProjects/molt/src/molt/frontend/__init__.py:28119` |  |
| Vector | VecSumIntRangeTrusted | implemented | `VEC_SUM_INT_RANGE_TRUSTED` | `/Users/adpena/PycharmProjects/molt/src/molt/frontend/__init__.py:27967` |  |
| Vector | VecProdIntRangeTrusted | implemented | `VEC_PROD_INT_RANGE_TRUSTED` | `/Users/adpena/PycharmProjects/molt/src/molt/frontend/__init__.py:28063` |  |
| Vector | VecMinIntRangeTrusted | implemented | `VEC_MIN_INT_RANGE_TRUSTED` | `/Users/adpena/PycharmProjects/molt/src/molt/frontend/__init__.py:28095` |  |
| Vector | VecMaxIntRangeTrusted | implemented | `VEC_MAX_INT_RANGE_TRUSTED` | `/Users/adpena/PycharmProjects/molt/src/molt/frontend/__init__.py:28127` |  |
| Guards | GuardType | implemented | `GUARD_TYPE` | `/Users/adpena/PycharmProjects/molt/src/molt/frontend/__init__.py:3792` |  |
| Guards | GuardTag | partial | `GUARD_TAG` -> `guard_tag` | `/Users/adpena/PycharmProjects/molt/src/molt/frontend/__init__.py` (`_emit_guard_type` + `map_ops_to_json`) | First-class dedicated lane is emitted/lowered; runtime-feedback deopt counter `deopt_reasons.guard_tag_type_mismatch` is landed. Broader specialization/deopt differential hardening remains. |
| Guards | GuardLayout | implemented | `GUARD_LAYOUT` | `/Users/adpena/PycharmProjects/molt/src/molt/frontend/__init__.py:8614` |  |
| Guards | GuardDictShape | partial | `GUARD_DICT_SHAPE` -> `guard_dict_shape` | `/Users/adpena/PycharmProjects/molt/src/molt/frontend/__init__.py` (`_emit_guard_dict_shape` + `map_ops_to_json`) | First-class dedicated lane is emitted/lowered; runtime-feedback aggregate deopt counter `deopt_reasons.guard_dict_shape_layout_mismatch` plus per-reason breakdown counters (`*_null_obj`, `*_non_object`, `*_class_mismatch`, `*_non_type_class`, `*_expected_version_invalid`, `*_version_mismatch`) are landed. Broader dict-shape invalidation/deopt differential hardening remains. |
| RC (LIR) | IncRef | partial | `INC_REF` -> `inc_ref` | `/Users/adpena/PycharmProjects/molt/src/molt/frontend/__init__.py` (`map_ops_to_json`) | Explicit ownership lane is emitted/lowered; invariant checks and broader parity evidence are pending. |
| RC (LIR) | DecRef | partial | `DEC_REF` -> `dec_ref` | `/Users/adpena/PycharmProjects/molt/src/molt/frontend/__init__.py` (`map_ops_to_json`) | Explicit ownership lane is emitted/lowered; invariant checks and broader parity evidence are pending. |
| RC (LIR) | Borrow | partial | `BORROW` -> `borrow` | `/Users/adpena/PycharmProjects/molt/src/molt/frontend/__init__.py` (`map_ops_to_json`) | Explicit ownership lane is emitted/lowered; borrow/release lifetime validation remains pending. |
| RC (LIR) | Release | partial | `RELEASE` -> `release` | `/Users/adpena/PycharmProjects/molt/src/molt/frontend/__init__.py` (`map_ops_to_json`) | Explicit ownership lane is emitted/lowered; borrow/release lifetime validation remains pending. |
| Conversions | Box | partial | `BOX` -> `box` | `/Users/adpena/PycharmProjects/molt/src/molt/frontend/__init__.py` (`map_ops_to_json`) | Explicit conversion lane is emitted/lowered; deterministic conversion semantics are still being hardened. |
| Conversions | Unbox | partial | `UNBOX` -> `unbox` | `/Users/adpena/PycharmProjects/molt/src/molt/frontend/__init__.py` (`map_ops_to_json`) | Explicit conversion lane is emitted/lowered; deterministic conversion semantics are still being hardened. |
| Conversions | Cast | partial | `CAST` -> `cast` | `/Users/adpena/PycharmProjects/molt/src/molt/frontend/__init__.py` (`map_ops_to_json`) | Explicit conversion lane is emitted/lowered; deterministic conversion semantics are still being hardened. |
| Conversions | Widen | partial | `WIDEN` -> `widen` | `/Users/adpena/PycharmProjects/molt/src/molt/frontend/__init__.py` (`map_ops_to_json`) | Explicit conversion lane is emitted/lowered; deterministic conversion semantics are still being hardened. |
| Conversions | StrFromObj | implemented | `STR_FROM_OBJ` | `/Users/adpena/PycharmProjects/molt/src/molt/frontend/__init__.py:9274` |  |
