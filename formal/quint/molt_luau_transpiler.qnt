// molt_luau_transpiler.qnt — Quint model for Molt Luau transpiler correctness.
//
// Models the critical semantic gaps the Luau backend must bridge when
// transpiling Python IR to Luau source (~4300 lines in luau.rs):
//   - 0-based -> 1-based index adjustment
//   - Builtin function resolution (no nil callables)
//   - Module import caching
//   - Prelude helper emission completeness
//   - Local variable declaration tracking
//
// The transpiler operates in three phases matching the real code:
//   Phase 0 (IR scan):   nondeterministically determine which builtins, vars,
//                        and modules the IR program uses
//   Phase 1 (preamble):  emit prelude helpers, module imports, local declarations
//                        for everything discovered in the scan
//   Phase 2 (body):      emit index accesses, builtin refs, function calls
//
// Usage:
//   quint run  formal/quint/molt_luau_transpiler.qnt --invariant=Inv --max-steps=15
//   quint verify formal/quint/molt_luau_transpiler.qnt --invariant=Inv

module molt_luau_transpiler {

  // =========================================================================
  // Types
  // =========================================================================
  type VarName = str
  type BuiltinName = str
  type ModuleName = str
  type HelperName = str

  type IndexAccess = { var_name: VarName, original_index: int, emitted_index: int }

  // =========================================================================
  // Constants (bounded for tractable model checking)
  // =========================================================================

  // Universe of variable names the transpiler may encounter.
  pure val AllVars: Set[VarName] = Set("x", "y", "z", "w")

  // Universe of builtin function names and their Luau mappings.
  // Models builtinMap in luau.rs: Python builtin -> Luau implementation.
  pure val AllBuiltins: Set[BuiltinName] = Set("len", "range", "print", "type", "int")

  // Map each Python builtin to its Luau implementation name.
  // An empty string means unmapped (would produce nil callable at runtime).
  pure def builtinLuau(name: BuiltinName): str =
    if (name == "len") "molt_len"
    else if (name == "range") "molt_range"
    else if (name == "print") "print"
    else if (name == "type") "molt_type"
    else if (name == "int") "molt_int"
    else ""

  // Universe of modules.
  pure val AllModules: Set[ModuleName] = Set("math", "os", "json")

  // Universe of prelude helpers.
  pure val AllHelpers: Set[HelperName] = Set("molt_len", "molt_range", "molt_type", "molt_int", "molt_iter")

  // Determine which prelude helper a builtin requires (empty if none needed).
  pure def requiredHelper(name: BuiltinName): str = builtinLuau(name)

  // =========================================================================
  // State
  // =========================================================================

  // Emission phase: 0 = IR scan, 1 = preamble, 2 = body.
  var phase: int

  // The current indexing base: Python IR uses 0, emitted Luau must use 1.
  var indexBase: int

  // --- IR scan results (populated in phase 0) ---
  // Which vars, builtins, and modules the IR program needs.
  var irVars: Set[VarName]
  var irBuiltins: Set[BuiltinName]
  var irModules: Set[ModuleName]

  // --- Emission tracking (populated in phases 1-2) ---
  // Variables that have been declared with `local`.
  var emittedVars: Set[VarName]

  // Modules that have been loaded into the require cache.
  var moduleCache: Set[ModuleName]

  // Prelude helper functions that have been emitted at top of output.
  var preludeHelpers: Set[HelperName]

  // Track all index accesses emitted (for invariant checking).
  var indexAccesses: Set[IndexAccess]

  // Set of callable references that resolved to nil (should always be empty).
  var nilCallables: Set[BuiltinName]

  // Set of builtins that have been referenced during body emission.
  var referencedBuiltins: Set[BuiltinName]

  // Set of helpers that have been used in emitted code.
  var usedHelpers: Set[HelperName]

  // Set of variables that have been referenced in emitted code.
  var referencedVars: Set[VarName]

  // Set of modules that have been referenced in body code.
  var referencedModules: Set[ModuleName]

  // =========================================================================
  // Init
  // =========================================================================
  action init = all {
    phase' = 0,
    indexBase' = 1,  // Luau target: 1-based indexing
    irVars' = Set(),
    irBuiltins' = Set(),
    irModules' = Set(),
    emittedVars' = Set(),
    moduleCache' = Set(),
    preludeHelpers' = Set(),
    indexAccesses' = Set(),
    nilCallables' = Set(),
    referencedBuiltins' = Set(),
    usedHelpers' = Set(),
    referencedVars' = Set(),
    referencedModules' = Set(),
  }

