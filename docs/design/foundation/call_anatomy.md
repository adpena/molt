# Call anatomy: why compiled Python runs like an interpreter, and the architecture that fixes it

Status: accepted design, 2026-10-29, for ledger family J (runtime execution context), with I
(frames and exceptions), A (runtime-call ABI) and P (mid-end facts). Ledger rows: HF-185 (frame
cost, staged here), HF-561 (raise and catch) and the exception `__init__` descriptor row. Stage 1
is the next implementation step. Section 9 holds the programs and the method to reproduce every
number.

## 0. Summary

**Finding.** Molt's native code is a call-threaded interpreter. A compiled function is a sequence
of calls into the Rust runtime. Each call re-derives the execution context from thread-locals and
does one Python-level bookkeeping step: push a frame, store a local into a frame home, set the
line, set the column, look up a global by its string name, admit a call, charge the recursion
counter, poll for async work, change a reference count. CPython 3.12 does the same steps inside one
function with the thread state in a local variable, frames on a bump-allocated data stack, and
caches inside the bytecode. So Molt pays more per Python-level step than the interpreter it should
replace.

**Headline numbers** (macOS arm64, release profile, origin/main `06d37eafd2`, CPython 3.12.15 on
the same host; instructions retired per unit with startup subtracted):

| unit | Molt | CPython | Molt / CPython instructions | wall speed vs CPython |
|---|---:|---:|---:|---:|
| Python call, `total = add(total, i)` | 3,623 | 866 | 4.2x | 0.24x |
| `fib(30)`, per call | 4,346 | 520 | 8.4x | 0.15x |
| `for i in range: total += i`, per iteration (inline int) | 361 | 521 | 0.7x | 1.25x |
| same loop when `total` exceeds 47 bits (heap int) | 2,377 | 521 | 4.6x | 0.20x |
| `p.x = i; total += p.x`, per iteration | 1,736 | 614 | 2.8x | 0.38x |
| `total = c.inc(total)` method call | 4,697 | 832 | 5.6x | 0.18x |
| `Point(0, 0)` + 2 stores (perf-gate `struct`) | 27,001 | 2,474 | 10.9x | 0.08x |
| `struct.pack("<ii", i, i + 1)` | 44,669 | 1,627 | 27.5x | 0.026x |
| `raise ValueError("x")` caught | 210,274 | 2,038 | 103x | 0.006x |

The one loop where Molt wins (inline-int sum) still costs 361 instructions for one add. A
compiled loop needs about 4.

**Top three structural causes.**

1. **Frames are eager and runtime-owned.** Every compiled function pushes a `FrameEntry` onto a
   thread-local `RefCell<Vec>`, retains code, globals and builtins, takes an invocation namespace,
   borrows a homes array, sets an argument-zero context, mirrors each local into its home, and
   writes the line and column through runtime calls. In the callee `add` that is 11 runtime calls
   per invocation. Frame push and pop (`molt_trace_enter_slot` 17.7%, `molt_trace_exit` 17.9%,
   `molt_frame_context_set` 4.1%, `molt_frame_homes` 2.5%) plus locations (`molt_frame_set_col`,
   `molt_trace_set_line`, `molt_frame_set_line_col`, 8.1%) take 51% of the call loop. In `fib` the
   same group takes 48%. CPython pushes a `_PyInterpreterFrame` with a pointer bump and about ten
   stores (`Include/internal/pycore_frame.h:_PyFrame_PushUnchecked`) and creates a frame object only
   when code observes it (`Python/frame.c:_PyFrame_MakeAndSetFrameObject`).
2. **Every name is bound late, through the runtime, by its spelling.** A call to a module function
   reads the global by a string-keyed dictionary lookup on each call (`molt_module_get_global`:
   11.7% of the call loop, 10.6% of `fib`). A method call hashes the method name's bytes on each
   call (`molt_load_method`, 24% of the method loop). An attribute get or set is a runtime call that
   recomputes the object's shape id before it reads its own cache (72% of the attribute loop). A
   constructor resolves `__init__` through the MRO and allocates a bound method on each
   construction (50% of the `struct` loop). `ValueError("x")` resolves `__init__` to a CPython-ABI
   slot wrapper and round-trips through the C-API bridge (62% of the raise loop). CPython
   specializes each such site to a version guard and an index
   (`Python/bytecodes.c:LOAD_GLOBAL_MODULE`, `LOAD_ATTR_INSTANCE_VALUE`, `CALL_PY_EXACT_ARGS`).
3. **Compiled code holds no execution context, so every helper re-enters the runtime.** Safepoint
   polls (`molt_async_work_poll_and_exception_pending`) are runtime calls at every loop backedge and
   call return: 44% of the inline-int loop. Reference counts are out-of-line calls, also on
   immediate ints that need no count: 23% of the same loop. Call admission
   (`molt_function_direct_call_eligible`, `molt_handle_resolve`, `molt_recursion_enter_fast`/
   `exit_fast`) is four calls per direct call. `_tlv_get_addr` is 8.7% self time in the call loop and
   9.6% in `fib`. CPython reads one `eval_breaker` word (`Python/ceval_macros.h:CHECK_EVAL_BREAKER`)
   and passes `tstate` as an argument.

**Plan in brief** (section 5). Seven migrations, each landable alone, each deleting the lane it
replaces:

| stage | migration | closes or moves | target |
|---|---|---|---|
| 1 | Frame records: native-stack frames, plain-store locations, lazy frame objects | HF-185 (frame cost 2, 3, 6), HF-261, part of HF-229 | call loop 3,623 → ≤ 1,900 instr; `fib` 4,346 → ≤ 2,300 |
| 2 | Thread-record state: eval-breaker word, inline recursion depth, inline call admission, inline refcount | HF-185 (stages 4, 5), HF-205, HF-198 (part) | inline-int loop 361 → ≤ 130; call loop → ≤ 1,000 |
| 3 | Compile-time binding: global slots, native signatures, direct calls, context argument, literal table | HF-198 (stage 2), HF-507, HF-79, HF-200 | call loop → ≤ 250; `fib` → ≤ 300 |
| 4 | Inline caches in compiled code (attributes, methods, constructors, module attributes, typed names) and typed-layout deallocation | HF-185 (attr, `struct`), HF-202, HF-206 (part) | attr 1,736 → ≤ 150; method 4,697 → ≤ 350; `struct` 27,001 → ≤ 600 |
| 5 | Exceptions: native exception classes, lazy tracebacks, class-handle matching | HF-561, HF-207, new row (C-API detour) | raise 210,274 → ≤ 1,500 |
| 6 | Unboxed loop carriers and observation-point locals | HF-185 (loop half), HF-229 | inline-int loop → ≤ 8; call loop → ≤ 80; attr → ≤ 40 |
| 7 | TIR inlining of proven small callees with inline frames in the location table | new | call loop → ≤ 12 |

A stage's target assumes the stages it depends on have landed. The end state is 10-80 instructions
for a Python call and single-digit instructions for a scalar loop iteration: 45-300x below today on
every row, and 5-70x below CPython. Stages 1, 4 and 5 touch disjoint authorities and can run in
parallel lanes; stages 2 and 3 follow stage 1; stage 7 follows stage 3.

