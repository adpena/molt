<!-- Foundation design 45. Supervisor-authored from the #77 diagnosis + recovery
agent's lifecycle proof (baton: memory/project_exception_loop_leak_baton.md) +
council directive (2026-06-08). Executable design, not a survey. HEAD-anchored
at origin/main 0e233db5f. -->

# ExceptionRegion ownership — the exception-lifetime substrate

## 0. Why this exists (the discovery, not a benchmark patch)

`bench_exception_heavy` measured 0.68× warm vs CPython, cycle-attributed (#76,
quiet, 100% in-binary) to **inc_ref/dec_ref ~22%** + GIL ~11% + exception-stack
bookkeeping ~12%, with a coupled **~70 MiB / 30-iteration leak**. The #77 attack
proved this is not a peephole: exception ownership is **region-scoped**, and
molt's RC model reasons mostly in value / global-last-use space. The single-
global-last-use model **structurally cannot place** the correct release of
handler-owned exception state. So `exception_heavy` is a *missing abstraction*,
not a slow path — and the fix retires a whole class (the leak, the churn, LLVM
exception-CFG fragility, drop placement on handler edges, finalizer/unwind
ordering).

## 1. Problem: the leak has TWO independently-owned components

Per raised-and-immediately-caught exception in a loop (diagnosed op-by-op via
`MOLT_DUMP_FINAL_FUNC_IR` + `MOLT_TRACE_EXC_RC`):

- **Component A — CreationRef** (`exception_new*` result): per-iteration-dead,
  SSA last-use = the `raise`. **Value-tracking-expressible** — a #46-style
  per-iteration-temp analysis releases it (prototype: rc 2→1, preserved at
  `memory/recovery/excfix_wip/function_compiler_excregion_wip.patch`).
- **Component B — MatchRef** (`exception_last_pending` result): **still leaks.**
  Its SSA last-use is the re-raise in the **no-match ELSE branch that never
  executes** on the caught path; on the matched path it is only *borrowed*. The
  correct release point is **handler-region exit (`exception_pop`)** — CPython's
  implicit clear of the caught exception — a **per-PATH exception-CFG liveness
  fact** the single-global-last-use model cannot express.

Net per caught exception today: 3 inc / 2 dec → rc=2 leaked (with the Component-A
prototype: 3 inc / 3 dec but the lone `dec` sits in the dead ELSE branch → rc=1,
still leaked).

## 2. Falsified vs supported (binding)

- **FALSIFIED:** "exception objects are missing from value-tracking
  registration." They ARE tracked (generic per-op tail registration,
  `function_compiler.rs:~24992`).
- **SUPPORTED:** the issue is **release-boundary PLACEMENT**, not registration.
  CreationRef releases at the raise; MatchRef must release at handler-region
  exit. No UAF exists in the prototype: every exception-STATE SLOT
  (`global_last_exception`, the task slot, `ACTIVE_EXCEPTION_STACK`) holds its
  **own independent inc'd reference**, and `sys.exc_info()` / `sys.exception()`
  lower to `molt_exception_active` reading the *slot*, not the SSA temp — so
  releasing an exception SSA temp at its last use can never dangle a slot.

## 3. The CPython semantic contract (Python-visible lifetime rules — not impl details)

- `sys.exception()` returns the caught exception **only while a handler is
  executing**, `None` otherwise; the stored active exception is **reset on
  leaving the handler** (Python language ref, §try; sys docs).
- `except E as e` is cleared at the end of the except clause — effectively
  `finally: del e` — *specifically* because exceptions with tracebacks form
  cycles with stack frames and keep locals alive.
- A traceback attached to an exception keeps **frame + local** state alive;
  `__traceback__` / `__context__` / `__cause__` / `__suppress_context__` are
  reachable while the exception is.
- `finally` can save / discard / re-raise the active exception across nonlocal
  exits (return/break/continue/raise).

These are the placement obligations the abstraction must honor exactly.

## 4. IR model: ExceptionRegion / HandlerState

Exception ownership is **a property of a region state machine, not of one SSA
value.** Introduce a per-`try` `HandlerState` with explicit lifetime points:
`entry → match → bind → (body) → pop | reraise | finally-save/restore/discard`.
A `HandlerState` is the owner of the active-exception roots for its region; its
boundary (`pop`/`reraise`/transfer) is where release/transfer obligations fire.

## 5. Ownership model (who owns each root, released/transferred where)

- **CreationRef** — owned by the `raise` site; released or **transferred** into
  the pending/HandlerState at the raise boundary (a propagating exception
  transfers; a caught one hands to HandlerState).
- **MatchRef** — owned by the HandlerState; released at **region exit** unless
  transferred to user-visible storage (stored binding, `__context__`/`__cause__`
  of an escaping exception, a returned value).
- **BindingRef** — `except E as e`: the local `e` owns/references the handler
  exception; **cleared at handler exit** (`del e`) unless the value escaped/was
  stored.
