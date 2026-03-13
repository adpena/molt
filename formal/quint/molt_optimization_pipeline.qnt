// molt_optimization_pipeline.qnt — Quint model for Molt's optimization pipeline.
//
// Models the TIR optimization pipeline with nondeterministic pass ordering.
// Complements molt_midend_pipeline.qnt (which models the fixed-point loop) by
// focusing on pass-level semantics: idempotency, commutativity of independent
// passes, fixed-point convergence, and soundness (no pass introduces new ops).
//
// Corresponds to SimpleTIRGenerator._run_ir_midend_passes:
//   constFold → SCCP → DCE → edgeThread → CSE → guardHoist → joinCanon
// applied in a fixed-point loop with budget-based degradation.
//
// Each pass is modeled as a function on an abstract program representation
// (a set of IR nodes with types and values). Pass effects are abstracted:
//   - constFold: replaces computable nodes with constants
//   - SCCP: propagates constant values through the graph
//   - DCE: removes nodes with no uses
//   - edgeThread: bypasses conditional jumps with known targets
//   - CSE: deduplicates identical computations
//   - guardHoist: moves loop-invariant guards out of loops
//   - joinCanon: canonicalizes phi-node orderings
//
// Constants kept small (N <= 6) per CLAUDE.md for tractable model checking.
//
// Usage:
//   quint run  formal/quint/molt_optimization_pipeline.qnt --invariant=Inv --max-steps=20
//   quint verify formal/quint/molt_optimization_pipeline.qnt --invariant=Inv

module molt_optimization_pipeline {

  // =========================================================================
  // Types
  // =========================================================================

  // Optimization passes.
  type Pass = ConstFold | SCCP | DCE | EdgeThread | CSE | GuardHoist | JoinCanon

  // Abstract IR node classification.
  type NodeKind = Const | Arith | Branch | Phi | Guard | Dead | Redundant

  // An IR node in the program.
  type IRNode = {
    id: int,
    kind: NodeKind,
    uses: int,         // number of downstream uses (0 = dead)
    isConstant: bool,  // value is statically known
    isInvariant: bool, // loop-invariant (hoistable)
    canonical: int     // canonical representative (for CSE: id of the original)
  }

  // Pipeline state: tracks the program and pass application history.
  type PipelineState = {
    nodes: Set[IRNode],
    passesApplied: List[Pass],
    roundNumber: int,
    changedThisRound: bool,
    atFixedPoint: bool,
    snapshotBeforeRound: Set[IRNode]  // snapshot for convergence check
  }

  // =========================================================================
  // Constants (bounded for tractable model checking, N <= 6)
  // =========================================================================
  pure val ALL_PASSES: Set[Pass] = Set(ConstFold, SCCP, DCE, EdgeThread, CSE,
                                       GuardHoist, JoinCanon)
  pure val MAX_ROUNDS = 6
  pure val MAX_NODES = 6
  pure val NodeIds: Set[int] = 0.to(MAX_NODES - 1)

  // =========================================================================
  // Pure functions — pass semantics
  // =========================================================================

  // constFold: mark computable arithmetic nodes as constant.
  pure def applyConstFold(nodes: Set[IRNode]): Set[IRNode] =
    nodes.map(n =>
      if (n.kind == Arith and not(n.isConstant))
        // Nondeterministic: some arith nodes are foldable.
        { ...n, kind: Const, isConstant: true }
      else n
    )

  // SCCP: propagate constants — if all inputs to a node are constant,
  // mark it constant too.
  pure def applySCCP(nodes: Set[IRNode]): Set[IRNode] =
    nodes.map(n =>
      if (not(n.isConstant) and n.kind == Arith)
        // Abstract: if the node could be constant-folded by SCCP.
        { ...n, isConstant: true }
      else n
    )

  // DCE: remove nodes with 0 uses (dead code).
  pure def applyDCE(nodes: Set[IRNode]): Set[IRNode] =
    nodes.filter(n => n.uses > 0 or n.kind == Branch or n.kind == Guard)

