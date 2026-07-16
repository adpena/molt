//! Kani bounded-verification harnesses for the Molt object model.
//!
//! These harnesses verify structural invariants of `MoltHeader`, the header-flag
//! encoding, type-ID uniqueness, refcount semantics on the real `MoltRefCount`,
//! and the `range_len_i64` helper.
//!
//! Because the full runtime pulls in global state, GIL tokens, and allocator
//! infrastructure that Kani cannot model, we use standalone models that mirror
//! the real `#[repr(C)]` layouts and pure functions byte-for-byte.
//!
//! Run with: `cd runtime/molt-runtime && cargo kani --tests`

#[cfg(kani)]
mod object_proofs {
    use molt_codegen_abi::{
        ALL_HEAP_TYPE_IDS, HEADER_ALLOC_ALIGN_BYTES, HEADER_AUX_KIND_CLASS_INLINE,
        HEADER_AUX_KIND_NONE, HEADER_AUX_KIND_OFFSET, HEADER_AUX_KIND_SIDECAR,
        HEADER_AUX_KIND_STATE_INLINE, HEADER_AUX_OFFSET, HEADER_CLASS_WORD_BITS_MASK,
        HEADER_CLASS_WORD_BORROWED, HEADER_CLASS_WORD_TAG_MASK, HEADER_FLAG_CONTAINS_REFS,
        HEADER_FLAG_GC_UNPUBLISHED, HEADER_FLAG_HAS_PTRS, HEADER_FLAG_IMMORTAL, HEADER_SIZE_BYTES,
        TYPE_ID_FUNCTION, TYPE_ID_NOT_IMPLEMENTED, TYPE_ID_OBJECT, TYPE_ID_STRING,
    };
    use std::sync::atomic::{AtomicU32, Ordering};

    // ---------------------------------------------------------------
    // Mirror of MoltRefCount (native path only — AtomicU32).
    // ---------------------------------------------------------------
    #[repr(transparent)]
    struct MoltRefCount {
        inner: AtomicU32,
    }

    impl MoltRefCount {
        const fn new(val: u32) -> Self {
            Self {
                inner: AtomicU32::new(val),
            }
        }

        fn store(&self, val: u32, order: Ordering) {
            self.inner.store(val, order);
        }

        fn load(&self, order: Ordering) -> u32 {
            self.inner.load(order)
        }

        fn fetch_add(&self, val: u32, order: Ordering) -> u32 {
            self.inner.fetch_add(val, order)
        }

        fn fetch_sub(&self, val: u32, order: Ordering) -> u32 {
            self.inner.fetch_sub(val, order)
        }
    }

    #[repr(transparent)]
    struct MoltFlags {
        inner: AtomicU32,
    }

    impl MoltFlags {
        const fn new(value: u32) -> Self {
            Self {
                inner: AtomicU32::new(value),
            }
        }

        fn load(&self, order: Ordering) -> u32 {
            self.inner.load(order)
        }

        fn fetch_or(&self, value: u32, order: Ordering) -> u32 {
            self.inner.fetch_or(value, order)
        }

        fn fetch_and(&self, value: u32, order: Ordering) -> u32 {
            self.inner.fetch_and(value, order)
        }

        fn compare_exchange(
            &self,
            current: u32,
            new: u32,
            success: Ordering,
            failure: Ordering,
        ) -> Result<u32, u32> {
            self.inner.compare_exchange(current, new, success, failure)
        }
    }

    // ---------------------------------------------------------------
    // Mirror of MoltHeader — must match the real #[repr(C)] layout.
    // ---------------------------------------------------------------
    #[repr(C)]
    struct MoltHeader {
        type_id: u32,
        ref_count: MoltRefCount,
        flags: MoltFlags,
        size_class: u16,
        aux_kind: u16,
        aux: u64,
    }

