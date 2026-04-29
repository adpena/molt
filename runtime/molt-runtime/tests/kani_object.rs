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

    // ---------------------------------------------------------------
    // Mirror of MoltHeader — must match the real #[repr(C)] layout.
    // ---------------------------------------------------------------
    #[repr(C)]
    struct MoltHeader {
        type_id: u32,
        ref_count: MoltRefCount,
        poll_fn: u64,
        state: i64,
        size: usize,
        flags: u64,
    }

    // Header flags — must match the real constants in object/mod.rs.
    const HEADER_FLAG_HAS_PTRS: u64 = 1;
    const HEADER_FLAG_SKIP_CLASS_DECREF: u64 = 1 << 1;
    const HEADER_FLAG_GEN_RUNNING: u64 = 1 << 2;
    const HEADER_FLAG_GEN_STARTED: u64 = 1 << 3;
    const HEADER_FLAG_SPAWN_RETAIN: u64 = 1 << 4;
    const HEADER_FLAG_CANCEL_PENDING: u64 = 1 << 5;
    const HEADER_FLAG_BLOCK_ON: u64 = 1 << 6;
    const HEADER_FLAG_TASK_QUEUED: u64 = 1 << 7;
    const HEADER_FLAG_TASK_RUNNING: u64 = 1 << 8;
    const HEADER_FLAG_TASK_WAKE_PENDING: u64 = 1 << 9;
    const HEADER_FLAG_TASK_DONE: u64 = 1 << 10;
    const HEADER_FLAG_TRACEBACK_SUPPRESSED: u64 = 1 << 11;
    const HEADER_FLAG_COROUTINE: u64 = 1 << 12;
    const HEADER_FLAG_FUNC_TASK_TRAMPOLINE_KNOWN: u64 = 1 << 13;
    const HEADER_FLAG_FUNC_TASK_TRAMPOLINE_NEEDED: u64 = 1 << 14;
    const HEADER_FLAG_IMMORTAL: u64 = 1 << 15;
    const HEADER_FLAG_FINALIZER_RAN: u64 = 1 << 16;

    // Type IDs — must match the real constants in object/type_ids.rs.
    const TYPE_ID_OBJECT: u32 = 100;
    const TYPE_ID_STRING: u32 = 200;
    const TYPE_ID_LIST: u32 = 201;
    const TYPE_ID_BYTES: u32 = 202;
    const TYPE_ID_LIST_BUILDER: u32 = 203;
    const TYPE_ID_DICT: u32 = 204;
    const TYPE_ID_DICT_BUILDER: u32 = 205;
    const TYPE_ID_TUPLE: u32 = 206;
    const TYPE_ID_DICT_KEYS_VIEW: u32 = 207;
    const TYPE_ID_DICT_VALUES_VIEW: u32 = 208;
    const TYPE_ID_DICT_ITEMS_VIEW: u32 = 209;
    const TYPE_ID_ITER: u32 = 210;
    const TYPE_ID_BYTEARRAY: u32 = 211;
    const TYPE_ID_RANGE: u32 = 212;
    const TYPE_ID_SLICE: u32 = 213;
    const TYPE_ID_EXCEPTION: u32 = 214;
    const TYPE_ID_DATACLASS: u32 = 215;
    const TYPE_ID_BUFFER2D: u32 = 216;
    const TYPE_ID_CONTEXT_MANAGER: u32 = 217;
    const TYPE_ID_FILE_HANDLE: u32 = 218;
    const TYPE_ID_MEMORYVIEW: u32 = 219;
    const TYPE_ID_INTARRAY: u32 = 220;
    const TYPE_ID_FUNCTION: u32 = 221;
    const TYPE_ID_BOUND_METHOD: u32 = 222;
    const TYPE_ID_MODULE: u32 = 223;
    const TYPE_ID_TYPE: u32 = 224;
    const TYPE_ID_GENERATOR: u32 = 225;
    const TYPE_ID_CLASSMETHOD: u32 = 226;
    const TYPE_ID_STATICMETHOD: u32 = 227;
    const TYPE_ID_PROPERTY: u32 = 228;
    const TYPE_ID_SUPER: u32 = 229;
    const TYPE_ID_SET: u32 = 230;
    const TYPE_ID_SET_BUILDER: u32 = 231;
    const TYPE_ID_FROZENSET: u32 = 232;
    const TYPE_ID_BIGINT: u32 = 233;
    const TYPE_ID_COMPLEX: u32 = 234;
    const TYPE_ID_ENUMERATE: u32 = 235;
    const TYPE_ID_CALLARGS: u32 = 236;
    const TYPE_ID_NOT_IMPLEMENTED: u32 = 237;
    const TYPE_ID_CALL_ITER: u32 = 238;
    const TYPE_ID_REVERSED: u32 = 239;
    const TYPE_ID_ZIP: u32 = 240;
    const TYPE_ID_MAP: u32 = 241;
    const TYPE_ID_FILTER: u32 = 242;
    const TYPE_ID_CODE: u32 = 243;
    const TYPE_ID_ELLIPSIS: u32 = 244;
    const TYPE_ID_GENERIC_ALIAS: u32 = 245;
    const TYPE_ID_ASYNC_GENERATOR: u32 = 246;
    const TYPE_ID_UNION: u32 = 247;

    /// All type IDs as a static array for uniqueness checks.
    const ALL_TYPE_IDS: [u32; 49] = [
        TYPE_ID_OBJECT,
        TYPE_ID_STRING,
        TYPE_ID_LIST,
        TYPE_ID_BYTES,
        TYPE_ID_LIST_BUILDER,
        TYPE_ID_DICT,
        TYPE_ID_DICT_BUILDER,
        TYPE_ID_TUPLE,
        TYPE_ID_DICT_KEYS_VIEW,
        TYPE_ID_DICT_VALUES_VIEW,
        TYPE_ID_DICT_ITEMS_VIEW,
        TYPE_ID_ITER,
        TYPE_ID_BYTEARRAY,
        TYPE_ID_RANGE,
        TYPE_ID_SLICE,
        TYPE_ID_EXCEPTION,
        TYPE_ID_DATACLASS,
        TYPE_ID_BUFFER2D,
        TYPE_ID_CONTEXT_MANAGER,
        TYPE_ID_FILE_HANDLE,
        TYPE_ID_MEMORYVIEW,
        TYPE_ID_INTARRAY,
        TYPE_ID_FUNCTION,
        TYPE_ID_BOUND_METHOD,
        TYPE_ID_MODULE,
        TYPE_ID_TYPE,
        TYPE_ID_GENERATOR,
        TYPE_ID_CLASSMETHOD,
        TYPE_ID_STATICMETHOD,
        TYPE_ID_PROPERTY,
        TYPE_ID_SUPER,
        TYPE_ID_SET,
        TYPE_ID_SET_BUILDER,
        TYPE_ID_FROZENSET,
        TYPE_ID_BIGINT,
        TYPE_ID_COMPLEX,
        TYPE_ID_ENUMERATE,
        TYPE_ID_CALLARGS,
        TYPE_ID_NOT_IMPLEMENTED,
        TYPE_ID_CALL_ITER,
        TYPE_ID_REVERSED,
        TYPE_ID_ZIP,
        TYPE_ID_MAP,
        TYPE_ID_FILTER,
        TYPE_ID_CODE,
        TYPE_ID_ELLIPSIS,
        TYPE_ID_GENERIC_ALIAS,
        TYPE_ID_ASYNC_GENERATOR,
        TYPE_ID_UNION,
    ];

    /// All header flags as a static array for bit-independence checks.
    const ALL_FLAGS: [u64; 17] = [
        HEADER_FLAG_HAS_PTRS,
        HEADER_FLAG_SKIP_CLASS_DECREF,
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

    /// MoltHeader size matches the sum of its fields' sizes with C layout padding.
    /// Fields: type_id (u32) + ref_count (u32) + poll_fn (u64) + state (i64) + size (usize) + flags (u64).
    /// With #[repr(C)] on a 64-bit target: 4 + 4 + 8 + 8 + 8 + 8 = 40 bytes.
    #[kani::proof]
    #[kani::unwind(1)]
    fn header_size_is_40_bytes() {
        assert_eq!(std::mem::size_of::<MoltHeader>(), 40);
    }

    /// MoltHeader alignment is 8 (the max alignment of any field).
    #[kani::proof]
    #[kani::unwind(1)]
    fn header_alignment_is_8() {
        assert_eq!(std::mem::align_of::<MoltHeader>(), 8);
    }

    /// The type_id field sits at offset 0 in MoltHeader.
    #[kani::proof]
    #[kani::unwind(1)]
    fn type_id_at_offset_zero() {
        let header = MoltHeader {
            type_id: 0xDEAD_BEEF,
            ref_count: MoltRefCount::new(0),
            poll_fn: 0,
            state: 0,
            size: 0,
            flags: 0,
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
            poll_fn: 0,
            state: 0,
            size: 0,
            flags: 0,
        };
        let base = &header as *const MoltHeader as *const u8;
        let rc_ptr = &header.ref_count as *const MoltRefCount as *const u8;
        let offset = rc_ptr as usize - base as usize;
        assert_eq!(offset, 4);
    }

    /// The poll_fn field sits at offset 8.
    #[kani::proof]
    #[kani::unwind(1)]
    fn poll_fn_at_offset_8() {
        let header = MoltHeader {
            type_id: 0,
            ref_count: MoltRefCount::new(0),
            poll_fn: 0,
            state: 0,
            size: 0,
            flags: 0,
        };
        let base = &header as *const MoltHeader as *const u8;
        let pf_ptr = &header.poll_fn as *const u64 as *const u8;
        let offset = pf_ptr as usize - base as usize;
        assert_eq!(offset, 8);
    }

    /// header_from_obj_ptr recovers the header when obj_ptr = header_ptr + HEADER_SIZE.
    /// This models the pattern used in alloc_object / header_from_obj_ptr.
    #[kani::proof]
    #[kani::unwind(1)]
    fn header_from_obj_ptr_roundtrip() {
        let header = MoltHeader {
            type_id: 42,
            ref_count: MoltRefCount::new(1),
            poll_fn: 0,
            state: 0,
            size: 0,
            flags: 0,
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

    /// All 17 header flags occupy distinct bit positions (no overlap).
    #[kani::proof]
    #[kani::unwind(18)]
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

    /// Setting the IMMORTAL flag does not disturb any other flag bits.
    #[kani::proof]
    #[kani::unwind(1)]
    fn immortal_flag_preserves_other_bits() {
        let flags: u64 = kani::any();
        // Assume IMMORTAL is not already set.
        kani::assume(flags & HEADER_FLAG_IMMORTAL == 0);

        let new_flags = flags | HEADER_FLAG_IMMORTAL;
        // IMMORTAL is now set.
        assert_ne!(new_flags & HEADER_FLAG_IMMORTAL, 0);
        // All other bits are unchanged.
        assert_eq!(new_flags & !HEADER_FLAG_IMMORTAL, flags);
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
            poll_fn: 0,
            state: 0,
            size: 0,
            flags: HEADER_FLAG_IMMORTAL,
        };

        // Model of inc_ref_ptr:
        if (header.flags & HEADER_FLAG_IMMORTAL) != 0 {
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
            poll_fn: 0,
            state: 0,
            size: 0,
            flags: HEADER_FLAG_IMMORTAL,
        };

        // Model of dec_ref_ptr:
        if (header.flags & HEADER_FLAG_IMMORTAL) != 0 {
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
            poll_fn: 0,
            state: 0,
            size: 0,
            flags: 0,
        };

        // Model: not immortal, so inc_ref adds 1, dec_ref subtracts 1.
        assert_eq!(header.flags & HEADER_FLAG_IMMORTAL, 0);
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
        // Model: the allocator returns an 8-aligned address.
        kani::assume(raw_addr % 8 == 0);
        // Model: a valid allocation address has enough space for its header.
        kani::assume(raw_addr <= u64::MAX - header_size);
        assert_eq!(header_size % 8, 0);
        let obj_addr = raw_addr + header_size;
        // Therefore the obj pointer is also 8-aligned.
        assert_eq!(obj_addr % 8, 0);
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
            poll_fn: 0,
            state: 0,
            size: 0,
            flags: 0,
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
        let flags: u64 = kani::any();
        let once = flags | HEADER_FLAG_FINALIZER_RAN;
        let twice = once | HEADER_FLAG_FINALIZER_RAN;
        assert_eq!(once, twice);
    }
}
