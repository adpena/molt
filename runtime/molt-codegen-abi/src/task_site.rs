//! Task construction: the one place the allocation, initialization and
//! completion protocol of a generator, coroutine, async generator or future
//! is written.
//!
//! Every task constructor runs [`generate_task_construction`]: a compiled
//! `alloc_task` (its function named by operand), a task trampoline (the
//! function its caller published as the pending invocation), and an async
//! call (no Python frame). A backend implements [`TaskSiteBuilder`] beside its
//! [`FrameBuilder`] and spells each primitive with its own instructions;
//! the control flow, the frame layout and the runtime protocol live here.
//!
//! The protocol:
//! 1. `molt_task_new(poll, frame bytes, kind, function)` allocates the task.
//!    `function` is the frame-context source: its globals, builtins and code
//!    become the task frame's, or None for a task with no Python frame.
//! 2. A None result means allocation failed with an exception pending; the
//!    constructor's result is that None and nothing else runs.
//! 3. Otherwise each payload slot takes one owned word, in order, at
//!    `payload base + 8 * slot`. Every payload source is read and every raw
//!    integer boxed only here, once the task exists: a failed allocation does
//!    no payload work at all.
//! 4. Completion: a future or coroutine registers the current cancel token;
//!    an async generator wraps the generator task and releases the task's
//!    own reference; a generator returns the task.

use crate::boxed_word::{generate_box_inline_int, generate_inline_int_fits};
use crate::frame_record::FrameBuilder;
use crate::word::WordComparison;
use crate::{
    POINTER_MASK, QNAN, TAG_NONE, TASK_KIND_COROUTINE, TASK_KIND_FUTURE, TASK_KIND_GENERATOR,
    THREAD_RECORD_PENDING_INVOCATION_OFFSET,
};

const NONE_BITS: u64 = QNAN | TAG_NONE;

/// The runtime object kind `molt_task_new` allocates.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TaskRuntimeKind {
    Future,
    Generator,
    Coroutine,
}

impl TaskRuntimeKind {
    /// The `kind` word `molt_task_new` takes.
    pub const fn bits(self) -> i64 {
        match self {
            Self::Future => TASK_KIND_FUTURE,
            Self::Generator => TASK_KIND_GENERATOR,
            Self::Coroutine => TASK_KIND_COROUTINE,
        }
    }
}

/// What a constructor does with a task it initialized.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TaskCompletion {
    /// A generator: the task is the result.
    ReturnTask,
    /// A future or coroutine: it records the current cancel token.
    RegisterCancelToken,
    /// An async generator: the result wraps the generator task, which the
    /// wrapper retains; the constructor releases the task's own reference.
    WrapAsyncGen,
}

/// The frame layout and completion of one kind of task constructor.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct TaskConstructorLayout {
    diagnostic_name: &'static str,
    runtime_kind: TaskRuntimeKind,
    completion: TaskCompletion,
}

impl TaskConstructorLayout {
    pub const fn generator() -> Self {
        Self {
            diagnostic_name: "generator",
            runtime_kind: TaskRuntimeKind::Generator,
            completion: TaskCompletion::ReturnTask,
        }
    }

    pub const fn coroutine() -> Self {
        Self {
            diagnostic_name: "coroutine",
            runtime_kind: TaskRuntimeKind::Coroutine,
            completion: TaskCompletion::RegisterCancelToken,
        }
    }

    pub const fn future() -> Self {
        Self {
            diagnostic_name: "future",
            runtime_kind: TaskRuntimeKind::Future,
            completion: TaskCompletion::RegisterCancelToken,
        }
    }

    pub const fn async_generator() -> Self {
        Self {
            diagnostic_name: "async generator",
            runtime_kind: TaskRuntimeKind::Generator,
            completion: TaskCompletion::WrapAsyncGen,
        }
    }

    pub const fn for_call_async() -> Self {
        Self {
            diagnostic_name: "async call",
            runtime_kind: TaskRuntimeKind::Future,
            completion: TaskCompletion::ReturnTask,
        }
    }

    /// The layout an `alloc_task`'s `task_kind` names (a future when absent).
    pub fn for_alloc_kind(kind: Option<&str>) -> Self {
        match kind.unwrap_or("future") {
            "generator" => Self::generator(),
            "future" => Self::future(),
            "coroutine" => Self::coroutine(),
            other => panic!("unknown task kind: {other}"),
        }
    }

    pub const fn runtime_kind(self) -> TaskRuntimeKind {
        self.runtime_kind
    }

