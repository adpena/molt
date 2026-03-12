// molt_concurrency.qnt — Quint model for Molt's concurrency and async/await semantics.
//
// Models the GIL-based concurrency model used by Molt's runtime:
//   - GIL acquisition/release with fairness guarantees
//   - Thread lifecycle (spawn, run, block on I/O, terminate)
//   - Async task scheduling (spawn, await, resolve)
//   - Channel-based communication (send/recv with linearizability)
//   - I/O-triggered GIL release (the key Python threading optimization)
//
// The GIL serializes all access to shared state, so data races are
// structurally impossible. Async tasks run cooperatively within a
// single thread's GIL-holding window. Channels provide the safe
// cross-thread communication primitive.
//
// Usage:
//   quint run  formal/quint/molt_concurrency.qnt --invariant=Inv --max-steps=20
//   quint verify formal/quint/molt_concurrency.qnt --invariant=Inv

module molt_concurrency {

  // =========================================================================
  // Types
  // =========================================================================
  type ThreadState = Idle | Running | BlockedIO | Terminated
  type TaskStatus = Pending | Executing | Suspended | Completed
  type FutureState = Unresolved | Resolved

  type Thread = {
    id: int,
    state: ThreadState,
    isAsync: bool           // true = this thread runs async tasks
  }

  type Task = {
    id: int,
    status: TaskStatus,
    ownerThread: int,       // thread that spawned this task
    awaitingFuture: int,    // -1 = not awaiting, >= 0 = future id
    isAsyncContext: bool    // true = spawned via asyncSpawn
  }

  type Channel = {
    id: int,
    buffer: List[int],     // message values in FIFO order
    capacity: int,
    closed: bool
  }

  type Future = {
    id: int,
    state: FutureState,
    value: int,            // result value once resolved
    taskId: int            // task that will produce this future's value
  }

  // =========================================================================
  // Constants (bounded for tractable model checking, N <= 6)
  // =========================================================================
  pure val MAX_THREADS = 4
  pure val MAX_TASKS = 6
  pure val MAX_CHANNELS = 2
  pure val MAX_FUTURES = 6
  pure val CHANNEL_CAPACITY = 3

  pure val ThreadIds: Set[int] = 0.to(MAX_THREADS - 1)
  pure val TaskIds: Set[int] = 0.to(MAX_TASKS - 1)
  pure val ChannelIds: Set[int] = 0.to(MAX_CHANNELS - 1)
  pure val FutureIds: Set[int] = 0.to(MAX_FUTURES - 1)

  // =========================================================================
  // State
  // =========================================================================
  var threads: int -> Thread
  var activeThreads: Set[int]        // set of spawned thread ids
  var tasks: int -> Task
  var activeTasks: Set[int]          // set of spawned task ids
  var gilOwner: int                  // -1 = no holder, >= 0 = thread id
  var readyQueue: List[int]          // thread ids waiting for GIL (FIFO for fairness)
  var channels: int -> Channel
  var activeChannels: Set[int]       // set of created channel ids
  var futures: int -> Future
  var activeFutures: Set[int]        // set of created future ids
  var nextTaskId: int
  var nextFutureId: int
  var gilAcquisitions: int -> int    // per-thread GIL acquisition count (for fairness)

  // =========================================================================
  // Pure helpers
  // =========================================================================

  // Check if a thread can acquire the GIL: it must be active, not terminated,
  // and the GIL must be free or held by no one.
  pure def canAcquireGIL(t: Thread, owner: int): bool = and {
    t.state != Terminated,
    t.state != BlockedIO,
    owner == -1,
  }

  // Check if a thread is the current GIL holder.
  pure def holdsGIL(tid: int, owner: int): bool = owner == tid