## 1. Method and environment

- Host: macOS 26.4 (Darwin 25.4.0), Apple arm64, 18 cores, load average 11-13 during the runs
  (other lanes were building). Instructions retired and cycles come from `/usr/bin/time -l` and do
  not depend on load. Wall times are medians of 5 runs (3 for the 15 s raise run).
- Molt: a managed development image whose compiler, frontend, backend and runtime equal origin/main
  `06d37eafd2` (`git diff origin/main HEAD -- runtime src/molt/frontend src/molt/cli` is empty). Build: `bin/python -B -m molt.cli build --build-profile
  release` with `MOLT_KEEP_SYMBOLS=1`, Cranelift native backend.
  Binaries are 14.0 MB (23.2 MB with `import struct`).
- CPython: the same image's interpreter, 3.12.15 (clang 22.1.3), run as `python -B -I -S prog.py`.
- Startup subtracted: Molt empty program 192.8M instructions; CPython `-c pass` 88.7M.
- Per-unit instruction counts divide the run's instructions minus startup by the number of units.
  Small-N differential runs (1,000 vs 2,000 iterations) agree for the expensive rows (raise 215K,
  `struct` 26.7K) and are too noisy for cheap rows, so the large-N numbers are authoritative.
- Anatomy: `tools/opt_explorer.py anatomy` (static instructions, branches and call sites per
  function and loop), `objdump` with resolved callees (`disasm_sym.py`), and a sampler
  (`profile_calls.py`) that charges each `sample` call-graph node whose parent is compiled code to
  that runtime entry point (inclusive time of each runtime call that compiled code makes).
- IR: `molt debug ir --stage post-midend --format json --profile release` (SimpleIR after the
  mid-end; the native backend consumes this form).

Dynamic per-call-site counts with `lldb` were blocked: a launched process did not progress within
120 s (likely a debugger authorization prompt on this host), so runtime calls per unit come from reading the hot
path in the disassembly, confirmed by the sampler's per-callee shares.

## 2. Measured anatomy

### 2.1 One Python call: `total = add(total, i)` (p8)

Source: `def add(a, b): return a + b`, called 10M times from a `for` loop in `main()`.
3,623 instructions per iteration; CPython 866. The loop body without the call (p2) costs 361, so
**the call itself costs about 3,260 instructions in Molt and about 345 in CPython (9.4x).**

The frontend emits this SimpleIR for `add` (20 operations; `ir/p8_call.txt`):

```
trace_enter_slot; check_exception
frame_home_store a -> home[0]; check_exception
frame_home_store b -> home[1]; check_exception
const_none; const 3; frame_context_set(None, 3, None)
line 2
add v56, v57; check_exception
trace_exit; ret
(two cold labels: trace_exit; ret_void)
```

Native code for `add` is 248 instructions with 19 call sites. Its non-raising path executes 137
instructions and makes 11 runtime calls (disassembly with resolved callees). The integer add itself
is inline: tag test, 47-bit sign extension, add, range check, retag, about 15 instructions.

| # | runtime call in `add` (hot path) | what it does | why it is there | cost share of loop |
|---|---|---|---|---|
| 1 | `molt_thread_record` | returns the thread record pointer from TLS | compiled code has no context argument | 1.0% |
| 2 | `molt_trace_enter_slot(code_id)` | enters runtime; takes the invocation namespace; pushes a `FrameEntry` on `FRAME_STACK` (TLS `RefCell<Vec>`), retains code, globals, builtins; pushes `TRACE_FRAME_PUSH_STACK`; sizes and lends homes from the code slot's frame plan | eager frame push | 17.7% |
| 3 | `molt_frame_homes(2)` | enters runtime; checks pending exception; returns the top entry's homes pointer | locals live in runtime homes for introspection | 2.5% |
| 4 | `molt_frame_context_set(None, 3, None)` | enters runtime; decodes argument-zero kind; writes the frame's Python context | zero-argument `super()` support, emitted in every function | 4.1% |
| 5 | `molt_frame_set_col(4, 16)` | TLS `FRAME_STACK` borrow; writes col/end_col of the top entry | caret positions in tracebacks | 3.5% (all four `set_col`) |
| 6 | `molt_trace_set_line(2)` | enters runtime; checks two trace flags; writes the line of the top entry | traceback line | 3.3% |
| 7 | `molt_frame_set_line_col(2, 4, 16)` | writes line and columns again | same | 1.3% |
| 8 | `molt_frame_set_col(11, 16)` | column of the `+` | same | (in 5) |
| 9-10 | `molt_frame_set_col(4, 16)` x2 after the add | as above, twice with equal values | column after the add and again before `trace_exit` | (in 5) |
| 11 | `molt_trace_exit` | enters runtime; pops `TRACE_FRAME_PUSH_STACK`; pops and releases the `FrameEntry` (`FrameEntry::release`, `release_parts`, `frame_bindings_exit`) | eager frame pop | 17.9% |
| - | `frame_home_store` x2 | inline: load old home, store tag and value, `dec_ref` the old value if owned (not taken here) | mirror locals into homes | in compiled self |
| - | 4 `check_exception` | inline: load the pending byte through the record pointer, branch | after operations that the effect oracle treats as may-throw, including the home stores | in compiled self |

The caller loop is 141 static instructions in 18 blocks. Its hot path executes 119 instructions and
13 runtime calls per iteration, plus the direct call of `add` (`molt_call_func_owned` is the cold
fallback; two `molt_dec_ref_obj` sites run only when an old home value is owned):

| runtime call in the caller loop | what it does | why it is there | share |
|---|---|---|---|
| `molt_module_get_global(None, "add")` | enters runtime; reads the active globals from `FRAME_STACK`; checks a trace flag; string-keyed dict lookup in globals, then builtins | the global binding is read by name every call | 11.7% |
| `molt_function_direct_call_eligible(f, 2, shape)` | enters runtime; checks type, arity, binder flag, execution kind | call admission is a runtime query | 3.3% |
| `molt_handle_resolve(f)` | decodes the boxed handle to a pointer | to load the code pointer to compare with the static target | 0.5% |
| `molt_recursion_enter_fast` / `molt_recursion_exit_fast` | TLS depth charge and release | Python recursion limit | 2.7% |
| `molt_async_work_poll_and_exception_pending` x2 | enters runtime; `service_async_work` (signals, pending GC, retirement); reads pending exception | eval-breaker poll at the call return and at the backedge | 8.6% |
| `molt_inc_ref_obj` x3, `molt_dec_ref_obj` x3 | out-of-line; return at once for immediate ints | no type fact elides counts on immediates | 8.9% (with callee) |
| `const_str "add"` | heap literal handle for the global name | name-keyed global read | small (cached) |

Remaining shares: compiled code self time 8.3%, `_tlv_get_addr` is spread across the entries
above (8.7% self). Summed: frame push and pop 43.2%, global lookup 11.7%, polls 8.6%, counts 8.9%,
locations 8.1%, call admission 6.5%, compiled code 8.3%.

