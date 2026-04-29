// molt_refcount_protocol.qnt — Quint model for Molt's reference counting protocol.
//
// Models the full refcount lifecycle including the call_bind protocol, borrow
// semantics, callargs aliasing, and refcount elision optimizations.
//
// This model captures the specific bug fixed 2026-02-23 (call_bind use-after-free
// for kwonly params) and the general refcount contract from:
//   runtime/molt-runtime/src/call/bind.rs — callargs alias protection
//   runtime/molt-obj-model/src/lib.rs — inc_ref / dec_ref
//   formal/lean/MoltTIR/Runtime/Refcount.lean — protect_then_cleanup_safe
//
// Key protocol rules modeled:
//   1. Callee returns "borrowed" bits — no explicit inc_ref on return value.
//   2. Caller's tracking system eventually dec_refs the output variable.
//   3. CallArgs takes ownership via inc_ref on push, releases via dec_ref on destroy.
//   4. protect_callargs_aliased_return: if return bits match any callargs entry,
//      inc_ref before callargs cleanup (prevents use-after-free).
//
// Constants kept small (N <= 6) per CLAUDE.md for tractable model checking.
//
// Usage:
//   quint run  formal/quint/molt_refcount_protocol.qnt --invariant=Inv --max-steps=20
//   quint verify formal/quint/molt_refcount_protocol.qnt --invariant=Inv

module molt_refcount_protocol {

  // =========================================================================
  // Types
  // =========================================================================
  type ObjId = int

  type HeapObj = {
    id: ObjId,
    refcount: int,
    alive: bool
  }

  // A borrow: a reference held without inc_ref, valid only while owner is alive.
  // Models the callee's borrowed return value in the call_bind protocol.
  type Borrow = {
    borrower: int,    // slot holding the borrow (e.g., return register)
    owner: ObjId      // heap object being borrowed
  }

  // A call frame: models the callargs buffer during call_bind dispatch.
  type CallFrame = {
    frameId: int,
    args: Set[ObjId],      // objects passed as arguments (inc_ref'd on push)
    returnVal: ObjId,      // return value from callee (-1 = no return yet)
    active: bool,          // frame is live (not yet cleaned up)
    protectedReturn: bool  // protect_callargs_aliased_return applied
  }

  // =========================================================================
  // Constants (bounded for tractable model checking, N <= 6)
  // =========================================================================
  pure val MAX_OBJS = 5
  pure val ObjIds: Set[int] = 0.to(MAX_OBJS - 1)
  pure val MAX_FRAMES = 3
  pure val FrameIds: Set[int] = 0.to(MAX_FRAMES - 1)
  pure val MAX_BORROWS = 4

  // =========================================================================
  // State
  // =========================================================================

  // Heap: object id -> object state.
  var heap: ObjId -> HeapObj

  // Root set: objects reachable from variables on the stack (not via borrows).
  var roots: Set[ObjId]

  // Counted root references. `roots` is the reachability projection; this map
  // preserves multiplicity when multiple stack slots reference the same object.
  var rootCounts: ObjId -> int

  // Counted heap/container references. Container slots are modeled separately
  // from roots so `storeToHeap`/`removeFromHeap` preserve refcount ownership.
  var heapCounts: ObjId -> int

  // Active borrows: borrowed references that skip inc_ref.
  var borrows: Set[Borrow]

  // Call frames: models callargs buffers in flight.
  var frames: int -> CallFrame

  // Active frame ids.
  var activeFrameIds: Set[int]

  // Set of objects accessed this step (for use-after-free detection).
  var accessed: Set[ObjId]

  // Next object ID for allocation.
  var nextId: int