  // =========================================================================
  // Phase 0: IR Scan — discover what the program needs
  // =========================================================================

  // Discover a variable used in the IR.
  action scanVar = {
    nondet name = AllVars.oneOf()
    all {
      irVars' = irVars.union(Set(name)),
      phase' = phase,
      indexBase' = indexBase,
      irBuiltins' = irBuiltins,
      irModules' = irModules,
      emittedVars' = emittedVars,
      moduleCache' = moduleCache,
      preludeHelpers' = preludeHelpers,
      indexAccesses' = indexAccesses,
      nilCallables' = nilCallables,
      referencedBuiltins' = referencedBuiltins,
      usedHelpers' = usedHelpers,
      referencedVars' = referencedVars,
      referencedModules' = referencedModules,
    }
  }

  // Discover a builtin used in the IR.
  action scanBuiltin = {
    nondet name = AllBuiltins.oneOf()
    all {
      irBuiltins' = irBuiltins.union(Set(name)),
      phase' = phase,
      indexBase' = indexBase,
      irVars' = irVars,
      irModules' = irModules,
      emittedVars' = emittedVars,
      moduleCache' = moduleCache,
      preludeHelpers' = preludeHelpers,
      indexAccesses' = indexAccesses,
      nilCallables' = nilCallables,
      referencedBuiltins' = referencedBuiltins,
      usedHelpers' = usedHelpers,
      referencedVars' = referencedVars,
      referencedModules' = referencedModules,
    }
  }

  // Discover a module import in the IR.
  action scanModule = {
    nondet name = AllModules.oneOf()
    all {
      irModules' = irModules.union(Set(name)),
      phase' = phase,
      indexBase' = indexBase,
      irVars' = irVars,
      irBuiltins' = irBuiltins,
      emittedVars' = emittedVars,
      moduleCache' = moduleCache,
      preludeHelpers' = preludeHelpers,
      indexAccesses' = indexAccesses,
      nilCallables' = nilCallables,
      referencedBuiltins' = referencedBuiltins,
      usedHelpers' = usedHelpers,
      referencedVars' = referencedVars,
      referencedModules' = referencedModules,
    }
  }

  // Finish IR scan, transition to preamble.
  action finishScan = all {
    phase' = 1,
    indexBase' = indexBase,
    irVars' = irVars,
    irBuiltins' = irBuiltins,
    irModules' = irModules,
    emittedVars' = emittedVars,
    moduleCache' = moduleCache,
    preludeHelpers' = preludeHelpers,
    indexAccesses' = indexAccesses,
    nilCallables' = nilCallables,
    referencedBuiltins' = referencedBuiltins,
    usedHelpers' = usedHelpers,
    referencedVars' = referencedVars,
    referencedModules' = referencedModules,
  }

  // =========================================================================
  // Phase 1: Preamble — emit declarations for everything the IR needs
  // =========================================================================

  // Emit a local variable declaration for an IR-discovered var.
  action emitLocalVar = {
    val remaining = irVars.exclude(emittedVars)
    nondet name = remaining.oneOf()
    all {
      emittedVars' = emittedVars.union(Set(name)),
      phase' = phase,
      indexBase' = indexBase,
      irVars' = irVars,
      irBuiltins' = irBuiltins,
      irModules' = irModules,
      moduleCache' = moduleCache,
      preludeHelpers' = preludeHelpers,
      indexAccesses' = indexAccesses,
      nilCallables' = nilCallables,
      referencedBuiltins' = referencedBuiltins,
      usedHelpers' = usedHelpers,
      referencedVars' = referencedVars,
      referencedModules' = referencedModules,
    }
  }

  // Import a module discovered in the IR scan.
  action emitModuleImport = {
    val remaining = irModules.exclude(moduleCache)
    nondet name = remaining.oneOf()
    all {
      moduleCache' = moduleCache.union(Set(name)),
      phase' = phase,
      indexBase' = indexBase,
      irVars' = irVars,
      irBuiltins' = irBuiltins,
      irModules' = irModules,
      emittedVars' = emittedVars,
      preludeHelpers' = preludeHelpers,
      indexAccesses' = indexAccesses,
      nilCallables' = nilCallables,
      referencedBuiltins' = referencedBuiltins,
      usedHelpers' = usedHelpers,
      referencedVars' = referencedVars,
      referencedModules' = referencedModules,
    }
  }