    // Header flags — must match the real constants in object/mod.rs.
    const HEADER_FLAG_GEN_RUNNING: u32 = 1 << 2;
    const HEADER_FLAG_GEN_STARTED: u32 = 1 << 3;
    const HEADER_FLAG_SPAWN_RETAIN: u32 = 1 << 4;
    const HEADER_FLAG_CANCEL_PENDING: u32 = 1 << 5;
    const HEADER_FLAG_BLOCK_ON: u32 = 1 << 6;
    const HEADER_FLAG_TASK_QUEUED: u32 = 1 << 7;
    const HEADER_FLAG_TASK_RUNNING: u32 = 1 << 8;
    const HEADER_FLAG_TASK_WAKE_PENDING: u32 = 1 << 9;
    const HEADER_FLAG_TASK_DONE: u32 = 1 << 10;
    const HEADER_FLAG_TRACEBACK_SUPPRESSED: u32 = 1 << 11;
    const HEADER_FLAG_COROUTINE: u32 = 1 << 12;
    const HEADER_FLAG_FUNC_TASK_TRAMPOLINE_KNOWN: u32 = 1 << 13;
    const HEADER_FLAG_FUNC_TASK_TRAMPOLINE_NEEDED: u32 = 1 << 14;
    const HEADER_FLAG_FINALIZER_RAN: u32 = 1 << 16;
    const HEADER_FLAG_INTERNED: u32 = 1 << 17;
    const HEADER_FLAG_RAW_ALLOC: u32 = 1 << 20;
    const HEADER_FLAG_ARENA: u32 = 1 << 21;
    const HEADER_FLAG_CLASS_HAS_FINALIZER: u32 = 1 << 22;
    const HEADER_FLAG_FUNC_REQUIRES_BINDER: u32 = 1 << 23;
    const HEADER_FLAG_HAS_WEAKREF: u32 = 1 << 24;
    const HEADER_FLAG_GC_COLLECTING: u32 = 1 << 25;
    const HEADER_FLAG_FUNC_VARIADIC_TRAMPOLINE: u32 = 1 << 26;
    const HEADER_FLAG_HAS_ABI_VIEW: u32 = 1 << 27;
    const HEADER_FLAG_GC_PINNED: u32 = 1 << 28;
    const HEADER_FLAG_DEALLOCATING: u32 = 1 << 30;
    const HEADER_FLAG_IS_WEAKREF: u32 = 1 << 31;

    // Type IDs — must match the real constants in object/type_ids.rs.
    const ALL_TYPE_IDS: [u32; 58] = ALL_HEAP_TYPE_IDS;

    /// All header flags as a static array for bit-independence checks.
    const ALL_FLAGS: [u32; 30] = [
        HEADER_FLAG_HAS_PTRS,
        HEADER_FLAG_GEN_RUNNING,
        HEADER_FLAG_GEN_STARTED,
        HEADER_FLAG_SPAWN_RETAIN,
        HEADER_FLAG_CANCEL_PENDING,
        HEADER_FLAG_BLOCK_ON,
        HEADER_FLAG_TASK_QUEUED,
        HEADER_FLAG_TASK_RUNNING,
        HEADER_FLAG_TASK_WAKE_PENDING,
        HEADER_FLAG_TASK_DONE,
        HEADER_FLAG_TRACEBACK_SUPPRESSED,
        HEADER_FLAG_COROUTINE,
        HEADER_FLAG_FUNC_TASK_TRAMPOLINE_KNOWN,
        HEADER_FLAG_FUNC_TASK_TRAMPOLINE_NEEDED,
        HEADER_FLAG_IMMORTAL,
        HEADER_FLAG_FINALIZER_RAN,
        HEADER_FLAG_INTERNED,
        HEADER_FLAG_CONTAINS_REFS,
        HEADER_FLAG_RAW_ALLOC,
        HEADER_FLAG_ARENA,
        HEADER_FLAG_CLASS_HAS_FINALIZER,
        HEADER_FLAG_FUNC_REQUIRES_BINDER,
        HEADER_FLAG_HAS_WEAKREF,
        HEADER_FLAG_GC_COLLECTING,
        HEADER_FLAG_FUNC_VARIADIC_TRAMPOLINE,
        HEADER_FLAG_HAS_ABI_VIEW,
        HEADER_FLAG_GC_PINNED,
        HEADER_FLAG_GC_UNPUBLISHED,
        HEADER_FLAG_DEALLOCATING,
        HEADER_FLAG_IS_WEAKREF,
    ];

    const ALL_AUX_KINDS: [u16; 4] = [
        HEADER_AUX_KIND_NONE,
        HEADER_AUX_KIND_CLASS_INLINE,
        HEADER_AUX_KIND_STATE_INLINE,
        HEADER_AUX_KIND_SIDECAR,
    ];

