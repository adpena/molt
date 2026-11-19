//! The task constructor run on the interpreting test machine: the runtime
//! calls, the payload it writes, the references it moves and the result, for
//! every completion, every frame-context source and allocation failure.

use std::vec;
use std::vec::Vec;

use super::*;
use crate::TAG_PTR;
use crate::test_machine::{Event, MODEL_ASYNCGEN, MODEL_BIG_INT, MODEL_CANCEL_TOKEN, Machine};
use crate::thread_record::RecordWordBuilder;

const CONTROL: i32 = 48;
const POLL: u64 = 0x1000;
const FRAME_BYTES: u64 = 64;
const TASK_ADDRESS: u64 = 0x8000;
const TASK: u64 = QNAN | TAG_PTR | TASK_ADDRESS;
const FUNCTION: u64 = QNAN | TAG_PTR | 0x9000;
const NONE: u64 = QNAN | TAG_NONE;

/// Run one construction whose payload slot `i` holds word `0xA0 + i` with
/// custody `custody[i]`.
fn construct(
    machine: &mut Machine,
    layout: TaskConstructorLayout,
    context: TaskFrameContext<u64>,
    custody: &[PayloadCustody],
) -> u64 {
    let payload: Vec<(u64, PayloadCustody)> = custody
        .iter()
        .enumerate()
        .map(|(slot, &custody)| (0xA0 + slot as u64, custody))
        .collect();
    generate_task_construction(
        machine,
        layout,
        CONTROL,
        POLL,
        FRAME_BYTES,
        context,
        TaskPayload::Words(&payload),
    )
}

use PayloadCustody::{Borrowed, RawInt, SameAs, Unowned};

fn machine(task_new_result: u64) -> Machine {
    Machine {
        task_new_result,
        ..Machine::default()
    }
}

#[test]
fn a_compiled_generator_names_its_function_and_fills_its_slots() {
    let mut m = machine(TASK);
    let result = construct(
        &mut m,
        TaskConstructorLayout::generator(),
        TaskFrameContext::Function(FUNCTION),
        &[Borrowed, Unowned],
    );
    assert_eq!(result, TASK);
    // The borrowed word is retained into its slot; the unowned word is not.
    assert_eq!(
        m.events,
        vec![
            Event::Call(
                "task_new",
                vec![POLL, FRAME_BYTES, TASK_KIND_GENERATOR as u64, FUNCTION]
            ),
            Event::Retain(0xA0),
        ]
    );
    // Payload slots follow the generator control area.
    assert_eq!(m.read_bytes(TASK_ADDRESS, CONTROL, 8), 0xA0);
    assert_eq!(m.read_bytes(TASK_ADDRESS, CONTROL + 8, 8), 0xA1);
}

#[test]
fn a_failed_allocation_returns_none_and_takes_no_payload() {
    for layout in [
        TaskConstructorLayout::generator(),
        TaskConstructorLayout::coroutine(),
        TaskConstructorLayout::async_generator(),
    ] {
        let mut m = machine(NONE);
        let result = construct(
            &mut m,
            layout,
            TaskFrameContext::Function(FUNCTION),
            &[Borrowed, RawInt, SameAs(1)],
        );
        assert_eq!(result, NONE, "{}", layout.diagnostic_name());
        // No payload work at all: nothing boxed, stored or retained.
        assert_eq!(
            m.events,
            vec![Event::Call(
                "task_new",
                vec![
                    POLL,
                    FRAME_BYTES,
                    layout.runtime_kind().bits() as u64,
                    FUNCTION
                ]
            )]
        );
        assert!(m.bytes.is_empty(), "nothing is written");
    }
}

#[test]
fn a_coroutine_registers_the_current_cancel_token() {
    let mut m = machine(TASK);
    let result = construct(
        &mut m,
        TaskConstructorLayout::coroutine(),
        TaskFrameContext::Function(FUNCTION),
        &[],
    );
    assert_eq!(result, TASK);
    assert_eq!(
        m.events,
        vec![
            Event::Call(
                "task_new",
                vec![POLL, FRAME_BYTES, TASK_KIND_COROUTINE as u64, FUNCTION]
            ),
            Event::Call("cancel_token_get_current", vec![]),
            Event::Call("task_register_execution", vec![TASK, MODEL_CANCEL_TOKEN]),
        ]
    );
}

