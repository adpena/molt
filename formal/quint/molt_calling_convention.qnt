// molt_calling_convention.qnt — Quint model for Molt's function calling convention.
//
// Models the Luau backend's function reference and call-site mechanics:
//   - Builtin, user-defined, and closure function references
//   - The args-tuple calling convention used by call_func IR ops
//   - The builtin_func IR op that creates function references
//
// Prevents regressions like the builtin_func nil bug where builtins
// (max, min, round, etc.) emitted nil instead of callable references,
// causing runtime crashes when called via the args-tuple convention.
//
// Usage:
//   quint run  formal/quint/molt_calling_convention.qnt --invariant=Inv --max-steps=15
//   quint verify formal/quint/molt_calling_convention.qnt --invariant=Inv

module molt_calling_convention {

  // =========================================================================
  // Types
  // =========================================================================
  type FuncKind = Builtin | UserDefined | Closure
  type Convention = Direct | ArgsTuple | Kwargs

  type FuncRef = {
    id: int,
    kind: FuncKind,
    convention: Convention,
    isNil: bool,           // true = reference resolved to nil (the bug)
    hasWrapper: bool       // true = wrapped in args-tuple-compatible closure
  }

  type CallSite = {
    callId: int,
    calleeId: int,
    argsAsTuple: bool,     // true = args passed as tuple in args[0]
    hasKwargs: bool
  }

  type CallResult = {
    callId: int,
    success: bool
  }

  // =========================================================================
  // Constants
  // =========================================================================
  pure val MAX_FUNCS = 6
  pure val MAX_CALLS = 8
  pure val FuncIds: Set[int] = 0.to(MAX_FUNCS - 1)

  // Builtin IDs: 0=max, 1=min, 2=round, 3=len, 4=abs
  pure val BUILTIN_IDS: Set[int] = Set(0, 1, 2, 3, 4)
  // User-defined function ID
  pure val USER_IDS: Set[int] = Set(5)

  // =========================================================================
  // Pure functions
  // =========================================================================

  // A call succeeds iff: callee is not nil AND conventions are compatible.
  // The args-tuple convention requires the callee to accept a tuple in args[0].
  // Builtins need a wrapper to accept the args-tuple convention.
  pure def callSucceeds(func: FuncRef, site: CallSite): bool = and {
    not(func.isNil),
    // If caller passes args as tuple, callee must accept args-tuple or be wrapped
    site.argsAsTuple implies (
      func.convention == ArgsTuple or func.hasWrapper
    ),
    // Direct calls require direct or args-tuple convention
    not(site.argsAsTuple) implies (
      func.convention == Direct or func.convention == ArgsTuple
    ),
  }

  // =========================================================================
  // State
  // =========================================================================
  var funcs: int -> FuncRef
  var registeredIds: Set[int]
  var callSites: Set[CallSite]
  var callResults: Set[CallResult]
  var nextCallId: int

