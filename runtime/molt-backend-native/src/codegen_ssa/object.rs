//! Objects (design: `docs/design/foundation/codegen_walker.md`, W2,
//! "object"): attribute and item access, written once.
//!
//! A proven field or list item access calls the backend's site helper, the
//! shared `molt_codegen_abi` generator's body placed where the backend
//! chooses; every other access is a borrowed runtime call. An attribute name
//! is a borrowed immortal literal cell.

use molt_codegen_abi as nanbox;
use molt_codegen_abi::list_item::ItemContainers;
use molt_codegen_abi::literal_cell::LiteralKind;
use molt_ir::OpKind;
use molt_ir::runtime_fn_generated::RuntimeFn;

use super::builder::{CodegenBuilder, IntComparison, MachineKind, SiteBody};
use super::constants::{fill_literal_cell, literal_cell};
use super::custody::{CallTarget, RuntimeArg};
use super::function::{FunctionCx, ResultCustody};
use super::value_table::HeldCarrier;
use crate::tir::ops::{AttrValue, OpCode, TirOp};
use crate::tir::types::TirType;
use crate::tir::values::ValueId;

/// The boxed word of `None`.
const NONE_WORD: u64 = nanbox::QNAN | nanbox::TAG_NONE;

/// Run `consume` with the attribute name `name` as a borrowed str word, and
/// return what it produced. The name is a literal cell (call_anatomy.md 4.9):
/// once filled, one load and one branch, with no runtime call and no
/// reference count, since the object is immortal. A failed first fill skips
/// `consume` and yields None with the exception pending, so the enclosing
/// operation's exception check keeps handler and cleanup flow.
pub(crate) fn with_name<B: CodegenBuilder>(
    b: &mut B,
    name: &str,
    consume: impl FnOnce(&mut B, B::Word) -> B::Word,
) -> B::Word {
    let cell = literal_cell(b, LiteralKind::Str, name.as_bytes());
    let cached = b.load_word(cell, 0);
    let cached = b.word_value(cached);
    let empty = b.is_zero(cached);
    let fill = b.create_block("name_fill");
    let ready = b.create_block("name_ready");
    let merge = b.create_block("name_merge");
    let word = b.append_block_param(ready, MachineKind::Word);
    b.set_name(word, "name");
    let result = b.append_block_param(merge, MachineKind::Word);
    b.set_name(result, "name_result");
    b.branch_if(empty, (fill, &[]), (ready, &[cached]));
    b.switch_to_block(fill);
    let filled = fill_literal_cell(b, cell, LiteralKind::Str, name.as_bytes());
    let none = b.constant(NONE_WORD);
    let failed = b.int_compare(IntComparison::Equal, b.expect_word(filled), none);
    let none = b.word_value(none);
    b.branch_if(failed, (merge, &[none]), (ready, &[filled]));
    b.switch_to_block(ready);
    let word = b.expect_word(word);
    let consumed = consume(b, word);
    let consumed = b.word_value(consumed);
    b.jump(merge, &[consumed]);
    b.switch_to_block(merge);
    b.expect_word(result)
}

/// The op's attribute name (`name`), or `<unknown>`.
fn attr_name(op: &TirOp) -> &str {
    match op.attrs.get("name") {
        Some(AttrValue::Str(name)) => name.as_str(),
        _ => "<unknown>",
    }
}

/// The op's integer `value` attribute, or 0.
fn int_attr(op: &TirOp) -> i64 {
    match op.attrs.get("value") {
        Some(AttrValue::Int(value)) => *value,
        _ => 0,
    }
}

/// The boxed inline int a word of compile-time value `value` is.
fn inline_int<B: CodegenBuilder>(b: &mut B, value: i64) -> B::Word {
    assert!(
        (nanbox::INT_MIN_INLINE..=nanbox::INT_MAX_INLINE).contains(&value),
        "compile-time integer word {value} does not fit the inline payload"
    );
    b.constant(nanbox::box_int_bits(value) as u64)
}

