# Spliced frames: fuse and inline observable bodies

Status: design, 2026-11-18 (lane spliced-frames). Owner of the per-exception-edge frame state that
generator fusion and the TIR inliner share. Parent: [call_anatomy.md](call_anatomy.md), section
4.3 (frame records) and Stage 7 (inlining). Ledger row: HF-893.

## 1. Problem

The `generator` idiom (`for v in gen(n): total += v`) runs 1,668 instructions per yield on LLVM
release (slope 200k to 4.2M yields, main 4813d716b8). CPython 3.12 `-I` runs 971. C runs about 0:
clang inlines the iterator into the loop. Molt runs the generator as a heap task: each yield is a
poll call, a state dispatch, a frame-record link and unlink, and slot loads and stores.

Generator fusion (`passes/generator_fusion`) can splice the poll into the consumer loop, but it
fuses nothing. `MOLT_INLINE_STATS=1` prints `poll py.gen.gen_poll not fused: recursive`.
`CallGraph::recursive_set` puts every function with an opaque call in the recursive set, and the
poll calls `molt_frame_locals_begin` and `molt_exception_trace_prepend`. That rule stands in for
frame observability. A splice is a compile-time copy, so only a static cycle makes it unbounded.

The poll's frame is observable. `range(n)` over an untyped `n` and `i * 2` may raise, and the
traceback must show `in gen` with its line. The TIR inliner has the same limit: it splices only a
callee whose frame elision removed every observer (`ExecutionContextPolicy::None`).

## 2. Model

A **spliced activation** is a callee body that a splice copied into its caller: an inlined
synchronous function, or a fused generator poll. Its frame record is gone from the hot path. Its
frame is still observable at three kinds of point:

1. an **Origin exception edge**: an operation of the body failed, so the traceback gains an entry
   for the activation at its own line (call_anatomy.md, "Executing source locations");