**A Python call makes 24 runtime calls (13 in the caller, 11 in the callee); 9 of them take the
full runtime entry** (`RuntimeExecutionGuard::enter`), the rest touch one or more thread-locals.
Compiled code executes 256 of the 3,623 instructions per iteration (119 caller, 137 callee); **93%
of the instructions of a Python call run inside the runtime.** CPython's `CALL_PY_EXACT_ARGS`
performs five `DEOPT_IF` tests, a frame push and argument copies, with no calls
(`Python/bytecodes.c:2761`).

### 2.2 `fib(30)` (p1)

4,346 instructions per call; CPython 520. `fib` compiles to 812 instructions with 89 call sites
(`fib` is not inside a loop, so the backend emits a `molt_frame_set_col` call before every
operation that carries a column: 30 sites). Inclusive shares (1,298 samples of `fib(33)`):
frame push/pop/context/homes 29.9%, location writes 18.2% (`molt_frame_set_col` alone 13.1%),
`molt_module_get_global` for the two recursive reads of `fib` 10.6%, `molt_string_from_bytes` for
the `"fib"` name literal 3.3% (HF-507: the literal is rebuilt per call), counts 6.3%, polls 2.7%,
admission 3.8%, `molt_is_truthy` 2.1%, compiled code 10.2%.

HF-185 recorded `fib` at 0.01x of CPython on 2026-10-14. It is 0.15x now: the landed fixes removed
the run-time name resolution and the 56 binding checks, but not the frame model.

### 2.3 Counted loops (p2, q2b, q2)

`for i in range(N): total += i` in a function: 361 instructions per iteration while `total` fits
47 bits; 2,377 when it does not. CPython 521 in both regimes (it allocates a 2-digit int each
iteration).

The loop body is 104 static instructions with 8 call sites. The SimpleIR per iteration has 20
operations: `frame_home_store` for `i` and for `total`, five `check_exception`, `inplace_add`,
`loop_index_next`. Shares on the inline regime (2,179 samples): the backedge poll 44.0%
(`service_async_work` 13.1% self, `runtime_state_for_gil_in` 7.4%), `molt_dec_ref_obj` and
`molt_inc_ref_obj` on immediate ints 23.2%, compiled code 24.3% (home stores and checks).

In the heap-int regime the add takes the runtime path: `molt_inplace_add` 30.2%, and freeing each
dead int costs `dec_ref_ptr_terminal` 33.7% because a leaf int runs the generic detach cascade
(`DetachedEdgeSink`, `visit_common_payload_owned_values`, `clear_cycle_edges_with_sink`).

### 2.4 Attribute, method, constructor, module function

- **Attribute (q3b, inline regime):** `molt_get_attr_object_ic` 42.3% and `molt_set_attr_object_ic`
  30.3% of the loop. Inside them, the cache check recomputes identity: `object_shape_id` 11.3%,
  `instance_dict_bits_ptr` 8.7%, `class_instance_shape_id` 7.4%, `object_has_class_shape` 7.1%.
  The per-site field cache from bd8585aab exists, but it lives behind a call and its key is
  recomputed by four calls. CPython: `LOAD_ATTR_INSTANCE_VALUE` compares `tp_version_tag` and reads
  `values[index]`.
- **Method (q4):** `molt_call_loaded_method1` 58.6% (frame push/pop again inside),
  `molt_load_method` 23.7%. `molt_load_method` takes the name as `(ptr, len)` bytes, so each call
  hashes and validates the name (`hash_string_bytes`, `from_utf8`) and pins an IC entry.
- **Constructor (p5, the perf-gate `struct` row):** `call_type_with_arguments` 51.6%:
  `construct_regular_class` looks up `__init__` through the MRO (`class_namespace_lookup_mro`
  12.3%), allocates a bound method per construction (`molt_bound_method_new` 10.4%), allocates
  the instance with GC admission (`gc_admit_membership` 5.2%, a registry shard mutex). Freeing
  the instance costs 31.3% (`dec_ref_ptr_terminal`, field visits through pinned tuples). The
  `__init__` body is 11.7%.