/// The class statement's program slot a layout op names (HF-220).
pub(crate) fn class_definition_slot<B: CodegenBuilder>(b: &mut B, op: &TirOp) -> B::Word {
    let owner = match op.attrs.get("class_definition") {
        Some(AttrValue::Str(owner)) => owner.as_str(),
        _ => panic!("admitted class statement symbol"),
    };
    b.program_slot(molt_ir::code_slots::ProgramSlotKind::Code, owner)
}

impl<B: CodegenBuilder> FunctionCx<'_, B> {
    /// The inline-int site id of an inline-cache lane at `op`'s source op.
    fn site_bits(&self, b: &mut B, op: &TirOp, lane: &str) -> B::Word {
        let source = op
            .source_op_index()
            .unwrap_or_else(|| panic!("{lane} requires source op index"));
        let site = molt_codegen_abi::stable_ic_site_id(self.func.name.as_str(), source, lane);
        inline_int(b, site)
    }

    /// The exact builtins `container`'s refined TIR type lets an item read
    /// admit, or `None` for a dict, set or str, which keeps its runtime
    /// entry without the tests.
    fn item_read_containers(&self, container: ValueId) -> Option<ItemContainers> {
        match self.func.value_types.get(&container) {
            Some(TirType::Dict(_, _) | TirType::Set(_) | TirType::Str) => None,
            Some(TirType::Tuple(_)) => Some(ItemContainers::Tuple),
            Some(TirType::List(_)) => Some(ItemContainers::Lists),
            _ => Some(ItemContainers::ListsOrTuple),
        }
    }

    /// Whether `container`'s refined TIR type admits a list store.
    fn container_may_be_list(&self, container: ValueId) -> bool {
        !matches!(
            self.func.value_types.get(&container),
            Some(TirType::Dict(_, _) | TirType::Tuple(_) | TirType::Set(_) | TirType::Str)
        )
    }

    fn bind_boxed(&mut self, b: &mut B, id: ValueId, word: B::Word) {
        let value = b.word_value(word);
        self.bind(id, value, HeldCarrier::of(&TirType::DynBox));
    }

    /// Lower an object op: `LoadAttr`, `StoreAttr`, `DelAttr`, `Index`,
    /// `StoreIndex` or `DelIndex`.
    pub(crate) fn object_op(&mut self, b: &mut B, op: &TirOp) {
        match op.opcode {
            OpCode::LoadAttr => self.load_attr(b, op),
            OpCode::StoreAttr => self.store_attr(b, op),
            OpCode::DelAttr => self.del_attr(b, op),
            OpCode::Index => self.index(b, op),
            OpCode::StoreIndex => self.store_index(b, op),
            OpCode::DelIndex => self.del_index(b, op),
            other => unreachable!("{other:?} is not an object op"),
        }
    }

    /// `object()` (`class` None) or a proven `C()` allocation: the shared
    /// lifecycle allocation (`SiteBody::ObjectAlloc`), whose miss is the
    /// runtime constructor. Only `object()` reaches it until compiled code
    /// admits GC members inline (S4).
    pub(crate) fn object_alloc(&mut self, b: &mut B, op: &TirOp, class: Option<ValueId>) {
        let profile = b.profile_enabled();
        let admission = b.refcount_admission();
        let (root, class) = match class {
            Some(class) => (false, RuntimeArg::Operand(class)),
            None => {
                let thread = b.thread_record_word();
                let thread = b.word_to_address(thread);
                let class = b.load_word(
                    thread,
                    molt_codegen_abi::thread_record::ThreadRecordWord::ObjectClass.offset(),
                );
                (true, RuntimeArg::Word(b.word_value(class)))
            }
        };
        let args = [
            class,
            RuntimeArg::Word(b.word_value(profile)),
            RuntimeArg::Word(b.word_value(admission)),
        ];
        let word = self.operands().call(
            b,
            // The closed-world class model's proven plan replaces the guarded
            // load where it holds (design step 7).
            CallTarget::Site(SiteBody::ObjectAlloc {
                root,
                plan: molt_codegen_abi::object_lifecycle::PlanSource::Guarded,
            }),
            &args,
            false,
            "object_new",
            "object_alloc",
        );
        self.bind_owned(b, op, b.word_value(word));
    }

    fn load_attr(&mut self, b: &mut B, op: &TirOp) {
        let origin = op.origin;
        if origin == Some(OpKind::GetAttrName) && op.operands.len() >= 2 {
            let callee = CallTarget::Callee(b.runtime_callee(RuntimeFn::GetAttrName));
            let args = [
                RuntimeArg::Operand(op.operands[0]),
                RuntimeArg::Operand(op.operands[1]),
            ];
            let word = self.operands().call(
                b,
                callee,
                &args,
                false,
                "get_attr_name",
                "get_attr_name_dyn",
            );
            self.bind_owned(b, op, b.word_value(word));
            return;
        }
        if matches!(origin, Some(OpKind::Load | OpKind::GuardedLoad)) && !op.operands.is_empty() {
            let callee = CallTarget::Site(SiteBody::FieldGet {
                offset: field_offset(op),
            });
            let admission = b.refcount_admission();
            let args = [
                RuntimeArg::Operand(op.operands[0]),
                RuntimeArg::Word(b.word_value(admission)),
            ];
            let word = self
                .operands()
                .call(b, callee, &args, false, "field_load", "field_get");
            self.bind_owned(b, op, b.word_value(word));
            return;
        }
        if origin == Some(OpKind::ExceptionFieldGet) {
            // operands = [exception]; `value` is the member's typed id
            // (call_anatomy.md 4.5a, "Builtin layouts").
            let callee = CallTarget::Site(SiteBody::BuiltinFieldGet {
                field: exception_field(op),
            });
            let admission = b.refcount_admission();
            let args = [
                RuntimeArg::Operand(op.operands[0]),
                RuntimeArg::Word(b.word_value(admission)),
            ];
            let word = self.operands().call(
                b,
                callee,
                &args,
                false,
                "exception_field_load",
                "exception_field_get",
            );
            self.bind_owned(b, op, b.word_value(word));
            return;
        }
        if origin == Some(OpKind::GuardedFieldGet) {
            // operands = [obj, expected_version]; the class statement is the
            // op's `class_definition` (HF-220).
            assert_eq!(op.operands.len(), 2, "admitted guarded_field_get operands");
            let name = attr_name(op);
            let (name_ptr, name_len) = raw_name(b, name);
            let callee = CallTarget::Callee(b.runtime_callee(RuntimeFn::GuardedFieldGet));
            let offset = b.constant(int_attr(op) as u64);
            let slot = class_definition_slot(b, op);
            let args = [
                RuntimeArg::Operand(op.operands[0]),
                RuntimeArg::Word(b.word_value(slot)),
                RuntimeArg::Operand(op.operands[1]),
                RuntimeArg::Word(b.word_value(offset)),
                RuntimeArg::Word(name_ptr),
                RuntimeArg::Word(b.word_value(name_len)),
            ];
            let word = self.operands().call(
                b,
                callee,
                &args,
                false,
                "guarded_field_get",
                "guarded_field_get",
            );
            self.bind_owned(b, op, b.word_value(word));
            return;
        }
        let name = attr_name(op);
        let operands = self.operands();
        let mut custody = operands.begin(b, &op.operands[..1], "get_attr");
        let object = operands.operand(b, &mut custody, op.operands[0]);
        let site_kind = origin
            .filter(|kind| matches!(kind, OpKind::GetAttrGenericObj | OpKind::GetAttrGenericPtr));
        let word = if let Some(site_kind) = site_kind {
            let runtime = RuntimeFn::GetAttrObjectIc;
            let callee = b.runtime_callee(runtime);
            let (name_ptr, name_len) = raw_name_pointer(b, name);
            let site = self.site_bits(b, op, site_kind.as_str());
            let args = [
                b.word_value(object),
                name_ptr,
                b.word_value(name_len),
                b.word_value(site),
            ];
            call_named(b, callee, &args, runtime.symbol())
        } else {
            let runtime = RuntimeFn::GetAttrName;
            let callee = b.runtime_callee(runtime);
            with_name(b, name, |b, name| {
                let args = [b.word_value(object), b.word_value(name)];
                call_named(b, callee, &args, runtime.symbol())
            })
        };
        // Runtime getattr entries return one owned result on every
        // successful path, including IC hits; a backend-side retain would
        // leak bound-method receivers.
        let operands = self.operands();
        let word = operands.finish(b, custody, word, "get_attr_result", |_| {});
        self.bind_owned(b, op, b.word_value(word));
    }

    fn store_attr(&mut self, b: &mut B, op: &TirOp) {
        let origin = op.origin;
        // Attribute setters borrow the receiver and value and return the
        // immortal None, so a discarded result needs no release.
        if origin == Some(OpKind::SetAttrName) && op.operands.len() >= 3 {
            let callee = CallTarget::Callee(b.runtime_callee(RuntimeFn::SetAttrName));
            let args = [
                RuntimeArg::Operand(op.operands[0]),
                RuntimeArg::Operand(op.operands[1]),
                RuntimeArg::Operand(op.operands[2]),
            ];
            self.runtime_call(
                b,
                op,
                callee,
                &args,
                ResultCustody::Unowned,
                "set_attr_name",
                "set_attr_name_dyn",
            );
            return;
        }
        if origin == Some(OpKind::Store) && op.operands.len() >= 2 {
            let callee = CallTarget::Site(SiteBody::FieldSet {
                offset: field_offset(op),
            });
            let admission = b.refcount_admission();
            let args = [
                RuntimeArg::Operand(op.operands[0]),
                RuntimeArg::Operand(op.operands[1]),
                RuntimeArg::Word(b.word_value(admission)),
            ];
            self.runtime_call(
                b,
                op,
                callee,
                &args,
                ResultCustody::Unowned,
                "field_store",
                "field_set",
            );
            return;
        }
        if origin == Some(OpKind::GuardedFieldSet) {
            // operands = [obj, expected_version, value] (HF-220).
            assert_eq!(op.operands.len(), 3, "admitted guarded_field_set operands");
            let name = attr_name(op);
            let (name_ptr, name_len) = raw_name(b, name);
            let callee = CallTarget::Callee(b.runtime_callee(RuntimeFn::GuardedFieldSet));
            let offset = b.constant(int_attr(op) as u64);
            let slot = class_definition_slot(b, op);
            let args = [
                RuntimeArg::Operand(op.operands[0]),
                RuntimeArg::Word(b.word_value(slot)),
                RuntimeArg::Operand(op.operands[1]),
                RuntimeArg::Word(b.word_value(offset)),
                RuntimeArg::Operand(op.operands[2]),
                RuntimeArg::Word(name_ptr),
                RuntimeArg::Word(b.word_value(name_len)),
            ];
            self.runtime_call(
                b,
                op,
                callee,
                &args,
                ResultCustody::Unowned,
                "guarded_field_set",
                "guarded_field_set",
            );
            return;
        }
        let name = attr_name(op);
        let operands = self.operands();
        let mut custody = operands.begin(b, &op.operands[..2], "set_attr");
        let object = operands.operand(b, &mut custody, op.operands[0]);
        let value = operands.operand(b, &mut custody, op.operands[1]);
        let site_kind = origin
            .filter(|kind| matches!(kind, OpKind::SetAttrGenericObj | OpKind::SetAttrGenericPtr));
        let word = if let Some(site_kind) = site_kind {
            // A generic write site carries its id, so a warm site writes the
            // inferred field without building the name string.
            let runtime = RuntimeFn::SetAttrObjectIc;
            let callee = b.runtime_callee(runtime);
            let (name_ptr, name_len) = raw_name_pointer(b, name);
            let site = self.site_bits(b, op, site_kind.as_str());
            let args = [
                b.word_value(object),
                name_ptr,
                b.word_value(name_len),
                b.word_value(value),
                b.word_value(site),
            ];
            call_named(b, callee, &args, runtime.symbol())
        } else {
            let callee = b.runtime_callee(RuntimeFn::SetAttrName);
            with_name(b, name, |b, name| {
                let args = [
                    b.word_value(object),
                    b.word_value(name),
                    b.word_value(value),
                ];
                call_named(b, callee, &args, "setattr")
            })
        };
        let operands = self.operands();
        let word = operands.finish(b, custody, word, "set_attr_result", |_| {});
        if let Some(&result) = op.results.first() {
            self.bind_boxed(b, result, word);
        }
    }

    fn del_attr(&mut self, b: &mut B, op: &TirOp) {
        // Attribute deletion borrows the receiver and returns the immortal
        // None.
        if op.origin == Some(OpKind::DelAttrName) && op.operands.len() >= 2 {
            let callee = CallTarget::Callee(b.runtime_callee(RuntimeFn::DelAttrName));
            let args = [
                RuntimeArg::Operand(op.operands[0]),
                RuntimeArg::Operand(op.operands[1]),
            ];
            self.runtime_call(
                b,
                op,
                callee,
                &args,
                ResultCustody::Unowned,
                "del_attr_name",
                "del_attr_name_dyn",
            );
            return;
        }
        let name = attr_name(op);
        let operands = self.operands();
        let mut custody = operands.begin(b, &op.operands[..1], "del_attr");
        let object = operands.operand(b, &mut custody, op.operands[0]);
        let callee = b.runtime_callee(RuntimeFn::DelAttrName);
        let word = with_name(b, name, |b, name| {
            let args = [b.word_value(object), b.word_value(name)];
            call_named(b, callee, &args, "del_attr_name")
        });
        let operands = self.operands();
        let word = operands.finish(b, custody, word, "del_attr_result", |_| {});
        if let Some(&result) = op.results.first() {
            self.bind_boxed(b, result, word);
        }
    }

    fn index(&mut self, b: &mut B, op: &TirOp) {
        // A bounds-check-elimination proof calls the entry that skips the
        // runtime bounds check.
        let runtime = if matches!(op.attrs.get("bce_safe"), Some(AttrValue::Bool(true))) {
            RuntimeFn::GetitemUnchecked
        } else {
            RuntimeFn::GetitemMethod
        };
        // A container that may be a list or tuple takes the shared read,
        // which inlines an exact list or tuple at an inline int; its miss is
        // the runtime entry (`molt_codegen_abi::list_item`).
        let mut args = vec![
            RuntimeArg::Operand(op.operands[0]),
            RuntimeArg::Operand(op.operands[1]),
        ];
        let callee = if let Some(containers) = self.item_read_containers(op.operands[0]) {
            let admission = b.refcount_admission();
            args.push(RuntimeArg::Word(b.word_value(admission)));
            CallTarget::Site(SiteBody::ItemGet {
                containers,
                cold: runtime,
            })
        } else {
            CallTarget::Callee(b.runtime_callee(runtime))
        };
        let word = self
            .operands()
            .call(b, callee, &args, false, "index", runtime.symbol());
        self.bind_owned(b, op, b.word_value(word));
    }

    /// Item assignment and deletion borrow their operands; the container
    /// retains what it stores, and the runtime returns only None or a
    /// boolean.
    fn store_index(&mut self, b: &mut B, op: &TirOp) {
        // A container that may be a list takes the shared list store; its
        // miss is `molt_setitem_method`.
        let mut args = vec![
            RuntimeArg::Operand(op.operands[0]),
            RuntimeArg::Operand(op.operands[1]),
            RuntimeArg::Operand(op.operands[2]),
        ];
        let callee = if self.container_may_be_list(op.operands[0]) {
            let admission = b.refcount_admission();
            args.push(RuntimeArg::Word(b.word_value(admission)));
            CallTarget::Site(SiteBody::ListItemSet)
        } else {
            CallTarget::Callee(b.runtime_callee(RuntimeFn::SetitemMethod))
        };
        self.runtime_call(
            b,
            op,
            callee,
            &args,
            ResultCustody::Unowned,
            "store_index",
            "setitem",
        );
    }

    fn del_index(&mut self, b: &mut B, op: &TirOp) {
        let callee = CallTarget::Callee(b.runtime_callee(RuntimeFn::DelitemMethod));
        let args = [
            RuntimeArg::Operand(op.operands[0]),
            RuntimeArg::Operand(op.operands[1]),
        ];
        self.runtime_call(
            b,
            op,
            callee,
            &args,
            ResultCustody::Unowned,
            "del_index",
            RuntimeFn::DelitemMethod.symbol(),
        );
    }
}