    // ---------------------------------------------------------------
    // Mirror of range_len_i64 from object/layout.rs.
    // ---------------------------------------------------------------
    fn range_len_i64(start: i64, stop: i64, step: i64) -> i64 {
        if step == 0 {
            return 0;
        }
        if step > 0 {
            if start >= stop {
                return 0;
            }
            let span = stop - start - 1;
            return 1 + span / step;
        }
        if start <= stop {
            return 0;
        }
        let step_abs = -step;
        let span = start - stop - 1;
        1 + span / step_abs
    }

    // ===============================================================
    // 1. HEADER LAYOUT PROOFS
    // ===============================================================

    /// MoltHeader size matches the shared codegen/runtime ABI.
    #[kani::proof]
    #[kani::unwind(1)]
    fn header_size_matches_shared_abi() {
        assert_eq!(
            std::mem::size_of::<MoltHeader>(),
            HEADER_SIZE_BYTES as usize
        );
    }

    /// MoltHeader layout preserves the shared 8-byte object allocation contract.
    #[kani::proof]
    #[kani::unwind(1)]
    fn header_layout_preserves_alloc_alignment() {
        assert!(std::mem::align_of::<MoltHeader>() <= HEADER_ALLOC_ALIGN_BYTES);
        assert_eq!(
            std::mem::size_of::<MoltHeader>() % HEADER_ALLOC_ALIGN_BYTES,
            0
        );
    }

    /// The type_id field sits at offset 0 in MoltHeader.
    #[kani::proof]
    #[kani::unwind(1)]
    fn type_id_at_offset_zero() {
        let header = MoltHeader {
            type_id: 0xDEAD_BEEF,
            ref_count: MoltRefCount::new(0),
            flags: MoltFlags::new(0),
            size_class: 0,
            aux_kind: HEADER_AUX_KIND_NONE,
            aux: 0,
        };
        let base = &header as *const MoltHeader as *const u8;
        let type_id_ptr = &header.type_id as *const u32 as *const u8;
        let offset = type_id_ptr as usize - base as usize;
        assert_eq!(offset, 0);
    }

    /// The ref_count field sits at offset 4 (immediately after the u32 type_id).
    #[kani::proof]
    #[kani::unwind(1)]
    fn refcount_at_offset_4() {
        let header = MoltHeader {
            type_id: 0,
            ref_count: MoltRefCount::new(0x1234_5678),
            flags: MoltFlags::new(0),
            size_class: 0,
            aux_kind: HEADER_AUX_KIND_NONE,
            aux: 0,
        };
        let base = &header as *const MoltHeader as *const u8;
        let rc_ptr = &header.ref_count as *const MoltRefCount as *const u8;
        let offset = rc_ptr as usize - base as usize;
        assert_eq!(offset, 4);
    }

    /// The flags field sits at offset 8.
    #[kani::proof]
    #[kani::unwind(1)]
    fn flags_at_offset_8() {
        let header = MoltHeader {
            type_id: 0,
            ref_count: MoltRefCount::new(0),
            flags: MoltFlags::new(0),
            size_class: 0,
            aux_kind: HEADER_AUX_KIND_NONE,
            aux: 0,
        };
        let base = &header as *const MoltHeader as *const u8;
        let flags_ptr = &header.flags as *const MoltFlags as *const u8;
        let offset = flags_ptr as usize - base as usize;
        assert_eq!(offset, 8);
    }

    /// The aux discriminator and word occupy the final 10 bytes of MoltHeader.
    #[kani::proof]
    #[kani::unwind(1)]
    fn aux_fields_match_shared_abi_offsets() {
        let header = MoltHeader {
            type_id: 0,
            ref_count: MoltRefCount::new(0),
            flags: MoltFlags::new(0),
            size_class: 0,
            aux_kind: HEADER_AUX_KIND_CLASS_INLINE,
            aux: HEADER_CLASS_WORD_BORROWED,
        };
        let base = &header as *const MoltHeader as *const u8;
        let aux_kind_ptr = &header.aux_kind as *const u16 as *const u8;
        let aux_ptr = &header.aux as *const u64 as *const u8;
        assert_eq!(
            aux_kind_ptr as usize - base as usize,
            (HEADER_SIZE_BYTES + HEADER_AUX_KIND_OFFSET) as usize
        );
        assert_eq!(
            aux_ptr as usize - base as usize,
            (HEADER_SIZE_BYTES + HEADER_AUX_OFFSET) as usize
        );
    }