  // Helpers needed: for each IR builtin, if its Luau name is a helper, emit it.
  // Models used_call() scan + emit_prelude_conditional in luau.rs.
  val neededHelpers: Set[HelperName] =
    irBuiltins.fold(Set(), (acc, b) =>
      val h = requiredHelper(b)
      if (AllHelpers.contains(h)) acc.union(Set(h)) else acc
    )

  // Emit a prelude helper function that the IR scan determined is needed.
  action emitPreludeHelper = {
    val remaining = neededHelpers.exclude(preludeHelpers)
    nondet name = remaining.oneOf()
    all {
      preludeHelpers' = preludeHelpers.union(Set(name)),
      phase' = phase,
      indexBase' = indexBase,
      irVars' = irVars,
      irBuiltins' = irBuiltins,
      irModules' = irModules,
      emittedVars' = emittedVars,
      moduleCache' = moduleCache,
      indexAccesses' = indexAccesses,
      nilCallables' = nilCallables,
      referencedBuiltins' = referencedBuiltins,
      usedHelpers' = usedHelpers,
      referencedVars' = referencedVars,
      referencedModules' = referencedModules,
    }
  }

  // Preamble is complete when all IR-needed items are emitted.
  val preambleDone: bool =
    irVars.forall(v => emittedVars.contains(v)) and
    irModules.forall(m => moduleCache.contains(m)) and
    neededHelpers.forall(h => preludeHelpers.contains(h))

  // Transition from preamble to body phase (only when preamble is complete).
  action finishPreamble = all {
    phase' = 2,
    indexBase' = indexBase,
    irVars' = irVars,
    irBuiltins' = irBuiltins,
    irModules' = irModules,
    emittedVars' = emittedVars,
    moduleCache' = moduleCache,
    preludeHelpers' = preludeHelpers,
    indexAccesses' = indexAccesses,
    nilCallables' = nilCallables,
    referencedBuiltins' = referencedBuiltins,
    usedHelpers' = usedHelpers,
    referencedVars' = referencedVars,
    referencedModules' = referencedModules,
  }

  // =========================================================================
  // Phase 2: Body — emit code using only declared/imported/emitted resources
  // =========================================================================

  // Emit an array index access: adjusts 0-based IR index to 1-based Luau.
  // Models the +1 offset applied in luau.rs emit_index_access.
  action emitIndexAccess = {
    nondet name = irVars.oneOf()
    nondet idx = Set(0, 1, 2, 3).oneOf()
    val adjusted = idx + indexBase  // Python 0 + Luau base 1 = correct 1-based
    val access: IndexAccess = { var_name: name, original_index: idx, emitted_index: adjusted }
    all {
      indexAccesses' = indexAccesses.union(Set(access)),
      referencedVars' = referencedVars.union(Set(name)),
      phase' = phase,
      indexBase' = indexBase,
      irVars' = irVars,
      irBuiltins' = irBuiltins,
      irModules' = irModules,
      emittedVars' = emittedVars,
      moduleCache' = moduleCache,
      preludeHelpers' = preludeHelpers,
      nilCallables' = nilCallables,
      referencedBuiltins' = referencedBuiltins,
      usedHelpers' = usedHelpers,
      referencedModules' = referencedModules,
    }
  }

  // Resolve a builtin function reference from the IR-discovered set.
  action emitBuiltinRef = {
    nondet name = irBuiltins.oneOf()
    val luauName = builtinLuau(name)
    val isNil = luauName == ""
    all {
      referencedBuiltins' = referencedBuiltins.union(Set(name)),
      nilCallables' = if (isNil) nilCallables.union(Set(name)) else nilCallables,
      usedHelpers' = if (AllHelpers.contains(luauName))
                       usedHelpers.union(Set(luauName))
                     else usedHelpers,
      phase' = phase,
      indexBase' = indexBase,
      irVars' = irVars,
      irBuiltins' = irBuiltins,
      irModules' = irModules,
      emittedVars' = emittedVars,
      moduleCache' = moduleCache,
      preludeHelpers' = preludeHelpers,
      indexAccesses' = indexAccesses,
      referencedVars' = referencedVars,
      referencedModules' = referencedModules,
    }
  }