- **`struct.pack` (p6):** `struct.pack` is a compiled Python wrapper (`struct__pack`) around the
  `molt_struct_pack` intrinsic. Per call: `molt_load_method` on the module 22.2% (it walks the
  module class's MRO before the module dict: `class_namespace_lookup_mro`), runtime argument
  binding and frame push for the wrapper (`bind_python_frame` 10.4%, `frame_stack_pop` 6.7%),
  `_normalize_format` 11.0%, the intrinsic 16.2%, of which packing proper is 5.7%.

### 2.5 Raise and catch (p7, HF-561)

210,274 instructions per iteration; CPython 2,038 (103x; wall 15.46 s against 0.09 s). Shares
(12,372 samples):

- **Constructing `ValueError("x")`: 62.6%.** `construct_exception_from_args` finds `__init__` with
  `class_attr_lookup`, which returns a CPython-ABI descriptor. `bind_descriptor` calls
  `molt_cpython_abi::api::descriptor::get`, and the call goes through
  `call_foreign_with_arguments` → `slot_wrappers::method_call` → `raw_call`. The bridge publishes
  a `PyObject` view of the new exception (`publish_cold_pyobj` 20.8%), refreshes and commits its
  exception view (`refresh_exception_view` 20.7%, `commit_exception_view` 9.0%), and keeps handle
  tables in SipHash `HashMap`s behind `parking_lot` mutexes (SipHash 10%+, mutex lock and unlock 3.2% self).
  A probe (`x1`, section 9) shows the same detour at the language level: Molt
  prints `<object object at ...>` for `type(ValueError.__init__)` and for
  `type(ValueError("x").__init__)`; CPython prints `wrapper_descriptor` and `method-wrapper`. This
  is a correctness defect as well as the cost.
- **Raising: 16.7%** (`molt_raise` → `record_exception_with_caller_frame` 16.1%, which builds the
  traceback object eagerly: `exception_materialize_traceback_bits` 6.5%).
- **Freeing the exception: 11.8%** (`molt_dec_ref_obj` → terminal detach, view release).
- Handler match, context set, clear, push, pop: about 3%.

CPython's raise costs one exception allocation, one traceback entry and one frame object per
frame activation (`Python/ceval.c:924-941`, `PyTraceBack_Here(_PyFrame_GetFrameObject(frame))`),
and the handler comes from the code object's exception table (`Python/ceval.c:get_exception_handler`).

## 3. Root causes, stated as missing invariants

1. **No frame invariant.** Nothing states when a frame must exist. The backend emits a frame push,
   homes, argument-zero context and per-operation locations for every function, and the runtime
   implements them as heap-backed thread-local vectors. CPython's invariant: an interpreter frame
   is a fixed-size record on a per-thread stack; a frame object exists only after an observer asks
   for one, and `take_ownership` copies the record out when the object outlives it
   (`Python/frame.c`).
2. **No binding invariant at use sites.** The frontend proves stable module functions
   (`frontend/lowering/sema_state.py:_module_stable_funcs`) and emits `call_guarded`, but the site
   still reads the global by name and asks the runtime for admission. Attribute, method and
   constructor sites carry names as bytes and keep their caches in the runtime. No typed fact says
   "this site reads module slot k" or "this site reads field j of class C".
3. **No context invariant.** Compiled code never owns the execution context. Each helper rebuilds
   it, and per-thread state that is a single word (eval breaker, recursion depth, pending
   exception, current location) is reached through calls. The thread record from frame-cost
   stage 1 (`THREAD_RECORD_EXCEPTION_PENDING_OFFSET`, `THREAD_RECORD_PENDING_INVOCATION_OFFSET` in
   `molt-codegen-abi/src/lib.rs:165-168`) is the correct seed: it has two fields.

Two contributing causes follow from these: the effect oracle treats frame-home stores as may-throw,
which keeps `check_exception` after each one (HF-185), and immediate values carry no type fact that
elides reference counts.

## 4. Target architecture

### 4.1 Execution context

Every compiled function takes `ctx: *mut ThreadRecord` as its first native argument, as CPython
passes `tstate` (`Include/internal/pycore_pystate.h:_PyThreadState_GET` reads `_Py_tss_tstate`
once; internal functions take `tstate`) and Wasmtime passes `VMContext`. Entry trampolines (the
`*_molt_trampoline_N` symbols and the C-API entry points) read TLS once and pass `ctx`. Runtime
slow paths take `ctx` and never read TLS.

`ThreadRecord` layout is generated in `molt-codegen-abi` (one authority, consumed by every
backend):

| field | type | written by | read by |
|---|---|---|---|
| `pending_exception` | u8 (exists) | runtime raise paths | inline checks |
| `eval_breaker` | u32, atomic relaxed | signal handler, GC scheduler, async queue, stop-the-world requests | polls |
| `py_recursion_remaining` | i32 | inline call prologue | inline check, `sys.setrecursionlimit` |
| `frame_top` | `*mut FrameRecord` | inline push/pop | tracebacks, `sys._getframe`, frame objects |
| `pending_invocation` | u64 (exists) | direct-call handoff | callee entry (removed in stage 3) |
| `runtime_generation` | u64 | runtime init/shutdown | cache validity (HF-200) |
| `owner_thread_id` | u64 | thread attach | biased refcount (free threading) |

**Invariant J1.** On the non-raising path, compiled code reaches per-thread execution state only
by loads and stores at generated offsets from `ctx`. Check: a codegen census test lists the runtime
symbols each backend may import on a fast path; `molt_thread_record`,
`molt_async_work_poll_and_exception_pending`, `molt_recursion_enter_fast`/`exit_fast`,
`molt_frame_set_col`, `molt_frame_set_line_col`, `molt_trace_set_line`, `molt_trace_enter_slot`,
`molt_trace_exit`, `molt_frame_homes`, `molt_frame_context_set` and
`molt_function_direct_call_eligible` are not in any list. The type form: runtime fast-path helpers
take `&ExecCtx<'t>`, a token that only the attach transition creates, so a helper cannot be called
without a context in scope.

### 4.2 Calling convention

Each compiled function has two entries (mypyc's `CPyDef_f` native function and `CPyPy_f` wrapper;
Cython's `cdef`/`cpdef`; CPython's PEP 590 vectorcall, `Objects/call.c:_PyFunction_Vectorcall`):

- **Native entry** `f$n(ctx, a0..an) -> Value` with exact positional arity, closure environment as
  an explicit parameter, and a declared result carrier. Its signature is a typed fact,
  `NativeSignature { arity, closure, params: [Carrier], result: Carrier, may_raise, frame: FramePlan }`,
  produced once per function by the frontend/TIR and consumed by every backend.
- **Generic entry** `f$g(ctx, callable, args*, nargs, kwnames) -> Value` binds defaults, keywords,
  `*args` and `**kwargs` per the call-binding contract, then calls `f$n`. Python callers, the
  runtime, and unproven sites use it.

Each call site carries a typed binding class, decided by the frontend from binding-flow facts:

| class | when | lowering | cost |
|---|---|---|---|
| `Proven(FunctionId)` | the verified namespace premise proves the slot holds this function at the site (`docs/design/verified_namespace_premises.md`: only the owning module writes it, verified over the closure) | `call f$n(ctx, args)` | call + moves |
| `Guarded(FunctionId, GlobalSlot)` | stable by the frontend's premise, not proven over the whole closure | load slot; compare with the function object's address; equal: `call f$n`; else `call f$g` | 1 load, 1 compare, 1 branch |
| `Monomorphic(SiteId)` | unknown callee | inline compare of the callee with the site record's cached function; hit: `call` cached native entry; miss: `f$g` path that refills the site record | 2 loads, 1 compare |
| `Dynamic` | anything else | generic call | runtime |

The callee prologue does the recursion check as CPython does (`Python/ceval.c:_Py_EnterRecursivePy`:
`py_recursion_remaining-- <= 0`): one decrement and one branch in `ctx`, cold path
`molt_raise_recursion_error(ctx)`. A native stack-pointer check against a limit in `ctx` guards the
host stack on native (CPython 3.14 stack protection; HotSpot guard zones); WASM uses Wasmtime's
`max_wasm_stack` trap plus the depth count.

**Invariant J2 (call refinement).** For every site, when its guard holds, the native call and the
generic call produce the same observable result, exceptions and reference effects. Check: a test
build mode forces every guard to miss; the differential corpus must produce identical output in
both modes (the "reference meaning" rule of HF-485). `Proven` sites carry the premise's proof
identity; the premise checker refuses the build when a write to the slot appears.

### 4.3 Frame records and lazy frame objects

A function whose frame can be observed keeps a `FrameRecord` in its own native stack frame (on
WASM, in the shadow stack):

```
FrameRecord { prev: *FrameRecord, code: CodeId, function: *Function, location: u32,
              flags: u32, homes: [Value; plan.home_count] }
```

- **Push** (prologue): `rec.prev = ctx.frame_top; rec.code = K; rec.function = callee;
  ctx.frame_top = &rec` — four stores, no reference counts. The record borrows the function and
  its code: the call site keeps the callable alive until the call returns (it releases the
  callable after the return today). Globals and builtins come from the function, as
  `_PyInterpreterFrame` borrows `f_globals` and `f_builtins`. Materialization takes strong
  references.
- **Pop** (epilogue and every exit edge): `ctx.frame_top = rec.prev` — one store, plus the
  copy-out below when `flags & MATERIALIZED`.
- **Location**: before an operation that may raise or may observe frames, one store of a 32-bit
  location index into `rec.location`. The index selects (line, col, end_col) from a static
  per-code location table, decoded only when a traceback or `f_lineno` needs it (CPython
  `Objects/locations.md`; HF-261). Native may later derive the index from the return address and
  drop the store; WASM keeps the store.
- **Homes** are the record's own slots. In stage 1 every local store still writes its home, as
  today, but as a plain store that cannot raise and needs no check. Stage 6 writes homes only at
  observation points.
- **Argument-zero context** (`super()`, `__class__`) is written only by functions whose body uses
  zero-argument `super` or `__class__`; the frontend already knows this (`class_cell` in
  `frontend/lowering/function_lifecycle.py`).
- **Frame objects** materialize on observation: traceback construction, `sys._getframe`,
  `f_back`, `inspect.currentframe`, `locals()` and C-API `PyEval_GetFrame`. Materializing sets
  `MATERIALIZED` and links the object to the record. On pop, a materialized record copies its
  homes and location into the frame object, which then owns them (`Python/frame.c:take_ownership`).
- **Generators and coroutines** keep their record in the task payload, as CPython keeps
  `gi_iframe` in the generator; resume links it to `ctx.frame_top`, suspend unlinks it.
- **Module globals for name reads** come from `rec.function`'s globals (CPython reads
  `frame->f_globals`), which replaces `frame_stack_active_globals_bits()`.