/// A proven field's byte offset (the op's `value` attribute).
fn exception_field(op: &TirOp) -> molt_codegen_abi::exception_layout::ExceptionTypedField {
    match op.attrs.get("value") {
        Some(AttrValue::Int(tag)) => u8::try_from(*tag)
            .ok()
            .and_then(molt_codegen_abi::exception_layout::ExceptionTypedField::from_u8)
            .expect("a proven exception member carries its typed id"),
        _ => panic!("a proven exception member carries its typed id"),
    }
}

fn field_offset(op: &TirOp) -> i32 {
    match op.attrs.get("value") {
        Some(AttrValue::Int(offset)) => {
            i32::try_from(*offset).expect("a proven field offset fits an i32")
        }
        _ => panic!("a proven field access carries its offset"),
    }
}

/// `name`'s bytes as a private constant: its address and its length.
pub(crate) fn raw_name_pointer<B: CodegenBuilder>(b: &mut B, name: &str) -> (B::Value, B::Word) {
    let pointer = b.bytes_constant(
        name.as_bytes(),
        "__guard_attr_str_",
        &format!("_{}", name.replace(|c: char| !c.is_alphanumeric(), "_")),
    );
    let len = b.constant(name.len() as u64);
    (pointer, len)
}

/// [`raw_name_pointer`] with the address as a word.
fn raw_name<B: CodegenBuilder>(b: &mut B, name: &str) -> (B::Value, B::Word) {
    let (pointer, len) = raw_name_pointer(b, name);
    let word = b.address_to_word(pointer);
    let word = b.word_value(word);
    b.set_name(word, "guard_attr_ptr");
    (word, len)
}