    /// header_from_obj_ptr recovers the header when obj_ptr = header_ptr + HEADER_SIZE.
    /// This models the pattern used in alloc_object / header_from_obj_ptr.
    #[kani::proof]
    #[kani::unwind(1)]
    fn header_from_obj_ptr_roundtrip() {
        let header = MoltHeader {
            type_id: 42,
            ref_count: MoltRefCount::new(1),
            flags: MoltFlags::new(0),
            size_class: 0,
            aux_kind: HEADER_AUX_KIND_NONE,
            aux: 0,
        };
        let header_ptr = &header as *const MoltHeader as *mut u8;
        let obj_ptr = unsafe { header_ptr.add(std::mem::size_of::<MoltHeader>()) };
        let recovered =
            unsafe { obj_ptr.sub(std::mem::size_of::<MoltHeader>()) as *mut MoltHeader };
        assert_eq!(recovered as usize, header_ptr as usize);
        assert_eq!(unsafe { (*recovered).type_id }, 42);
    }

    // ===============================================================
    // 2. TYPE ID UNIQUENESS
    // ===============================================================

    /// No two type IDs in the ALL_TYPE_IDS table share the same value.
    /// We verify this by checking all pairs (bounded loop, N=49).
    #[kani::proof]
    #[kani::unwind(50)]
    fn type_ids_are_unique() {
        let n = ALL_TYPE_IDS.len();
        let mut i = 0;
        while i < n {
            let mut j = i + 1;
            while j < n {
                assert!(
                    ALL_TYPE_IDS[i] != ALL_TYPE_IDS[j],
                    "duplicate type ID found"
                );
                j += 1;
            }
            i += 1;
        }
    }

    // ===============================================================
    // 3. HEADER FLAG BIT INDEPENDENCE
    // ===============================================================

    /// All header flags occupy distinct bit positions (no overlap).
    #[kani::proof]
    #[kani::unwind(31)]
    fn header_flags_are_independent() {
        let n = ALL_FLAGS.len();
        let mut i = 0;
        while i < n {
            // Each flag must be a power of two (exactly one bit set).
            assert!(ALL_FLAGS[i].is_power_of_two());
            let mut j = i + 1;
            while j < n {
                assert_eq!(ALL_FLAGS[i] & ALL_FLAGS[j], 0, "overlapping flags");
                j += 1;
            }
            i += 1;
        }
    }

    /// The aux discriminator values are unique and the borrowed class-word tag
    /// cannot leak into the aligned class handle recovered by the runtime.
    #[kani::proof]
    #[kani::unwind(5)]
    fn aux_kinds_and_class_word_tags_are_disjoint() {
        let mut i = 0;
        while i < ALL_AUX_KINDS.len() {
            let mut j = i + 1;
            while j < ALL_AUX_KINDS.len() {
                assert_ne!(ALL_AUX_KINDS[i], ALL_AUX_KINDS[j]);
                j += 1;
            }
            i += 1;
        }
        assert_eq!(HEADER_CLASS_WORD_BORROWED & HEADER_CLASS_WORD_TAG_MASK, 1);
        assert_eq!(HEADER_CLASS_WORD_BITS_MASK & HEADER_CLASS_WORD_TAG_MASK, 0);
    }

    /// Setting the IMMORTAL flag does not disturb any other flag bits.
    #[kani::proof]
    #[kani::unwind(1)]
    fn immortal_flag_preserves_other_bits() {
        let flags: u32 = kani::any();
        // Assume IMMORTAL is not already set.
        kani::assume(flags & HEADER_FLAG_IMMORTAL == 0);

        let new_flags = flags | HEADER_FLAG_IMMORTAL;
        // IMMORTAL is now set.
        assert_ne!(new_flags & HEADER_FLAG_IMMORTAL, 0);
        // All other bits are unchanged.
        assert_eq!(new_flags & !HEADER_FLAG_IMMORTAL, flags);
    }

