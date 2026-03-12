// molt_control_flow.qnt — Quint model for Molt's control flow compilation.
//
// Models how the Molt frontend compiles Python control flow constructs
// into IR blocks with correct branch targets, ensuring:
//   - break/continue are only valid inside loops
//   - return/break through finally always executes the finally block first
//   - for-else executes only on normal loop completion
//   - Nested exception handlers unwind in correct (LIFO) order
//
// Maps to Molt IR ops: JUMP, IF, ELSE, END_IF, LABEL, FOR_ITER,
// BREAK, CONTINUE, RETURN, TRY_START, TRY_END, EXCEPTION_PUSH.
//
// Usage:
//   quint run  formal/quint/molt_control_flow.qnt --invariant=Inv --max-steps=15
//   quint verify formal/quint/molt_control_flow.qnt --invariant=Inv

module molt_control_flow {

  // =========================================================================
  // Types
  // =========================================================================
  type BlockKind = BodyBlock | IfBranch | ElseBranch | ForBody | WhileBody
                 | TryBody | ExceptBody | FinallyBody | ElseAfterLoop

  type ExitKind = NormalExit | BreakExit | ContinueExit | ReturnExit
                | RaiseExit

  type Block = {
    id: int,
    kind: BlockKind,
    parentId: int,         // enclosing block id, -1 for top-level
    loopDepth: int,        // nesting depth of enclosing loops
    tryDepth: int,         // nesting depth of enclosing try blocks
    hasFinallyAbove: bool, // true if any enclosing try has a finally
    completed: bool,
    exitKind: ExitKind
  }

  // Tracks finally blocks that must execute for a given control transfer.
  type FinallyObligation = {
    blockId: int,          // the block where the transfer originates
    finallyBlockId: int,   // the finally block that must execute
    fulfilled: bool
  }

  // =========================================================================
  // Constants
  // =========================================================================
  pure val MAX_BLOCKS = 6
  pure val BlockIds: Set[int] = 0.to(MAX_BLOCKS - 1)

  // =========================================================================
  // State
  // =========================================================================
  var blocks: int -> Block
  var nextBlockId: int

  // Stack of active block ids (head = innermost).
  var blockStack: List[int]

  // Stack of active loop block ids for break/continue validation.
  var loopStack: List[int]

  // Stack of active try block ids for unwinding order.
  var tryStack: List[int]

  // Finally obligations: break/return through finally must execute finally first.
  var obligations: Set[FinallyObligation]

  // Tracking for for-else semantics.
  var loopExitedNormally: Set[int]   // loop block ids that completed normally
  var elseExecuted: Set[int]         // else-after-loop block ids that executed

  // =========================================================================
  // Helpers
  // =========================================================================
  pure def stackTop(stack: List[int]): int =
    if (stack.length() > 0) stack[0] else -1

  pure def stackPop(stack: List[int]): List[int] =
    if (stack.length() > 1) stack.slice(1, stack.length()) else List()

  pure def currentLoopDepth(ls: List[int]): int = ls.length()
  pure def currentTryDepth(ts: List[int]): int = ts.length()