2. an **explicit raise** (`molt_exception_trace_prepend`, the explicit raise's Origin);
3. an operation that **may run Python code or reads the frame** (`frame_elision::op_observer`):
   Python code can walk `f_back` through the activation, or call `sys._getframe()` in it.

A **frame state** describes the activation at one such point. It holds the activation's function
object (an SSA value of the caller), its `LocationId` in the callee's own location table, and the
binding of each callee local that holds one there (a key and an SSA value). Nested splices form a
chain, outermost first: an inlined callee of a fused poll has two levels. This is Graal's
`FrameState` with its `outerFrameState`, HotSpot's `ScopeDesc` chain at a debug-info PC, and V8's
translation of inlined frames.

**The hot path pays nothing.** A frame state is data on cold edges. The splice places it there;
the runtime builds a frame record from it only when an observer runs.

## 3. Invariants

- **I1 (observational equivalence, call_anatomy.md 4.3).** At every observer point the spliced
  activation reads as the eager model shows it: `f_code`, `f_lineno`, `f_back`, `f_globals`, the
  traceback entry order and lines, and `f_locals` at that point.
- **I2 (traceback order).** An exception that leaves a spliced body carries, outermost first, the
  record-owning caller's entry at its call site, then one entry per spliced level, then the
  entries already built below them. Each entry appears once.
- **I3 (no hot cost).** A splice adds no operation to a path that does not raise and runs no
  operation that may call Python code, once the merged caller's facts are known (4.4). Bindings a
  frame state names stay live to the observer points that name them, as Graal's values stay live
  to their `FrameState`.
- **I4 (bounded copy).** A splice refuses only a static call cycle. An opaque call is no reason
  to refuse.
- **I5 (one authority).** One module (`passes/spliced_frame.rs`) builds every frame state's
  operations. Fusion and the inliner compute the bindings from their own facts (the fusion slot
  plan, the callee's home stores) and hand them to it. No backend has a spliced-frame case.

## 4. Mechanism

### 4.1 Static cycles, not opaque calls

`CallGraph` keeps two sets: the static-cycle set (a member of a multi-node SCC, or a self-edge)
and the opaque set (a body with an opaque call). `recursive_set()` stays their union for the
native recursion guard. Generator fusion and the inliner ask only `in_static_cycle(id)`.

### 4.1a Fusion's consumer

The consumer is the frontend's `for` loop over a proven generator call: `iter` of the
`alloc_task` result, one `IterNextUnboxed` (element, done) followed by its Origin check, a branch
chain to the done test, and the loop header the body continues to. A reduction prelude over the
iterator declines first and the caller is re-optimized, so the loop is the iterator's one
consumer. The splice retires the header, the step and the chain. A value the header carries (an
accumulator) is defined twice after fusion, at the generator's entry and where the consumer body
continues the generator; `ssa_update::repair` gives every use the definition that reaches it.

### 4.2 The Origin edge keeps its kind

An Origin check inside the spliced body stays an Origin check. Its backend capture
(`molt_exception_frame_origin`, M1) records the **record-owning** frame, the outermost caller, at
the caller's current location. The splice publishes the call-site location at the splice entry
(and, for fusion, again where the consumer body re-enters the generator), so location placement
(M1, M2) gives the check the call site's `frame_location`. The check's exceptional successor
becomes a fresh cold block:

```
check_exception (Origin) -> X                        // backend: caller entry at the call site
X:  call molt_spliced_frame_link(function, location, identity_slot)   // per level, outermost first
    call molt_spliced_frame_bind(key_space, key, value)           // per binding of the level
    call molt_spliced_frame_origin(None, levels)
    br <the caller's handler of the call site>(<what the call site's check passes>)
```

`link` pushes one owned frame record for the level (`prev` the record below it, homes unbound,
the level's location in its location word); `bind` writes one binding into the newest record's
homes; `origin` captures the `levels` newest records as traceback entries, inserts them directly
beneath the caller's entry, and pops them. A captured record's payload takes its bindings over, as
CPython's `take_ownership` does, so `tb_frame.f_locals` reads them later. None of the three
allocates a Python object or refuses a pending exception. (A first cut built the state as a
tuple: the tuple constructor refuses to run while an exception is pending, so the state arrived
as `None` on exactly the edges that need it.)

Every exceptional edge of the spliced body then branches **straight to the caller's handler of
the call site**, with the values the call site's own check passed it: the inliner's post-call
check target, or the consumer's iteration check target for fusion. A handler-free callee's
exception targets are its exits, which hold nothing a spliced body keeps. The edges never pass
the caller's own Origin check, which would capture the caller twice, and each edge has its own
block, so the caller's carried values (a loop's accumulator) reach the handler without a join at
an exception target. A splice whose call site has no such target refuses an observable callee.

Nested splices: an inner splice's cold block already links its level. The outer splice must put
its level in front (outermost first). Until that extension exists, fusion refuses a poll that
already holds a frame state (`PollRefusal::NestedSplice`).

**Why the check keeps its kind.** The exception event verifier
(`exception_regions::compute_exception_region_facts`) requires an Origin check after an operation
that may raise. Changing the check to Unwind would make that a hard verification failure, and
the outer capture would still need an Origin. Keeping the kind keeps the event model, the
verifier and both backends unchanged.

### 4.3 An explicit raise

The body's explicit-raise Origin (`call molt_exception_trace_prepend(exc)`) prepends the executing
record's entry, which is the caller's at the call site. The splice places the links and
`molt_spliced_frame_origin(exc, levels)` right after it, which insert the spliced levels beneath
that entry. The raise and its Unwind check follow unchanged.

### 4.4 Operations that may run Python

The splice brackets each operation that may run Python code or reads the frame
(`spliced_frame::may_observe_frames`: `frame_elision::op_reads_frame` or typed
`may_call_python`; frame bookkeeping is already dropped):

```
call molt_spliced_frame_link(...) ; call molt_spliced_frame_bind(...) ...
<operation>
call molt_spliced_frame_unlink(levels)
```

The links put the chain's records over the caller's, so Python code the operation runs sees the
spliced frames in `f_back` and in `sys._getframe()`; `unlink` pops exactly those records. At
splice time the generator's operands are untyped, so most of its operations qualify. After the
merged caller is re-optimized, `prune_brackets` deletes each bracket whose operations the
caller's typed facts now prove callback-free (the multiply and compare on the loop counter), and
the caller is optimized again. What stays is once per call (a range bound's `operator.index` on
an untyped argument) or cold. A bracket that stays on a hot operation costs one link per level,
one bind per binding and one unlink, still far less than the heap task it replaces. The
operation's own Origin check follows the unlink and takes 4.2.

### 4.5 Frame-state bindings

- **Fusion.** The slot plan (`generator_fusion/slots.rs`) gives every user frame slot its
  reaching definition at each block entry; the clone walks each block from there. A binding's
  key is its payload offset (`KeySpace::StatefulOffset`). The runtime maps offsets to code slots
  through the poll's registered stateful locals layout (`stateful_locals_register`), keyed by the
  code's poll target; offsets the layout does not name (iterator and split temporaries) are
  ignored, and an offset with no reaching store is unbound. A publication reads the incoming
  bindings before the first store to each slot. Those reads and ordinary slot loads share the
  liveness that places join arguments. If a required join has a bound and an unbound predecessor,
  the ordinary generator preserves the distinction between an absent local and Python `None`.
  A poll with no possible first observer needs no join for yield or completion alone.
  A mixed join can still prevent fusion when only later caller specialization would remove
  its last observer. Supporting that case requires an explicit presence value at the join.
  Fusion reads the poll with its resume
  edges made plain control flow (`generator_fusion/linearize.rs`: each yield continues at the
  dispatch case it saves, the dispatch enters at its default), so a read after a resume sees the
  store before the yield. A poll whose prologue publishes cells into frame slots
  (`molt_frame_cell_publish`) is refused: the slot plan cannot see that write.
- **Inliner.** A home store (`frame_home_store`, `value` = code slot) is the binding's
  definition. A forward reaching-definition analysis over the callee gives each observer point
  its bindings; key = code slot (`KeySpace::CodeSlot`). A callee with cell homes
  (`frame_home_cell`, `frame_home_private_cell`), `frame_home_take`, `frame_home_clear` or
  `frame_locals_set` is refused until those transitions have a frame-state form.
- **Location.** The callee's reaching publication at the point
  (`frame_location_placement::reaching_locations`, the M1 analysis, run on the callee before its
  publications are dropped). A point that no single publication reaches records an unknown
  location; the frame then reports its code's first site.
- A binding whose value is the `missing` sentinel is unbound.

### 4.6 Lowering and runtime

The splice emits typed runtime calls and the caller's identity-slot operations.
`SplicedFrameSlot` provides the activation's zeroed word and `SplicedFrameObserved`
reads whether a cold observer has published a bindings payload there. Link receives
the word's raw address. Finish consumes the owned payload taken from it. Neither
entry is a generic Python callable.

The five runtime entries are declared in `runtime/runtime_abi.toml`.

| entry | arguments | effect |
|---|---|---|
| `molt_spliced_frame_link` | `(function, location, identity_slot)` | link one record for the level and reattach its observed bindings |
| `molt_spliced_frame_bind` | `(key_space, key, value)` | bind one local in the newest record |
| `molt_spliced_frame_origin` | `(exception or None, levels)` | insert the newest `levels` records' entries beneath the caller's head entry, then pop them |
| `molt_spliced_frame_unlink` | `(levels)` | pop the newest `levels` records |
| `molt_spliced_frame_finish` | `(bindings)` | retire the observed activation and release its ownership |

`link` always links exactly one record (one without code for a malformed function, which
observers skip), so every pop stays paired with its link.

## 5. Locals writes, signal handlers and frame identity

The design review (2026-11-18) required a mechanism for each of these, not a stated boundary.

### 5.1 Frame mutation by an observer

From 3.13 a write through `frame.f_locals` rebinds the running frame's local, and Molt's
unspliced frames honour it (HF-901). A bracket gives Python code a snapshot of the spliced
activation's locals, so a write through it would not reach the compiled bindings. HF-901
(412df7e3f8) made the writer a closed-world fact: `PythonNamespacePremise.frame_locals_writer`,
exposed to binding flow as `frame_locals_exposed(target, writer)`. Before 3.13, clearing a
suspended generator's frame synchronously closes its continuation. The frontend therefore marks
generator poll bodies containing a yield on these targets, independent of the writer fact.
Both cases use `FunctionIR.frame_locals_exposed` (TIR attribute `frame_locals_exposed`).
After bracket pruning, a marked splice stays unfused if a callback or signal poll can expose its
frame (`PollRefusal::MutableFrame` under `MOLT_INLINE_STATS`). Refreshes of an already observed
frame at yield and return do not themselves expose it.

A poll's bracket counts. Unspliced Molt frames honour a signal handler's write at a loop poll:
in a loop whose only callback point is its poll (`while limit and i < bound: i += 1`, all
operands exact ints), a handler that sets `frame.f_locals["limit"] = 0` stops the loop on LLVM
release, as it does on CPython 3.15.2. A fused body must do the same.

The writer is a use that may write, not a read (2026-11-18). The scanner classifies each
`f_locals` use as it classifies a namespace mapping's (`_item_write`, `_MappingFlow` in
`python_namespace_writes.py`): an item store or delete, a writing method, a return, a store or a
call argument a callee may write through records a writer; an item read, a read method,
membership, iteration, a whole-read builtin or a local alias only read does not. Measured on
LLVM release at the 3.15 target, the generator idiom in a program whose only `f_locals` use is
`sys._getframe(1).f_locals.get(name)`: 1,686.5 instructions per yield before (refused), 21.0
after (fused).

A program that imports `logging` still stays unfused, for a different reason: it pulls
`importlib._bootstrap` into the graph, whose bodies write `sys.meta_path` and `sys.path_hooks`, so
every import "may be redirected", no module cell is proven and the call of the generator function
stays generic (no `alloc_task`; build diagnostic `static_module_cells`). HF-693 owns that premise.

Not built here:

- **Reload after exit.** Before it unlinks, a bracket reloads each binding from the record's home
  (a runtime entry that returns the home's value, or the bound value when nothing rebound it),
  placed in the poll before the slot plan so the plan's SSA construction joins the reloaded
  values. Building it found two obstacles. A reload returns an untyped value, so a bracket that
  later facts would prune keeps itself alive through its own reload; pruning needs an optimistic
  pass that treats reloads as transparent until a bracket is proven to stay. And TIR does not know
  which payload offsets name Python locals (the layout reaches the runtime only through
  `stateful_locals_register`), so a reload of a compiler temporary, such as the loop's iterator,
  would lose its type for nothing.

### 5.2 Signal handlers

A Python signal handler receives the executing frame. CPython services signals at its eval
breaker (a backward jump or a call), in whichever frame runs there.

- The consumer's latch poll stays. The consumer body runs once per fused iteration, so the fused
  loop services pending work there in the consumer's own frame, where CPython's `FOR_ITER` loop
  would. Before this design the splice retired that poll, so a fused loop serviced no work at
  all. `async_work_poll::loop_only_poll_sites` goes with its only consumer.
- A poll inside the generator body services work in the generator's frame. On a target with a
  pending-work test, a poll whose only job is the work (an Unwind edge) splits before the slot
  plan (`generator_fusion/frame_states.rs`): `async_work_pending` branches to a cold service
  block that links the generator's record, polls and unlinks. No pending work, no bracket, so
  the split costs one test per loop iteration, as the unspliced poll does. A poll with an Origin
  edge also observes the exception of the call it follows, and its lowering tests that
  exception on the path with no work pending, so it stays in line inside its bracket; moving it
  behind the test dropped the generator's traceback entry when the call raised (caught by
  `generator_fused_frame_observers.py` on a 3.12 target). The handler receives the generator's
  frame with the consumer as `f_back`. The poll's exceptional edge leaves with the record still
  linked; the backend's Origin capture then sees a record that is not the caller's and records
  nothing, so the cold block's `molt_spliced_frame_origin` captures the consumer and the
  generator together and pops the record. A target without the pending-work test brackets every
  poll in line.
- The check that observed the locals prologue (`molt_frame_locals_begin`) goes with that call.
- The runtime passed `None` as every handler's frame (`signal_ext::run_pending_handlers`). It now
  passes the executing frame, as CPython does: `_PyErr_CheckSignalsTstate` (`Modules/signalmodule.c`) calls the
  handler with the thread's current frame object.

Out-of-band observers (`sys._current_frames`, faulthandler from another thread) see the last
surviving publication. call_anatomy.md states that boundary for every frame (M2.1, its last
paragraph); spliced frames add nothing to it.

### 5.3 Frame identity

CPython 3.11+ caches one frame object per activation (`_PyInterpreterFrame.frame_obj`), so
`sys._getframe() is sys._getframe()` holds, and `f_back` and a later `tb_frame` are one object.
Molt publishes the completed Python frame on its activation's bindings payload. Ordinary
records, generator tasks and lazy traceback nodes can all materialize that same frame.
The activation and its lazy traceback nodes collectively retain one reference to it. The
last such owner drops that reference. The materialized traceback and any escaped frame
continue to own their binding edges, and ordinary refcounted exit promptly releases an
unescaped frame. Cycle traversal and detachment account for both directions.

A live `f_back` lookup follows the executing records from the observed activation. An
unlinked, suspended generator has no caller. An escaped synchronous frame retains its
terminal caller, including when the traceback is first materialized after unwind. A started
generator retains its terminal resumer on 3.12. Newer targets clear that link.
Traceback line and instruction offsets stay frozen independently of the live frame.
Signal dispatch, back links, warning sites and traceback capture share the live record's
Python visibility. A compiled adapter whose function has a native callable class contributes
no Python frame. An ordinary Python stdlib function retains its frame.

Each spliced activation borrows a zero-initialized identity word in the caller. Its first
observer publishes the activation's bindings there. Subsequent brackets reattach that same
payload to their current homes, and unlink freezes the location and bindings. The compiler
transfers the word to the runtime when the activation ends, so a repeated invocation of
the same splice site starts with its own identity. HF-905 records backend qualification.

## 6. Alternatives rejected

1. **Keep one frame record linked around the spliced body** (the step-B sketch in the
   generator-fusion handoff). The consumer body runs inside the fused loop, so the record would
   need an unlink before each consumer body and a link after it: four stores per yield, plus the
   location word. It also costs every inlined call a link and an unlink.
2. **A new `ExceptionEdgeKind` for spliced Origins.** It needs a new arm in both backends'
   exception lowering, the verifier and the generated tables, for an edge that 4.2 expresses
   with existing operations.
3. **Change the spliced check to Unwind and capture explicitly.** The event verifier rejects an
   Unwind check after a may-raise operation, and the caller's capture still needs an Origin.
4. **Return-address tables (DWARF inlined subroutines, HotSpot PcDesc, Stage 7's sketch).**
   They cost nothing on the hot path, but WASM cannot read a return address, both backends would
   need inline-location tables, and they do not carry `f_locals`.
5. **Rematerialize the generator task on the cold edge** (HotSpot's `realloc_objects` for a
   scalar-replaced object). It gives `tb_frame` the generator's own frame, but it ties the cold
   path to task construction and the stateful payload, and it does not serve the inliner.
6. **Refuse observable bodies** (the status quo). The generator idiom stays a heap task.

## 7. Sources

- Graal: `FrameState` (values, `bci`, `outerFrameState`); G. Duboscq et al., "An Intermediate
  Representation for Speculative Optimizations in a Dynamic Compiler", VMIL 2013.
- HotSpot: `code/scopeDesc.hpp` and `code/debugInfoRec.hpp` (a scope chain per debug-info PC,
  `sender()` for the caller scope); `runtime/deoptimization.cpp` (`realloc_objects`,
  `reassign_fields`: rematerialize scalar-replaced objects at deoptimization).
- V8: `src/deoptimizer/translated-state.h` (`TranslatedFrame` per inlined function);
  `OptimizedFrame::Summarize` (`src/execution/frames.cc`) builds stack-trace frames for inlined
  functions from deoptimization data.
- DWARF 5, section 3.3.8 (`DW_TAG_inlined_subroutine`).
- CPython 3.12: `Python/frame.c` `take_ownership`; `Python/traceback.c` `PyTraceBack_Here`.

## 8. Proof and cost

- Unit tests: the call graph's two sets; fusion of an observable poll (frame-state blocks on each
  Origin edge and bracket, exits routed to the iteration handler); brackets pruned once facts
  prove their operations; the poll's cold service path; an exposed poll that keeps a bracket
  stays unfused; the inliner's observable callee.
- Differential programs on native and LLVM, identical to CPython:
  `tests/differential/basic/generator_proven_call_frame.py`;
  `tests/differential/basic/generator_fused_frame_observers.py` (a traceback raised inside a
  fused generator body, `sys._getframe()` read inside it, `tb_frame.f_locals`; fused on a 3.12
  target, unfused on 3.15 because its `f_locals` reads are a writer, 5.1);
  `tests/differential/stdlib/generator_fused_signal_frame.py` (a handler's frame argument inside
  a fused body, 5.2); `tests/differential/basic/generator_fused_locals_writer.py` (an `f_locals`
  write under 3.13+, 5.1); `tests/differential/basic/frame_object_identity.py` (HF-905, 5.3).
- Cost: instructions per yield of the generator idiom against C and CPython
  (`cref/measure.sh` method), and the `call_two_args` and `method_call` idioms for the inliner.
  Measured 2026-11-18 on LLVM release at the 3.15 target: 1,665.4 unfused, 20.9 fused, CPython
  3.15.2 857.3, C ~0. The disassembled hot loop holds no spliced-frame call and no location
  store (I3); the call-site republish runs only on the range's slow path and the poll's cold
  path. The ledger row HF-893 holds the per-yield breakdown and the next cost classes.
