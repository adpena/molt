use crate::tir::blocks::Terminator;
use crate::tir::function::TirFunction;
use crate::tir::ops::{AttrValue, OpCode, TirOp};
use crate::tir::types::TirType;
use crate::tir::values::ValueId;
use molt_ir::OpKind;

/// A builder for one block over parameters `seq`, `other` and `k`, an int.
struct Body {
    func: TirFunction,
    ops: Vec<TirOp>,
}

impl Body {
    fn new() -> Self {
        let func = TirFunction::new(
            "f".into(),
            vec![TirType::DynBox, TirType::DynBox, TirType::I64],
            TirType::DynBox,
            molt_ir::FunctionReturnAbi::Value,
        );
        Self {
            func,
            ops: Vec::new(),
        }
    }

    fn param(&self, index: u32) -> ValueId {
        ValueId(index)
    }

    fn value(&mut self, ty: TirType) -> ValueId {
        let value = self.func.fresh_value();
        self.func.value_types.insert(value, ty);
        value
    }

    fn none(&mut self) -> ValueId {
        let value = self.value(TirType::None);
        self.ops
            .push(TirOp::new(OpCode::ConstNone, vec![], vec![value]));
        value
    }

    fn int(&mut self, constant: i64) -> ValueId {
        let value = self.value(TirType::I64);
        let mut op = TirOp::new(OpCode::ConstInt, vec![], vec![value]);
        op.attrs.insert("value".into(), AttrValue::Int(constant));
        self.ops.push(op);
        value
    }

    fn binary(&mut self, opcode: OpCode, lhs: ValueId, rhs: ValueId, ty: TirType) -> ValueId {
        let value = self.value(ty);
        self.ops
            .push(TirOp::new(opcode, vec![lhs, rhs], vec![value]));
        value
    }

    fn slice(&mut self, start: ValueId, stop: ValueId, step: ValueId) -> ValueId {
        let value = self.value(TirType::DynBox);
        let mut op = TirOp::new(OpCode::Copy, vec![start, stop, step], vec![value]);
        op.origin = Some(OpKind::SliceNew);
        self.ops.push(op);
        value
    }

    fn index(&mut self, seq: ValueId, key: ValueId) -> ValueId {
        let value = self.value(TirType::DynBox);
        self.ops
            .push(TirOp::new(OpCode::Index, vec![seq, key], vec![value]));
        value
    }

    fn store(&mut self, seq: ValueId, key: ValueId, value: ValueId) {
        self.ops.push(TirOp::new(
            OpCode::StoreIndex,
            vec![seq, key, value],
            vec![],
        ));
    }

    /// Run the pass on the block and return whether it rewrote it, after
    /// checking the function still verifies.
    fn run(mut self) -> (bool, TirFunction) {
        let entry = self.func.entry_block;
        let none = self.value(TirType::None);
        self.ops
            .push(TirOp::new(OpCode::ConstNone, vec![], vec![none]));
        let block = self.func.blocks.get_mut(&entry).unwrap();
        block.ops = self.ops;
        block.terminator = Terminator::Return { values: vec![none] };
        let stats = super::run(&mut self.func);
        crate::tir::verify::verify_function(&self.func).expect("rewritten function verifies");
        (stats.values_changed > 0, self.func)
    }
}

/// `seq[lo:hi] = seq[start:stop:-1]` with the given bounds.
fn statement(
    body: &mut Body,
    seq: ValueId,
    source: ValueId,
    [start, stop]: [ValueId; 2],
    [lo, hi]: [ValueId; 2],
) {
    let minus_one = body.int(-1);
    let source_slice = body.slice(start, stop, minus_one);
    let reversed = body.index(source, source_slice);
    let step = body.none();
    let target = body.slice(lo, hi, step);
    body.store(seq, target, reversed);
}

fn constant(func: &TirFunction, value: ValueId) -> i64 {
    let op = func
        .blocks
        .values()
        .flat_map(|block| &block.ops)
        .find(|op| op.results == [value])
        .expect("the constant is defined");
    match op.attrs.get("value") {
        Some(AttrValue::Int(constant)) => *constant,
        other => panic!("not an int constant: {other:?}"),
    }
}

fn kernel(func: &TirFunction) -> Option<&TirOp> {
    func.blocks
        .values()
        .flat_map(|block| &block.ops)
        .find(|op| op.opcode == OpCode::Copy && op.origin == Some(OpKind::ListReverseSlice))
}

#[test]
fn prefix_flip_becomes_an_in_place_reverse_with_the_statement_as_fallback() {
    // seq[:k + 1] = seq[k::-1]
    let mut body = Body::new();
    let (seq, k) = (body.param(0), body.param(2));
    let none = body.none();
    let one = body.int(1);
    let hi = body.binary(OpCode::Add, k, one, TirType::I64);
    statement(&mut body, seq, seq, [k, none], [none, hi]);
    let (rewritten, func) = body.run();
    assert!(rewritten);
    let op = kernel(&func).expect("the kernel is emitted");
    assert_eq!(
        [op.operands[0], op.operands[1], op.operands[3]],
        [seq, k, none]
    );
    assert_eq!(constant(&func, op.operands[2]), 0);
    // The slow path keeps the read and the store, so a decline runs them.
    let stores = func
        .blocks
        .values()
        .flat_map(|block| &block.ops)
        .filter(|op| op.opcode == OpCode::StoreIndex)
        .count();
    assert_eq!(stores, 1);
}