  // Emit a function call to a declared variable or known builtin.
  action emitCallFunc = {
    nondet callee = irVars.union(irBuiltins).oneOf()
    val isVar = irVars.contains(callee)
    val isBuiltin = irBuiltins.contains(callee)
    all {
      referencedVars' = if (isVar) referencedVars.union(Set(callee)) else referencedVars,
      referencedBuiltins' = if (isBuiltin) referencedBuiltins.union(Set(callee)) else referencedBuiltins,
      nilCallables' = if (isBuiltin and builtinLuau(callee) == "")
                        nilCallables.union(Set(callee))
                      else nilCallables,
      usedHelpers' = if (isBuiltin and AllHelpers.contains(builtinLuau(callee)))
                       usedHelpers.union(Set(builtinLuau(callee)))
                     else usedHelpers,
      referencedModules' = referencedModules,
      phase' = phase,
      indexBase' = indexBase,
      irVars' = irVars,
      irBuiltins' = irBuiltins,
      irModules' = irModules,
      emittedVars' = emittedVars,
      moduleCache' = moduleCache,
      preludeHelpers' = preludeHelpers,
      indexAccesses' = indexAccesses,
    }
  }

  // Reference a module member (e.g., math.sin).
  action useModuleMember = {
    nondet name = irModules.oneOf()
    all {
      referencedModules' = referencedModules.union(Set(name)),
      phase' = phase,
      indexBase' = indexBase,
      irVars' = irVars,
      irBuiltins' = irBuiltins,
      irModules' = irModules,
      emittedVars' = emittedVars,
      moduleCache' = moduleCache,
      preludeHelpers' = preludeHelpers,
      indexAccesses' = indexAccesses,
      nilCallables' = nilCallables,
      referencedBuiltins' = referencedBuiltins,
      usedHelpers' = usedHelpers,
      referencedVars' = referencedVars,
    }
  }

  // =========================================================================
  // Stutter + Step
  // =========================================================================

  action stutter = all {
    phase' = phase,
    indexBase' = indexBase,
    irVars' = irVars,
    irBuiltins' = irBuiltins,
    irModules' = irModules,
    emittedVars' = emittedVars,
    moduleCache' = moduleCache,
    preludeHelpers' = preludeHelpers,
    indexAccesses' = indexAccesses,
    nilCallables' = nilCallables,
    referencedBuiltins' = referencedBuiltins,
    usedHelpers' = usedHelpers,
    referencedVars' = referencedVars,
    referencedModules' = referencedModules,
  }

  // Phase-gated step modeling the real transpiler's three-pass architecture:
  //   Phase 0: scan IR to discover vars, builtins, modules
  //   Phase 1: emit preamble (local decls, imports, prelude helpers)
  //   Phase 2: emit body (index accesses, builtin refs, calls)
  action step =
    if (phase == 0) any {
      scanVar,
      scanBuiltin,
      scanModule,
      finishScan,
    }
    else if (phase == 1) any {
      if (irVars.exclude(emittedVars) != Set()) emitLocalVar else stutter,
      if (irModules.exclude(moduleCache) != Set()) emitModuleImport else stutter,
      if (neededHelpers.exclude(preludeHelpers) != Set()) emitPreludeHelper else stutter,
      if (preambleDone) finishPreamble else stutter,
    }
    else any {
      if (irVars.size() > 0) emitIndexAccess else stutter,
      if (irBuiltins.size() > 0) emitBuiltinRef else stutter,
      if (irVars.size() > 0 or irBuiltins.size() > 0) emitCallFunc else stutter,
      if (irModules.size() > 0) useModuleMember else stutter,
    }

  // =========================================================================
  // Invariants
  // =========================================================================

  // I1: All emitted index accesses use 1-based indexing.
  // Models the critical Python -> Luau index adjustment (+1 offset).
  val indexAdjusted =
    indexAccesses.forall(a => a.emitted_index >= 1)

  // I2: No callable reference resolves to nil.
  // A nil callable in Luau means a runtime "attempt to call a nil value" error.
  val noNilCallables =
    nilCallables == Set()

  // I3: All referenced modules are in the cache.
  // Models require() caching: a module must be imported before referencing.
  val modulesCached =
    referencedModules.forall(m => moduleCache.contains(m))

  // I4: All used prelude helpers have been emitted.
  // Models emit_prelude_conditional: if a helper is called, it must exist.
  val preludeComplete =
    usedHelpers.forall(h => preludeHelpers.contains(h))

  // I5: All referenced variables have been declared with `local`.
  // Undeclared variables in Luau become globals — a correctness hazard.
  val localsDeclared =
    referencedVars.forall(v => emittedVars.contains(v))

  // I6: All referenced builtins have non-empty Luau mappings.
  // An unmapped builtin means the transpiler emits a call to nil.
  val builtinsCovered =
    referencedBuiltins.forall(b => builtinLuau(b) != "")

  // Combined invariant.
  val Inv = and {
    indexAdjusted,
    noNilCallables,
    modulesCached,
    preludeComplete,
    localsDeclared,
    builtinsCovered,
  }
}
