//! Activation locals and pre-entry frames project one registered layout.
use super::*;
use crate::object::layout::code_set_frame_slot_id;

const GEN_POLL: u64 = 0x5eed_0001;
const CORO_POLL: u64 = 0x5eed_0002;
const REJECTED_POLL: u64 = 0x5eed_0003;
const BASE: usize = crate::GEN_CONTROL_SIZE;

fn string(py: &PyToken<'_>, text: &[u8]) -> u64 {
    // Ownership assertions need mortal names, including identifier-like strings.
    let ptr = crate::object::builders::alloc_string_nointern(py, text);
    assert!(!ptr.is_null());
    MoltObject::from_ptr(ptr).bits()
}

fn tuple(py: &PyToken<'_>, items: &[u64]) -> u64 {
    let ptr = crate::alloc_tuple(py, items);
    assert!(!ptr.is_null());
    MoltObject::from_ptr(ptr).bits()
}

fn dict(py: &PyToken<'_>) -> u64 {
    let ptr = alloc_dict_with_pairs(py, &[]);
    assert!(!ptr.is_null());
    MoltObject::from_ptr(ptr).bits()
}

fn int(value: usize) -> u64 {
    MoltObject::from_int(value as i64).bits()
}

fn flag(value: bool) -> u64 {
    MoltObject::from_bool(value).bits()
}

fn cell(value: u64) -> u64 {
    let bits = crate::object::cells::molt_cell_new(value);
    assert!(crate::object::cells::cell_ptr_from_bits(bits).is_some());
    bits
}

fn refs(bits: u64) -> u32 {
    unsafe {
        (*crate::header_from_obj_ptr(obj_from_bits(bits).as_ptr().unwrap())).ref_count_snapshot()
    }
}

fn lookup(py: &PyToken<'_>, mapping: u64, key: u64) -> Option<u64> {
    unsafe { dict_get_in_place(py, obj_from_bits(mapping).as_ptr().unwrap(), key) }
}

/// Transfer one owned value into an activation payload slot.
fn store_owned(py: &PyToken<'_>, task: *mut u8, offset: usize, bits: u64) {
    unsafe { crate::object::payload_refs::store_borrowed(py, task, offset, bits) };
    dec_ref_bits(py, bits);
}

fn take_error(py: &PyToken<'_>, name: &str) {
    assert!(crate::exception_pending(py));
    let exception = crate::molt_exception_last_pending();
    assert!(crate::builtins::exceptions::exception_matches_builtin_name(
        py, exception, name
    ));
    crate::clear_exception(py);
    dec_ref_bits(py, exception);
}

fn unregister(py: &PyToken<'_>, poll: u64) {
    let layout = crate::runtime_state(py)
        .stateful_locals
        .lock()
        .unwrap()
        .remove(&poll);
    if let Some(layout) = layout {
        crate::state::runtime_state::release_stateful_locals_layout(py, layout);
    }
}

fn register(py: &PyToken<'_>, poll: u64, names: &[u64], fields: [u64; 4]) {
    let names = tuple(py, names);
    let layout = tuple(py, &fields);
    super::activation::molt_stateful_locals_register(poll, names, layout);
    for bits in fields {
        dec_ref_bits(py, bits);
    }
    dec_ref_bits(py, layout);
    dec_ref_bits(py, names);
}

