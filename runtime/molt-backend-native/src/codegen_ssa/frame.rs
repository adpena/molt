//! Frame entry and exit. The walker selects one transition. LLVM places its
//! generated body in an inline helper and Cranelift places it in the caller.

use molt_codegen_abi::frame_record::{
    FrameBuilder, HomeCount, generate_frame_pop, generate_frame_push, generate_linked_frame_pop,
    generate_stateful_frame_link, generate_stateful_frame_unlink,
};
use molt_ir::deterministic_hash::{HashMap, HashSet};
use molt_ir::frame_entry::{FrameEntryKind, FramePrefix, FrameShape};
use molt_ir::{ExecutionContextPolicy, OpKind};

use crate::tir::blocks::{BlockId, Terminator};
use crate::tir::function::TirFunction;
use crate::tir::ops::{AttrValue, OpCode, TirOp};

/// The physical activation an operation observes. A lent record is addressable
/// by the body, while only an owned record can be installed or retired.
#[derive(Clone, Copy)]
pub(crate) enum ActiveFrame<S> {
    Absent,
    Lent(S),
    Owned(super::returns::OwnedFrame<S>),
}

impl<S: Copy> ActiveFrame<S> {
    pub(crate) fn record(self) -> Option<S> {
        match self {
            Self::Absent => None,
            Self::Lent(slot) => Some(slot),
            Self::Owned(frame) => Some(frame.slot),
        }
    }

    pub(crate) fn owned(&self) -> Option<&super::returns::OwnedFrame<S>> {
        match self {
            Self::Owned(frame) => Some(frame),
            Self::Absent | Self::Lent(_) => None,
        }
    }
}

pub(crate) type HomeInitializations = HashSet<(BlockId, usize)>;