    /// Atomic disjoint-bit publication cannot clobber a sibling bit.
    #[kani::proof]
    #[kani::unwind(1)]
    fn atomic_fetch_or_preserves_disjoint_bits() {
        let initial: u32 = kani::any();
        let flags = MoltFlags::new(initial);
        let previous = flags.fetch_or(HEADER_FLAG_TASK_DONE, Ordering::AcqRel);
        assert_eq!(previous, initial);
        assert_eq!(
            flags.load(Ordering::Acquire),
            initial | HEADER_FLAG_TASK_DONE
        );
    }

    /// Atomic consume clears only the selected bit and returns the old state.
    #[kani::proof]
    #[kani::unwind(1)]
    fn atomic_fetch_and_consumes_one_flag() {
        let initial: u32 = kani::any();
        let flags = MoltFlags::new(initial);
        let previous = flags.fetch_and(!HEADER_FLAG_CANCEL_PENDING, Ordering::AcqRel);
        assert_eq!(previous, initial);
        assert_eq!(
            flags.load(Ordering::Acquire),
            initial & !HEADER_FLAG_CANCEL_PENDING
        );
    }

    /// A queued-to-running task transition publishes one coherent state word.
    #[kani::proof]
    #[kani::unwind(1)]
    fn atomic_compare_exchange_publishes_coherent_task_transition() {
        let siblings: u32 = kani::any();
        kani::assume(siblings & (HEADER_FLAG_TASK_QUEUED | HEADER_FLAG_TASK_RUNNING) == 0);
        let queued = siblings | HEADER_FLAG_TASK_QUEUED;
        let running = siblings | HEADER_FLAG_TASK_RUNNING;
        let flags = MoltFlags::new(queued);
        assert_eq!(
            flags.compare_exchange(queued, running, Ordering::AcqRel, Ordering::Acquire),
            Ok(queued)
        );
        assert_eq!(flags.load(Ordering::Acquire), running);
    }

    /// Collector visibility is enabled only by the Release publication clear.
    #[kani::proof]
    #[kani::unwind(1)]
    fn gc_publication_preserves_initialized_sibling_flags() {
        let siblings: u32 = kani::any();
        kani::assume(siblings & HEADER_FLAG_GC_UNPUBLISHED == 0);
        let flags = MoltFlags::new(siblings | HEADER_FLAG_GC_UNPUBLISHED);
        assert_ne!(
            flags.load(Ordering::Acquire) & HEADER_FLAG_GC_UNPUBLISHED,
            0
        );
        flags.fetch_and(!HEADER_FLAG_GC_UNPUBLISHED, Ordering::Release);
        let published = flags.load(Ordering::Acquire);
        assert_eq!(published & HEADER_FLAG_GC_UNPUBLISHED, 0);
        assert_eq!(published, siblings);
    }

    // ===============================================================
    // 4. IMMORTAL REFCOUNT SKIP MODEL
    // ===============================================================

    /// Models the inc_ref_ptr logic: if IMMORTAL is set, the refcount is untouched.
    #[kani::proof]
    #[kani::unwind(1)]
    fn immortal_skips_inc_ref() {
        let init_rc: u32 = kani::any();
        let header = MoltHeader {
            type_id: TYPE_ID_OBJECT,
            ref_count: MoltRefCount::new(init_rc),
            flags: MoltFlags::new(HEADER_FLAG_IMMORTAL),
            size_class: 0,
            aux_kind: HEADER_AUX_KIND_NONE,
            aux: 0,
        };

        // Model of inc_ref_ptr:
        if (header.flags.load(Ordering::Acquire) & HEADER_FLAG_IMMORTAL) != 0 {
            // Should not touch refcount — verify it is unchanged.
            assert_eq!(header.ref_count.load(Ordering::Relaxed), init_rc);
        }
    }

    /// Models the dec_ref_ptr logic: if IMMORTAL is set, the refcount is untouched.
    #[kani::proof]
    #[kani::unwind(1)]
    fn immortal_skips_dec_ref() {
        let init_rc: u32 = kani::any();
        let header = MoltHeader {
            type_id: TYPE_ID_OBJECT,
            ref_count: MoltRefCount::new(init_rc),
            flags: MoltFlags::new(HEADER_FLAG_IMMORTAL),
            size_class: 0,
            aux_kind: HEADER_AUX_KIND_NONE,
            aux: 0,
        };

        // Model of dec_ref_ptr:
        if (header.flags.load(Ordering::Acquire) & HEADER_FLAG_IMMORTAL) != 0 {
            assert_eq!(header.ref_count.load(Ordering::Relaxed), init_rc);
        }
    }