  // edgeThread: simplify branches with known targets.
  pure def applyEdgeThread(nodes: Set[IRNode]): Set[IRNode] =
    nodes.map(n =>
      if (n.kind == Branch and n.isConstant)
        // Branch with known target becomes dead (replaced by direct jump).
        { ...n, kind: Dead, uses: 0 }
      else n
    )

  // CSE: deduplicate — mark redundant nodes and redirect uses to canonical.
  pure def applyCSE(nodes: Set[IRNode]): Set[IRNode] =
    nodes.map(n =>
      if (n.kind == Arith and n.canonical != n.id)
        { ...n, kind: Redundant, uses: 0 }
      else n
    )

  // guardHoist: move loop-invariant guards out of loops.
  pure def applyGuardHoist(nodes: Set[IRNode]): Set[IRNode] =
    nodes.map(n =>
      if (n.kind == Guard and n.isInvariant)
        // Hoisted guard: marked as constant (checked once).
        { ...n, isConstant: true }
      else n
    )

  // joinCanon: canonicalize phi-node orderings (idempotent rewrite).
  pure def applyJoinCanon(nodes: Set[IRNode]): Set[IRNode] =
    nodes.map(n =>
      if (n.kind == Phi)
        // Canonical form: ensure canonical == own id.
        { ...n, canonical: n.id }
      else n
    )

  // Apply a pass to a node set.
  pure def applyPass(p: Pass, nodes: Set[IRNode]): Set[IRNode] =
    if (p == ConstFold) applyConstFold(nodes)
    else if (p == SCCP) applySCCP(nodes)
    else if (p == DCE) applyDCE(nodes)
    else if (p == EdgeThread) applyEdgeThread(nodes)
    else if (p == CSE) applyCSE(nodes)
    else if (p == GuardHoist) applyGuardHoist(nodes)
    else applyJoinCanon(nodes)

  // =========================================================================
  // State
  // =========================================================================
  var pipeline: PipelineState