    pub const fn completion(self) -> TaskCompletion {
        self.completion
    }

    pub const fn diagnostic_name(self) -> &'static str {
        self.diagnostic_name
    }

    /// Byte offset of payload slot 0: past the generator control area for a
    /// generator, at the start for a future or coroutine.
    pub const fn payload_base_offset(self, generator_control_bytes: i32) -> i32 {
        match self.runtime_kind {
            TaskRuntimeKind::Generator => generator_control_bytes,
            TaskRuntimeKind::Future | TaskRuntimeKind::Coroutine => 0,
        }
    }

    pub fn validate_closure_size(
        self,
        closure_size: i64,
        arity: usize,
        has_closure: bool,
        generator_control_bytes: i32,
    ) {
        if closure_size < 0 {
            panic!(
                "{} closure size must be non-negative",
                self.diagnostic_name()
            );
        }
        let needed = self.required_closure_size(arity, has_closure, generator_control_bytes);
        if closure_size < needed {
            panic!(
                "{} closure size too small for trampoline",
                self.diagnostic_name()
            );
        }
    }

    /// Minimum extent for every ordinary or trampoline task constructor.
    pub fn required_closure_size(
        self,
        arity: usize,
        has_closure: bool,
        generator_control_bytes: i32,
    ) -> i64 {
        if generator_control_bytes < 0 {
            panic!("generator control size must be non-negative");
        }
        let payload_slots = arity
            .checked_add(usize::from(has_closure))
            .unwrap_or_else(|| panic!("{} payload slot count overflow", self.diagnostic_name()));
        let payload_bytes = i64::try_from(payload_slots)
            .ok()
            .and_then(|slots| slots.checked_mul(8))
            .unwrap_or_else(|| panic!("{} payload byte size overflow", self.diagnostic_name()));
        i64::from(self.payload_base_offset(generator_control_bytes))
            .checked_add(payload_bytes)
            .unwrap_or_else(|| panic!("{} closure size overflow", self.diagnostic_name()))
    }
}

/// Where a new task's frame context (globals, builtins, code) comes from.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TaskFrameContext<W> {
    /// A compiled `alloc_task`: the function it names, borrowed.
    Function(W),
    /// A task trampoline: the function its caller published in this thread
    /// record's pending-invocation word.
    PendingInvocation { thread: W },
    /// An async call: a runtime future with no Python frame.
    NoFrame,
}

impl<W> TaskFrameContext<W> {
    /// Carry the same frame source across a linkage boundary, translating
    /// its single machine word exactly once when it has one.
    pub fn map<T>(self, convert: impl FnOnce(W) -> T) -> TaskFrameContext<T> {
        match self {
            Self::Function(word) => TaskFrameContext::Function(convert(word)),
            Self::PendingInvocation { thread } => TaskFrameContext::PendingInvocation {
                thread: convert(thread),
            },
            Self::NoFrame => TaskFrameContext::NoFrame,
        }
    }
}

/// How a payload word reaches its slot.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PayloadCustody {
    /// The constructor borrows the word: the slot retains it.
    Borrowed,
    /// The word owns no reference (an immediate, or a value whose refcount the
    /// compiler proved a no-op): it is stored and never retained or released.
    Unowned,
    /// The word is a raw `i64`: the task boxes it (inline when it fits, else
    /// `molt_int_from_i64`, whose new integer the slot owns).
    RawInt,
    /// The same value as slot `k`: the slot retains slot `k`'s word, so a
    /// value repeated in the payload keeps one identity.
    SameAs(usize),
}

/// Where a task's payload words come from. Every source is read only once the
/// task exists, so a failed allocation reads no argument.
#[derive(Clone, Copy, Debug)]
pub enum TaskPayload<'a, W> {
    /// Words the constructor already holds, slot `i` at index `i`, each with
    /// its custody.
    Words(&'a [(W, PayloadCustody)]),
    /// A task trampoline's borrowed call: its closure word, if any, fills slot
    /// 0, then `count` words read from the `arguments` array; each slot
    /// retains its word.
    BorrowedArguments {
        closure: Option<W>,
        arguments: W,
        count: usize,
    },
}

