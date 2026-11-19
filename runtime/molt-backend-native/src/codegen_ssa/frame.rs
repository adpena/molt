//! Frame entry and exit. The walker selects one transition. LLVM places its
//! generated body in an inline helper and Cranelift places it in the caller.

use molt_codegen_abi::frame_record::{
    FrameBuilder, FrameEntryKind, FrameShape, HomeCount, generate_frame_pop, generate_frame_push,
    generate_linked_frame_pop, generate_stateful_frame_link, generate_stateful_frame_unlink,
};

/// A function without an entry can borrow its caller's homes. A runtime-linked
/// entry owns a record too, but its exit uses the runtime ABI.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum FrameEntry {
    Absent,
    RuntimeLinked,
    Inline(FrameEntryKind),
}

impl FrameEntry {
    pub(crate) fn kind(self) -> Option<FrameEntryKind> {
        match self {
            Self::Inline(kind) => Some(kind),
            Self::Absent | Self::RuntimeLinked => None,
        }
    }
}

/// The entry and the range of homes used by a terminal body, including cold
/// location stores on exception edges. A location without locals needs a base
/// but no home slots.
pub(crate) struct FrameLayout {
    pub(crate) entry: FrameEntry,
    pub(crate) homes: Option<u64>,
}

impl FrameLayout {
    pub(crate) fn for_function(func: &crate::tir::TirFunction) -> Result<Self, String> {
        use crate::tir::ops::{AttrValue, OpCode};
        use molt_ir::OpKind;
        let mut entry = FrameEntry::Absent;
        let mut homes: Option<u64> = None;
        for op in func.blocks.values().flat_map(|block| &block.ops) {
            match op.origin {
                Some(OpKind::TraceEnterSlot) => {
                    let declared = match op.attrs.get("value") {
                        Some(AttrValue::Int(value)) => {
                            FrameEntry::Inline(FrameEntryKind::from_entry_value(*value))
                        }
                        _ => FrameEntry::RuntimeLinked,
                    };
                    if entry != FrameEntry::Absent && entry != declared {
                        return Err(format!(
                            "{}: frame entries declare incompatible record layouts",
                            func.name
                        ));
                    }
                    entry = declared;
                }
                Some(
                    OpKind::FrameHomeStore
                    | OpKind::FrameHomeCell
                    | OpKind::FrameHomePrivateCell
                    | OpKind::FrameHomeLoad
                    | OpKind::FrameHomeTake
                    | OpKind::FrameHomeClear,
                ) => {
                    let Some(AttrValue::Int(slot)) = op.attrs.get("value") else {
                        return Err(format!("{}: a frame home needs its slot index", func.name));
                    };
                    let slot = u64::try_from(*slot)
                        .map_err(|_| format!("{}: negative frame home slot {slot}", func.name))?;
                    homes = Some(homes.unwrap_or(0).max(slot + 1));
                }
                _ => {}
            }
            if op.opcode == OpCode::FrameSetLocation || op.frame_location().is_some() {
                homes.get_or_insert(0);
            }
        }
        if let FrameEntry::Inline(FrameEntryKind::Synchronous(shape)) = entry
            && homes.is_some_and(|used| used > u64::from(shape.homes))
        {
            return Err(format!(
                "{}: compiled frame addresses more homes than its entry declares",
                func.name
            ));
        }
        Ok(Self { entry, homes })
    }
}

/// Bind a chunk to the record that is current on entry. The caller emits its
/// ordinary empty return on the no-frame edge before any home can be read.
pub(crate) fn lend_homes<B: super::CodegenBuilder>(
    b: &mut B,
    slot: B::Slot,
    failed: impl FnOnce(&mut B),
) {
    let thread = b.thread_record_word();
    let address = b.word_to_address(thread);
    let homes = b.load_word(address, molt_codegen_abi::THREAD_RECORD_FRAME_TOP_OFFSET);
    b.store_slot(slot, homes);
    let zero = b.constant(0);
    let missing = b.int_compare(super::IntComparison::Equal, homes, zero);
    let fail = b.create_block("frame_homes_unlent");
    let lent = b.create_block("frame_homes_lent");
    b.branch_if(missing, (fail, &[]), (lent, &[]));
    b.switch_to_block(fail);
    failed(b);
    b.switch_to_block(lent);
}