#[test]
fn created_and_suspended_generator_locals_follow_the_registered_layout() {
    let _transaction = crate::test_support::RuntimeTestTransaction::new();
    crate::with_gil_entry_nopanic!(py, {
        let (code_ptr, code) = super::tests::alloc_test_code(py);
        unsafe { code_set_frame_slot_id(code_ptr, 0) };
        let globals = dict(py);
        let builtins = dict(py);
        // Payload: closure tuple, parameters `x` and cell parameter `c`, body
        // local `body` and body cell `boxed`; free variable `free`.
        let task =
            crate::molt_task_new(GEN_POLL, (BASE + 5 * 8) as u64, crate::TASK_KIND_GENERATOR);
        let ptr = obj_from_bits(task).as_ptr().expect("generator task");
        assert!(unsafe {
            crate::object::aux_header::object_init_frame_context_unpublished(
                py, ptr, globals, builtins, code,
            )
        });
        let names = [
            string(py, b"x"),
            string(py, b"c"),
            string(py, b"body"),
            string(py, b"boxed"),
            string(py, b"free"),
        ];
        let [x, c, body, boxed, free] = names;
        register(
            py,
            GEN_POLL,
            &names,
            [
                int(2),
                tuple(
                    py,
                    &[
                        int(BASE + 8),
                        int(BASE + 16),
                        int(BASE + 24),
                        int(BASE + 32),
                    ],
                ),
                tuple(
                    py,
                    &[
                        MoltObject::from_int(-1).bits(),
                        int(0),
                        MoltObject::from_int(-1).bits(),
                        int(1),
                    ],
                ),
                int(BASE),
            ],
        );
        assert!(!crate::exception_pending(py));
        // The constructor wrote the closure and bound arguments only; body
        // slots still hold the allocator's None.
        let free_cell = cell(int(7));
        store_owned(py, ptr, BASE, tuple(py, &[free_cell]));
        dec_ref_bits(py, free_cell);
        store_owned(py, ptr, BASE + 8, int(19));
        store_owned(py, ptr, BASE + 16, int(5));

        let created = unsafe { activation_locals_bits(py, ptr) }.expect("created locals");
        assert_eq!(lookup(py, created, x), Some(int(19)));
        assert_eq!(lookup(py, created, c), Some(int(5)));
        assert_eq!(lookup(py, created, free), Some(int(7)));
        assert_eq!(lookup(py, created, body), None, "body locals are unbound");
        assert_eq!(lookup(py, created, boxed), None, "body cells are unbound");
        dec_ref_bits(py, created);

        // Throw before first execution: exactly one owned target frame, no
        // compiled-entry handoff, and a traceback payload that outlives it.
        // The frame's bindings are the task's, read through its payload.
        let depth = FRAME_STACK.with(|stack| stack.borrow().len());
        let owners = [refs(globals), refs(builtins)];
        let snapshot_of =
            |bindings: u64| bindings::frame_bindings_snapshot(py, bindings).expect("frame locals");
        let payload = {
            let scope = unsafe { ActivationFrameScope::enter(py, ptr) }.expect("admitted");
            let top = FRAME_STACK.with(|stack| *stack.borrow().last().unwrap());
            assert_eq!(FRAME_STACK.with(|stack| stack.borrow().len()), depth + 1);
            assert_eq!(
                [
                    top.code_bits,
                    top.globals_bits,
                    top.builtins_bits,
                    top.activation_bits
                ],
                [code, globals, builtins, task]
            );
            assert_eq!(top.line, unsafe { crate::code_firstlineno(code_ptr) });
            let live = snapshot_of(bindings::frame_bindings_observe(py, depth).unwrap());
            assert_eq!(lookup(py, live, x), Some(int(19)));
            assert_eq!(lookup(py, live, body), None);
            dec_ref_bits(py, live);
            assert!(take_invocation_namespace(0).is_none());
            let payload = frame_stack_trace_payload_bits(py, None, false).unwrap();
            drop(scope);
            payload
        };
        assert_eq!(FRAME_STACK.with(|stack| stack.borrow().len()), depth);
        let entry =
            unsafe { traceback_payload_frame_entry(obj_from_bits(payload).as_ptr().unwrap()) };
        assert_eq!(entry.code_bits, code);
        let recorded = snapshot_of(entry.bindings.payload_bits);
        assert_eq!(lookup(py, recorded, x), Some(int(19)));
        dec_ref_bits(py, recorded);
        dec_ref_bits(py, payload);
        // The task holds its bindings payload, and so its code, while it lives.
        assert_eq!([refs(globals), refs(builtins)], owners);

        // The compiled prologue boxes cell bindings and initializes body
        // locals; suspended views dereference exactly those cells.
        unsafe {
            (*crate::header_from_obj_ptr(ptr)).fetch_or_flags(crate::HEADER_FLAG_GEN_STARTED)
        };
        store_owned(py, ptr, BASE + 16, cell(int(5)));
        store_owned(py, ptr, BASE + 24, int(11));
        store_owned(py, ptr, BASE + 32, cell(crate::missing_bits(py)));
        unsafe { crate::object::aux_header::object_set_frame_locals_phase(ptr, 3) };
        let started = unsafe { activation_locals_bits(py, ptr) }.expect("suspended locals");
        assert_eq!(lookup(py, started, x), Some(int(19)));
        assert_eq!(
            lookup(py, started, c),
            Some(int(5)),
            "cells are dereferenced"
        );
        assert_eq!(lookup(py, started, body), Some(int(11)));
        assert_eq!(lookup(py, started, boxed), None, "an empty cell is unbound");
        assert_eq!(lookup(py, started, free), Some(int(7)));
        dec_ref_bits(py, started);

        // A started cell slot without a cell violates the layout; it is never
        // reported as the raw slot word.
        store_owned(py, ptr, BASE + 16, int(5));
        assert!(unsafe { activation_locals_bits(py, ptr) }.is_none());
        take_error(py, "SystemError");

        unsafe {
            crate::object::payload_refs::store_borrowed(
                py,
                ptr,
                crate::GEN_CLOSED_OFFSET,
                flag(true),
            )
        };
        let closed = unsafe { activation_locals_bits(py, ptr) }.expect("closed locals");
        assert_eq!(
            lookup(py, closed, x),
            None,
            "terminal activations report nothing"
        );
        dec_ref_bits(py, closed);

        unregister(py, GEN_POLL);
        dec_ref_bits(py, task);
        for bits in names.into_iter().chain([code, globals, builtins]) {
            dec_ref_bits(py, bits);
        }
    });
}