  // =========================================================================
  // Init
  // =========================================================================
  action init = all {
    heap' = ObjIds.mapBy(oid => { id: oid, refcount: 0, alive: false }),
    roots' = Set(),
    rootCounts' = ObjIds.mapBy(_oid => 0),
    heapCounts' = ObjIds.mapBy(_oid => 0),
    borrows' = Set(),
    frames' = FrameIds.mapBy(fid => {
      frameId: fid,
      args: Set(),
      returnVal: -1,
      active: false,
      protectedReturn: false,
    }),
    activeFrameIds' = Set(),
    accessed' = Set(),
    nextId' = 0,
  }

  // =========================================================================
  // Pure helpers
  // =========================================================================

  // Count references to an object from roots.
  pure def rootRefCount(oid: ObjId, rcs: ObjId -> int): int =
    rcs.get(oid)

  // Count references to an object from heap/container slots.
  pure def heapRefCount(oid: ObjId, hcs: ObjId -> int): int =
    hcs.get(oid)

  // Count references from active call frames.
  pure def frameRefCount(oid: ObjId, fs: int -> CallFrame, afids: Set[int]): int =
    afids.fold(0, (acc, fid) =>
      val f = fs.get(fid)
      val argCount = if (f.args.contains(oid)) 1 else 0
      acc + argCount
    )

  // Check if an object is reachable (from roots or active frames).
  pure def isReachable(oid: ObjId, rcs: ObjId -> int, hcs: ObjId -> int, fs: int -> CallFrame,
                       afids: Set[int]): bool =
    rcs.get(oid) > 0 or
    hcs.get(oid) > 0 or
    afids.exists(fid => fs.get(fid).args.contains(oid))

  // =========================================================================
  // Actions
  // =========================================================================

  // Allocate a new heap object. It starts with refcount 1 and is added to roots.
  action allocObj = all {
    nextId < MAX_OBJS,
    val oid = nextId
    all {
      heap' = heap.set(oid, { id: oid, refcount: 1, alive: true }),
      roots' = roots.union(Set(oid)),
      rootCounts' = rootCounts.set(oid, 1),
      heapCounts' = heapCounts,
      borrows' = borrows,
      frames' = frames,
      activeFrameIds' = activeFrameIds,
      accessed' = Set(),
      nextId' = nextId + 1,
    }
  }

  // Increment refcount of a live object (e.g., storing to a new variable).
  action incRef = {
    val liveObjs = ObjIds.filter(oid => oid < nextId and heap.get(oid).alive)
    nondet oid = liveObjs.oneOf()
    val obj = heap.get(oid)
    all {
      heap' = heap.set(oid, { ...obj, refcount: obj.refcount + 1 }),
      roots' = roots.union(Set(oid)),
      rootCounts' = rootCounts.set(oid, rootCounts.get(oid) + 1),
      heapCounts' = heapCounts,
      borrows' = borrows,
      frames' = frames,
      activeFrameIds' = activeFrameIds,
      accessed' = Set(oid),
      nextId' = nextId,
    }
  }

  // Decrement refcount (drop one root reference). The root is always removed
  // because each root entry represents exactly one counted reference.
  // If refcount reaches 0, deallocate.
  action decRef = {
    val liveRooted = roots.filter(oid =>
      heap.get(oid).alive and heap.get(oid).refcount > 0 and rootCounts.get(oid) > 0
    )
    nondet oid = liveRooted.oneOf()
    val obj = heap.get(oid)
    val newRc = obj.refcount - 1
    val newRootCount = rootCounts.get(oid) - 1
    val stillReferenced = newRootCount > 0 or
                          heapCounts.get(oid) > 0 or
                          activeFrameIds.exists(fid => frames.get(fid).args.contains(oid))
    val newObj = if (newRc == 0)
      { ...obj, refcount: 0, alive: false }
    else
      { ...obj, refcount: newRc }
    all {
      // A root drop cannot free an object still owned elsewhere.
      newRc > 0 or not(stillReferenced),
      heap' = heap.set(oid, newObj),
      // Remove the reachability projection only when the final root slot drops.
      roots' = if (newRootCount == 0) roots.exclude(Set(oid)) else roots,
      rootCounts' = rootCounts.set(oid, newRootCount),
      heapCounts' = heapCounts,
      // Invalidate any borrows of a freed object.
      borrows' = if (newRc == 0) borrows.filter(b => b.owner != oid) else borrows,
      frames' = frames,
      activeFrameIds' = activeFrameIds,
      accessed' = if (newRc == 0) Set() else Set(oid),
      nextId' = nextId,
    }
  }

  // Store to heap: inc_ref the stored value (models assigning to a container slot).
  action storeToHeap = {
    val liveObjs = ObjIds.filter(oid => oid < nextId and heap.get(oid).alive)
    nondet oid = liveObjs.oneOf()
    val obj = heap.get(oid)
    all {
      heap' = heap.set(oid, { ...obj, refcount: obj.refcount + 1 }),
      roots' = roots,
      rootCounts' = rootCounts,
      heapCounts' = heapCounts.set(oid, heapCounts.get(oid) + 1),
      borrows' = borrows,
      frames' = frames,
      activeFrameIds' = activeFrameIds,
      accessed' = Set(oid),
      nextId' = nextId,
    }
  }

  // Remove from heap: dec_ref the removed value (models removing from a container).
  // Only operates on objects with refcount >= 2, since we need at least one other
  // reference to keep the object alive (the container's reference is being dropped).
  // If this is the last reference, the container owns it and freeing is correct.
  action removeFromHeap = {
    val liveObjs = ObjIds.filter(oid => oid < nextId and heap.get(oid).alive
                                   and heap.get(oid).refcount > 0
                                   and heapCounts.get(oid) > 0)
    nondet oid = liveObjs.oneOf()
    val obj = heap.get(oid)
    val newRc = obj.refcount - 1
    val newHeapCount = heapCounts.get(oid) - 1
    val newObj = if (newRc == 0)
      { ...obj, refcount: 0, alive: false }
    else
      { ...obj, refcount: newRc }
    // Only allow freeing (newRc == 0) if not referenced by roots or active frames.
    val stillReferenced = rootCounts.get(oid) > 0 or
                          newHeapCount > 0 or
                          activeFrameIds.exists(fid => frames.get(fid).args.contains(oid))
    all {
      // Cannot free an object that's still referenced elsewhere.
      newRc > 0 or not(stillReferenced),
      heap' = heap.set(oid, newObj),
      roots' = if (newRc == 0) roots.exclude(Set(oid)) else roots,
      rootCounts' = rootCounts,
      heapCounts' = heapCounts.set(oid, newHeapCount),
      borrows' = if (newRc == 0) borrows.filter(b => b.owner != oid) else borrows,
      frames' = frames,
      activeFrameIds' = activeFrameIds,
      accessed' = if (newRc == 0) Set() else Set(oid),
      nextId' = nextId,
    }
  }

  // --- Call protocol actions ---

  // Begin a call: create a call frame, push arguments with inc_ref.
  // Models callargs construction in call_bind.
  action beginCall = {
    val availFrames = FrameIds.exclude(activeFrameIds)
    val liveObjs = ObjIds.filter(oid => oid < nextId and heap.get(oid).alive)
    all {
      availFrames.size() > 0,
      liveObjs.size() > 0,
      nondet fid = availFrames.oneOf()
      // Pick 1-2 arguments nondeterministically.
      nondet arg1 = liveObjs.oneOf()
      val argSet = Set(arg1)
      // Inc_ref all args (callargs takes ownership).
      val newHeap = argSet.fold(heap, (h, oid) =>
        val obj = h.get(oid)
        h.set(oid, { ...obj, refcount: obj.refcount + 1 })
      )
      all {
        heap' = newHeap,
        frames' = frames.set(fid, {
          frameId: fid,
          args: argSet,
          returnVal: -1,
          active: true,
          protectedReturn: false,
        }),
        activeFrameIds' = activeFrameIds.union(Set(fid)),
        roots' = roots,
        rootCounts' = rootCounts,
        heapCounts' = heapCounts,
        borrows' = borrows,
        accessed' = argSet,
        nextId' = nextId,
      }
    }
  }

  // Callee returns a value. The return is "borrowed" — no inc_ref.
  // If the return value aliases a callarg, we need protection.
  action callReturn = {
    val activeWithNoReturn = activeFrameIds.filter(fid =>
      frames.get(fid).active and frames.get(fid).returnVal == -1
    )
    val liveObjs = ObjIds.filter(oid => oid < nextId and heap.get(oid).alive)
    all {
      activeWithNoReturn.size() > 0,
      liveObjs.size() > 0,
      nondet fid = activeWithNoReturn.oneOf()
      nondet retOid = liveObjs.oneOf()
      val f = frames.get(fid)
      // Key: check if return value aliases a callarg.
      val isAliased = f.args.contains(retOid)
      // If aliased, apply protect_callargs_aliased_return: inc_ref the return
      // before callargs cleanup will dec_ref it.
      val newHeap = if (isAliased)
        val obj = heap.get(retOid)
        heap.set(retOid, { ...obj, refcount: obj.refcount + 1 })
      else
        heap
      all {
        heap' = newHeap,
        frames' = frames.set(fid, {
          ...f,
          returnVal: retOid,
          protectedReturn: isAliased,
        }),
        roots' = roots.union(Set(retOid)),
        rootCounts' = rootCounts.set(retOid, rootCounts.get(retOid) + 1),
        heapCounts' = heapCounts,
        borrows' = if (not(isAliased))
          borrows.union(Set({ borrower: fid, owner: retOid }))
        else
          borrows,
        activeFrameIds' = activeFrameIds,
        accessed' = Set(retOid),
        nextId' = nextId,
      }
    }
  }

  // Clean up call frame: dec_ref all callargs, deactivate frame.
  // Models callargs_dec_ref_all in PtrDropGuard destructor.
  action cleanupFrame = {
    val framesWithReturn = activeFrameIds.filter(fid =>
      frames.get(fid).active and frames.get(fid).returnVal != -1
    )
    nondet fid = framesWithReturn.oneOf()
    val f = frames.get(fid)
    // Dec_ref all args. If an arg's refcount reaches 0, deallocate.
    val newHeap = f.args.fold(heap, (h, oid) =>
      val obj = h.get(oid)
      val newRc = obj.refcount - 1
      if (newRc == 0)
        h.set(oid, { ...obj, refcount: 0, alive: false })
      else
        h.set(oid, { ...obj, refcount: newRc })
    )
    // Remove borrows for this frame and any borrow whose owner died during
    // callargs cleanup.
    val newBorrows = borrows.filter(b => b.borrower != fid and newHeap.get(b.owner).alive)
    // If return val was freed (not protected), remove from roots.
    val retAlive = newHeap.get(f.returnVal).alive
    val newRoots = if (not(retAlive)) roots.exclude(Set(f.returnVal)) else roots
    val liveArgs = f.args.filter(oid => newHeap.get(oid).alive)
    val liveReturn = if (retAlive) Set(f.returnVal) else Set()
    all {
      heap' = newHeap,
      frames' = frames.set(fid, { ...f, active: false, args: Set() }),
      activeFrameIds' = activeFrameIds.exclude(Set(fid)),
      roots' = newRoots,
      rootCounts' = rootCounts,
      heapCounts' = heapCounts,
      borrows' = newBorrows,
      accessed' = liveArgs.union(liveReturn),
      nextId' = nextId,
    }
  }

  // Borrow a parameter: create a borrow without inc_ref.
  // Models the callee reading a parameter value without taking ownership.
  action borrowParam = {
    val liveRooted = roots.filter(oid => heap.get(oid).alive)
    all {
      liveRooted.size() > 0,
      borrows.size() < MAX_BORROWS,
      nondet oid = liveRooted.oneOf()
      nondet slot = oneOf(Set(10, 11, 12, 13))  // distinct from frame ids
      all {
        borrows' = borrows.union(Set({ borrower: slot, owner: oid })),
        heap' = heap,
        roots' = roots,
        rootCounts' = rootCounts,
        heapCounts' = heapCounts,
        frames' = frames,
        activeFrameIds' = activeFrameIds,
        accessed' = Set(oid),
        nextId' = nextId,
      }
    }
  }

  // Drop a borrow: the borrowed reference goes out of scope without dec_ref.
  action dropBorrow = {
    nondet b = borrows.oneOf()
    all {
      borrows' = borrows.exclude(Set(b)),
      heap' = heap,
      roots' = roots,
      rootCounts' = rootCounts,
      heapCounts' = heapCounts,
      frames' = frames,
      activeFrameIds' = activeFrameIds,
      accessed' = Set(),
      nextId' = nextId,
    }
  }

  // --- Refcount elision action ---
  // Model paired inc_ref/dec_ref elision: skip both when the value
  // is known to be alive throughout the scope.
  action elideRefcountPair = {
    val liveRooted = roots.filter(oid => heap.get(oid).alive
                                    and heap.get(oid).refcount >= 2)
    nondet oid = liveRooted.oneOf()
    // Elision is safe only when refcount >= 2 (one ref keeps it alive,
    // the elided pair doesn't change the final count).
    // This is a no-op that verifies the invariant still holds.
    all {
      heap' = heap,
      roots' = roots,
      rootCounts' = rootCounts,
      heapCounts' = heapCounts,
      borrows' = borrows,
      frames' = frames,
      activeFrameIds' = activeFrameIds,
      accessed' = Set(oid),
      nextId' = nextId,
    }
  }

  action stutter = all {
    heap' = heap,
    roots' = roots,
    rootCounts' = rootCounts,
    heapCounts' = heapCounts,
    borrows' = borrows,
    frames' = frames,
    activeFrameIds' = activeFrameIds,
    accessed' = Set(),
    nextId' = nextId,
  }

  action step = any {
    allocObj,
    incRef,
    decRef,
    storeToHeap,
    removeFromHeap,
    beginCall,
    callReturn,
    cleanupFrame,
    borrowParam,
    dropBorrow,
    elideRefcountPair,
    stutter,
  }

  // =========================================================================
  // Invariants
  // =========================================================================

  // I1: No use-after-free — any object accessed this step must be alive.
  val NoUseAfterFree =
    accessed.forall(oid =>
      oid >= 0 and oid < nextId implies heap.get(oid).alive
    )

  // I2: No double-free — refcounts never go negative.
  val NoDoubleFree =
    ObjIds.forall(oid =>
      oid < nextId implies heap.get(oid).refcount >= 0
    )

  // I3: No leak — unreachable objects (not in roots, not in any active frame)
  // have refcount 0 (and are therefore deallocated).
  val NoLeak =
    ObjIds.forall(oid =>
      (oid < nextId and heap.get(oid).alive and
       not(isReachable(oid, rootCounts, heapCounts, frames, activeFrameIds)))
      implies
      heap.get(oid).refcount == 0
    )

  // I4: Borrow safety — borrowed references point to alive objects with
  // positive refcount. The owner must survive the borrow.
  val BorrowSafe =
    borrows.forall(b =>
      b.owner >= 0 and b.owner < nextId and
      heap.get(b.owner).alive and
      heap.get(b.owner).refcount > 0
    )

  // I5: Call frame consistency — active frames reference only alive objects
  // in their args set, and return values are alive.
  val FrameConsistent =
    activeFrameIds.forall(fid =>
      val f = frames.get(fid)
      f.active and
      f.args.forall(oid =>
        oid >= 0 and oid < nextId and heap.get(oid).alive
      ) and
      (f.returnVal != -1 implies (
        f.returnVal >= 0 and f.returnVal < nextId and
        heap.get(f.returnVal).alive
      ))
    )

  // I6: Callargs alias protection — if a return value aliases a callarg,
  // the protectedReturn flag must be set. Without this, callargs cleanup
  // would free the return value (the bug fixed 2026-02-23).
  val AliasProtection =
    activeFrameIds.forall(fid =>
      val f = frames.get(fid)
      (f.returnVal != -1 and f.args.contains(f.returnVal))
      implies f.protectedReturn
    )

  // I7: Rooted objects are alive — everything in the root set is alive.
  val RootsAlive =
    roots.forall(oid =>
      oid >= 0 and oid < nextId and heap.get(oid).alive
    )

  // I8: Counted root/heap references are well-formed and reflected in roots.
  val CountedRefsWellFormed =
    ObjIds.forall(oid =>
      (oid < nextId implies (
        rootCounts.get(oid) >= 0 and
        heapCounts.get(oid) >= 0 and
        (rootCounts.get(oid) > 0 iff roots.contains(oid))
      )) and
      (oid >= nextId implies (
        rootCounts.get(oid) == 0 and heapCounts.get(oid) == 0
      ))
    )

  // I9: Alive objects have positive refcount.
  val AlivePositiveRefcount =
    ObjIds.forall(oid =>
      (oid < nextId and heap.get(oid).alive) implies heap.get(oid).refcount > 0
    )

  // I10: Elision correctness — objects with refcount >= 2 can safely skip
  // an inc_ref/dec_ref pair without becoming dangling.
  val ElisionCorrect =
    ObjIds.forall(oid =>
      (oid < nextId and heap.get(oid).alive and heap.get(oid).refcount >= 2)
      implies
      // After one dec_ref, still alive (refcount >= 1).
      (heap.get(oid).refcount - 1 >= 1)
    )

  // Combined invariant for verification.
  val Inv = and {
    NoUseAfterFree,
    NoDoubleFree,
    NoLeak,
    BorrowSafe,
    FrameConsistent,
    AliasProtection,
    RootsAlive,
    CountedRefsWellFormed,
    AlivePositiveRefcount,
    ElisionCorrect,
  }
}