#[test]
fn bounds_are_proven_from_values_not_spelling() {
    // n = k + 1; seq[:n] = seq[n - 1::-1]
    let mut body = Body::new();
    let (seq, k) = (body.param(0), body.param(2));
    let none = body.none();
    let one = body.int(1);
    let n = body.binary(OpCode::Add, k, one, TirType::I64);
    let start = body.binary(OpCode::Sub, n, one, TirType::I64);
    statement(&mut body, seq, seq, [start, none], [none, n]);
    assert!(body.run().0);

    // seq[i:j] = seq[j - 1:i - 1:-1], with i = k and j = k + 3
    let mut body = Body::new();
    let (seq, k) = (body.param(0), body.param(2));
    let one = body.int(1);
    let three = body.int(3);
    let j = body.binary(OpCode::Add, k, three, TirType::I64);
    let start = body.binary(OpCode::Sub, j, one, TirType::I64);
    let stop = body.binary(OpCode::Sub, k, one, TirType::I64);
    statement(&mut body, seq, seq, [start, stop], [k, j]);
    assert!(body.run().0);

    // seq[:] = seq[::-1]
    let mut body = Body::new();
    let seq = body.param(0);
    let none = body.none();
    statement(&mut body, seq, seq, [none, none], [none, none]);
    let (rewritten, func) = body.run();
    assert!(rewritten);
    let shape = kernel(&func).unwrap().operands[5];
    assert_eq!(constant(&func, shape), 0b11);
}

#[test]
fn other_statements_keep_their_lowering() {
    let cases: Vec<fn(&mut Body)> = vec![
        // seq[:k] = seq[k::-1]: one item more than the target holds.
        |body| {
            let (seq, k) = (body.param(0), body.param(2));
            let none = body.none();
            statement(body, seq, seq, [k, none], [none, k]);
        },
        // seq[:k + 1] = other[k::-1]: another list.
        |body| {
            let (seq, other, k) = (body.param(0), body.param(1), body.param(2));
            let none = body.none();
            let one = body.int(1);
            let hi = body.binary(OpCode::Add, k, one, TirType::I64);
            statement(body, seq, other, [k, none], [none, hi]);
        },
        // seq[1:k + 1] = seq[k::-1]: a lower bound with no partner.
        |body| {
            let (seq, k) = (body.param(0), body.param(2));
            let none = body.none();
            let one = body.int(1);
            let hi = body.binary(OpCode::Add, k, one, TirType::I64);
            statement(body, seq, seq, [k, none], [one, hi]);
        },
        // seq[:k + 1] = seq[k:0:-1]: the source stops one item short.
        |body| {
            let (seq, k) = (body.param(0), body.param(2));
            let none = body.none();
            let zero = body.int(0);
            let one = body.int(1);
            let hi = body.binary(OpCode::Add, k, one, TirType::I64);
            statement(body, seq, seq, [k, zero], [none, hi]);
        },
    ];
    for build in cases {
        let mut body = Body::new();
        build(&mut body);
        assert!(!body.run().0);
    }
}

#[test]
fn the_kernel_checks_the_shared_base() {
    // n = other + 1; seq[:n] = seq[n - 1::-1] with `other` of unknown type:
    // `n - 1` could be an int while `n` is not, so the kernel takes the base
    // `other` with offset 0 and checks it is an int.
    let mut body = Body::new();
    let (seq, other) = (body.param(0), body.param(1));
    let none = body.none();
    let one = body.int(1);
    let n = body.binary(OpCode::Add, other, one, TirType::DynBox);
    let start = body.binary(OpCode::Sub, n, one, TirType::DynBox);
    statement(&mut body, seq, seq, [start, none], [none, n]);
    let (rewritten, func) = body.run();
    assert!(rewritten);
    let op = kernel(&func).unwrap();
    assert_eq!(op.operands[1], other);
    assert_eq!(constant(&func, op.operands[2]), 0);

    // seq[i:j] = seq[j - 1:i - 1:-1] passes each base with its offset.
    let mut body = Body::new();
    let (seq, i, j) = (body.param(0), body.param(1), body.param(2));
    let one = body.int(1);
    let start = body.binary(OpCode::Sub, j, one, TirType::DynBox);
    let stop = body.binary(OpCode::Sub, i, one, TirType::DynBox);
    statement(&mut body, seq, seq, [start, stop], [i, j]);
    let (rewritten, func) = body.run();
    assert!(rewritten);
    let op = kernel(&func).unwrap();
    assert_eq!([op.operands[1], op.operands[3]], [j, i]);
    assert_eq!(constant(&func, op.operands[2]), -1);
    assert_eq!(constant(&func, op.operands[4]), -1);
}
