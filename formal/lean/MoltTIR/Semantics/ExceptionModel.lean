/-
  MoltTIR.Semantics.ExceptionModel — exception handling semantics for Molt TIR.

  Extends the core execution model with raise/try-catch/propagation semantics.
  This is a standalone module: it imports Syntax and Types but does not modify
  ExecBlock, ExecFunc, or State.

  Models only the core semantics:
    - raise: produce an exception outcome
    - try/catch: intercept exceptions and run a handler
    - propagation: uncaught exceptions pass through sequential blocks

  Deliberately out of scope: finally, exception chaining, __context__/__cause__,
  bare except, exception groups (PEP 654).
-/
import MoltTIR.Syntax
import MoltTIR.Types

namespace MoltTIR

/-! ## Core exception types -/

/-- An exception value carrying a kind (class name) and a human-readable message.
    Corresponds to Molt's `SideEffectOp.raise` raising a runtime exception.
    In the real runtime this is a NaN-boxed pointer to an exception object;
    here we model only the two fields needed for matching semantics. -/
structure ExceptionValue where
  kind    : String   -- e.g. "TypeError", "ValueError", "KeyError"
  message : String
  deriving DecidableEq, Repr

/-- Execution outcome that distinguishes normal return, exception, and divergence.
    This refines `Outcome` from State.lean without replacing it — code that does
    not need exception semantics can continue using the simpler `Outcome` type. -/
inductive ExecOutcome where
  | ok        (v : Value)            -- normal completion with a value
  | exception (e : ExceptionValue)   -- an unhandled exception was raised
  | diverge                          -- non-termination (replaces fuel exhaustion)
  deriving Repr

/-- Exception context threaded through block execution.
    `active` holds the currently propagating exception (if any).
    `handlers` is a stack of catch-label targets (innermost first) — not used
    in the denotational model below, but useful if we later model TIR-level
    exception dispatch tables. -/
structure ExceptionState where
  active   : Option ExceptionValue := none
  handlers : List Label            := []
  deriving Repr

/-! ## Primitive operations -/

/-- Raise an exception, producing an `ExecOutcome.exception`. -/
def raiseException (e : ExceptionValue) : ExecOutcome :=
  .exception e

/-- Inspect the exception state for an active exception. -/
def checkException (st : ExceptionState) : Option ExceptionValue :=
  st.active

/-- Clear the active exception (used after a successful catch). -/
def clearException (st : ExceptionState) : ExceptionState :=
  { st with active := none }

/-- Set an active exception in the state (used when `raise` is executed). -/
def setException (st : ExceptionState) (e : ExceptionValue) : ExceptionState :=
  { st with active := some e }

/-! ## Try / catch combinator

  `tryBlock body handler` models Python's:
      try:
          <body>
      except <kind> as e:
          <handler e>

  Semantics:
    - If `body` returns `ok v`, the result is `ok v` and the handler is never called.
    - If `body` returns `exception e`, the handler is invoked with `e`.
    - If `body` diverges, the whole try-block diverges.
-/

/-- Execute a try-block: run the body; if it raises, delegate to the handler. -/
def tryBlock (body : ExecOutcome) (handler : ExceptionValue → ExecOutcome) : ExecOutcome :=
  match body with
  | .ok v        => .ok v
  | .exception e => handler e
  | .diverge     => .diverge

/-! ## Block-level propagation

  In a sequence of blocks `b₁ ; b₂`, if `b₁` raises an exception the remaining
  blocks are skipped and the exception propagates outward.  This models the
  "early exit" semantics of Python's implicit exception propagation.
-/

/-- Sequence two outcomes: if the first succeeds, feed its value to the
    continuation; otherwise propagate the exception or divergence. -/
def seqOutcome (first : ExecOutcome) (cont : Value → ExecOutcome) : ExecOutcome :=
  match first with
  | .ok v        => cont v
  | .exception e => .exception e
  | .diverge     => .diverge

/-! ## Kind-based catch (match on exception class name) -/

/-- A handler that only catches exceptions of a specific kind, re-raising others.
    Models `except TypeError as e: ...` where non-matching exceptions propagate. -/