  // =========================================================================
  // Init
  // =========================================================================
  action init = all {
    funcs' = FuncIds.mapBy(id => {
      id: id,
      kind: Builtin,
      convention: Direct,
      isNil: true,
      hasWrapper: false,
    }),
    registeredIds' = Set(),
    callSites' = Set(),
    callResults' = Set(),
    nextCallId' = 0,
  }

  // =========================================================================
  // Actions
  // =========================================================================

  // Register a builtin function with args-tuple convention (the correct fix).
  // Builtins are wrapped in closures: function(a, ...) return math.max(table.unpack(a)) end
  action registerBuiltin = {
    nondet name = oneOf(BUILTIN_IDS)
    all {
      not(name.in(registeredIds)),
      funcs' = funcs.set(name, {
        id: name,
        kind: Builtin,
        convention: ArgsTuple,
        isNil: false,
        hasWrapper: true,
      }),
      registeredIds' = registeredIds.union(Set(name)),
      callSites' = callSites,
      callResults' = callResults,
      nextCallId' = nextCallId,
    }
  }

  // Register a user-defined function.
  // In Molt, all user-defined functions compiled to Luau use the args-tuple
  // convention: they accept (args_tuple, ...) and unpack internally.
  action registerUser = {
    nondet name = oneOf(USER_IDS)
    all {
      not(name.in(registeredIds)),
      funcs' = funcs.set(name, {
        id: name,
        kind: UserDefined,
        convention: ArgsTuple,
        isNil: false,
        hasWrapper: false,
      }),
      registeredIds' = registeredIds.union(Set(name)),
      callSites' = callSites,
      callResults' = callResults,
      nextCallId' = nextCallId,
    }
  }

  // Emit a builtin_func IR op — creates a function reference for a builtin.
  // In the buggy version, this produced nil. In the fixed version, it produces
  // a non-nil closure wrapper.
  action emitBuiltinFunc = {
    nondet name = oneOf(BUILTIN_IDS)
    all {
      name.in(registeredIds),
      val current = funcs.get(name)
      funcs' = funcs.set(name, { ...current, isNil: false, hasWrapper: true }),
      registeredIds' = registeredIds,
      callSites' = callSites,
      callResults' = callResults,
      nextCallId' = nextCallId,
    }
  }

  // Emit a call_func IR op — function call via the args-tuple convention.
  // call_func passes (callable, args_tuple, kwargs, varkw).
  action emitCall = {
    nondet calleeId = oneOf(FuncIds)
    all {
      calleeId.in(registeredIds),
      nextCallId < MAX_CALLS,
      val func = funcs.get(calleeId)
      val site: CallSite = {
        callId: nextCallId,
        calleeId: calleeId,
        argsAsTuple: true,
        hasKwargs: false,
      }
      val success = callSucceeds(func, site)
      all {
        funcs' = funcs,
        registeredIds' = registeredIds,
        callSites' = callSites.union(Set(site)),
        callResults' = callResults.union(Set({ callId: nextCallId, success: success })),
        nextCallId' = nextCallId + 1,
      }
    }
  }

  // Emit a direct call (not via call_func, e.g. inlined builtin).
  action emitDirectCall = {
    nondet calleeId = oneOf(FuncIds)
    all {
      calleeId.in(registeredIds),
      nextCallId < MAX_CALLS,
      val func = funcs.get(calleeId)
      val site: CallSite = {
        callId: nextCallId,
        calleeId: calleeId,
        argsAsTuple: false,
        hasKwargs: false,
      }
      val success = callSucceeds(func, site)
      all {
        funcs' = funcs,
        registeredIds' = registeredIds,
        callSites' = callSites.union(Set(site)),
        callResults' = callResults.union(Set({ callId: nextCallId, success: success })),
        nextCallId' = nextCallId + 1,
      }
    }
  }

  action stutter = all {
    funcs' = funcs,
    registeredIds' = registeredIds,
    callSites' = callSites,
    callResults' = callResults,
    nextCallId' = nextCallId,
  }

  action step = any {
    registerBuiltin,
    registerUser,
    emitBuiltinFunc,
    emitCall,
    emitDirectCall,
    stutter,
  }

  // =========================================================================
  // Invariants
  // =========================================================================

  // I1: No registered function reference resolves to nil.
  // This is the core invariant that the builtin_func nil bug violated.
  val no_nil_callables =
    registeredIds.forall(id => not(funcs.get(id).isNil))

  // I2: Every call succeeds — no convention mismatch at any call site.
  val convention_match =
    callResults.forall(r => r.success)

  // I3: All registered builtins have non-nil implementations.
  val builtin_coverage =
    BUILTIN_IDS.forall(id =>
      id.in(registeredIds) implies not(funcs.get(id).isNil)
    )

  // I4: Args-tuple calls always target a callee that can handle tuples.
  // Either the callee's convention is ArgsTuple, or it has a wrapper.
  val call_args_valid =
    callSites.forall(site =>
      site.argsAsTuple implies (
        val func = funcs.get(site.calleeId)
        func.convention == ArgsTuple or func.hasWrapper
      )
    )

  // I5: Registered builtins always have wrapper closures for args-tuple compat.
  // This encodes the fix: every builtin emitted via builtin_func gets wrapped.
  val builtins_wrapped =
    registeredIds.forall(id =>
      funcs.get(id).kind == Builtin implies funcs.get(id).hasWrapper
    )

  // I6: No call result records a failure.
  val all_calls_succeed =
    callResults.forall(r => r.success)

  // Combined invariant.
  val Inv = and {
    no_nil_callables,
    convention_match,
    builtin_coverage,
    call_args_valid,
    builtins_wrapped,
    all_calls_succeed,
  }
}