#[test]
fn an_async_generator_wraps_the_task_and_releases_its_own_reference() {
    let mut m = machine(TASK);
    let result = construct(
        &mut m,
        TaskConstructorLayout::async_generator(),
        TaskFrameContext::Function(FUNCTION),
        &[Borrowed],
    );
    assert_eq!(result, MODEL_ASYNCGEN);
    assert_eq!(
        m.events,
        vec![
            Event::Call(
                "task_new",
                vec![POLL, FRAME_BYTES, TASK_KIND_GENERATOR as u64, FUNCTION]
            ),
            Event::Retain(0xA0),
            Event::Call("asyncgen_new", vec![TASK]),
            Event::Release(TASK),
        ]
    );
}

#[test]
fn a_trampoline_passes_the_invocation_its_caller_published() {
    const THREAD: u64 = 0x4000;
    let mut m = machine(TASK);
    m.store_word(
        THREAD,
        crate::THREAD_RECORD_PENDING_INVOCATION_OFFSET,
        FUNCTION,
    );
    let result = construct(
        &mut m,
        TaskConstructorLayout::generator(),
        TaskFrameContext::PendingInvocation { thread: THREAD },
        &[],
    );
    assert_eq!(result, TASK);
    assert_eq!(
        m.events[0],
        Event::Call(
            "task_new",
            vec![POLL, FRAME_BYTES, TASK_KIND_GENERATOR as u64, FUNCTION]
        )
    );
}

#[test]
fn an_async_call_has_no_frame_and_fills_from_the_task_start() {
    let mut m = machine(TASK);
    let result = construct(
        &mut m,
        TaskConstructorLayout::for_call_async(),
        TaskFrameContext::NoFrame,
        &[Borrowed],
    );
    assert_eq!(result, TASK);
    assert_eq!(
        m.events,
        vec![
            Event::Call(
                "task_new",
                vec![POLL, FRAME_BYTES, TASK_KIND_FUTURE as u64, NONE]
            ),
            Event::Retain(0xA0),
        ]
    );
    // A future has no control area: slot 0 is at the payload's start.
    assert_eq!(m.read_bytes(TASK_ADDRESS, 0, 8), 0xA0);
}

#[test]
fn a_trampoline_reads_its_arguments_only_once_the_task_exists() {
    const THREAD: u64 = 0x4000;
    const ARGUMENTS: u64 = 0x6000;
    const CLOSURE: u64 = 0xC1;
    for (result, reads) in [(TASK, true), (NONE, false)] {
        let mut m = machine(result);
        m.store_word(
            THREAD,
            crate::THREAD_RECORD_PENDING_INVOCATION_OFFSET,
            FUNCTION,
        );
        if reads {
            m.store_word(ARGUMENTS, 0, 0xA0);
            m.store_word(ARGUMENTS, 8, 0xA1);
        }
        // An argument array nothing wrote: the model panics on any read of it,
        // so a failed allocation proves no argument is read.
        let task = generate_task_construction(
            &mut m,
            TaskConstructorLayout::generator(),
            CONTROL,
            POLL,
            FRAME_BYTES,
            TaskFrameContext::PendingInvocation { thread: THREAD },
            TaskPayload::BorrowedArguments {
                closure: Some(CLOSURE),
                arguments: ARGUMENTS,
                count: 2,
            },
        );
        assert_eq!(task, result);
        if reads {
            assert_eq!(
                m.events[1..],
                [
                    Event::Retain(CLOSURE),
                    Event::Retain(0xA0),
                    Event::Retain(0xA1)
                ]
            );
            assert_eq!(m.read_bytes(TASK_ADDRESS, CONTROL, 8), CLOSURE);
            assert_eq!(m.read_bytes(TASK_ADDRESS, CONTROL + 8, 8), 0xA0);
            assert_eq!(m.read_bytes(TASK_ADDRESS, CONTROL + 16, 8), 0xA1);
        } else {
            assert_eq!(m.events.len(), 1, "only the allocation: {:?}", m.events);
        }
    }
}