def catchByKind (kind : String) (handler : ExceptionValue → ExecOutcome)
    (e : ExceptionValue) : ExecOutcome :=
  if e.kind == kind then handler e
  else .exception e

/-- Convenience: try-block with a kind-restricted handler. -/
def tryCatchKind (body : ExecOutcome) (kind : String)
    (handler : ExceptionValue → ExecOutcome) : ExecOutcome :=
  tryBlock body (catchByKind kind handler)

/-! ## Properties -/

/-- If the body completes normally, the handler is never invoked. -/
theorem try_catch_normal (v : Value) (handler : ExceptionValue → ExecOutcome) :
    tryBlock (.ok v) handler = .ok v := by
  rfl

/-- If the body raises, the handler receives the exception. -/
theorem try_catch_exception (e : ExceptionValue) (handler : ExceptionValue → ExecOutcome) :
    tryBlock (.exception e) handler = handler e := by
  rfl

/-- If the body diverges, the try-block diverges regardless of the handler. -/
theorem try_catch_diverge (handler : ExceptionValue → ExecOutcome) :
    tryBlock .diverge handler = .diverge := by
  rfl

/-- Uncaught exceptions propagate through sequential composition. -/
theorem exception_propagation (e : ExceptionValue) (cont : Value → ExecOutcome) :
    seqOutcome (.exception e) cont = .exception e := by
  rfl

/-- Divergence propagates through sequential composition. -/
theorem diverge_propagation (cont : Value → ExecOutcome) :
    seqOutcome .diverge cont = .diverge := by
  rfl

/-- Normal completion threads through sequential composition. -/
theorem seq_ok (v : Value) (cont : Value → ExecOutcome) :
    seqOutcome (.ok v) cont = cont v := by
  rfl

/-- A kind-restricted handler re-raises non-matching exceptions. -/
theorem catch_kind_mismatch (kind : String) (handler : ExceptionValue → ExecOutcome)
    (e : ExceptionValue) (h : e.kind ≠ kind) :
    catchByKind kind handler e = .exception e := by
  unfold catchByKind
  simp [bne_iff_ne, beq_iff_eq, h]

/-- A kind-restricted handler invokes the handler on matching exceptions. -/
theorem catch_kind_match (kind : String) (handler : ExceptionValue → ExecOutcome)
    (e : ExceptionValue) (h : e.kind = kind) :
    catchByKind kind handler e = handler e := by
  unfold catchByKind
  simp [beq_iff_eq, h]

/-- Try-catch distributes over sequential composition when the handler
    always produces a terminal result (ok or exception, never feeding
    back into the continuation). This is the common case: catch blocks
    return a recovery value or re-raise. -/
theorem try_seq_ok_distributes (v : Value) (cont : Value → ExecOutcome)
    (handler : ExceptionValue → ExecOutcome) :
    tryBlock (seqOutcome (.ok v) cont) handler =
    tryBlock (cont v) handler := by
  simp [tryBlock, seqOutcome]

/-- If the first part of a sequence raises, the continuation is skipped
    and try-catch handles the exception directly. -/
theorem try_seq_exception (e : ExceptionValue) (cont : Value → ExecOutcome)
    (handler : ExceptionValue → ExecOutcome) :
    tryBlock (seqOutcome (.exception e) cont) handler = handler e := by
  simp [tryBlock, seqOutcome]

/-- raiseException always produces an exception outcome. -/
theorem raise_is_exception (e : ExceptionValue) :
    raiseException e = .exception e := by
  rfl

/-- checkException returns the active exception when one is set. -/
theorem check_set_exception (st : ExceptionState) (e : ExceptionValue) :
    checkException (setException st e) = some e := by
  rfl

/-- checkException returns none after clearing. -/
theorem check_clear_exception (st : ExceptionState) :
    checkException (clearException st) = none := by
  rfl

/-- Nested try-blocks: inner handler takes priority. -/
theorem nested_try (body : ExecOutcome) (inner outer : ExceptionValue → ExecOutcome) :
    tryBlock (tryBlock body inner) outer =
    tryBlock body (fun e => tryBlock (inner e) outer) := by
  cases body with
  | ok v => simp [tryBlock]
  | exception e => simp [tryBlock]
  | diverge => simp [tryBlock]

end MoltTIR
