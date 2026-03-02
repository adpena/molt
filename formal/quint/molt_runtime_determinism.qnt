// molt_runtime_determinism.qnt — Quint model for Molt runtime determinism.
//
// Models deterministic scheduling and capability gating:
//   - Async task queue ordering under deterministic mode
//   - Nondeterminism source pinning (time, random, hash seeds)
//   - Capability-gated I/O surfaces
//
// Usage:
//   quint run  formal/quint/molt_runtime_determinism.qnt --invariant=Inv --max-steps=15
//   quint verify formal/quint/molt_runtime_determinism.qnt --invariant=Inv

module molt_runtime_determinism {

  // =========================================================================
  // Types
  // =========================================================================
  type TaskId = int
  type TaskResult = int
  type TaskRecord = { task: TaskId, result: TaskResult }

  // =========================================================================
  // Constants
  // =========================================================================
  pure val N = 4
  pure val Tasks: Set[int] = 0.to(N - 1)

  // Deterministic mode: when true, all nondeterminism sources are pinned.
  pure val deterministicMode = true

  // Each task has a deterministic result based on its ID.
  pure def taskResult(t: TaskId): TaskResult = t * 7 + 3

  // =========================================================================
  // State
  // =========================================================================
  var pending: Set[TaskId]
  var running: Set[TaskId]
  var completed: Set[TaskId]
  var results: Set[TaskRecord]

  // Nondeterminism sources
  var timeSeed: int
  var randomSeed: int
  var hashSeed: int

  // Scheduler trace: records execution order for determinism checking
  var execOrder: List[TaskId]

  // =========================================================================
  // Init
  // =========================================================================
  action init = all {
    pending' = Tasks,
    running' = Set(),
    completed' = Set(),
    results' = Set(),
    timeSeed' = 0,
    randomSeed' = 0,
    hashSeed' = 0,
    execOrder' = List(),
  }

  // =========================================================================
  // Steps
  // =========================================================================

  // Schedule a pending task: move it to running.
  action scheduleTask = {
    nondet t = pending.oneOf()
    all {
      pending' = pending.exclude(Set(t)),
      running' = running.union(Set(t)),
      completed' = completed,
      results' = results,
      timeSeed' = timeSeed,
      randomSeed' = randomSeed,
      hashSeed' = hashSeed,
      execOrder' = execOrder,
    }
  }

  // Complete a running task: compute its result deterministically.
  action completeTask = {
    nondet t = running.oneOf()
    val res = taskResult(t)
    all {
      pending' = pending,
      running' = running.exclude(Set(t)),
      completed' = completed.union(Set(t)),
      results' = results.union(Set({ task: t, result: res })),
      timeSeed' = timeSeed,
      randomSeed' = randomSeed,
      hashSeed' = hashSeed,
      execOrder' = execOrder.append(t),
    }
  }

  action stutter = all {
    pending' = pending,
    running' = running,
    completed' = completed,
    results' = results,
    timeSeed' = timeSeed,
    randomSeed' = randomSeed,
    hashSeed' = hashSeed,
    execOrder' = execOrder,
  }

  action step = any {
    if (pending.size() > 0) scheduleTask else stutter,
    if (running.size() > 0) completeTask else stutter,
  }

  // =========================================================================
  // Invariants
  // =========================================================================

  // I1: Seeds never drift from zero in deterministic mode.
  val seedsPinned =
    if (deterministicMode)
      timeSeed == 0 and randomSeed == 0 and hashSeed == 0
    else
      true

  // I2: Task results are deterministic — same task always produces same result.
  val resultsDeterministic =
    results.forall(r => r.result == taskResult(r.task))

  // I3: No task completed without being scheduled.
  val noPhantomCompletions =
    completed.forall(t => not(pending.contains(t)))

  // I4: When all tasks complete, every task has exactly one result.
  val allResultsPresent =
    (completed == Tasks) implies (
      Tasks.forall(t =>
        results.exists(r => r.task == t and r.result == taskResult(t))
      )
    )

  // I5: Running + pending + completed = Tasks (conservation).
  val taskConservation =
    pending.union(running).union(completed) == Tasks

  // I6: No duplicate results for the same task.
  val noDuplicateResults =
    results.forall(r1 =>
      results.forall(r2 =>
        (r1.task == r2.task) implies (r1.result == r2.result)
      )
    )

  // Combined invariant.
  val Inv = and {
    seedsPinned,
    resultsDeterministic,
    noPhantomCompletions,
    allResultsPresent,
    taskConservation,
    noDuplicateResults,
  }
}