#[test]
fn coroutine_locals_use_the_same_layout_without_generator_control() {
    let _transaction = crate::test_support::RuntimeTestTransaction::new();
    crate::with_gil_entry_nopanic!(py, {
        let task = crate::molt_task_new(CORO_POLL, 16, crate::TASK_KIND_COROUTINE);
        let ptr = obj_from_bits(task).as_ptr().expect("coroutine task");
        let x = string(py, b"x");
        let local = string(py, b"local");
        register(
            py,
            CORO_POLL,
            &[x, local],
            [
                int(1),
                tuple(py, &[int(0), int(8)]),
                tuple(
                    py,
                    &[
                        MoltObject::from_int(-1).bits(),
                        MoltObject::from_int(-1).bits(),
                    ],
                ),
                MoltObject::none().bits(),
            ],
        );
        assert!(!crate::exception_pending(py));
        store_owned(py, ptr, 0, int(3));
        let created = unsafe { activation_locals_bits(py, ptr) }.expect("created coroutine");
        assert_eq!(lookup(py, created, x), Some(int(3)));
        assert_eq!(lookup(py, created, local), None);
        dec_ref_bits(py, created);
        // A runtime-native coroutine has no code identity and never fabricates
        // a Python frame, even before first execution.
        let depth = FRAME_STACK.with(|stack| stack.borrow().len());
        drop(unsafe { ActivationFrameScope::enter(py, ptr) }.expect("inert scope"));
        assert_eq!(FRAME_STACK.with(|stack| stack.borrow().len()), depth);
        crate::task_mark_done(py, ptr);
        let done = unsafe { activation_locals_bits(py, ptr) }.expect("done coroutine");
        assert_eq!(lookup(py, done, x), None);
        dec_ref_bits(py, done);
        unregister(py, CORO_POLL);
        for bits in [task, x, local] {
            dec_ref_bits(py, bits);
        }
    });
}