/// Call `callee` for its word, named `name`.
fn call_named<B: CodegenBuilder>(
    b: &mut B,
    callee: B::Callee,
    args: &[B::Value],
    name: &str,
) -> B::Word {
    let value = b
        .call(callee, args)
        .unwrap_or_else(|| panic!("{name} returns a word"));
    b.set_name(value, name);
    b.expect_word(value)
}

#[cfg(test)]
mod tests {
    //! The object family on the interpreting builder and as Cranelift
    //! machine code. The oracle is the runtime model: each entry answers an
    //! object determined by its arguments (a name by its bytes), and a site
    //! body's arm for a receiver that is no object pointer is the one the
    //! shared generator documents (a field read answers None, an item access
    //! takes its miss).

    use std::cell::RefCell;

    use super::super::differential::{Run, both, kernel, machine};
    use super::super::scalar::tests::{NONE_WORD, Value, literal_word};
    use super::super::value_table::ValueTable;
    use super::*;
    use crate::representation_plan::ProducerCarrierPlan;
    use crate::tir::function::TirFunction;
    use crate::tir::ops::{AttrDict, Dialect};

    /// A receiver no site body treats as an object: an inline int.
    const RECEIVER: u64 = (0x7ff8 << 48) | (1 << 48) | 5;
    const KEY: u64 = (0x7ff8 << 48) | (1 << 48) | 9;
    const VALUE: u64 = (0x7ff8 << 48) | (1 << 48) | 11;

