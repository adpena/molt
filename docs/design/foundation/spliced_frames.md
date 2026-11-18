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
X:  call molt_spliced_frame_link(function, location, key_space)   // per level, outermost first
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
  ignored, and an offset with no reaching store is unbound. Fusion reads the poll with its resume
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

The splice emits only operations every backend already lowers: `ConstInt` and the runtime-call
form of `Call` (a `Call` whose typed runtime entry its `s_value` names), which the native
back-conversion carries to SimpleIR as a `call` and LLVM lowers through the boxed runtime ABI.
No backend changes.

Four runtime entries (`runtime_abi.toml`, one row each), all nothrow:

| entry | arguments | effect |
|---|---|---|
| `molt_spliced_frame_link` | `(function, location, key_space)` | link one owned record for the level |
| `molt_spliced_frame_bind` | `(key_space, key, value)` | bind one local in the newest record |
| `molt_spliced_frame_origin` | `(exception or None, levels)` | insert the newest `levels` records' entries beneath the caller's head entry, then pop them |
| `molt_spliced_frame_unlink` | `(levels)` | pop the newest `levels` records |

`link` always links exactly one record (one without code for a malformed function, which
observers skip), so every pop stays paired with its link.

## 5. Stated boundaries

These follow the existing M2.1 boundaries of call_anatomy.md and do not change I1's list:

- **Frame identity.** Each observation of a spliced activation builds a new frame record, so two
  `sys._getframe()` results from one activation are equal in every field but are not the same
  object, and a frame reached by `f_back` is not the traceback's `tb_frame` of that activation.
- **PEP 667 writes.** A write through a materialized frame's `f_locals` proxy changes that frame
  object, not the compiled bindings. This is the proxy boundary of M2.1.
- **Signal handlers.** A handler that runs at an asynchronous-work poll inside a spliced body
  sees the caller as the executing frame. An exception it raises takes 4.2, so the traceback is
  complete. Out-of-band observers (`sys._current_frames`, faulthandler) see the caller too.

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
  Origin edge and bracket, exits routed to the iteration handler); the inliner's observable
  callee; the runtime entries (chain order, unbound slots, the layout mapping, the token).
- Differential programs on native and LLVM, identical to CPython:
  `tests/differential/basic/generator_proven_call_frame.py` and
  `tests/differential/basic/generator_fused_frame_observers.py` (a traceback raised inside a
  fused generator body, `sys._getframe()` read inside it, `tb_frame.f_locals`).
- Cost: instructions per yield of the generator idiom against C and CPython
  (`cref/measure.sh` method), and the `call_two_args` and `method_call` idioms for the inliner.
