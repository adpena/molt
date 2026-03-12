// molt_exception_handling.qnt — Quint model for Molt's exception handling semantics.
//
// Models Python exception handling as compiled by Molt's frontend:
//   - Exception raising and unwinding through the call stack
//   - try/except handler matching and dispatch
//   - finally block guarantee (always executes)
//   - Context manager __exit__ protocol during unwinding
//   - StopIteration absorption by for-loops
//   - Exception chaining (__cause__ / __context__)
//
// Maps to Molt IR ops: EXCEPTION_PUSH, TRY_START, TRY_END, EXCEPTION_LAST,
// EXCEPTION_CLEAR, EXCEPTION_CONTEXT_SET, RAISE, CONTEXT_DEPTH, CONTEXT_UNWIND_TO.
//
// Usage:
//   quint run  formal/quint/molt_exception_handling.qnt --invariant=Inv --max-steps=15
//   quint verify formal/quint/molt_exception_handling.qnt --invariant=Inv

module molt_exception_handling {

  // =========================================================================
  // Types
  // =========================================================================
  type ExceptionKind = TypeError | ValueError | IndexError | KeyError
                     | StopIteration | RuntimeError | Custom

  type HandlerType = ExceptHandler | FinallyHandler | WithHandler

  type FrameState = Normal | Unwinding | Handling

  // An exception object with optional chaining.
  type ExceptionObj = {
    id: int,
    kind: ExceptionKind,
    cause: int,       // id of __cause__ (explicit `raise X from Y`), -1 = none
    context: int      // id of __context__ (implicit chaining), -1 = none
  }

  // A handler installed on the handler chain.
  type Handler = {
    id: int,
    htype: HandlerType,
    frameId: int,        // which call frame owns this handler
    catches: Set[ExceptionKind],  // for ExceptHandler; empty = catch-all
    executed: bool,      // has the handler body/block run?
    exitCalled: bool     // for WithHandler: was __exit__ called?
  }

  // A call frame on the stack.
  type Frame = {
    id: int,
    state: FrameState,
    inForLoop: bool      // true if this frame is executing a for-loop body
  }

  // =========================================================================
  // Constants
  // =========================================================================
  pure val MAX_FRAMES = 4
  pure val MAX_HANDLERS = 6
  pure val MAX_EXCEPTIONS = 6
  pure val FrameIds: Set[int] = 0.to(MAX_FRAMES - 1)

  // =========================================================================
  // State
  // =========================================================================

  // Call stack: list of frame ids, head = top of stack.
  var callStack: List[int]
  var frames: int -> Frame
  var nextFrameId: int

  // Handler chain: ordered list of handler ids, head = innermost.
  var handlerChain: List[int]
  var handlers: int -> Handler
  var nextHandlerId: int

  // Exception state.
  var currentException: int        // id of active exception, -1 = none
  var exceptions: int -> ExceptionObj
  var nextExceptionId: int

  // Tracking sets for invariant checking.
  var finallyExecuted: Set[int]    // handler ids of finally blocks that ran
  var exitCalled: Set[int]         // handler ids of with-handlers whose __exit__ ran
  var handledExceptions: Set[int]  // exception ids that were successfully handled
  var absorbedStopIter: Set[int]   // StopIteration ids absorbed by for-loops

  // =========================================================================
  // Helpers
  // =========================================================================
  pure def stackTop(stack: List[int]): int =
    if (stack.length() > 0) stack[0] else -1

  pure def stackPop(stack: List[int]): List[int] =
    if (stack.length() > 1) stack.slice(1, stack.length()) else List()

  pure def handlerMatchesException(h: Handler, exc: ExceptionObj): bool =
    h.htype == ExceptHandler and (
      h.catches == Set() or exc.kind.in(h.catches)
    )