    fn key(words: &[u64]) -> Vec<u8> {
        words.iter().flat_map(|word| word.to_le_bytes()).collect()
    }

    fn name_word(name: &str) -> u64 {
        literal_word(RuntimeFn::StringFromBytes, name.as_bytes())
    }

    /// Lower `op` over boxed operands %0, %1, %2 (the inputs), %0 typed
    /// `container`; the result is %3's word, or None when the op binds none.
    fn object_op(
        op: TirOp,
        container: TirType,
        inputs: &[Value],
        on_machine_only: Option<bool>,
    ) -> Run<()> {
        let lower = kernel!(op: TirOp = op, container: TirType = container; |b, inputs| -> () {
            let mut func = TirFunction::new(
                "objects".into(),
                vec![],
                TirType::None,
                molt_ir::FunctionReturnAbi::Value,
            );
            func.value_types.insert(ValueId(0), container.clone());
            let plan = ProducerCarrierPlan::default();
            let mut values = ValueTable::with_capacity(4);
            for (index, &input) in inputs.iter().enumerate() {
                values.set_value(ValueId(index as u32), input);
                values.set_repr(ValueId(index as u32), HeldCarrier::Boxed);
            }
            let diagnostics = RefCell::new(Vec::new());
            let mut cx = FunctionCx::<B> {
                func: &func,
                plan: &plan,
                values: &mut values,
                diagnostics: &diagnostics,
                runtime: &super::super::differential::every_runtime_entry(),
                linkage: &Default::default(),
                functions: &Default::default(),
                call_sites: &std::cell::Cell::new(0),
            };
            cx.object_op(b, op);
            assert!(diagnostics.borrow().is_empty(), "{:?}", diagnostics.borrow());
            let result = match values.value(ValueId(3)) {
                Some(value) => value,
                None => {
                    let none = b.constant(NONE_WORD);
                    b.word_value(none)
                }
            };
            ((), vec![result])
        });
        match on_machine_only {
            None => both(&lower, inputs),
            Some(fail_allocation) => machine(&lower, inputs, fail_allocation),
        }
    }

