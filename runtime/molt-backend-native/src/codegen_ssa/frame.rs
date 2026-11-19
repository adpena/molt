//! Frame entry and exit. The walker selects one transition. LLVM places its
//! generated body in an inline helper and Cranelift places it in the caller.

use molt_codegen_abi::frame_record::{
    FrameBuilder, FrameEntryKind, FrameShape, HomeCount, generate_frame_pop, generate_frame_push,
    generate_linked_frame_pop, generate_stateful_frame_link, generate_stateful_frame_unlink,
};

/// An inline frame helper's three logical words. An entry returns the new
/// base. An exit has no result. The third word is the RC admission except
/// for StatefulLink, whose persistent homes need no RC operations.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum FrameBody {
    Push { homes: u32 },
    Pop(FrameShape),
    PopRuntimeLinked,
    StatefulLink,
    StatefulUnlink,
}

impl FrameBody {
    pub(crate) fn generate<B: FrameBuilder>(
        self,
        b: &mut B,
        thread: B::Word,
        subject: B::Word,
        third: B::Word,
    ) -> Option<B::Word> {
        let base = match self {
            Self::Push { homes } => {
                generate_frame_push(b, thread, subject, HomeCount::Constant(homes))
            }
            Self::StatefulLink => generate_stateful_frame_link(b, thread, subject, third),
            Self::Pop(shape) => {
                generate_frame_pop(
                    b,
                    thread,
                    subject,
                    HomeCount::Constant(shape.homes),
                    shape.descending,
                );
                return None;
            }
            Self::StatefulUnlink => {
                generate_stateful_frame_unlink(b, thread, subject);
                return None;
            }
            Self::PopRuntimeLinked => {
                generate_linked_frame_pop(b, subject);
                return None;
            }
        };
        Some(b.get(base))
    }
}

/// Per-function inputs and placement. Each operand is requested only on the
/// path that uses it. A runtime-linked entry needs neither the execution
/// record nor reference-count admission.
pub(crate) trait FramePlacement {
    type Word: Copy;
    fn thread(&mut self) -> Self::Word;
    fn payload(&mut self) -> Self::Word;
    fn admission(&mut self, op: molt_codegen_abi::inline_refcount::RefcountOperation)
    -> Self::Word;
    fn zero(&mut self) -> Self::Word;
    fn enter_runtime(&mut self, code: Self::Word) -> Self::Word;
    fn place(&mut self, body: FrameBody, args: [Self::Word; 3]) -> Option<Self::Word>;
}

pub(crate) fn enter<P: FramePlacement>(
    p: &mut P,
    code: P::Word,
    kind: Option<FrameEntryKind>,
) -> P::Word {
    use molt_codegen_abi::inline_refcount::RefcountOperation;
    let Some(kind) = kind else {
        return p.enter_runtime(code);
    };
    let thread = p.thread();
    let (body, subject, third) = match kind {
        FrameEntryKind::Stateful => (FrameBody::StatefulLink, p.payload(), code),
        FrameEntryKind::Synchronous(shape) => (
            FrameBody::Push { homes: shape.homes },
            code,
            p.admission(RefcountOperation::Retain),
        ),
    };
    p.place(body, [thread, subject, third])
        .expect("frame entry returns its base")
}

pub(crate) fn leave<P: FramePlacement>(p: &mut P, base: P::Word, kind: Option<FrameEntryKind>) {
    use molt_codegen_abi::inline_refcount::RefcountOperation;
    let (body, thread, admission) = match kind {
        Some(FrameEntryKind::Synchronous(shape)) => (
            FrameBody::Pop(shape),
            p.thread(),
            p.admission(RefcountOperation::Release),
        ),
        Some(FrameEntryKind::Stateful) => (FrameBody::StatefulUnlink, p.thread(), p.zero()),
        None => {
            let zero = p.zero();
            (FrameBody::PopRuntimeLinked, zero, zero)
        }
    };
    p.place(body, [thread, base, admission]);
}