fn frame_dispatch_kind(op: &TirOp) -> Option<OpKind> {
    if op.opcode == OpCode::FrameSetLocation {
        Some(OpKind::FrameSetLocation)
    } else if op.opcode == OpCode::Copy {
        op.origin
    } else {
        None
    }
}

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
    pub(crate) fn for_function(func: &TirFunction) -> Result<Self, String> {
        let mut entry = FrameEntry::Absent;
        let mut homes: Option<u64> = None;
        for op in func.blocks.values().flat_map(|block| &block.ops) {
            let kind = frame_dispatch_kind(op);
            match kind {
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
            if kind == Some(OpKind::FrameSetLocation) || op.frame_location().is_some() {
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

    /// The first publications into a synchronous record's zeroed homes.
    /// Follow the entry's unique normal continuation through bookkeeping and
    /// first stores. Stop at a callback, a service poll, a rebind or a join:
    /// beyond that point the home may already own a value.
    pub(crate) fn initial_home_stores(
        &self,
        func: &TirFunction,
        reachable: &[BlockId],
        labels: &HashMap<i64, BlockId>,
    ) -> HomeInitializations {
        let mut initial = HomeInitializations::default();
        if !matches!(
            self.entry,
            FrameEntry::Inline(FrameEntryKind::Synchronous(_))
        ) || !self.homes.is_some_and(|homes| homes != 0)
            || func.execution_context != ExecutionContextPolicy::Local
            || func.frame_locals_exposed()
        {
            return initial;
        }

        // The normal edge must be the only route in. A labeled block can
        // also be the target of a check before a store in its predecessor.
        let mut predecessors = HashMap::default();
        for &id in reachable {
            let block = &func.blocks[&id];
            block.terminator.for_each_edge(|target, _| {
                let incoming = predecessors.entry(target).or_insert(Some(id));
                if *incoming != Some(id) {
                    *incoming = None;
                }
            });
        }
        for &target in labels.values() {
            predecessors.insert(target, None);
        }
        let mut id = func.entry_block;
        if predecessors.contains_key(&id) {
            return initial;
        }
        let mut ops = func.blocks[&id].ops.iter();
        if ops.next().and_then(frame_dispatch_kind) != Some(OpKind::TraceEnterSlot) {
            return initial;
        }
        // A failed push may link a record without usable homes. The ordinary
        // continuation of its entry check has the fully initialized record.
        if !ops
            .next()
            .is_some_and(|op| op.opcode == OpCode::CheckException && !op.is_async_work_poll())
        {
            return initial;
        }
        let mut skip = 2;
        let mut written = HashSet::default();
        loop {
            let block = &func.blocks[&id];
            for (index, op) in block.ops.iter().enumerate().skip(skip) {
                match frame_dispatch_kind(op) {
                    Some(OpKind::FrameSetLocation | OpKind::Line) => {}
                    Some(OpKind::FrameHomeStore) => {
                        let Some(AttrValue::Int(slot)) = op.attrs.get("value") else {
                            return initial;
                        };
                        if !written.insert(*slot) {
                            return initial;
                        }
                        initial.insert((id, index));
                        if !op.has_nothrow_proof() {
                            return initial;
                        }
                    }
                    None if op.opcode == OpCode::CheckException && !op.is_async_work_poll() => {}
                    _ => return initial,
                }
            }
            let Terminator::Branch { target, ref args } = block.terminator else {
                return initial;
            };
            // A carrier coercion on an argument edge can invoke the runtime.
            if !args.is_empty() || predecessors.get(&target) != Some(&Some(id)) {
                return initial;
            }
            id = target;
            skip = 0;
        }
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

impl FrameLayout {
    /// Resolve the physical activation in the machine entry, which executes
    /// once even when the TIR entry is a loop. Only a declared entry can
    /// acquire a record to retire. Its base also identifies a zero-home frame
    /// to a cold traceback capture.
    pub(crate) fn activate<B: super::CodegenBuilder>(
        &self,
        b: &mut B,
        policy: ExecutionContextPolicy,
        on_lend_failure: impl FnOnce(&mut B),
    ) -> ActiveFrame<B::Slot> {
        if self.entry == FrameEntry::Absent {
            if self.homes.is_some() {
                let slot = b.entry_slot("borrowed_frame_homes");
                lend_homes(b, slot, on_lend_failure);
                return ActiveFrame::Lent(slot);
            }
            return ActiveFrame::Absent;
        }
        if policy != ExecutionContextPolicy::Local {
            return ActiveFrame::Absent;
        }
        let slot = b.entry_slot("owned_frame_base");
        let zero = b.constant(0);
        b.store_slot(slot, zero);
        ActiveFrame::Owned(super::returns::OwnedFrame {
            slot,
            kind: self.entry.kind(),
        })
    }
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
pub(crate) trait RecordPlacement: super::task::TaskPlacement {
    fn push_frame(
        &mut self,
        code: Self::Word,
        kind: Option<FrameEntryKind>,
        payload: Option<Self::Word>,
    ) -> Self::Word;
}

enum PreviousHome {
    Unbound,
    Unknown,
}

/// The projection of a frame home borrowed by loads and taken by stores.
/// Publish both words (and the boxed result view) before a finalizer can run.
fn write_home<B: super::CodegenBuilder>(
    b: &mut B,
    home: B::Word,
    kind: i64,
    bits: B::Word,
    boxed_view: bool,
    previous: PreviousHome,
) -> B::Word {
    use abi::word::WordComparison;
    use molt_codegen_abi as abi;
    let addr = b.word_to_address(home);
    let previous = match previous {
        PreviousHome::Unbound => None,
        PreviousHome::Unknown => Some((
            b.load_word(addr, abi::FRAME_HOME_KIND_OFFSET),
            b.load_word(addr, abi::FRAME_HOME_BITS_OFFSET),
        )),
    };
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
    let Some((old_kind, old_bits)) = previous else {
        return view;
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
    pub(crate) fn frame_admits(&mut self, b: &mut B, op: &TirOp) {
        use super::builder::SiteBody;
        use super::value_table::HeldCarrier;
        use molt_codegen_abi::frame_record::{FrameProbe, FrameReservation};

        let plan = FramePrefix::reservation_from_attrs(&op.attrs)
            .and_then(|counts| FrameReservation::from_counts(counts.homes, counts.depth));
        let operands = self.operands();
        let zero = b.constant(0);
        let empty = b.word_value(zero);
        let mut callable = empty;
        let mut code = empty;
        let probe = plan.and_then(|mut reservation| {
            if op.opcode == OpCode::FrameRegionAdmits {
                return Some(FrameProbe::Region {
                    arena_bytes: reservation.arena_bytes,
                    recursion_depth: reservation.recursion_depth,
                    virtual_exception_level: FramePrefix::requires_virtual_level(&op.attrs),
                });
            }
            assert_eq!(op.opcode, OpCode::FrameEntryAdmits);
            let (Some(AttrValue::Str(owner)), [function]) =
                (op.attrs.get("s_value"), op.operands.as_slice())
            else {
                return None;
            };
            if operands.held(*function) != HeldCarrier::Boxed {
                return None;
            }
            reservation.virtual_exception_level = FramePrefix::requires_virtual_level(&op.attrs);
            reservation.callable_admitted = FramePrefix::region_callable_admitted(&op.attrs);
            callable = operands.resolve(b, *function);
            let slot = b.program_slot(molt_ir::code_slots::ProgramSlotKind::Code, owner);
            code = b.word_value(slot);
            Some(FrameProbe::Entry(reservation))
        });
        let admitted = if let Some(probe) = probe {
            let thread = b.thread_record_word();
            let result = b.site(
                SiteBody::FrameProbe { probe },
                &[b.word_value(thread), callable, code, empty],
            );
            b.low_bit(b.expect_word(result))
        } else {
            b.bit_constant(false)
        };
        self.bind(op.results[0], b.bit_value(admitted), HeldCarrier::RawBool);
    }

    pub(crate) fn frame_op(
        &mut self,
        b: &mut B,
        op: &TirOp,
        frame: ActiveFrame<B::Slot>,
        poll_frame: Option<B::Word>,
        initializes_home: bool,
    ) -> bool {
        use super::value_table::HeldCarrier;
        use molt_codegen_abi as abi;
        use molt_ir::runtime_fn_generated::RuntimeFn;
        let Some(kind) = frame_dispatch_kind(op) else {
            return false;
        };
        let integer = || match op.attrs.get("value") {
            Some(AttrValue::Int(v)) => Some(*v),
            _ => None,
        };
        match kind {
            OpKind::TraceExit => return true,
            OpKind::TraceEnterSlot => {
                let Some(owned) = frame.owned() else {
                    self.fatal("trace_enter_slot requires local execution-context ownership");
                    return true;
                };
                let Some(AttrValue::Str(owner)) = op.attrs.get("s_value") else {
                    self.fatal("trace_enter_slot lacks its program slot");
                    return true;
                };
                let payload = if owned.kind == Some(FrameEntryKind::Stateful) {
                    let Some(parameter) = poll_frame else {
                        self.fatal("stateful frame entry needs its poll payload");
                        return true;
                    };
                    Some(b.and(parameter, abi::POINTER_MASK))
                } else {
                    None
                };
                let code = b.program_slot(molt_ir::code_slots::ProgramSlotKind::Code, owner);
                let base = b.push_frame(code, owned.kind, payload);
                b.store_slot(owned.slot, base);
                return true;
            }
            OpKind::FrameSetLocation
            | OpKind::FrameHomeStore
            | OpKind::FrameHomeLoad
            | OpKind::FrameHomeCell
            | OpKind::FrameHomePrivateCell
            | OpKind::FrameHomeClear
            | OpKind::FrameHomeTake => {}
            _ => return false,
        }
        let (Some(slot), Some(index)) = (frame.record(), integer()) else {
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
                write_home(
                    b,
                    home,
                    abi::FRAME_HOME_UNBOUND,
                    zero,
                    false,
                    PreviousHome::Unknown,
                );
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
                let previous = if initializes_home {
                    PreviousHome::Unbound
                } else {
                    PreviousHome::Unknown
                };
                let view = write_home(b, home, tag, bits, boxed, previous);
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tir::dominators::{exception_label_to_block, executable_reverse_postorder};

    #[test]
    fn fresh_frame_home_writes_end_at_a_rebind_or_exceptional_entry() {
        let ir: molt_ir::ir::FunctionIR = serde_json::from_value(serde_json::json!({
            "name": "f",
            "execution_context": "local",
            "return_abi": "void",
            "params": ["p", "q"],
            "ops": [
                {"kind": "trace_enter_slot", "s_value": "f", "value": 3},
                {"kind": "check_exception", "exception_edge": "origin", "value": 9},
                {"kind": "frame_home_store", "args": ["p"], "out": "x", "value": 0},
                {"kind": "check_exception", "exception_edge": "origin", "value": 9},
                {"kind": "frame_home_store", "args": ["q"], "out": "y", "value": 1},
                {"kind": "frame_home_store", "args": ["q"], "out": "z", "value": 0},
                {"kind": "frame_home_store", "args": ["p"], "out": "w", "value": 2},
                {"kind": "trace_exit"},
                {"kind": "ret_void"},
                {"kind": "label", "value": 9},
                {"kind": "trace_exit"},
                {"kind": "ret_void"}
            ]
        }))
        .unwrap();
        let mut func = crate::tir::lower_from_simple::lower_to_tir(&ir);
        for block in func.blocks.values_mut() {
            for op in &mut block.ops {
                if op.origin == Some(OpKind::FrameHomeStore) {
                    op.mark_nothrow_proof();
                }
            }
        }
        let layout = FrameLayout::for_function(&func).unwrap();
        let rpo = executable_reverse_postorder(&func);
        let mut labels = exception_label_to_block(&func);
        let initial = layout.initial_home_stores(&func, &rpo, &labels);
        let &(second, _) = initial
            .iter()
            .find(|&&(block, index)| {
                func.blocks[&block].ops[index].attrs.get("value") == Some(&AttrValue::Int(1))
            })
            .expect("the second argument is also in the entry prefix");
        assert_eq!(initial.len(), 2);

        labels.insert(10, second);
        let initial = layout.initial_home_stores(&func, &rpo, &labels);
        assert_eq!(initial.len(), 1);
    }
}