  // =========================================================================
  // Init
  // =========================================================================
  action init = all {
    // Thread 0 is the main thread, always present and starts with the GIL.
    threads' = ThreadIds.mapBy(id => {
      id: id,
      state: if (id == 0) Running else Idle,
      isAsync: id == 0,  // main thread runs the async event loop
    }),
    activeThreads' = Set(0),
    tasks' = TaskIds.mapBy(id => {
      id: id,
      status: Pending,
      ownerThread: -1,
      awaitingFuture: -1,
      isAsyncContext: false,
    }),
    activeTasks' = Set(),
    gilOwner' = 0,          // main thread starts with GIL
    readyQueue' = List(),
    channels' = ChannelIds.mapBy(id => {
      id: id,
      buffer: List(),
      capacity: CHANNEL_CAPACITY,
      closed: false,
    }),
    activeChannels' = Set(),
    futures' = FutureIds.mapBy(id => {
      id: id,
      state: Unresolved,
      value: 0,
      taskId: -1,
    }),
    activeFutures' = Set(),
    nextTaskId' = 0,
    nextFutureId' = 0,
    gilAcquisitions' = ThreadIds.mapBy(id => if (id == 0) 1 else 0),
  }

  // =========================================================================
  // Actions
  // =========================================================================

  // --- GIL operations ---

  // acquireGIL: A thread waiting in the ready queue acquires the free GIL.
  // The GIL must be unowned and the thread must be first in the ready queue
  // (FIFO fairness).
  action acquireGIL = {
    all {
      gilOwner == -1,
      readyQueue.length() > 0,
      val tid = readyQueue.head()
      val t = threads.get(tid)
      all {
        t.state != Terminated,
        threads' = threads.set(tid, { ...t, state: Running }),
        gilOwner' = tid,
        readyQueue' = readyQueue.tail(),
        gilAcquisitions' = gilAcquisitions.set(tid, gilAcquisitions.get(tid) + 1),
        // unchanged
        activeThreads' = activeThreads,
        tasks' = tasks,
        activeTasks' = activeTasks,
        channels' = channels,
        activeChannels' = activeChannels,
        futures' = futures,
        activeFutures' = activeFutures,
        nextTaskId' = nextTaskId,
        nextFutureId' = nextFutureId,
      }
    }
  }

  // releaseGIL: Current GIL holder voluntarily releases.
  // Thread moves to Idle and joins the back of the ready queue
  // (cooperative yield).
  action releaseGIL = {
    all {
      gilOwner != -1,
      val tid = gilOwner
      val t = threads.get(tid)
      all {
        t.state == Running,
        threads' = threads.set(tid, { ...t, state: Idle }),
        gilOwner' = -1,
        readyQueue' = readyQueue.append(tid),
        // unchanged
        activeThreads' = activeThreads,
        tasks' = tasks,
        activeTasks' = activeTasks,
        channels' = channels,
        activeChannels' = activeChannels,
        futures' = futures,
        activeFutures' = activeFutures,
        nextTaskId' = nextTaskId,
        nextFutureId' = nextFutureId,
        gilAcquisitions' = gilAcquisitions,
      }
    }
  }

  // --- Thread operations ---

  // spawnThread: Create a new thread. The new thread enters the ready queue.
  // Only the GIL holder can spawn threads.
  action spawnThread = {
    all {
      gilOwner != -1,
      val spawner = gilOwner
      nondet tid = ThreadIds.exclude(activeThreads).oneOf()
      all {
        threads' = threads.set(tid, {
          id: tid,
          state: Idle,
          isAsync: false,
        }),
        activeThreads' = activeThreads.union(Set(tid)),
        readyQueue' = readyQueue.append(tid),
        gilAcquisitions' = gilAcquisitions.set(tid, 0),
        // unchanged
        gilOwner' = gilOwner,
        tasks' = tasks,
        activeTasks' = activeTasks,
        channels' = channels,
        activeChannels' = activeChannels,
        futures' = futures,
        activeFutures' = activeFutures,
        nextTaskId' = nextTaskId,
        nextFutureId' = nextFutureId,
      }
    }
  }

  // --- I/O release ---

  // ioRelease: The GIL holder releases the GIL for I/O, enters BlockedIO.
  // This is the key Python optimization: I/O-bound threads release the GIL
  // so compute-bound threads can proceed.
  action ioRelease = {
    all {
      gilOwner != -1,
      val tid = gilOwner
      val t = threads.get(tid)
      all {
        t.state == Running,
        threads' = threads.set(tid, { ...t, state: BlockedIO }),
        gilOwner' = -1,
        // BlockedIO thread does NOT enter ready queue — it will re-enter
        // when I/O completes (ioComplete action).
        readyQueue' = readyQueue,
        // unchanged
        activeThreads' = activeThreads,
        tasks' = tasks,
        activeTasks' = activeTasks,
        channels' = channels,
        activeChannels' = activeChannels,
        futures' = futures,
        activeFutures' = activeFutures,
        nextTaskId' = nextTaskId,
        nextFutureId' = nextFutureId,
        gilAcquisitions' = gilAcquisitions,
      }
    }
  }

  // ioComplete: An I/O-blocked thread finishes I/O and re-enters the ready queue.
  action ioComplete = {
    nondet tid = activeThreads.filter(id => threads.get(id).state == BlockedIO).oneOf()
    val t = threads.get(tid)
    all {
      threads' = threads.set(tid, { ...t, state: Idle }),
      readyQueue' = readyQueue.append(tid),
      // unchanged
      gilOwner' = gilOwner,
      activeThreads' = activeThreads,
      tasks' = tasks,
      activeTasks' = activeTasks,
      channels' = channels,
      activeChannels' = activeChannels,
      futures' = futures,
      activeFutures' = activeFutures,
      nextTaskId' = nextTaskId,
      nextFutureId' = nextFutureId,
      gilAcquisitions' = gilAcquisitions,
    }
  }

  // --- Async task operations ---

  // asyncSpawn: Create an async task with an associated future.
  // Only the GIL holder can spawn tasks. Tasks are pinned to the spawner thread.
  action asyncSpawn = {
    all {
      gilOwner != -1,
      nextTaskId < MAX_TASKS,
      nextFutureId < MAX_FUTURES,
      val spawner = gilOwner
      val tid = nextTaskId
      val fid = nextFutureId
      all {
        tasks' = tasks.set(tid, {
          id: tid,
          status: Pending,
          ownerThread: spawner,
          awaitingFuture: -1,
          isAsyncContext: true,
        }),
        activeTasks' = activeTasks.union(Set(tid)),
        futures' = futures.set(fid, {
          id: fid,
          state: Unresolved,
          value: 0,
          taskId: tid,
        }),
        activeFutures' = activeFutures.union(Set(fid)),
        nextTaskId' = nextTaskId + 1,
        nextFutureId' = nextFutureId + 1,
        // unchanged
        threads' = threads,
        activeThreads' = activeThreads,
        gilOwner' = gilOwner,
        readyQueue' = readyQueue,
        channels' = channels,
        activeChannels' = activeChannels,
        gilAcquisitions' = gilAcquisitions,
      }
    }
  }

  // executeTask: GIL holder picks a pending async task and begins executing it.
  action executeTask = {
    all {
      gilOwner != -1,
      val pendingTasks = activeTasks.filter(id =>
        tasks.get(id).status == Pending and tasks.get(id).ownerThread == gilOwner
      )
      nondet tid = pendingTasks.oneOf()
      val t = tasks.get(tid)
      all {
        tasks' = tasks.set(tid, { ...t, status: Executing }),
        // unchanged
        threads' = threads,
        activeThreads' = activeThreads,
        activeTasks' = activeTasks,
        gilOwner' = gilOwner,
        readyQueue' = readyQueue,
        channels' = channels,
        activeChannels' = activeChannels,
        futures' = futures,
        activeFutures' = activeFutures,
        nextTaskId' = nextTaskId,
        nextFutureId' = nextFutureId,
        gilAcquisitions' = gilAcquisitions,
      }
    }
  }

  // completeTask: An executing task finishes, resolving its future.
  action completeTask = {
    all {
      gilOwner != -1,
      val execTasks = activeTasks.filter(id =>
        tasks.get(id).status == Executing and tasks.get(id).ownerThread == gilOwner
      )
      nondet tid = execTasks.oneOf()
      val t = tasks.get(tid)
      // Find the future associated with this task.
      val assocFutures = activeFutures.filter(fid => futures.get(fid).taskId == tid)
      // First complete the task, then unsuspend any waiters in one fold.
      val updatedTasks = activeTasks.fold(
        tasks.set(tid, { ...t, status: Completed }),
        (ts, otherTid) =>
          if (otherTid != tid and ts.get(otherTid).status == Suspended)
            val otherTask = ts.get(otherTid)
            if (assocFutures.contains(otherTask.awaitingFuture))
              ts.set(otherTid, { ...otherTask, status: Pending, awaitingFuture: -1 })
            else
              ts
          else
            ts
      )
      all {
        tasks' = updatedTasks,
        // Resolve all futures associated with this task.
        futures' = assocFutures.fold(futures, (fs, fid) =>
          fs.set(fid, { ...fs.get(fid), state: Resolved, value: tid * 13 + 7 })
        ),
        // unchanged
        threads' = threads,
        activeThreads' = activeThreads,
        activeTasks' = activeTasks,
        gilOwner' = gilOwner,
        readyQueue' = readyQueue,
        channels' = channels,
        activeChannels' = activeChannels,
        activeFutures' = activeFutures,
        nextTaskId' = nextTaskId,
        nextFutureId' = nextFutureId,
        gilAcquisitions' = gilAcquisitions,
      }
    }
  }

  // awaitFuture: An executing async task suspends on an unresolved future.
  // Only valid in async context.
  action awaitFuture = {
    all {
      gilOwner != -1,
      val execTasks = activeTasks.filter(id =>
        tasks.get(id).status == Executing and
        tasks.get(id).ownerThread == gilOwner and
        tasks.get(id).isAsyncContext
      )
      nondet tid = execTasks.oneOf()
      val t = tasks.get(tid)
      // Pick an unresolved future to await (not our own).
      val awaitableFutures = activeFutures.filter(fid =>
        futures.get(fid).state == Unresolved and futures.get(fid).taskId != tid
      )
      nondet fid = awaitableFutures.oneOf()
      all {
        tasks' = tasks.set(tid, { ...t, status: Suspended, awaitingFuture: fid }),
        // unchanged
        threads' = threads,
        activeThreads' = activeThreads,
        activeTasks' = activeTasks,
        gilOwner' = gilOwner,
        readyQueue' = readyQueue,
        channels' = channels,
        activeChannels' = activeChannels,
        futures' = futures,
        activeFutures' = activeFutures,
        nextTaskId' = nextTaskId,
        nextFutureId' = nextFutureId,
        gilAcquisitions' = gilAcquisitions,
      }
    }
  }

  // --- Channel operations ---

  // createChannel: GIL holder creates a bounded channel.
  action createChannel = {
    all {
      gilOwner != -1,
      nondet cid = ChannelIds.exclude(activeChannels).oneOf()
      all {
        channels' = channels.set(cid, {
          id: cid,
          buffer: List(),
          capacity: CHANNEL_CAPACITY,
          closed: false,
        }),
        activeChannels' = activeChannels.union(Set(cid)),
        // unchanged
        threads' = threads,
        activeThreads' = activeThreads,
        tasks' = tasks,
        activeTasks' = activeTasks,
        gilOwner' = gilOwner,
        readyQueue' = readyQueue,
        futures' = futures,
        activeFutures' = activeFutures,
        nextTaskId' = nextTaskId,
        nextFutureId' = nextFutureId,
        gilAcquisitions' = gilAcquisitions,
      }
    }
  }

  // channelSend: GIL holder sends a value to a channel (if not full and not closed).
  action channelSend = {
    all {
      gilOwner != -1,
      val openChannels = activeChannels.filter(cid =>
        val ch = channels.get(cid)
        not(ch.closed) and ch.buffer.length() < ch.capacity
      )
      nondet cid = openChannels.oneOf()
      val ch = channels.get(cid)
      nondet msg = Set(1, 2, 3, 4, 5).oneOf()  // bounded message values
      all {
        channels' = channels.set(cid, { ...ch, buffer: ch.buffer.append(msg) }),
        // unchanged
        threads' = threads,
        activeThreads' = activeThreads,
        tasks' = tasks,
        activeTasks' = activeTasks,
        gilOwner' = gilOwner,
        readyQueue' = readyQueue,
        activeChannels' = activeChannels,
        futures' = futures,
        activeFutures' = activeFutures,
        nextTaskId' = nextTaskId,
        nextFutureId' = nextFutureId,
        gilAcquisitions' = gilAcquisitions,
      }
    }
  }

  // channelRecv: GIL holder receives a value from a non-empty channel.
  action channelRecv = {
    all {
      gilOwner != -1,
      val nonEmptyChannels = activeChannels.filter(cid =>
        channels.get(cid).buffer.length() > 0
      )
      nondet cid = nonEmptyChannels.oneOf()
      val ch = channels.get(cid)
      all {
        channels' = channels.set(cid, { ...ch, buffer: ch.buffer.tail() }),
        // unchanged
        threads' = threads,
        activeThreads' = activeThreads,
        tasks' = tasks,
        activeTasks' = activeTasks,
        gilOwner' = gilOwner,
        readyQueue' = readyQueue,
        activeChannels' = activeChannels,
        futures' = futures,
        activeFutures' = activeFutures,
        nextTaskId' = nextTaskId,
        nextFutureId' = nextFutureId,
        gilAcquisitions' = gilAcquisitions,
      }
    }
  }

  // --- Stutter ---

  action stutter = all {
    threads' = threads,
    activeThreads' = activeThreads,
    tasks' = tasks,
    activeTasks' = activeTasks,
    gilOwner' = gilOwner,
    readyQueue' = readyQueue,
    channels' = channels,
    activeChannels' = activeChannels,
    futures' = futures,
    activeFutures' = activeFutures,
    nextTaskId' = nextTaskId,
    nextFutureId' = nextFutureId,
    gilAcquisitions' = gilAcquisitions,
  }

  // =========================================================================
  // Step
  // =========================================================================
  action step = any {
    acquireGIL,
    releaseGIL,
    spawnThread,
    asyncSpawn,
    executeTask,
    completeTask,
    awaitFuture,
    ioRelease,
    ioComplete,
    createChannel,
    channelSend,
    channelRecv,
    stutter,
  }

  // =========================================================================
  // Invariants
  // =========================================================================

  // I1: Exactly zero or one thread holds the GIL at any time.
  // When gilOwner >= 0, that thread must be Running. No other thread is Running.
  val gilExclusive = and {
    gilOwner == -1 or gilOwner.in(activeThreads),
    gilOwner != -1 implies threads.get(gilOwner).state == Running,
    activeThreads.forall(tid =>
      threads.get(tid).state == Running implies tid == gilOwner
    ),
  }

  // I2: No data race on shared state — only the GIL holder can be Running.
  // At most one thread is in state Running at any time. This is the structural
  // guarantee that the GIL provides: serialized access to shared state.
  val noDataRace =
    activeThreads.forall(t1 =>
      activeThreads.forall(t2 =>
        (threads.get(t1).state == Running and threads.get(t2).state == Running)
          implies t1 == t2
      )
    )

  // I3: Async tasks make progress — no completed task has unresolved futures.
  // When a task completes, all futures it produces must be resolved.
  val completedTasksFuturesResolved =
    activeTasks.forall(tid =>
      tasks.get(tid).status == Completed implies
        activeFutures.forall(fid =>
          futures.get(fid).taskId == tid implies futures.get(fid).state == Resolved
        )
    )

  // I4: Channel operations are linearizable — buffer length never exceeds capacity
  // and recv always returns the oldest message (FIFO, enforced by List structure).
  val channelLinearizable =
    activeChannels.forall(cid =>
      val ch = channels.get(cid)
      ch.buffer.length() <= ch.capacity
    )

  // I5: GIL is fair — no thread in the ready queue is permanently starved.
  // Modeled as: if a thread is in the ready queue, it is active and not terminated.
  // Combined with FIFO ready queue, this ensures eventual service.
  // (Full liveness requires temporal logic; we check the safety component here:
  //  ready queue only contains eligible threads.)
  val gilFair = and {
    // Every thread in the ready queue is active and not terminated or blocked.
    0.to(readyQueue.length() - 1).forall(i =>
      val tid = readyQueue.nth(i)
      and {
        tid.in(activeThreads),
        threads.get(tid).state != Terminated,
        threads.get(tid).state != BlockedIO,
      }
    ),
    // No thread appears in the ready queue more than once.
    0.to(readyQueue.length() - 1).forall(i =>
      (i + 1).to(readyQueue.length() - 1).forall(j =>
        readyQueue.nth(i) != readyQueue.nth(j)
      )
    ),
  }

  // I6: await only in async context — no non-async task can be in Suspended state
  // awaiting a future.
  val awaitOnlyInAsync =
    activeTasks.forall(tid =>
      val t = tasks.get(tid)
      (t.status == Suspended and t.awaitingFuture != -1) implies t.isAsyncContext
    )

  // I7: Thread state consistency — blocked I/O threads do not hold the GIL.
  val blockedNoGIL =
    activeThreads.forall(tid =>
      threads.get(tid).state == BlockedIO implies tid != gilOwner
    )

  // I8: Task-thread affinity — executing/suspended tasks belong to active threads.
  val taskThreadAffinity =
    activeTasks.forall(tid =>
      val t = tasks.get(tid)
      (t.status == Executing or t.status == Suspended) implies
        t.ownerThread.in(activeThreads)
    )

  // Combined invariant.
  val Inv = and {
    gilExclusive,
    noDataRace,
    completedTasksFuturesResolved,
    channelLinearizable,
    gilFair,
    awaitOnlyInAsync,
    blockedNoGIL,
    taskThreadAffinity,
  }
}