#[test]
fn malformed_locals_layouts_fail_before_publication() {
    let _transaction = crate::test_support::RuntimeTestTransaction::new();
    crate::with_gil_entry_nopanic!(py, {
        let x = string(py, b"x");
        let free = string(py, b"free");
        let cases: [(&[u64], [u64; 4]); 4] = [
            // Unaligned slot offset.
            (
                &[x],
                [
                    int(1),
                    tuple(py, &[int(12)]),
                    tuple(py, &[MoltObject::from_int(-1).bits()]),
                    MoltObject::none().bits(),
                ],
            ),
            // More parameters than slots.
            (
                &[x],
                [
                    int(2),
                    tuple(py, &[int(8)]),
                    tuple(py, &[MoltObject::from_int(-1).bits()]),
                    MoltObject::none().bits(),
                ],
            ),
            // A closure slot without free variables.
            (
                &[x],
                [
                    int(1),
                    tuple(py, &[int(8)]),
                    tuple(py, &[MoltObject::from_int(-1).bits()]),
                    int(0),
                ],
            ),
            // Free variables without a closure slot.
            (
                &[x, free],
                [
                    int(1),
                    tuple(py, &[int(8)]),
                    tuple(py, &[MoltObject::from_int(-1).bits()]),
                    MoltObject::none().bits(),
                ],
            ),
        ];
        for (names, fields) in cases {
            register(py, REJECTED_POLL, names, fields);
            take_error(py, "TypeError");
            assert!(
                !crate::runtime_state(py)
                    .stateful_locals
                    .lock()
                    .unwrap()
                    .contains_key(&REJECTED_POLL)
            );
        }
        let bare = tuple(py, &[x]);
        super::activation::molt_stateful_locals_register(REJECTED_POLL, bare, int(1));
        take_error(py, "TypeError");
        for bits in [bare, x, free] {
            dec_ref_bits(py, bits);
        }
    });
}