/// Location publication uses the record base in this invocation's homes slot.
pub(crate) fn store_location<B: super::CodegenBuilder>(b: &mut B, slot: B::Slot, location: i64) {
    let homes = b.load_slot(slot);
    let address = b.word_to_address(homes);
    let value = b.constant(location as u64);
    b.store_word(address, molt_codegen_abi::FRAME_LOCATION_OFFSET, value);
}

/// Each local invocation starts with no linked frame. Initialize custody in
/// the physical entry, which executes once even when the TIR entry is a loop.
pub(crate) fn initialize_owned_execution_frame<B: super::CodegenBuilder>(
    b: &mut B,
    policy: molt_ir::ir::ExecutionContextPolicy,
    entry_kind: impl FnOnce() -> Option<FrameEntryKind>,
) -> Option<super::returns::OwnedFrame<B::Slot>> {
    if policy != molt_ir::ir::ExecutionContextPolicy::Local {
        return None;
    }
    let slot = b.entry_slot("owned_frame_base");
    let zero = b.constant(0);
    b.store_slot(slot, zero);
    Some(super::returns::OwnedFrame {
        slot,
        kind: entry_kind(),
    })
}

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
    payload: impl FnOnce(&mut P) -> P::Word,
) -> P::Word {
    use molt_codegen_abi::inline_refcount::RefcountOperation;
    let Some(kind) = kind else {
        return p.enter_runtime(code);
    };
    let thread = p.thread();
    let (body, subject, third) = match kind {
        FrameEntryKind::Stateful => (FrameBody::StatefulLink, payload(p), code),
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

/// Placement of the shared frame transition in a compiled procedure.
pub(crate) trait RecordPlacement: super::CodegenBuilder {
    /// The active record's homes slot, also used for cold traceback capture.
    fn frame_homes(&self) -> Option<Self::Slot>;
    fn set_frame_homes(&mut self, slot: Option<Self::Slot>);
}

/// The projection of a frame home borrowed by loads and taken by stores.
/// Publish both words (and the boxed result view) before a finalizer can run.
fn replace_home<B: super::CodegenBuilder>(
    b: &mut B,
    home: B::Word,
    kind: i64,
    bits: B::Word,
    boxed_view: bool,
) -> B::Word {
    use abi::word::WordComparison;
    use molt_codegen_abi as abi;
    let addr = b.word_to_address(home);
    let old_kind = b.load_word(addr, abi::FRAME_HOME_KIND_OFFSET);
    let old_bits = b.load_word(addr, abi::FRAME_HOME_BITS_OFFSET);
    let tag = b.constant(kind as u64);
    b.store_word(addr, abi::FRAME_HOME_KIND_OFFSET, tag);
    b.store_word(addr, abi::FRAME_HOME_BITS_OFFSET, bits);
    let view = if boxed_view {
        let fits = abi::boxed_word::generate_inline_int_fits(b, bits);
        b.choose(
            fits,
            |b| {
                let boxed = abi::boxed_word::generate_box_inline_int(b, bits);
                let plain = b.constant(abi::FRAME_HOME_PLAIN as u64);
                b.store_word(addr, abi::FRAME_HOME_BITS_OFFSET, boxed);
                b.store_word(addr, abi::FRAME_HOME_KIND_OFFSET, plain);
                boxed
            },
            |b| load_slow(b, home),
        )
    } else {
        bits
    };
    let owned = b.and(old_kind, abi::FRAME_HOME_HOLDS_REFERENCE as u64);
    let zero = b.constant(0);
    let no_owner = b.compare(WordComparison::Equal, owned, zero);
    let release = b.create_block("frame_home_release");
    let done = b.create_block("frame_home_replaced");
    b.branch_if(no_owner, (done, &[]), (release, &[]));
    b.switch_to_block(release);
    let site = b.refcount_site(abi::inline_refcount::RefcountOperation::Release);
    b.refcount(site, old_bits);
    b.jump(done, &[]);
    b.switch_to_block(done);
    view
}

fn load_slow<B: super::CodegenBuilder>(b: &mut B, home: B::Word) -> B::Word {
    b.call_runtime(
        molt_ir::runtime_fn_generated::RuntimeFn::FrameHomeLoad,
        &[home],
    )
    .expect("frame_home_load returns the borrowed binding")
}

impl<B: RecordPlacement> super::function::FunctionCx<'_, B> {
    pub(crate) fn frame_home_op(&mut self, b: &mut B, op: &crate::tir::ops::TirOp) -> bool {
        use super::value_table::HeldCarrier;
        use crate::tir::ops::{AttrValue, OpCode};
        use molt_codegen_abi as abi;
        use molt_ir::{OpKind, runtime_fn_generated::RuntimeFn};
        let kind = if op.opcode == OpCode::FrameSetLocation {
            OpKind::FrameSetLocation
        } else if let (OpCode::Copy, Some(kind)) = (op.opcode, op.origin) {
            kind
        } else {
            return false;
        };
        let integer = || match op.attrs.get("value") {
            Some(AttrValue::Int(v)) => Some(*v),
            _ => None,
        };
        match kind {
            OpKind::FrameSetLocation
            | OpKind::FrameHomeStore
            | OpKind::FrameHomeLoad
            | OpKind::FrameHomeCell
            | OpKind::FrameHomePrivateCell
            | OpKind::FrameHomeClear
            | OpKind::FrameHomeTake => {}
            _ => return false,
        }
        let (Some(slot), Some(index)) = (b.frame_homes(), integer()) else {
            self.fatal(format!("{kind} needs its frame homes and index"));
            return true;
        };
        if kind == OpKind::FrameSetLocation {
            store_location(b, slot, index);
            return true;
        }
        let Some(offset) = index.checked_mul(abi::FRAME_HOME_BYTES).filter(|&v| v >= 0) else {
            self.fatal("frame home index is outside the addressable range");
            return true;
        };
        let base = b.load_slot(slot);
        let home = b.add_immediate(base, offset);
        match kind {
            OpKind::FrameHomeLoad => {
                let address = b.word_to_address(home);
                let tag = b.load_word(address, abi::FRAME_HOME_KIND_OFFSET);
                let plain = b.constant(abi::FRAME_HOME_PLAIN as u64);
                let fast = b.compare(abi::word::WordComparison::Equal, tag, plain);
                let bits = b.choose(
                    fast,
                    |b| b.load_word(address, abi::FRAME_HOME_BITS_OFFSET),
                    |b| load_slow(b, home),
                );
                if let Some(&out) = op.results.first() {
                    self.bind(out, b.word_value(bits), HeldCarrier::Boxed);
                }
            }
            OpKind::FrameHomeTake => {
                let bits = b
                    .call_runtime(RuntimeFn::FrameHomeTake, &[home])
                    .expect("frame_home_take returns an owned word");
                let value = b.word_value(bits);
                self.bind_owned(b, op, value);
            }
            OpKind::FrameHomeClear => {
                let zero = b.constant(0);
                replace_home(b, home, abi::FRAME_HOME_UNBOUND, zero, false);
            }
            _ => {
                let Some(&input) = op.operands.first() else {
                    self.fatal("frame store lacks its binding");
                    return true;
                };
                let operands = self.operands();
                let value = operands.resolve(b, input);
                let held = operands.held(input);
                let (tag, bits) = match (kind, held) {
                    (OpKind::FrameHomeStore, HeldCarrier::RawI64) => {
                        (abi::FRAME_HOME_RAW_INT, b.expect_word(value))
                    }
                    (OpKind::FrameHomeCell, _) => {
                        (abi::FRAME_HOME_CELL, operands.boxed_word(b, input))
                    }
                    (OpKind::FrameHomePrivateCell, _) => {
                        (abi::FRAME_HOME_PRIVATE_CELL, operands.boxed_word(b, input))
                    }
                    _ => (abi::FRAME_HOME_PLAIN, operands.boxed_word(b, input)),
                };
                let boxed = tag == abi::FRAME_HOME_RAW_INT
                    && op
                        .results
                        .first()
                        .is_some_and(|&out| !self.plan.is_raw_int_carrier(out));
                let view = replace_home(b, home, tag, bits, boxed);
                if let Some(&out) = op.results.first() {
                    if boxed {
                        self.bind(out, b.word_value(view), HeldCarrier::Boxed);
                    } else {
                        self.bind(out, value, held);
                    }
                }
            }
        }
        true
    }
}