    fn op(opcode: OpCode, origin: Option<OpKind>, operands: u32, result: bool) -> TirOp {
        let mut op = TirOp {
            dialect: Dialect::Molt,
            opcode,
            operands: (0..operands).map(ValueId).collect(),
            results: if result { vec![ValueId(3)] } else { vec![] },
            attrs: AttrDict::default(),
            source_span: None,
            origin,
            runtime: None,
            callee: None,
        };
        op.set_source_op_index(4);
        op
    }

    fn named(mut op: TirOp, name: &str) -> TirOp {
        op.attrs.insert("name".into(), AttrValue::Str(name.into()));
        op
    }

    fn words(inputs: &[u64]) -> Vec<Value> {
        inputs.iter().copied().map(Value::Word).collect()
    }

    #[test]
    fn a_named_access_borrows_its_immortal_name_cell() {
        for (opcode, entry, operands) in [
            (OpCode::LoadAttr, RuntimeFn::GetAttrName, 1),
            (OpCode::DelAttr, RuntimeFn::DelAttrName, 1),
            (OpCode::StoreAttr, RuntimeFn::SetAttrName, 2),
        ] {
            let access = named(op(opcode, None, operands, true), "attr");
            let inputs = words(&[RECEIVER, VALUE][..operands as usize]);
            let run = object_op(access.clone(), TirType::DynBox, &inputs, Some(false));
            let mut args = vec![RECEIVER, name_word("attr")];
            if operands == 2 {
                args.push(VALUE);
            }
            assert_eq!(
                run.results,
                [Value::Word(literal_word(entry, &key(&args)))],
                "{opcode:?}"
            );
            // The first evaluation fills the name's cell, cold; the name is
            // immortal, so nothing is released after the access.
            assert_eq!(run.runtime_calls, [RuntimeFn::LiteralCellFill, entry]);
            assert!(run.refcounts.is_empty(), "{opcode:?}: {:?}", run.refcounts);
            // A name that cannot be built skips the access: None, the
            // exception pending, nothing released.
            let run = object_op(access, TirType::DynBox, &inputs, Some(true));
            assert_eq!(run.results, [Value::Word(NONE_WORD)], "{opcode:?}");
            assert_eq!(run.runtime_calls, [RuntimeFn::LiteralCellFill]);
            assert!(run.refcounts.is_empty());
        }
    }