#[test]
fn live_poll_frame_owns_locals_across_partial_cell_publication_and_terminal_unwind() {
    let _transaction = crate::test_support::RuntimeTestTransaction::new();
    crate::with_gil_entry_nopanic!(py, {
        let (code_ptr, code) = super::tests::alloc_test_code(py);
        unsafe { code_set_frame_slot_id(code_ptr, 0) };
        let globals = dict(py);
        let builtins = dict(py);
        let task = crate::molt_task_new(GEN_POLL, (BASE + 16) as u64, crate::TASK_KIND_GENERATOR);
        let ptr = obj_from_bits(task).as_ptr().unwrap();
        assert!(unsafe {
            crate::object::aux_header::object_init_frame_context_unpublished(
                py, ptr, globals, builtins, code,
            )
        });
        let x = string(py, b"x");
        let body = string(py, b"body");
        register(
            py,
            GEN_POLL,
            &[x, body],
            [
                int(1),
                tuple(py, &[int(BASE), int(BASE + 8)]),
                tuple(py, &[int(0), int(1)]),
                MoltObject::none().bits(),
            ],
        );
        // A Python cell is itself a lawful argument value. Before prologue
        // publication it must not be confused with the argument's own cell.
        let raw_cell = cell(int(19));
        unsafe { crate::object::payload_refs::store_borrowed(py, ptr, BASE, raw_cell) };
        unsafe {
            (*crate::header_from_obj_ptr(ptr)).fetch_or_flags(crate::HEADER_FLAG_GEN_STARTED)
        };
        let baseline = refs(task);
        let guard =
            FrameInvocationGuard::for_activation_namespace(py, code, globals, builtins, task)
                .unwrap();
        assert_eq!(refs(task), baseline + 1);
        crate::molt_trace_enter_slot(0);
        assert_eq!(frame_stack_active_activation_bits(), task);
        assert!(take_invocation_namespace(0).is_none());
        let before = unsafe { activation_locals_bits(py, ptr) }.unwrap();
        assert_eq!(lookup(py, before, x), Some(raw_cell));
        assert_eq!(lookup(py, before, body), None);
        dec_ref_bits(py, before);
        // Constructor admission is checked before any prologue stores. A
        // non-None body owner cannot be discarded by allocation-free init.
        store_owned(py, ptr, BASE + 8, int(77));
        molt_frame_locals_begin();
        take_error(py, "SystemError");
        assert_eq!(crate::object::aux_header::object_frame_locals_phase(ptr), 0);
        assert_eq!(unsafe { *ptr.add(BASE + 8).cast::<u64>() }, int(77));
        store_owned(py, ptr, BASE + 8, MoltObject::none().bits());
        molt_frame_locals_begin();
        assert!(!crate::exception_pending(py));
        // This observation has exactly the state seen by a finalizer reentering
        // during CELL_NEW, before the first canonical cell is published.
        let partial = unsafe { activation_locals_bits(py, ptr) }.unwrap();
        assert_eq!(lookup(py, partial, x), Some(raw_cell));
        assert_eq!(lookup(py, partial, body), None);
        dec_ref_bits(py, partial);
        let x_cell = cell(raw_cell);
        molt_frame_cell_publish(int(BASE), x_cell);
        dec_ref_bits(py, x_cell);
        let partial = unsafe { activation_locals_bits(py, ptr) }.unwrap();
        assert_eq!(lookup(py, partial, x), Some(raw_cell));
        assert_eq!(lookup(py, partial, body), None);
        dec_ref_bits(py, partial);
        let body_cell = cell(int(29));
        molt_frame_cell_publish(int(BASE + 8), body_cell);
        dec_ref_bits(py, body_cell);
        assert!(!crate::exception_pending(py));
        // Once published, a cell role cannot silently revert to a raw word.
        // Error capture must also avoid recursively projecting this bad slot.
        let published = unsafe { *ptr.add(BASE).cast::<u64>() };
        inc_ref_bits(py, published);
        store_owned(py, ptr, BASE, int(99));
        assert!(frame_at_depth(py, 0).is_err());
        take_error(py, "SystemError");
        store_owned(py, ptr, BASE, published);
        // Marking a task terminal does not invalidate the live compiled frame's
        // owned payload. Suspended views are empty; unwind snapshots keep locals.
        unsafe {
            crate::object::payload_refs::store_borrowed(
                py,
                ptr,
                crate::GEN_CLOSED_OFFSET,
                flag(true),
            )
        };
        let terminal = unsafe { activation_locals_bits(py, ptr) }.unwrap();
        assert_eq!(lookup(py, terminal, x), None);
        dec_ref_bits(py, terminal);
        let payload = frame_stack_trace_payload_bits(py, None, false).unwrap();
        let entry =
            unsafe { traceback_payload_frame_entry(obj_from_bits(payload).as_ptr().unwrap()) };
        let unwind_locals =
            || bindings::frame_bindings_snapshot(py, entry.bindings.payload_bits).expect("locals");
        let recorded = unwind_locals();
        assert_eq!(lookup(py, recorded, x), Some(raw_cell));
        assert_eq!(lookup(py, recorded, body), Some(int(29)));
        dec_ref_bits(py, recorded);
        crate::molt_trace_exit();
        drop(guard);
        assert_eq!(refs(task), baseline);
        dec_ref_bits(py, task);
        // The traceback's frame took the dead activation's bindings over.
        let recorded = unwind_locals();
        assert_eq!(lookup(py, recorded, x), Some(raw_cell));
        dec_ref_bits(py, recorded);
        dec_ref_bits(py, payload);
        unregister(py, GEN_POLL);
        for bits in [raw_cell, x, body, code, globals, builtins] {
            dec_ref_bits(py, bits);
        }
    });
}