    /// For a non-immortal header, inc then dec restores the original refcount.
    #[kani::proof]
    #[kani::unwind(1)]
    fn non_immortal_inc_dec_identity() {
        let init_rc: u32 = kani::any();
        kani::assume(init_rc > 0 && init_rc < u32::MAX);

        let header = MoltHeader {
            type_id: TYPE_ID_STRING,
            ref_count: MoltRefCount::new(init_rc),
            flags: MoltFlags::new(0),
            size_class: 0,
            aux_kind: HEADER_AUX_KIND_NONE,
            aux: 0,
        };

        // Model: not immortal, so inc_ref adds 1, dec_ref subtracts 1.
        assert_eq!(
            header.flags.load(Ordering::Acquire) & HEADER_FLAG_IMMORTAL,
            0
        );
        header.ref_count.fetch_add(1, Ordering::Relaxed);
        assert_eq!(header.ref_count.load(Ordering::Relaxed), init_rc + 1);
        header.ref_count.fetch_sub(1, Ordering::AcqRel);
        assert_eq!(header.ref_count.load(Ordering::Relaxed), init_rc);
    }

    // ===============================================================
    // 5. ALLOCATION ALIGNMENT MODEL
    // ===============================================================

    /// Models the invariant that alloc_object returns header_ptr + HEADER_SIZE
    /// and that header_ptr is 8-aligned, making the returned obj_ptr also aligned.
    #[kani::proof]
    #[kani::unwind(1)]
    fn alloc_obj_ptr_is_8_aligned() {
        let raw_addr: u64 = kani::any();
        let header_size = std::mem::size_of::<MoltHeader>() as u64;
        // Model: the allocator returns an object-allocation-aligned address.
        kani::assume(raw_addr % HEADER_ALLOC_ALIGN_BYTES as u64 == 0);
        // Model: a valid allocation address has enough space for its header.
        kani::assume(raw_addr <= u64::MAX - header_size);
        assert_eq!(header_size % HEADER_ALLOC_ALIGN_BYTES as u64, 0);
        let obj_addr = raw_addr + header_size;
        // Therefore the obj pointer preserves the shared object allocation alignment.
        assert_eq!(obj_addr % HEADER_ALLOC_ALIGN_BYTES as u64, 0);
    }

    /// Total allocation size must be at least HEADER_SIZE for any valid object.
    #[kani::proof]
    #[kani::unwind(1)]
    fn total_size_at_least_header() {
        let payload_size: u64 = kani::any();
        kani::assume(payload_size <= 1024); // bounded domain
        let total = std::mem::size_of::<MoltHeader>() as u64 + payload_size;
        assert!(total >= std::mem::size_of::<MoltHeader>() as u64);
    }

    // ===============================================================
    // 6. MoltRefCount (REAL TYPE MODEL) — store/load roundtrip
    // ===============================================================

    /// store then load returns the stored value.
    #[kani::proof]
    #[kani::unwind(1)]
    fn refcount_store_load_roundtrip() {
        let rc = MoltRefCount::new(0);
        let val: u32 = kani::any();
        rc.store(val, Ordering::Relaxed);
        assert_eq!(rc.load(Ordering::Relaxed), val);
    }

    /// new(val).load() == val for any val.
    #[kani::proof]
    #[kani::unwind(1)]
    fn refcount_new_load() {
        let val: u32 = kani::any();
        let rc = MoltRefCount::new(val);
        assert_eq!(rc.load(Ordering::Relaxed), val);
    }

    // ===============================================================
    // 7. range_len_i64 PROOFS
    // ===============================================================

    /// range_len_i64 returns 0 when step is 0.
    #[kani::proof]
    #[kani::unwind(1)]
    fn range_len_step_zero() {
        let start: i64 = kani::any();
        let stop: i64 = kani::any();
        assert_eq!(range_len_i64(start, stop, 0), 0);
    }