    #[test]
    fn a_cached_or_guarded_access_passes_its_name_bytes_and_site() {
        let site = molt_codegen_abi::stable_ic_site_id("objects", 4, "get_attr_generic_obj");
        let load = named(
            op(OpCode::LoadAttr, Some(OpKind::GetAttrGenericObj), 1, true),
            "field",
        );
        let run = object_op(load, TirType::DynBox, &words(&[RECEIVER]), None);
        let mut expected = key(&[RECEIVER]);
        expected.extend(b"field");
        expected.extend(key(&[nanbox::box_int_bits(site) as u64]));
        assert_eq!(
            run.results,
            [Value::Word(literal_word(
                RuntimeFn::GetAttrObjectIc,
                &expected
            ))]
        );
        let mut guarded = named(
            op(OpCode::LoadAttr, Some(OpKind::GuardedFieldGet), 2, true),
            "field",
        );
        guarded.attrs.insert("value".into(), AttrValue::Int(24));
        guarded
            .attrs
            .insert("class_definition".into(), AttrValue::Str("Point".into()));
        let run = object_op(guarded, TirType::DynBox, &words(&[RECEIVER, KEY]), None);
        let slot = super::super::scalar::tests::program_slot_word(
            &molt_ir::code_slots::program_slot_symbol(
                molt_ir::code_slots::ProgramSlotKind::Code,
                "Point",
            ),
        );
        let mut expected = key(&[RECEIVER, slot, KEY, 24]);
        expected.extend(b"field");
        assert_eq!(
            run.results,
            [Value::Word(literal_word(
                RuntimeFn::GuardedFieldGet,
                &expected
            ))]
        );
    }