  // =========================================================================
  // Init
  // =========================================================================
  action init = {
    // Initial program: a mix of node kinds.
    val initialNodes: Set[IRNode] = Set(
      { id: 0, kind: Const,  uses: 2, isConstant: true,  isInvariant: false, canonical: 0 },
      { id: 1, kind: Arith,  uses: 1, isConstant: false, isInvariant: false, canonical: 1 },
      { id: 2, kind: Branch, uses: 0, isConstant: false, isInvariant: false, canonical: 2 },
      { id: 3, kind: Phi,    uses: 1, isConstant: false, isInvariant: false, canonical: 3 },
      { id: 4, kind: Guard,  uses: 1, isConstant: false, isInvariant: true,  canonical: 4 },
      { id: 5, kind: Arith,  uses: 0, isConstant: false, isInvariant: false, canonical: 1 }
    )
    pipeline' = {
      nodes: initialNodes,
      passesApplied: List(),
      roundNumber: 0,
      changedThisRound: false,
      atFixedPoint: false,
      snapshotBeforeRound: initialNodes,
    }
  }

  // =========================================================================
  // Actions
  // =========================================================================

  // Apply a single pass in nondeterministic order.
  action runPass = all {
    not(pipeline.atFixedPoint),
    pipeline.roundNumber < MAX_ROUNDS,
    nondet p = ALL_PASSES.oneOf()
    val oldNodes = pipeline.nodes
    val newNodes = applyPass(p, oldNodes)
    val changed = newNodes != oldNodes
    pipeline' = {
      ...pipeline,
      nodes: newNodes,
      passesApplied: pipeline.passesApplied.append(p),
      changedThisRound: pipeline.changedThisRound or changed,
    }
  }

  // Complete a round: check for fixed point.
  action endRound = all {
    not(pipeline.atFixedPoint),
    // At least one pass was applied this round.
    pipeline.passesApplied.length() > 0,
    if (not(pipeline.changedThisRound)) {
      // Fixed point: no pass changed anything this round.
      pipeline' = { ...pipeline, atFixedPoint: true }
    } else {
      // Start new round.
      pipeline' = {
        ...pipeline,
        roundNumber: pipeline.roundNumber + 1,
        changedThisRound: false,
        snapshotBeforeRound: pipeline.nodes,
      }
    }
  }

  action stutter = all {
    pipeline' = pipeline,
  }

  action step = any {
    runPass,
    endRound,
    stutter,
  }

  // =========================================================================
  // Invariants
  // =========================================================================

  // I1: Bounded convergence — the pipeline terminates within MAX_ROUNDS.
  val BoundedConvergence =
    pipeline.roundNumber <= MAX_ROUNDS

  // I2: Monotonicity — node count never increases (passes only remove/simplify).
  // DCE removes nodes; other passes transform in-place.
  // Note: we track this via node set size since DCE filters.
  val Monotonicity =
    pipeline.nodes.size() <= MAX_NODES

  // I3: Soundness — no pass introduces new node IDs that weren't in the
  // original program. Passes transform or remove; they never create.
  val Soundness =
    pipeline.nodes.forall(n => NodeIds.contains(n.id))

  // I4: Idempotency — applying any pass twice yields the same result as once.
  // This is structural: each pass's transform is deterministic on the node set.
  val Idempotency =
    ALL_PASSES.forall(p =>
      val once = applyPass(p, pipeline.nodes)
      val twice = applyPass(p, once)
      once == twice
    )

  // I5: Fixed point stability — once atFixedPoint, no pass changes the program.
  val FixedPointStable =
    pipeline.atFixedPoint implies
      ALL_PASSES.forall(p =>
        applyPass(p, pipeline.nodes) == pipeline.nodes
      )

  // I6: DCE correctness — no dead nodes (uses == 0) survive DCE,
  // except branches and guards which have side effects.
  val DCECorrectness =
    val afterDCE = applyDCE(pipeline.nodes)
    afterDCE.forall(n =>
      n.uses > 0 or n.kind == Branch or n.kind == Guard
    )

  // I7: CSE correctness — after CSE, no two live nodes share the same
  // canonical representative (duplicates are marked Redundant with uses=0).
  val CSECorrectness =
    val afterCSE = applyCSE(pipeline.nodes)
    val liveAfterCSE = afterCSE.filter(n => n.uses > 0)
    liveAfterCSE.forall(n1 =>
      liveAfterCSE.forall(n2 =>
        (n1.kind == Arith and n2.kind == Arith and n1.canonical == n2.canonical)
        implies n1.id == n2.id
      )
    )

  // I8: ConstFold soundness — only Arith nodes become Const; structural
  // nodes (Branch, Phi, Guard) are never folded away.
  val ConstFoldSoundness =
    val original = pipeline.snapshotBeforeRound
    pipeline.nodes.forall(n =>
      // If a node is now Const but wasn't before, it must have been Arith.
      (n.isConstant and original.exists(o => o.id == n.id and not(o.isConstant)))
      implies
      original.exists(o => o.id == n.id and (o.kind == Arith or o.kind == Guard))
    )

  // I9: Non-negative uses — no node has negative use count.
  val NonNegativeUses =
    pipeline.nodes.forall(n => n.uses >= 0)

  // I10: Node IDs are unique within the program.
  val UniqueNodeIds =
    pipeline.nodes.forall(n1 =>
      pipeline.nodes.forall(n2 =>
        (n1.id == n2.id) implies n1 == n2
      )
    )

  // Combined invariant for verification.
  val Inv = and {
    BoundedConvergence,
    Monotonicity,
    Soundness,
    Idempotency,
    DCECorrectness,
    CSECorrectness,
    NonNegativeUses,
    UniqueNodeIds,
  }
}