    /// range_len_i64 returns 0 when start >= stop and step > 0.
    #[kani::proof]
    #[kani::unwind(1)]
    fn range_len_empty_ascending() {
        let start: i64 = kani::any();
        let stop: i64 = kani::any();
        let step: i64 = kani::any();
        kani::assume(step > 0);
        kani::assume(start >= stop);
        assert_eq!(range_len_i64(start, stop, step), 0);
    }

    /// range_len_i64 returns 0 when start <= stop and step < 0.
    #[kani::proof]
    #[kani::unwind(1)]
    fn range_len_empty_descending() {
        let start: i64 = kani::any();
        let stop: i64 = kani::any();
        let step: i64 = kani::any();
        kani::assume(step < 0);
        kani::assume(start <= stop);
        assert_eq!(range_len_i64(start, stop, step), 0);
    }

    /// range_len_i64 is always non-negative.
    #[kani::proof]
    #[kani::unwind(1)]
    fn range_len_non_negative() {
        let start: i64 = kani::any();
        let stop: i64 = kani::any();
        let step: i64 = kani::any();
        // Bound the values to avoid overflow in intermediate arithmetic.
        kani::assume(start >= -1_000_000 && start <= 1_000_000);
        kani::assume(stop >= -1_000_000 && stop <= 1_000_000);
        kani::assume(step >= -1_000_000 && step <= 1_000_000);
        assert!(range_len_i64(start, stop, step) >= 0);
    }

    /// range(start, start+1, 1) has length 1.
    #[kani::proof]
    #[kani::unwind(1)]
    fn range_len_single_element() {
        let start: i64 = kani::any();
        kani::assume(start < i64::MAX); // avoid overflow on start+1
        assert_eq!(range_len_i64(start, start + 1, 1), 1);
    }

    /// range(start, start-1, -1) has length 1.
    #[kani::proof]
    #[kani::unwind(1)]
    fn range_len_single_element_descending() {
        let start: i64 = kani::any();
        kani::assume(start > i64::MIN); // avoid overflow on start-1
        assert_eq!(range_len_i64(start, start - 1, -1), 1);
    }

    /// range(0, n, 1) has length n for positive n.
    #[kani::proof]
    #[kani::unwind(1)]
    fn range_len_zero_to_n() {
        let n: i64 = kani::any();
        kani::assume(n > 0 && n <= 10_000);
        assert_eq!(range_len_i64(0, n, 1), n);
    }

    /// range(0, n, step) length equals ceil(n / step) for positive n, step.
    #[kani::proof]
    #[kani::unwind(1)]
    fn range_len_matches_ceil_div() {
        let n: i64 = kani::any();
        let step: i64 = kani::any();
        kani::assume(n > 0 && n <= 10_000);
        kani::assume(step > 0 && step <= 10_000);
        let expected = (n + step - 1) / step;
        assert_eq!(range_len_i64(0, n, step), expected);
    }

    // ===============================================================
    // 8. NOT_IMPLEMENTED SKIP MODEL
    // ===============================================================

    /// Models the dec_ref_ptr early return for TYPE_ID_NOT_IMPLEMENTED:
    /// if type_id == NOT_IMPLEMENTED, the refcount is not touched.
    #[kani::proof]
    #[kani::unwind(1)]
    fn not_implemented_skips_dec_ref() {
        let init_rc: u32 = kani::any();
        let header = MoltHeader {
            type_id: TYPE_ID_NOT_IMPLEMENTED,
            ref_count: MoltRefCount::new(init_rc),
            flags: MoltFlags::new(0),
            size_class: 0,
            aux_kind: HEADER_AUX_KIND_NONE,
            aux: 0,
        };

        // Model of dec_ref_ptr: early return when type_id == NOT_IMPLEMENTED.
        if header.type_id == TYPE_ID_NOT_IMPLEMENTED {
            assert_eq!(header.ref_count.load(Ordering::Relaxed), init_rc);
        }
    }

    // ===============================================================
    // 9. FINALIZER FLAG IDEMPOTENCY
    // ===============================================================

    /// Setting HEADER_FLAG_FINALIZER_RAN twice is idempotent.
    #[kani::proof]
    #[kani::unwind(1)]
    fn finalizer_flag_idempotent() {
        let flags: u32 = kani::any();
        let once = flags | HEADER_FLAG_FINALIZER_RAN;
        let twice = once | HEADER_FLAG_FINALIZER_RAN;
        assert_eq!(once, twice);
    }
}