    #[test]
    fn a_site_body_takes_its_documented_arm_for_a_non_object() {
        let mut field = op(OpCode::LoadAttr, Some(OpKind::Load), 1, true);
        field.attrs.insert("value".into(), AttrValue::Int(16));
        let run = object_op(field, TirType::DynBox, &words(&[RECEIVER]), None);
        assert_eq!(
            run.results,
            [Value::Word(NONE_WORD)],
            "a field read of no object"
        );
        assert!(run.runtime_calls.is_empty());
        for (container, entry) in [
            (TirType::DynBox, RuntimeFn::GetitemMethod),
            (
                TirType::List(Box::new(TirType::DynBox)),
                RuntimeFn::GetitemMethod,
            ),
            (TirType::Str, RuntimeFn::GetitemMethod),
        ] {
            let index = op(OpCode::Index, None, 2, true);
            let run = object_op(index, container.clone(), &words(&[RECEIVER, KEY]), None);
            assert_eq!(
                run.results,
                [Value::Word(literal_word(entry, &key(&[RECEIVER, KEY])))],
                "{container:?}"
            );
            assert_eq!(run.runtime_calls, [entry], "{container:?}");
        }
        for container in [
            TirType::DynBox,
            TirType::Dict(Box::new(TirType::DynBox), Box::new(TirType::DynBox)),
        ] {
            let store = op(OpCode::StoreIndex, None, 3, false);
            let run = object_op(
                store,
                container.clone(),
                &words(&[RECEIVER, KEY, VALUE]),
                None,
            );
            assert_eq!(
                run.runtime_calls,
                [RuntimeFn::SetitemMethod],
                "{container:?}"
            );
        }
        let delete = op(OpCode::DelIndex, None, 2, false);
        let run = object_op(delete, TirType::DynBox, &words(&[RECEIVER, KEY]), None);
        assert_eq!(run.runtime_calls, [RuntimeFn::DelitemMethod]);
    }
}

/// The body of the program's one terminal-release helper: the release of an
/// object whose count read 1 (`SiteBody::ReleaseTerminal`). Every release
/// refcount site's slow call reaches it, so a plain instance nothing observes
/// frees without a runtime call; its miss is `molt_dec_ref_obj`.
pub(crate) struct ReleaseTerminalBody;

/// The helper's name; one per program.
pub(crate) const RELEASE_TERMINAL_HELPER: &str = "molt_release_terminal";

impl super::builder::HelperBody for ReleaseTerminalBody {
    fn build<H: super::task::TaskPlacement>(&self, h: &mut H, params: &[H::Word]) -> H::Word {
        let &[bits] = params else {
            unreachable!("the terminal-release helper takes one word")
        };
        let admission = h.refcount_admission();
        let args = [h.word_value(bits), h.word_value(admission)];
        let answer = h.site(SiteBody::ReleaseTerminal, &args);
        h.expect_word(answer)
    }
}