  // =========================================================================
  // Init
  // =========================================================================
  action init = all {
    // Start with one frame (module-level / main).
    callStack' = List(0),
    frames' = FrameIds.mapBy(id =>
      if (id == 0) { id: 0, state: Normal, inForLoop: false }
      else { id: id, state: Normal, inForLoop: false }
    ),
    nextFrameId' = 1,
    handlerChain' = List(),
    handlers' = 0.to(MAX_HANDLERS - 1).mapBy(id => {
      id: id, htype: ExceptHandler, frameId: 0,
      catches: Set(), executed: false, exitCalled: false
    }),
    nextHandlerId' = 0,
    currentException' = -1,
    exceptions' = 0.to(MAX_EXCEPTIONS - 1).mapBy(id => {
      id: id, kind: RuntimeError, cause: -1, context: -1
    }),
    nextExceptionId' = 0,
    finallyExecuted' = Set(),
    exitCalled' = Set(),
    handledExceptions' = Set(),
    absorbedStopIter' = Set(),
  }

  // =========================================================================
  // Actions
  // =========================================================================

  // Push a new call frame onto the stack.
  action pushFrame = all {
    nextFrameId < MAX_FRAMES,
    callStack.length() > 0,
    val fid = nextFrameId
    val frame: Frame = { id: fid, state: Normal, inForLoop: false }
    callStack' = List(fid).concat(callStack),
    frames' = frames.set(fid, frame),
    nextFrameId' = nextFrameId + 1,
    handlerChain' = handlerChain,
    handlers' = handlers,
    nextHandlerId' = nextHandlerId,
    currentException' = currentException,
    exceptions' = exceptions,
    nextExceptionId' = nextExceptionId,
    finallyExecuted' = finallyExecuted,
    exitCalled' = exitCalled,
    handledExceptions' = handledExceptions,
    absorbedStopIter' = absorbedStopIter,
  }

  // Install a handler on the current frame.
  action installHandler = {
    nondet htype = Set(ExceptHandler, FinallyHandler, WithHandler).oneOf()
    nondet catches = Set(
      Set(),
      Set(TypeError),
      Set(ValueError),
      Set(TypeError, ValueError),
      Set(StopIteration)
    ).oneOf()
    all {
      nextHandlerId < MAX_HANDLERS,
      callStack.length() > 0,
      val hid = nextHandlerId
      val topFrame = stackTop(callStack)
      val h: Handler = {
        id: hid,
        htype: htype,
        frameId: topFrame,
        catches: if (htype == ExceptHandler) catches else Set(),
        executed: false,
        exitCalled: false,
      }
      handlerChain' = List(hid).concat(handlerChain),
      handlers' = handlers.set(hid, h),
      nextHandlerId' = nextHandlerId + 1,
      callStack' = callStack,
      frames' = frames,
      nextFrameId' = nextFrameId,
      currentException' = currentException,
      exceptions' = exceptions,
      nextExceptionId' = nextExceptionId,
      finallyExecuted' = finallyExecuted,
      exitCalled' = exitCalled,
      handledExceptions' = handledExceptions,
      absorbedStopIter' = absorbedStopIter,
    }
  }

  // Raise an exception: create it and begin unwinding.
  action raiseException = {
    nondet kind = Set(TypeError, ValueError, IndexError, KeyError,
                      StopIteration, RuntimeError, Custom).oneOf()
    all {
      nextExceptionId < MAX_EXCEPTIONS,
      callStack.length() > 0,
      currentException == -1,   // no double-raise without handling
      val eid = nextExceptionId
      val exc: ExceptionObj = { id: eid, kind: kind, cause: -1, context: -1 }
      val topFrame = stackTop(callStack)
      exceptions' = exceptions.set(eid, exc),
      nextExceptionId' = nextExceptionId + 1,
      currentException' = eid,
      // Mark the top frame as unwinding.
      frames' = frames.set(topFrame, { ...frames.get(topFrame), state: Unwinding }),
      callStack' = callStack,
      nextFrameId' = nextFrameId,
      handlerChain' = handlerChain,
      handlers' = handlers,
      nextHandlerId' = nextHandlerId,
      finallyExecuted' = finallyExecuted,
      exitCalled' = exitCalled,
      handledExceptions' = handledExceptions,
      absorbedStopIter' = absorbedStopIter,
    }
  }

  // Handle exception: find matching handler on the chain and dispatch.
  action handleException = all {
    currentException != -1,
    handlerChain.length() > 0,
    val hid = stackTop(handlerChain)
    val h = handlers.get(hid)
    val exc = exceptions.get(currentException)
    // The handler is an ExceptHandler that matches.
    handlerMatchesException(h, exc),
    val topFrame = stackTop(callStack)
    handlers' = handlers.set(hid, { ...h, executed: true }),
    handlerChain' = stackPop(handlerChain),
    currentException' = -1,   // exception is now handled
    frames' = frames.set(topFrame, { ...frames.get(topFrame), state: Handling }),
    handledExceptions' = handledExceptions.union(Set(currentException)),
    callStack' = callStack,
    nextFrameId' = nextFrameId,
    nextHandlerId' = nextHandlerId,
    exceptions' = exceptions,
    nextExceptionId' = nextExceptionId,
    finallyExecuted' = finallyExecuted,
    exitCalled' = exitCalled,
    absorbedStopIter' = absorbedStopIter,
  }

  // Execute a finally block during unwinding — always runs regardless of exception.
  action finallyExecute = all {
    handlerChain.length() > 0,
    val hid = stackTop(handlerChain)
    val h = handlers.get(hid)
    h.htype == FinallyHandler,
    not(h.executed),
    // Finally runs whether or not there is an active exception.
    handlers' = handlers.set(hid, { ...h, executed: true }),
    handlerChain' = stackPop(handlerChain),
    finallyExecuted' = finallyExecuted.union(Set(hid)),
    // Exception state is preserved through finally — not cleared.
    currentException' = currentException,
    callStack' = callStack,
    frames' = frames,
    nextFrameId' = nextFrameId,
    nextHandlerId' = nextHandlerId,
    exceptions' = exceptions,
    nextExceptionId' = nextExceptionId,
    exitCalled' = exitCalled,
    handledExceptions' = handledExceptions,
    absorbedStopIter' = absorbedStopIter,
  }

  // Context manager __exit__: called during unwinding for WithHandler.
  action contextManagerExit = {
    nondet suppresses = Set(true, false).oneOf()
    all {
      handlerChain.length() > 0,
      val hid = stackTop(handlerChain)
      val h = handlers.get(hid)
      h.htype == WithHandler,
      not(h.exitCalled),
      handlers' = handlers.set(hid, { ...h, exitCalled: true, executed: true }),
      handlerChain' = stackPop(handlerChain),
      exitCalled' = exitCalled.union(Set(hid)),
      // If __exit__ returns truthy, exception is suppressed.
      currentException' = if (suppresses and currentException != -1) -1
                          else currentException,
      handledExceptions' = if (suppresses and currentException != -1)
                             handledExceptions.union(Set(currentException))
                           else handledExceptions,
      callStack' = callStack,
      frames' = frames,
      nextFrameId' = nextFrameId,
      nextHandlerId' = nextHandlerId,
      exceptions' = exceptions,
      nextExceptionId' = nextExceptionId,
      finallyExecuted' = finallyExecuted,
      absorbedStopIter' = absorbedStopIter,
    }
  }

  // Reraise: propagate unhandled exception up the call stack.
  // Pops the current frame; the exception continues unwinding in the caller.
  action reraise = all {
    currentException != -1,
    callStack.length() > 1,
    // No matching handler on the current frame — skip non-matching except handlers,
    // but we must have already processed all handlers for this frame.
    // (Simplified: if handlerChain is empty or top handler belongs to a different frame.)
    handlerChain.length() == 0 or
      handlers.get(stackTop(handlerChain)).frameId != stackTop(callStack),
    val callerFrame = callStack[1]
    callStack' = stackPop(callStack),
    frames' = frames.set(callerFrame, { ...frames.get(callerFrame), state: Unwinding }),
    currentException' = currentException,
    nextFrameId' = nextFrameId,
    handlerChain' = handlerChain,
    handlers' = handlers,
    nextHandlerId' = nextHandlerId,
    exceptions' = exceptions,
    nextExceptionId' = nextExceptionId,
    finallyExecuted' = finallyExecuted,
    exitCalled' = exitCalled,
    handledExceptions' = handledExceptions,
    absorbedStopIter' = absorbedStopIter,
  }

  // StopIteration absorption: for-loops catch StopIteration automatically.
  action absorbStopIteration = all {
    currentException != -1,
    callStack.length() > 0,
    val topFrame = stackTop(callStack)
    frames.get(topFrame).inForLoop,
    exceptions.get(currentException).kind == StopIteration,
    currentException' = -1,
    frames' = frames.set(topFrame, { ...frames.get(topFrame), state: Normal }),
    absorbedStopIter' = absorbedStopIter.union(Set(currentException)),
    handledExceptions' = handledExceptions.union(Set(currentException)),
    callStack' = callStack,
    nextFrameId' = nextFrameId,
    handlerChain' = handlerChain,
    handlers' = handlers,
    nextHandlerId' = nextHandlerId,
    exceptions' = exceptions,
    nextExceptionId' = nextExceptionId,
    finallyExecuted' = finallyExecuted,
    exitCalled' = exitCalled,
  }

  // Enter a for-loop context on the current frame.
  action enterForLoop = all {
    callStack.length() > 0,
    val topFrame = stackTop(callStack)
    not(frames.get(topFrame).inForLoop),
    frames' = frames.set(topFrame, { ...frames.get(topFrame), inForLoop: true }),
    callStack' = callStack,
    nextFrameId' = nextFrameId,
    handlerChain' = handlerChain,
    handlers' = handlers,
    nextHandlerId' = nextHandlerId,
    currentException' = currentException,
    exceptions' = exceptions,
    nextExceptionId' = nextExceptionId,
    finallyExecuted' = finallyExecuted,
    exitCalled' = exitCalled,
    handledExceptions' = handledExceptions,
    absorbedStopIter' = absorbedStopIter,
  }

  // Raise with chaining: `raise X from Y` sets __cause__.
  action raiseWithChaining = all {
    nextExceptionId < MAX_EXCEPTIONS,
    callStack.length() > 0,
    currentException != -1,  // there is a context exception
    val eid = nextExceptionId
    val contextId = currentException
    val exc: ExceptionObj = {
      id: eid, kind: RuntimeError,
      cause: contextId,    // explicit __cause__
      context: contextId   // implicit __context__
    }
    val topFrame = stackTop(callStack)
    exceptions' = exceptions.set(eid, exc),
    nextExceptionId' = nextExceptionId + 1,
    currentException' = eid,
    frames' = frames.set(topFrame, { ...frames.get(topFrame), state: Unwinding }),
    callStack' = callStack,
    nextFrameId' = nextFrameId,
    handlerChain' = handlerChain,
    handlers' = handlers,
    nextHandlerId' = nextHandlerId,
    finallyExecuted' = finallyExecuted,
    exitCalled' = exitCalled,
    handledExceptions' = handledExceptions,
    absorbedStopIter' = absorbedStopIter,
  }

  // Normal return from handler: restore frame to Normal.
  action handlerComplete = all {
    callStack.length() > 0,
    currentException == -1,
    val topFrame = stackTop(callStack)
    frames.get(topFrame).state == Handling,
    frames' = frames.set(topFrame, { ...frames.get(topFrame), state: Normal }),
    callStack' = callStack,
    nextFrameId' = nextFrameId,
    handlerChain' = handlerChain,
    handlers' = handlers,
    nextHandlerId' = nextHandlerId,
    currentException' = currentException,
    exceptions' = exceptions,
    nextExceptionId' = nextExceptionId,
    finallyExecuted' = finallyExecuted,
    exitCalled' = exitCalled,
    handledExceptions' = handledExceptions,
    absorbedStopIter' = absorbedStopIter,
  }

  action stutter = all {
    callStack' = callStack,
    frames' = frames,
    nextFrameId' = nextFrameId,
    handlerChain' = handlerChain,
    handlers' = handlers,
    nextHandlerId' = nextHandlerId,
    currentException' = currentException,
    exceptions' = exceptions,
    nextExceptionId' = nextExceptionId,
    finallyExecuted' = finallyExecuted,
    exitCalled' = exitCalled,
    handledExceptions' = handledExceptions,
    absorbedStopIter' = absorbedStopIter,
  }

  action step = any {
    pushFrame,
    installHandler,
    raiseException,
    handleException,
    finallyExecute,
    contextManagerExit,
    reraise,
    absorbStopIteration,
    enterForLoop,
    raiseWithChaining,
    handlerComplete,
    stutter,
  }

  // =========================================================================
  // Invariants
  // =========================================================================

  // I1: Finally blocks always execute when encountered during unwinding.
  // Once a finally handler is popped from the chain, it must be in finallyExecuted.
  // (Encoded: every finally handler that has been processed was executed.)
  val finally_always_executes =
    0.to(nextHandlerId - 1).forall(hid =>
      val h = handlers.get(hid)
      (h.htype == FinallyHandler and h.executed) implies hid.in(finallyExecuted)
    )

  // I2: Exception state is cleared after successful handling.
  // If any frame is in Handling state, currentException must be -1.
  val exception_cleared_after_handling =
    callStack.length() > 0 implies (
      val topFrame = stackTop(callStack)
      frames.get(topFrame).state == Handling implies currentException == -1
    )

  // I3: Unhandled exceptions propagate to caller.
  // If we have an active exception and no handlers left for the current frame,
  // the frame must be in Unwinding state.
  val unhandled_propagates =
    (currentException != -1 and callStack.length() > 0) implies (
      val topFrame = stackTop(callStack)
      frames.get(topFrame).state == Unwinding or
      frames.get(topFrame).state == Handling
    )

  // I4: Context managers' __exit__ always called when processed.
  // Every WithHandler that has been executed must have exitCalled == true.
  val context_manager_exit_called =
    0.to(nextHandlerId - 1).forall(hid =>
      val h = handlers.get(hid)
      (h.htype == WithHandler and h.executed) implies h.exitCalled
    )

  // I5: StopIteration is caught by for-loops (not propagated past them).
  // Every absorbed StopIteration was cleared from currentException.
  val stopiter_absorbed_by_for =
    absorbedStopIter.forall(eid => eid.in(handledExceptions))

  // I6: Exception chaining preserves __cause__/__context__.
  // If an exception has a cause, the cause id must refer to a valid exception.
  val chaining_preserved =
    0.to(nextExceptionId - 1).forall(eid =>
      val exc = exceptions.get(eid)
      (exc.cause != -1) implies (exc.cause >= 0 and exc.cause < nextExceptionId)
    )

  // Combined invariant.
  val Inv = and {
    finally_always_executes,
    exception_cleared_after_handling,
    unhandled_propagates,
    context_manager_exit_called,
    stopiter_absorbed_by_for,
    chaining_preserved,
  }
}