Which functions need no record at all: a function that cannot raise, cannot call unproven code and
cannot suspend (a leaf over proven scalar operations) has no observer. The frontend computes
`FramePlan::None | Record { home_count, uses_class_cell }` from TIR effect facts.

**Invariant I1 (observational equivalence).** Every frame observation returns what the eager model
returns: the chain of `f_code`, `f_lineno`, `f_back`, `f_globals` and the traceback lines, and
`f_locals` at the observation point. Typed state machine for a record:
`Unobserved → Materialized → Detached` (detached means the native frame returned and the frame
object owns the copied state); no transition skips a state, and only `Materialized` may detach.
Check: the existing traceback, `sys._getframe`, warnings `stacklevel`, logging `findCaller`,
generator and `super()` differential tests on native, LLVM and WASM, plus a new property test that
raises at random depths and compares the full traceback (file, line, column carets) with CPython.

### 4.4 Exceptions

Molt keeps the error-return protocol: a failing operation sets `ctx.pending_exception` and returns
a sentinel; the caller tests it. This is CPython's C protocol, Cython's and mypyc's. The protocol
costs one load and one branch per may-raise operation and nothing per call beyond that.

**Refuse Itanium-style landing pads for Python exceptions [I].** Table-driven unwinding (Codon uses
LLVM's) makes the non-raising path one branch cheaper but makes each raise cost a two-phase native
unwind. Python raises often (`StopIteration`, `KeyError`, `AttributeError` under `hasattr`,
`getattr` defaults), and CPython's raise-and-catch is 2,038 instructions. Keep CPython's trade:
zero-cost `try` (no instruction for entering a `try` block, exception tables in CPython
`Objects/exception_handling_notes.txt`), cheap raise. In compiled code a `try` already costs
nothing until a check fails; the handler is the branch target.

Changes:

- **Checks only where an operation may raise.** TIR purity facts (`op_kinds.toml`
  `primitive_operator_effect_cases`) decide; frame-home stores and location stores are `nothrow`.
- **Lazy tracebacks.** On the error edge out of a frame, compiled code appends `(rec.code,
  rec.location)` to the in-flight exception's traceback buffer (mypyc `CPy_AddTraceback`; CPython
  `PyTraceBack_Here` at `Python/ceval.c:941`). Python traceback objects and frame objects
  materialize when code reads `__traceback__`, `tb_frame` or `f_locals`, or when the exception
  reaches the top level.
- **Native exception classes.** Builtin exception classes are native Molt classes with native
  `__new__`/`__init__` slots; construction from compiled code is
  `alloc_exception(ctx, CLASS_HANDLE, args)` with generated class handles (HF-207). Exceptions never
  pass through the CPython-ABI bridge unless C code observes them.
- **Handler match** compares the pending exception's class pointer with the handler's class handle,
  with a subclass walk on miss.

**Invariant I2.** The non-raising path of a `try` body executes no instruction that a body without
`try` would not execute. Check: anatomy of a function with and without `try` around the same body
must be equal in hot-path instructions (gate in the anatomy tool).

### 4.5 Compile-time binding and inline caches