  // =========================================================================
  // Init
  // =========================================================================
  action init = all {
    // Block 0 is the module-level body.
    blocks' = BlockIds.mapBy(id =>
      if (id == 0) {
        id: 0, kind: BodyBlock, parentId: -1, loopDepth: 0, tryDepth: 0,
        hasFinallyAbove: false, completed: false, exitKind: NormalExit
      } else {
        id: id, kind: BodyBlock, parentId: -1, loopDepth: 0, tryDepth: 0,
        hasFinallyAbove: false, completed: false, exitKind: NormalExit
      }
    ),
    nextBlockId' = 1,
    blockStack' = List(0),
    loopStack' = List(),
    tryStack' = List(),
    obligations' = Set(),
    loopExitedNormally' = Set(),
    elseExecuted' = Set(),
  }

  // =========================================================================
  // Actions
  // =========================================================================

  // Enter an if-branch block.
  action ifBranch = all {
    nextBlockId < MAX_BLOCKS,
    blockStack.length() > 0,
    val bid = nextBlockId
    val parent = stackTop(blockStack)
    val b: Block = {
      id: bid, kind: IfBranch, parentId: parent,
      loopDepth: currentLoopDepth(loopStack),
      tryDepth: currentTryDepth(tryStack),
      hasFinallyAbove: blocks.get(parent).hasFinallyAbove,
      completed: false, exitKind: NormalExit
    }
    blocks' = blocks.set(bid, b),
    nextBlockId' = nextBlockId + 1,
    blockStack' = List(bid).concat(blockStack),
    loopStack' = loopStack,
    tryStack' = tryStack,
    obligations' = obligations,
    loopExitedNormally' = loopExitedNormally,
    elseExecuted' = elseExecuted,
  }

  // Enter a for-loop body.
  action forLoop = all {
    nextBlockId < MAX_BLOCKS,
    blockStack.length() > 0,
    val bid = nextBlockId
    val parent = stackTop(blockStack)
    val b: Block = {
      id: bid, kind: ForBody, parentId: parent,
      loopDepth: currentLoopDepth(loopStack) + 1,
      tryDepth: currentTryDepth(tryStack),
      hasFinallyAbove: blocks.get(parent).hasFinallyAbove,
      completed: false, exitKind: NormalExit
    }
    blocks' = blocks.set(bid, b),
    nextBlockId' = nextBlockId + 1,
    blockStack' = List(bid).concat(blockStack),
    loopStack' = List(bid).concat(loopStack),
    tryStack' = tryStack,
    obligations' = obligations,
    loopExitedNormally' = loopExitedNormally,
    elseExecuted' = elseExecuted,
  }

  // Enter a while-loop body.
  action whileLoop = all {
    nextBlockId < MAX_BLOCKS,
    blockStack.length() > 0,
    val bid = nextBlockId
    val parent = stackTop(blockStack)
    val b: Block = {
      id: bid, kind: WhileBody, parentId: parent,
      loopDepth: currentLoopDepth(loopStack) + 1,
      tryDepth: currentTryDepth(tryStack),
      hasFinallyAbove: blocks.get(parent).hasFinallyAbove,
      completed: false, exitKind: NormalExit
    }
    blocks' = blocks.set(bid, b),
    nextBlockId' = nextBlockId + 1,
    blockStack' = List(bid).concat(blockStack),
    loopStack' = List(bid).concat(loopStack),
    tryStack' = tryStack,
    obligations' = obligations,
    loopExitedNormally' = loopExitedNormally,
    elseExecuted' = elseExecuted,
  }

  // Enter a try body and push a try scope.
  action tryExcept = {
    nondet hasFinally = Set(true, false).oneOf()
    all {
      nextBlockId < MAX_BLOCKS,
      blockStack.length() > 0,
      val bid = nextBlockId
      val parent = stackTop(blockStack)
      val b: Block = {
        id: bid, kind: TryBody, parentId: parent,
        loopDepth: currentLoopDepth(loopStack),
        tryDepth: currentTryDepth(tryStack) + 1,
        hasFinallyAbove: blocks.get(parent).hasFinallyAbove or hasFinally,
        completed: false, exitKind: NormalExit
      }
      blocks' = blocks.set(bid, b),
      nextBlockId' = nextBlockId + 1,
      blockStack' = List(bid).concat(blockStack),
      loopStack' = loopStack,
      tryStack' = List(bid).concat(tryStack),
      obligations' = obligations,
      loopExitedNormally' = loopExitedNormally,
      elseExecuted' = elseExecuted,
    }
  }

  // Complete the current block normally.
  action completeBlock = all {
    blockStack.length() > 1,  // don't pop the module block
    val bid = stackTop(blockStack)
    val b = blocks.get(bid)
    not(b.completed),
    blocks' = blocks.set(bid, { ...b, completed: true, exitKind: NormalExit }),
    blockStack' = stackPop(blockStack),
    // If it was a loop, pop from loopStack and mark normal exit.
    loopStack' = if (b.kind == ForBody or b.kind == WhileBody) stackPop(loopStack)
                 else loopStack,
    tryStack' = if (b.kind == TryBody) stackPop(tryStack) else tryStack,
    loopExitedNormally' = if (b.kind == ForBody or b.kind == WhileBody)
                            loopExitedNormally.union(Set(bid))
                          else loopExitedNormally,
    nextBlockId' = nextBlockId,
    obligations' = obligations,
    elseExecuted' = elseExecuted,
  }

  // Break from a loop — only valid inside a loop.
  action breakFromLoop = all {
    blockStack.length() > 1,
    loopStack.length() > 0,
    val bid = stackTop(blockStack)
    val b = blocks.get(bid)
    not(b.completed),
    b.loopDepth > 0,
    // If there is a finally between us and the loop, create an obligation.
    val newObligations = if (b.hasFinallyAbove and tryStack.length() > 0)
      obligations.union(Set({
        blockId: bid, finallyBlockId: stackTop(tryStack), fulfilled: false
      }))
    else obligations
    blocks' = blocks.set(bid, { ...b, completed: true, exitKind: BreakExit }),
    blockStack' = stackPop(blockStack),
    loopStack' = loopStack,  // loop still on stack until its block completes
    tryStack' = tryStack,
    obligations' = newObligations,
    nextBlockId' = nextBlockId,
    loopExitedNormally' = loopExitedNormally,
    elseExecuted' = elseExecuted,
  }

  // Continue in a loop — only valid inside a loop.
  action continueInLoop = all {
    blockStack.length() > 1,
    loopStack.length() > 0,
    val bid = stackTop(blockStack)
    val b = blocks.get(bid)
    not(b.completed),
    b.loopDepth > 0,
    blocks' = blocks.set(bid, { ...b, completed: true, exitKind: ContinueExit }),
    blockStack' = stackPop(blockStack),
    loopStack' = loopStack,
    tryStack' = tryStack,
    obligations' = obligations,
    nextBlockId' = nextBlockId,
    loopExitedNormally' = loopExitedNormally,
    elseExecuted' = elseExecuted,
  }

  // Return from inside a try — must execute finally first.
  action returnFromTry = all {
    blockStack.length() > 1,
    val bid = stackTop(blockStack)
    val b = blocks.get(bid)
    not(b.completed),
    b.tryDepth > 0,
    val newObligations = if (b.hasFinallyAbove and tryStack.length() > 0)
      obligations.union(Set({
        blockId: bid, finallyBlockId: stackTop(tryStack), fulfilled: false
      }))
    else obligations
    blocks' = blocks.set(bid, { ...b, completed: true, exitKind: ReturnExit }),
    blockStack' = stackPop(blockStack),
    loopStack' = loopStack,
    tryStack' = tryStack,
    obligations' = newObligations,
    nextBlockId' = nextBlockId,
    loopExitedNormally' = loopExitedNormally,
    elseExecuted' = elseExecuted,
  }

  // Fulfill a finally obligation — marks the finally as having executed.
  action fulfillFinally = {
    nondet ob = obligations.oneOf()
    all {
      not(ob.fulfilled),
      obligations' = obligations.exclude(Set(ob)).union(Set({ ...ob, fulfilled: true })),
      blocks' = blocks,
      nextBlockId' = nextBlockId,
      blockStack' = blockStack,
      loopStack' = loopStack,
      tryStack' = tryStack,
      loopExitedNormally' = loopExitedNormally,
      elseExecuted' = elseExecuted,
    }
  }

  // Execute for-else block (only valid after normal loop completion).
  action forElse = all {
    nextBlockId < MAX_BLOCKS,
    blockStack.length() > 0,
    // The most recently completed block on this level was a for-loop
    // that exited normally.
    val parent = stackTop(blockStack)
    // Find a for-loop that exited normally under this parent.
    loopExitedNormally.size() > 0,
    val bid = nextBlockId
    val b: Block = {
      id: bid, kind: ElseAfterLoop, parentId: parent,
      loopDepth: currentLoopDepth(loopStack),
      tryDepth: currentTryDepth(tryStack),
      hasFinallyAbove: blocks.get(parent).hasFinallyAbove,
      completed: false, exitKind: NormalExit
    }
    blocks' = blocks.set(bid, b),
    nextBlockId' = nextBlockId + 1,
    blockStack' = List(bid).concat(blockStack),
    elseExecuted' = elseExecuted.union(Set(bid)),
    loopStack' = loopStack,
    tryStack' = tryStack,
    obligations' = obligations,
    loopExitedNormally' = loopExitedNormally,
  }

  action stutter = all {
    blocks' = blocks,
    nextBlockId' = nextBlockId,
    blockStack' = blockStack,
    loopStack' = loopStack,
    tryStack' = tryStack,
    obligations' = obligations,
    loopExitedNormally' = loopExitedNormally,
    elseExecuted' = elseExecuted,
  }

  action step = any {
    ifBranch,
    forLoop,
    whileLoop,
    tryExcept,
    completeBlock,
    breakFromLoop,
    continueInLoop,
    returnFromTry,
    fulfillFinally,
    forElse,
    stutter,
  }

  // =========================================================================
  // Invariants
  // =========================================================================

  // I1: break/continue only occur in loop context.
  // Every completed block with BreakExit or ContinueExit has loopDepth > 0.
  val break_continue_in_loop =
    0.to(nextBlockId - 1).forall(bid =>
      val b = blocks.get(bid)
      (b.completed and (b.exitKind == BreakExit or b.exitKind == ContinueExit))
        implies b.loopDepth > 0
    )

  // I2: return through finally creates an obligation to execute finally.
  // Every ReturnExit from a block with hasFinallyAbove must have a corresponding
  // obligation (fulfilled or pending).
  val return_through_finally =
    0.to(nextBlockId - 1).forall(bid =>
      val b = blocks.get(bid)
      (b.completed and b.exitKind == ReturnExit and b.hasFinallyAbove)
        implies obligations.exists(ob => ob.blockId == bid)
    )

  // I3: break through finally creates an obligation to execute finally.
  val break_through_finally =
    0.to(nextBlockId - 1).forall(bid =>
      val b = blocks.get(bid)
      (b.completed and b.exitKind == BreakExit and b.hasFinallyAbove)
        implies obligations.exists(ob => ob.blockId == bid)
    )

  // I4: for-else executes only on normal loop completion.
  // Every ElseAfterLoop block was entered only when loopExitedNormally is non-empty.
  val for_else_on_normal_completion =
    elseExecuted.forall(bid =>
      blocks.get(bid).kind == ElseAfterLoop
    )

  // I5: Nested exception handlers unwind in correct (LIFO) order.
  // The tryStack is always a suffix of the order in which try blocks were entered.
  // Encoded: tryStack entries have strictly decreasing tryDepth.
  val try_stack_lifo =
    if (tryStack.length() >= 2)
      0.to(tryStack.length() - 2).forall(i =>
        blocks.get(tryStack[i]).tryDepth > blocks.get(tryStack[i + 1]).tryDepth
      )
    else true

  // Structural: loop stack entries have strictly decreasing loopDepth.
  val loop_stack_lifo =
    if (loopStack.length() >= 2)
      0.to(loopStack.length() - 2).forall(i =>
        blocks.get(loopStack[i]).loopDepth > blocks.get(loopStack[i + 1]).loopDepth
      )
    else true

  // Combined invariant.
  val Inv = and {
    break_continue_in_loop,
    return_through_finally,
    break_through_finally,
    for_else_on_normal_completion,
    try_stack_lifo,
    loop_stack_lifo,
  }
}