#[test]
fn traceback_prefix_preserves_observed_tail_identity_and_lazy_tail_contents() {
    let _transaction = crate::test_support::RuntimeTestTransaction::new();
    crate::with_gil_entry_nopanic!(py, {
        let (_, code) = super::tests::alloc_test_code(py);
        let globals = dict(py);
        let builtins = dict(py);
        for bits in [code, globals, builtins] {
            inc_ref_bits(py, bits);
        }
        frame_stack_push_owned(py, code, globals, builtins, 0);
        frame_stack_set_line(10);
        let lazy = frame_stack_trace_payload_bits(py, None, false).unwrap();
        let eager = traceback_payload_to_traceback_bits(py, lazy);
        assert!(!obj_from_bits(eager).is_none());
        frame_stack_set_line(20);
        let prefix = frame_stack_trace_payload_prepend_bits(py, None, false, eager).unwrap();
        let observed = traceback_payload_to_traceback_bits(py, prefix);
        let key = string(py, b"tb_next");
        let next = unsafe {
            dict_get_in_place(
                py,
                obj_from_bits(crate::instance_dict_bits(
                    obj_from_bits(observed).as_ptr().unwrap(),
                ))
                .as_ptr()
                .unwrap(),
                key,
            )
        };
        assert_eq!(
            next,
            Some(eager),
            "materialization must retain the exact observed tail"
        );
        let prefix_lazy = frame_stack_trace_payload_prepend_bits(py, None, false, lazy).unwrap();
        let entries =
            crate::object::ops_sys::traceback_payload_from_lazy_chain(py, prefix_lazy, None);
        assert_eq!(
            entries.iter().map(|entry| entry.lineno).collect::<Vec<_>>(),
            [20, 10]
        );
        let mixed = crate::object::ops_sys::traceback_payload_from_lazy_chain(py, prefix, None);
        assert_eq!(
            mixed.iter().map(|entry| entry.lineno).collect::<Vec<_>>(),
            [20, 10]
        );
        frame_stack_pop(py);
        for bits in [
            observed,
            prefix,
            prefix_lazy,
            eager,
            lazy,
            key,
            code,
            globals,
            builtins,
        ] {
            dec_ref_bits(py, bits);
        }
    });
}

#[test]
fn shared_layout_names_remain_owned_until_the_last_reader_after_replacement() {
    use crate::state::runtime_state::StatefulLocalsLease;
    let _transaction = crate::test_support::RuntimeTestTransaction::new();
    crate::with_gil_entry_nopanic!(py, {
        let old_name = string(py, b"old_binding_name");
        let new_name = string(py, b"new_binding_name");
        let baseline = [refs(old_name), refs(new_name)];
        assert_eq!(baseline, [1, 1], "each fixture owns one mortal name");
        let register_name = |name| {
            register(
                py,
                GEN_POLL,
                &[name],
                [
                    int(1),
                    tuple(py, &[int(BASE)]),
                    tuple(py, &[MoltObject::from_int(-1).bits()]),
                    MoltObject::none().bits(),
                ],
            )
        };
        register_name(old_name);
        let old_reader = {
            let registry = crate::runtime_state(py).stateful_locals.lock().unwrap();
            StatefulLocalsLease::acquire(py, registry.get(&GEN_POLL).unwrap())
        };
        register_name(new_name);
        assert_eq!(refs(old_name), baseline[0] + 1);
        assert_eq!(old_reader.slots[0].name_bits, old_name);
        let new_reader = {
            let registry = crate::runtime_state(py).stateful_locals.lock().unwrap();
            StatefulLocalsLease::acquire(py, registry.get(&GEN_POLL).unwrap())
        };
        unregister(py, GEN_POLL);
        assert_eq!(refs(new_name), baseline[1] + 1);
        drop(old_reader);
        assert_eq!(refs(old_name), baseline[0]);
        drop(new_reader);
        assert_eq!(refs(new_name), baseline[1]);
        dec_ref_bits(py, old_name);
        dec_ref_bits(py, new_name);
    });
}