- **Module globals.** Each module has a slot array; its `__dict__` maps names to slots (CPython's
  dict keys and `LOAD_GLOBAL_MODULE` keys-version index; mypyc's static module attributes). A
  global read at a site is `load slot[k]` plus an unbound check. Writes through `globals()`,
  `setattr(module, ...)` or `module.__dict__[...]` go through the slot. Builtins read through a
  slot whose cell the module's builtins scope owns, guarded by a builtins version.
- **Attributes.** A site with a known class (`ClassId` from the binding flow) and a fixed layout
  lowers to: load the object's class pointer; compare with the expected class; load or store field
  `j` at a generated offset. A miss calls the runtime, which updates the site record in a data
  section (V8 hidden classes; HotSpot field offsets; CPython `LOAD_ATTR_INSTANCE_VALUE`). Stores
  apply the class's attribute protocol fact (53b88e901): plain classes take the inline store, data
  descriptors take the slow path.
- **Methods.** `obj.m(args)` with a class-version guard lowers to a native call of the resolved
  function with `self` as argument zero: no bound-method object, no name hashing (CPython
  `LOAD_ATTR_METHOD_WITH_VALUES` + `CALL_PY_EXACT_ARGS`).
- **Constructors.** `C(args)` with a proven or guarded class whose `__new__` is `object.__new__`
  lowers to inline allocation of the class layout and a native call of `C.__init__$n(ctx, self,
  args)` (CPython 3.13 `CALL_ALLOC_AND_ENTER_INIT`).
- **Module attributes.** `struct.pack` with `struct` a proven module binding is a global slot read
  of that module: no class MRO walk.
- **Names.** Attribute and method names are interned name handles in the op format (typed keys,
  HF-202), never `(ptr, len)` bytes.

**Invariant J3.** A site's cache record is the only cache for that site, and its guard compares
identities that the runtime bumps on every mutation that could change the result (class version,
module slot write, builtins version). Check: mutation tests per guard kind (rebind a global, add a
data descriptor to a class, assign `__class__`) run on native, LLVM and WASM and must observe the
new value at the next execution of the site.

### 4.6 Reference counts

- Values whose TIR type is an immediate carrier (`I64` within the tagged range, `Bool`, `None`,
  `F64` boxed as NaN) get no count operations.
- Other increments are inline: tag test, header load, immortal test (PEP 683), add. Decrements are
  inline with a cold call when the count reaches zero. Under free threading the generated sequence
  compares `ctx.owner_thread_id` with the object's owner and uses the non-atomic local count when
  equal (PEP 703 biased counting); the sequences come from one spec in `molt-codegen-abi` (HF-205).

### 4.7 Scalars in proven loops

Loop-carried values with an `I64` fact live in raw 64-bit carriers. Each add uses the hardware
overflow flag (`adds` + `b.vs`); on overflow the loop exits to a generic version of the same loop
with boxed values (loop versioning; no deoptimization runtime needed). Locals are written to their
frame homes only at observation points: calls to unproven code, raises, and suspension points.

### 4.8 Eval breaker

A poll is `ldr w, [ctx, #eval_breaker]; cbnz w, cold` at loop backedges and after calls that can run
Python code (CPython `CHECK_EVAL_BREAKER`; HotSpot safepoint polls). Producers set the word:
signal handlers, the GC scheduler on allocation thresholds, the async queue, and other threads'
stop-the-world requests. The cold path services the work and clears the word.

**Invariant J4.** Every loop backedge and every return from a call that may run Python code is
followed by a poll before the next backedge; signal latency stays bounded. Check: a CFG verifier
rule in the TIR verify-each pass.

### 4.9 Literals

Each code object owns a constant table filled at module initialization; a literal load is a table
read (CPython `co_consts`, HF-507).

### 4.10 Facts: what exists, what is missing

| fact | status | where |
|---|---|---|
| Stable module function bindings | exists, name-keyed | `frontend/lowering/sema_state.py:_module_stable_funcs` |
| Verified namespace premise over the closure | exists for discovery; frontend adoption open | `docs/design/verified_namespace_premises.md` (HF-226) |
| Callee symbol at a call site | exists as a string | `call_guarded` `metadata.target`; `type_hint="Func:<sym>"` (`frontend/visitors/call_dispatch_named.py:1262`) |
| Function arity, return, closure, definedness | exists, name-keyed maps in the backend | `known_function_arities`, `declared_func_arities`, `function_has_ret`, `closure_functions`, `defined_functions` (`fc/calls.rs`) |
| Op purity and may-raise | exists | `op_kinds.toml` purity tables |
| TIR scalar types | exists | `I64`, `F64`, `Bool`, `BigInt`, `Str`, `Bytes`, `None` |
| Argument custody per call | exists | `argument_custody` op metadata |
| Uses zero-arg `super`/`__class__` | exists | frontend `class_cell` |
| Thread record layout | exists, two fields | `molt-codegen-abi/src/lib.rs:165-168` |
| Object header layout | exists | `molt-codegen-abi` `HEADER_*` constants |
| `NativeSignature` per function | **missing** (pieces are name-keyed) | new typed fact in TIR |
| Call-site binding class (`Proven`/`Guarded`/`Monomorphic`/`Dynamic`) | **missing** as a typed fact | new, from binding flow |
| `GlobalSlot(ModuleId, k)` | **missing**; globals are dict entries | new, with the module slot array |
| `FramePlan` (record or none, home count, class cell) | partial (runtime code-slot frame plan) | move to compile time |
| `FieldSlot(ClassId, j)` for attribute sites | **missing** at compile time (runtime site caches exist) | new, from class layout facts |
| Interprocedural may-raise summary | **missing** | new TIR analysis |
| Location table per code object | data exists on ops (`source_line`, `col_offset`) | new generated table |

### 4.11 What this design refuses

- A pinned context register before measurement (prior art J): `ctx` is an ordinary argument.
- Landing-pad unwinding for Python exceptions (section 4.4).
- Name-matched fast paths (a `fib` or `sum` idiom match): every gain comes from the general
  mechanisms above (HF-185 records that the lost `vec_sum_int_range_iter` idiom hid the loop cost).
- Removing frame or traceback fidelity: every observation keeps CPython's answer (invariant I1).
- A second cache or registry beside an existing one: site records replace runtime-side site caches
  in the same migration.

## 5. Staged plan

Each stage is one complete migration: every backend (native/Cranelift, LLVM, WASM, the JIT, and the
Rust and Luau preludes where they emit the same operations), every runtime consumer, the generated
layout, and the tests. The replaced lane is deleted in the same stage.

### Stage 1: frame records (first stage, concrete)

**Goal.** Replace the runtime-owned frame stack with records in compiled frames. Locations become
plain stores into the record. Frame objects materialize on observation.

**Work.**

1. `molt-codegen-abi`: add `FRAME_RECORD_*` layout constants (prev, code, function, location,
   flags, homes base) and `THREAD_RECORD_FRAME_TOP_OFFSET`. Generate the location-table format.
2. Frontend/TIR: compute `FramePlan` per function at compile time (home count, uses class cell,
   suspends). Replace `trace_enter_slot`, `trace_exit`, `frame_context_set`, `line`, and the
   per-op column metadata calls with `frame_push`, `frame_pop` and `set_location(index)` ops
   declared in `op_kinds.toml` as `nothrow`, with their effect on the frame chain. Mark
   `frame_home_store` `nothrow`. Emit the per-code location table.
3. Native backend: allocate the record as a Cranelift stack slot; emit the four push stores, the
   pop store on every exit edge (normal return, exception return, the cold labels), and location
   stores before may-raise or may-observe operations only. Delete the emission of
   `molt_trace_enter_slot`, `molt_trace_exit`, `molt_frame_homes`, `molt_frame_context_set`
   (except functions with a class cell, which store argument zero directly), `molt_frame_set_col`,
   `molt_frame_set_line_col`, `molt_trace_set_line` (emission sites include `function_compiler.rs:1107`,
   `fc/funcobj.rs:108`, `fc/funcobj.rs:1154`, `shared.rs:1120`). The thread record pointer comes
   from the existing once-per-function `molt_thread_record` call until stage 3.
4. LLVM, WASM (shadow stack), JIT and the Rust/Luau preludes (`molt-backend-rust/src/rust/prelude/stateful.rs`
   defines `molt_trace_enter_slot`) emit the same ops.
5. Runtime: delete `FRAME_STACK` and `TRACE_FRAME_PUSH_STACK` (`state/tls.rs`) and migrate their
   consumers (about 148 references in 30 files: tracebacks, `sys._getframe`, frame objects and
   `f_back`, `locals_proxy`, warnings, `module_get_global`'s active globals, `super()` context,
   async poll, generator activation, class init, the C-API hooks) to walk `ctx.frame_top`.
   Materialization and copy-out follow section 4.3. Generators keep their record in the payload.
6. Keep runtime-called Python frames (the generic entry, `call_function_with_arguments`) on the
   same record type: the generic entry pushes a record for the native body it calls, so one chain
   holds both.

**Measurable target.** Call loop (p8) from 3,623 to at most 1,900 instructions per iteration;
`fib` from 4,346 to at most 2,300 per call; method loop from 4,697 to at most 3,000. These budgets
remove the measured frame and location shares (51% of the call loop's samples, 48% of `fib`'s,
39% of the method loop's) and add about 10 instructions of stores. Runtime calls in `add`'s hot
path from 11 to 1 (the once-per-function `molt_thread_record`, removed in stage 3). `add` from 248
to at most 120 static instructions.

**Proof.** The traceback/frame differential families on native, LLVM and WASM; the new random-depth
traceback property test (invariant I1); generator and coroutine suites; the census that no backend
imports the deleted symbols; the 8 programs of this study as benchmark rows on native and WASM;
binary-size and startup comparison (section 6).

**Ledger.** Moves HF-185 frame-cost stages 2, 3 and 6 to done (one migration instead of three);
closes HF-261 (per-statement location calls); moves HF-229 (suspended frames box every value) to
the record-in-payload form.

### Stage 2: thread-record state and inline fast paths

Eval-breaker word and inline polls (J4); inline recursion depth; inline call admission for
`call_guarded` (callee type tag, execution kind and entry pointer read at generated offsets,
replacing `molt_function_direct_call_eligible` and `molt_handle_resolve`); inline reference counts
with immediate elision (section 4.6); `molt_dec_ref_obj`'s per-call header validation moves to debug
builds. Targets: inline-int loop 361 → ≤ 130 (it removes the measured 44% poll and 23% count
shares); call loop → ≤ 1,000; no count operation on an immediate in any of the eight programs.
Closes HF-185 stages 4 and 5, HF-205; moves HF-198.

### Stage 3: compile-time binding and the native calling convention

`NativeSignature` and the two entries (4.2); `ctx` as the first argument on every backend (HF-198
stage 2); module slot arrays and `GlobalSlot` sites (4.5); `Proven`/`Guarded`/`Monomorphic` call
sites; per-code literal tables (HF-507); `runtime_generation` in the record (HF-200). Deletes
`molt_module_get_global` from module-function call sites, `molt_thread_record`, the
`pending_invocation` handoff, and `molt_string_from_bytes` in prologues. Targets: call loop ≤ 250
instructions per iteration; `fib` ≤ 300 per call. Closes HF-507, HF-200; closes HF-198; moves HF-79.

### Stage 4: inline caches in compiled code

Attribute, method, constructor and module-attribute sites (4.5) with typed name handles.
Typed-layout deallocation: an instance of a fixed-layout class frees by its layout's field list,
without the generic edge-visiting cascade (`DetachedEdgeSink`, `clear_cycle_edges_with_sink`;
31% of the `struct` loop). Deletes runtime-side site caches that the site records replace
(`field_sites.rs` cached fields, `PinnedMethodIcEntry`). Targets (with stages 1-3 landed):
attribute loop ≤ 150; method loop ≤ 350; `struct` (p5) ≤ 600; `struct.pack` ≤ 2,000 (the
remaining cost is the format plan and the wrapper, both cacheable). Closes the HF-185 attribute and
`struct` rows, HF-202; moves HF-206.

### Stage 5: exceptions

Native exception classes without the C-API detour; lazy tracebacks; class-handle matching (4.4).
Target: raise loop ≤ 1,500 instructions per iteration (CPython 2,038); `type(ValueError.__init__)`
prints `wrapper_descriptor`. Closes HF-561; completes HF-207's raise side. New ledger row needed:
builtin exception construction routes through the CPython-ABI bridge and `type()` of its `__init__`
is wrong. Independent of stages 1-4 except for the traceback buffer, which uses the stage 1 record
fields `(code, location)`; before stage 1 it can use the existing frame entry.

Lead for the worker: `construct_exception_from_args` (`runtime/molt-runtime/src/call/class_init.rs`)
has a fast path for `molt_exception_init` (`tuple_init`). It never fires for `ValueError`, because
`class_attr_lookup` for `__init__` returns a CPython-ABI descriptor, so the call takes
`call_bind_with_arguments` → `call_foreign_with_arguments`. Find which publication installs that
descriptor on the builtin exception classes (candidates under
`runtime/molt-cpython-abi/src/api/typeobj/`), make the native slot the class's `__init__`, and let C
code see a `wrapper_descriptor` view of it only when C code asks.

### Stage 6: scalar loops and observation-point locals

Raw `I64` carriers with overflow exits to a boxed loop version; home stores only at observation
points; heap-int frees through a leaf fast path (no edge-visiting cascade for objects without
references). Targets: inline-int loop ≤ 8 instructions per iteration; call loop ≤ 80; attribute
loop ≤ 40; heap-int loop 2,377 → ≤ 400. Closes the loop half of HF-185.

### Stage 7: inlining proven small callees

A TIR inliner inlines `Proven` callees under a size budget. The location table records inline
frames (DWARF inlined subroutines; HotSpot scope descriptors), so a traceback through an inlined
call shows both frames. Target: call loop ≤ 12 instructions per iteration (the loop becomes the
inline-int loop plus the guard-free call body).

### Projected end state

| row | today | after stage 6 [projection] | vs today | vs CPython |
|---|---:|---:|---:|---:|
| Python call (p8) | 3,623 | 40-80 (≤ 12 after stage 7) | 45-90x (300x) | 11-22x faster |
| `fib` per call | 4,346 | 50-100 | 43-87x | 5-10x faster |
| inline-int loop | 361 | 4-8 | 45-90x | 65-130x faster |
| attribute loop | 1,736 | 20-40 | 43-87x | 15-30x faster |
| method loop | 4,697 | 50-100 | 47-94x | 8-17x faster |
| `struct` (p5) | 27,001 | 250-600 | 45-108x | 4-10x faster |
| raise and catch | 210,274 | 800-1,500 | 140-260x | 1.4-2.5x faster |

These are estimates from the instruction budgets in section 4, not measurements. Each stage's
target above is the gate that tests the estimate.

## 6. Cost

**Compile time.** Stage 1 lowers fewer operations (the 20-operation `add` becomes about 8) and emits
fewer call sites (`fib`: 89 → about 15), so Cranelift does less work; the location table is a
linear pass. Stage 3 adds a binding-class decision per call site from facts the binding flow
already computes, and one native plus one generic entry per function (the generic entry already
exists as the trampoline). Stage 4 adds per-site records (data, not code). The interprocedural
may-raise summary is one bottom-up pass over the call graph. Expected net: compile time falls or
stays flat; each stage measures backend time on `tests/differential/stdlib/re_parity.py` before and
after.

**Code size.** Today a location call costs about 20 bytes per site (two `mov`, `adrp`, `add`, `blr`)
and `add` is 992 bytes for `return a + b`. A location store is 8 bytes, a frame push 16, a pop 4, an
inline guard 12-16, an inline increment about 16 with a shared cold call. Stage 1 shrinks compiled
code; stages 2 and 4 grow call and attribute sites by a few instructions each and remove calls.
The runtime dominates binary size (14.0 MB empty; the compiled code of these programs is under
10 KB), so whole-binary size moves little; deleting `FRAME_STACK` machinery shrinks the runtime.
Gate: no stage may grow the empty-program binary or the median compiled function by more than 2%
without a named, measured trade.

**Must not regress.** The differential corpus on native, LLVM and WASM (exact output, including
traceback text and carets); startup (empty program 192.8M instructions, 0.01 s); peak RSS; signal
latency (J4); recursion-limit behavior (`RecursionError` at the configured limit); free-threaded
refcount correctness; generator and coroutine semantics; C-API frame and exception observation.

## 7. Risks and what I could not verify

- **Not measured:** LLVM and WASM backends, Linux and x86_64, the JIT. HF-185 records that LLVM
  lowers `call_guarded` to a runtime call, so LLVM is likely worse on calls.
- **Dynamic call counts:** `lldb` could not run a traced process on this host (120 s without
  progress). Runtime calls per unit come from the hot path read in the disassembly plus sample
  shares, not from counted hits. `MOLT_PROFILE=1` counters exist but count only allocation and
  a few entry kinds. The stage gates need a dynamic count that does not depend on a debugger: one
  counter per exported runtime entry point under the existing `MOLT_PROFILE` authority
  (`state/metrics.rs`), reported per benchmark row as runtime calls per unit. Stage 1 should land
  it first, since it is the oracle for "runtime calls in the hot path".
- **Wall times** ran under load 11-13 on 18 cores; instruction and cycle counts are the primary
  evidence. Short CPython runs (0.06-0.09 s) include about 0.01 s of startup.
- **Projections** in section 5 are instruction budgets, not measurements.
- **Frame observation surface.** Stage 1 must find every reader of the frame stack, including
  C-API hooks and satellite crates bound by symbol name (HF-206). The 148-reference census is a
  `grep` lead, not a proof; the closed `FrameRecord` type makes rustc find the rest.
- **`f_locals` of a live optimized frame** shows values at the last observation point. CPython
  3.13 (PEP 667) shows live values through a write-through proxy. Stage 6 must keep homes current
  at every call to unproven code so a callee's `sys._getframe(1).f_locals` sees CPython's values;
  the property test covers it.
- **Global slot arrays** change the module dict representation; `globals()` identity, iteration
  order and `__dict__` mutation must keep CPython semantics. This is the largest semantic risk in
  stage 3.
- **The C-API detour in exceptions** may also affect other builtin types whose slots resolve to
  CPython-ABI descriptors. I did not survey them.
- The `struct` perf-gate row in HF-185 is object construction, not the `struct` module; this study
  measured both.

## 8. Prior art

- CPython 3.12.11 source (`third_party/cpython-3.12`): `_PyInterpreterFrame`
  (`Include/internal/pycore_frame.h:51`), `_PyFrame_PushUnchecked` (`:261`), lazy frame objects
  (`Python/frame.c:_PyFrame_MakeAndSetFrameObject`, `take_ownership`), `CALL_PY_EXACT_ARGS`
  (`Python/bytecodes.c:2761`), `LOAD_ATTR_INSTANCE_VALUE` (`:1808`), `LOAD_GLOBAL_MODULE`
  (`:1365`), `BINARY_OP_ADD_INT` (`:384`), `CHECK_EVAL_BREAKER` (`Python/ceval_macros.h:117`),
  `COMPUTE_EVAL_BREAKER` (`Python/ceval_gil.c:63`), `_Py_EnterRecursivePy` (`Python/ceval.c:627`),
  traceback at error (`Python/ceval.c:924-941`), `get_exception_handler` (`:1267`), exception tables
  (`Objects/exception_handling_notes.txt`, `Python/assemble.c:assemble_exception_table`),
  vectorcall (`Objects/call.c:_PyObject_VectorcallTstate`, `_PyFunction_Vectorcall`), thread state
  (`Include/internal/pycore_pystate.h:_PyThreadState_GET`).
- mypyc: native `CPyDef_` functions with early binding inside a compilation unit, `CPyPy_` wrappers
  for Python callers, tracebacks added on the error path (`CPy_AddTraceback`).
  https://mypyc.readthedocs.io/en/latest/differences_from_python.html
- Cython: `cdef` C functions, `cpdef` dual entries with an override check, traceback frames created
  only when an exception passes (`__Pyx_AddTraceback`).
- Codon: static compilation of Python to LLVM with monomorphized functions and LLVM exception
  handling.
- PyPy: virtualizable frames whose fields live in registers until something forces the frame.
- HotSpot: monomorphic inline caches with an unverified entry point that checks the receiver class,
  `r15_thread`/`rthread` thread register, safepoint polls, stack guard zones.
- V8: hidden classes and inline caches, `kRootRegister`, interrupt checks at loop backedges.
- Wasmtime: `VMContext` passed to every compiled function; `max_wasm_stack`.
- Cranelift: user stack maps for values live across safepoints; `tail` calling convention (not
  needed: Python has no tail-call elimination and tracebacks need every frame).
- PEP 590 (vectorcall), PEP 683 (immortal objects), PEP 703 (biased reference counting), PEP 669
  (instrument only enabled events), PEP 667 (`f_locals` proxy).

## 9. Programs and reproduction

Each program prints its result, so a run checks its output against CPython's before its count
counts. `p0` is an empty program (`pass`) for the startup subtraction. The long variants (`q*`)
raise the iteration count to make the per-unit share of startup negligible; `q2b` repeats the
`p2` loop 20 times with `total & 1` folded into `grand`, so `total` stays an inline integer. The
small-N variants (`r*`) run 1,000 and 2,000 iterations for the differential check in section 1.

```python
# p1: fib(30)
def fib(n):
    if n < 2:
        return n
    return fib(n - 1) + fib(n - 2)
def main():
    print(fib(30))
main()

# p2: counted loop (heap int once total passes 47 bits)
def main():
    total = 0
    for i in range(10_000_000):
        total += i
    print(total)
main()

# p3: attribute store and load
class Point:
    def __init__(self, x, y):
        self.x = x
        self.y = y
def main():
    p = Point(1, 2)
    total = 0
    for i in range(5_000_000):
        p.x = i
        total += p.x
    print(total)
main()

# p4: method call
class Counter:
    def __init__(self):
        self.step = 1
    def inc(self, v):
        return v + 1
def main():
    c = Counter()
    total = 0
    for i in range(5_000_000):
        total = c.inc(total)
    print(total)
main()

# p5: constructor and two stores (the perf gate's `struct` row)
class Point:
    def __init__(self, x=0, y=0):
        self.x = x
        self.y = y
def main():
    i = 0
    while i < 1_000_000:
        p = Point(0, 0)
        p.x = i
        p.y = i + 1
        i += 1
    print(i)
main()

# p6: struct.pack
import struct
def main():
    total = 0
    for i in range(1_000_000):
        b = struct.pack("<ii", i, i + 1)
        total += len(b)
    print(total)
main()

# p7: raise and catch (HF-561)
def main():
    total = 0
    for i in range(1_000_000):
        try:
            raise ValueError("x")
        except ValueError:
            total += 1
    print(total)
main()

# p8: module function call
def add(a, b):
    return a + b
def main():
    total = 0
    for i in range(10_000_000):
        total = add(total, i)
    print(total)
main()

# x1: exception __init__ descriptor kinds
print(type(ValueError.__init__))
print(type(BaseException.__init__))
print(type(ValueError.__new__))
print(ValueError.__init__ is BaseException.__init__)
print(type(ValueError("x").__init__))
```

Measurement: build each program with `molt build --build-profile release` (`MOLT_KEEP_SYMBOLS=1`
for the disassembly and sampler) and run it, and `python -B -I -S prog.py` for CPython, under
`/usr/bin/time -l`, which reports instructions retired, cycles and peak RSS on macOS. Record the
median of 5 runs; subtract the empty program's instructions and divide by the unit count. The
anatomy, disassembly and sampler steps are those of section 1. Stage 1 turns `p1`-`p8` into
benchmark rows with the instruction budgets of section 5, on native and WASM, so each stage proves
its budget through the benchmark authority (`tools/PERF_AUTHORITY.md`).