/// Backend instruction spelling for the runtime calls of task construction.
/// Each method is one call of the named runtime entry.
pub trait TaskSiteBuilder: FrameBuilder {
    /// `molt_task_new(poll, frame_bytes, kind, function)`: the owned task, or
    /// None with an exception pending.
    fn task_new(
        &mut self,
        poll: Self::Word,
        frame_bytes: Self::Word,
        kind: Self::Word,
        function: Self::Word,
    ) -> Self::Word;
    /// `molt_cancel_token_get_current()`.
    fn cancel_token_get_current(&mut self) -> Self::Word;
    /// `molt_task_register_execution(task, token, None)`.
    fn task_register_execution(&mut self, task: Self::Word, token: Self::Word);
    /// `molt_asyncgen_new(generator)`: the owned async generator.
    fn asyncgen_new(&mut self, generator: Self::Word) -> Self::Word;
    /// `molt_int_from_i64(raw)`: the owned integer, for a value outside the
    /// inline range.
    fn int_from_i64(&mut self, raw: Self::Word) -> Self::Word;
}

/// The owned word of raw integer `raw`: its inline box when it fits, else a
/// new integer from the runtime (the cold path).
fn generate_box_raw_int<B: TaskSiteBuilder>(b: &mut B, raw: B::Word) -> B::Word {
    let boxed = b.local();
    b.attempt(
        |b| {
            let fits = generate_inline_int_fits(b, raw);
            b.require(fits);
            let inline = generate_box_inline_int(b, raw);
            b.set(boxed, inline);
        },
        |b| {
            let integer = b.int_from_i64(raw);
            b.set(boxed, integer);
        },
    );
    b.get(boxed)
}

/// Construct one task (the module comment states the protocol).
pub fn generate_task_construction<B: TaskSiteBuilder>(
    b: &mut B,
    layout: TaskConstructorLayout,
    generator_control_bytes: i32,
    poll: B::Word,
    frame_bytes: B::Word,
    context: TaskFrameContext<B::Word>,
    payload: TaskPayload<'_, B::Word>,
) -> B::Word {
    let function = match context {
        TaskFrameContext::Function(function) => function,
        TaskFrameContext::PendingInvocation { thread } => {
            b.load_word(thread, THREAD_RECORD_PENDING_INVOCATION_OFFSET)
        }
        TaskFrameContext::NoFrame => b.constant(NONE_BITS),
    };
    let kind = b.constant(layout.runtime_kind().bits() as u64);
    let task = b.task_new(poll, frame_bytes, kind, function);
    let result = b.local();
    b.set(result, task);
    let none = b.constant(NONE_BITS);
    let allocated = b.compare(WordComparison::NotEqual, task, none);
    let payload_base = layout.payload_base_offset(generator_control_bytes);
    b.if_then(allocated, |b| {
        let address = b.and(task, POINTER_MASK);
        let slot_offset = |slot: usize| {
            i32::try_from(slot * 8)
                .ok()
                .and_then(|bytes| payload_base.checked_add(bytes))
                .unwrap_or_else(|| panic!("{} payload offset overflow", layout.diagnostic_name()))
        };
        match payload {
            TaskPayload::Words(words) => {
                for (slot, &(word, custody)) in words.iter().enumerate() {
                    let word = match custody {
                        PayloadCustody::Borrowed => {
                            b.retain(word);
                            word
                        }
                        PayloadCustody::Unowned => word,
                        PayloadCustody::RawInt => generate_box_raw_int(b, word),
                        PayloadCustody::SameAs(first) => {
                            assert!(first < slot, "payload slot {slot} repeats slot {first}");
                            // The first slot's word, as it stored it.
                            let word = b.load_word(address, slot_offset(first));
                            b.retain(word);
                            word
                        }
                    };
                    b.store_word(address, slot_offset(slot), word);
                }
            }
            TaskPayload::BorrowedArguments {
                closure,
                arguments,
                count,
            } => {
                let first = usize::from(closure.is_some());
                if let Some(closure) = closure {
                    b.store_word(address, slot_offset(0), closure);
                    b.retain(closure);
                }
                for index in 0..count {
                    let offset = i32::try_from(index * 8).unwrap_or_else(|_| {
                        panic!("{} argument offset overflow", layout.diagnostic_name())
                    });
                    let word = b.load_word(arguments, offset);
                    b.store_word(address, slot_offset(first + index), word);
                    b.retain(word);
                }
            }
        }
        match layout.completion() {
            TaskCompletion::ReturnTask => {}
            TaskCompletion::RegisterCancelToken => {
                let token = b.cancel_token_get_current();
                b.task_register_execution(task, token);
            }
            TaskCompletion::WrapAsyncGen => {
                let wrapped = b.asyncgen_new(task);
                b.release(task);
                b.set(result, wrapped);
            }
        }
    });
    b.get(result)
}

#[cfg(test)]
mod tests;