- **Traceback / context / cause** — owned by the exception object and/or explicit
  traceback roots; follow the exception's ownership boundary (released with it
  unless reachable via a stored ref).

## 6. Placement rules (every exit edge)

normal handler fallthrough · break/continue/return from a handler · exception
raised **inside** a handler (the new exception's region nests; the outer match
becomes `__context__`) · `raise` (re-raise → transfer back to propagating) ·
`raise X from inner` (transfer inner to `__cause__`) · `finally` (save before /
restore-or-discard after the protected region) · nested handlers (LIFO region
stack). **Invariant:** every handler-owned exception root is `pop`'d,
transferred, or re-raised **exactly once on every exit path**.

## 7. Event model (name the semantic events; not all become public TIR opcodes day 1)

```
ExceptionPush(exc)          pending exception roots exc (raise / propagate)
ExceptionMatch(exc, h)      HandlerState h acquires the match ref
ExceptionBind(name, exc)    local binding per `except as` rules
ExceptionPop(h)             leave handler: restore prior sys.exception, release match ref
ExceptionReraise(h)         transfer handler exception back to propagating state
ExceptionClearBinding(name) `except E as e` cleanup at handler exit
ExceptionFinallySave(exc) / ExceptionFinallyRestore(exc) / ExceptionFinallyDiscard(exc)
```
Once named, the compiler/runtime places DecRef / transfer obligations correctly
at each. This is the same generated-fact discipline as the op-semantics ladder
(#70/#72/#73/#74) — exception ownership becomes a *region* fact on the #58
ownership-boundary lattice (region-lifetime facts; `InteriorBorrowKeepAlive`
#73 and `ConditionalValidOnlyOnEdge` #74 are the path-sensitive siblings that
prove the lattice can carry exactly this kind of boundary).

## 8. Minimal implementation — phased (prove the model before the edge cases)

**Phase 1 — bare raise/catch loop** (the model proof):
```python
for i in range(N):
    try: raise ValueError(i)
    except ValueError: pass
```
Acceptance: `MOLT_ASSERT_NO_LEAK` passes (RSS plateaus under #76 `--inner-repeat`);
exception_heavy no longer leaks per iteration; `sys.exception()` is the exception
**inside** the handler and `None` **outside**; native + LLVM agree (or the backend
gap is documented); #76 quiet hot profile shows exception RC/churn **moved**.

**Phase 2 — `except E as e`**: `e` alive in-handler, cleared at exit; a stored `e`
remains usable; an unstored `e` does not retain traceback/frame/locals.

**Phase 3 — traceback / `__context__` / `__cause__`**: stored-vs-not lifetime;
`traceback.format_exception` works; chaining matches CPython.

**Phase 4 — finally / re-raise / nested / break-continue-return-from-handler.**

The Component-A prototype is **evidence in this note, not landed behavior** — it
lands only as part of A+B (no asymmetric half-fix that leaves the loop leaking).

## 9. Validator (Alive2-style, scaled to molt — add as soon as the event model exists; ties #TV-1)

Checkable obligations (not full formal verification):
1. For every `ExceptionMatch` there is exactly one `ExceptionPop` / `Reraise` /
   transfer on **every** exit path.
2. No handler-owned exception root reaches function exit without transfer.
3. No `ExceptionPop` runs before a `sys.exception()`/`sys.exc_info()` use inside
   the handler (the reset must not precede observers).
4. No `except E as e` binding survives handler exit unless explicitly stored.

## 10. The frontier lesson (CPython 3.11 zero-cost exceptions — inspiration, not copy)

3.11 made `try` impose ~zero overhead on the no-throw path and shrank the
catch-time exception representation. molt's AOT analogue: the **normal edge pays
nearly zero** for handler existence (no exception-stack churn on the no-throw
path); the **exceptional edge owns explicit region state**; handler exit is **one
structured release/reset**, not scattered RC cleanups; backend lowering sees
clear normal/exception edges. molt's representation stays AOT-native (typed TIR
regions + ownership events + generated facts + the validator), not interpreter
bytecode/exception-table mechanics.

## 11. Classification + status

`bench_exception_heavy` = **RED_STABLE + CORRECTNESS/OWNERSHIP ROOT OPEN** until
ExceptionRegion Phase 1 lands. No benchmark-only speed fix that leaves the leak.
Sequence: finish the op-semantics ladder (#73 → #74) to seed #58, write this note
(done), then ExceptionRegion Phase 1 on the #58 substrate — coordinated with the
parallel session's drop-pass / round-13 work (Component B re-enables drop-insertion
reasoning over exception CFG), not colliding with it.

Related: memory/project_exception_loop_leak_baton.md (the op-level map +
preserved prototype), #58 (ownership-boundary lattice), #24
(docs/design/llvm_async_state_resume_dominance.md — StateDispatch/exception-CFG),
#46 (the generator-temp per-iteration-dead pattern Component A reuses), #TV-1
(the ownership-event validator).