#[test]
fn raw_ints_box_once_the_task_exists_and_repeats_share_one_identity() {
    // A raw int that fits boxes inline; one that does not is minted by the
    // runtime. A repeated value reuses its first slot's word and retains it.
    let small = 7u64;
    let large = 1u64 << 60;
    let mut m = machine(TASK);
    let payload = [
        (small, RawInt),
        (large, RawInt),
        (0, SameAs(1)),
        (0xA3, Unowned),
    ];
    let result = generate_task_construction(
        &mut m,
        TaskConstructorLayout::generator(),
        CONTROL,
        POLL,
        FRAME_BYTES,
        TaskFrameContext::Function(FUNCTION),
        TaskPayload::Words(&payload),
    );
    assert_eq!(result, TASK);
    let inline_small = crate::QNAN | crate::boxed_word::BoxTag::Int.bits() | small;
    assert_eq!(
        m.events,
        vec![
            Event::Call(
                "task_new",
                vec![POLL, FRAME_BYTES, TASK_KIND_GENERATOR as u64, FUNCTION]
            ),
            Event::Call("int_from_i64", vec![large]),
            Event::Retain(MODEL_BIG_INT),
        ]
    );
    assert_eq!(m.read_bytes(TASK_ADDRESS, CONTROL, 8), inline_small);
    assert_eq!(m.read_bytes(TASK_ADDRESS, CONTROL + 8, 8), MODEL_BIG_INT);
    assert_eq!(m.read_bytes(TASK_ADDRESS, CONTROL + 16, 8), MODEL_BIG_INT);
    assert_eq!(m.read_bytes(TASK_ADDRESS, CONTROL + 24, 8), 0xA3);
}

#[test]
fn task_payload_extents_share_checked_sizing_for_every_constructor() {
    let layouts = [
        TaskConstructorLayout::for_alloc_kind(None),
        TaskConstructorLayout::for_alloc_kind(Some("generator")),
        TaskConstructorLayout::for_alloc_kind(Some("coroutine")),
        TaskConstructorLayout::for_call_async(),
        TaskConstructorLayout::generator(),
        TaskConstructorLayout::coroutine(),
        TaskConstructorLayout::async_generator(),
    ];
    for layout in layouts {
        for arity in [0, 1, 7] {
            for has_closure in [false, true] {
                let expected = i64::from(layout.payload_base_offset(32))
                    + (arity + usize::from(has_closure)) as i64 * 8;
                assert_eq!(
                    layout.required_closure_size(arity, has_closure, 32),
                    expected
                );
                layout.validate_closure_size(expected, arity, has_closure, 32);
                assert!(
                    std::panic::catch_unwind(|| {
                        layout.validate_closure_size(expected - 1, arity, has_closure, 32);
                    })
                    .is_err()
                );
            }
        }
        assert!(
            std::panic::catch_unwind(|| {
                layout.required_closure_size(usize::MAX, true, 32);
            })
            .is_err()
        );
        assert!(
            std::panic::catch_unwind(|| {
                layout.required_closure_size(0, false, -1);
            })
            .is_err()
        );
        // A byte extent can overflow even when the slot count fits usize.
        if let Ok(slots) = usize::try_from(i64::MAX / 8 + 1) {
            assert!(
                std::panic::catch_unwind(|| {
                    layout.required_closure_size(slots, false, 32);
                })
                .is_err()
            );
        }
    }
}

#[test]
fn each_layout_names_its_runtime_kind_and_completion() {
    let asyncgen = TaskConstructorLayout::async_generator();
    assert_eq!(asyncgen.runtime_kind(), TaskRuntimeKind::Generator);
    assert_eq!(asyncgen.payload_base_offset(32), 32);
    assert_eq!(asyncgen.completion(), TaskCompletion::WrapAsyncGen);
    assert_eq!(
        TaskConstructorLayout::for_alloc_kind(Some("coroutine")).completion(),
        TaskCompletion::RegisterCancelToken
    );
    assert_eq!(
        TaskConstructorLayout::for_call_async().completion(),
        TaskCompletion::ReturnTask
    );
}
