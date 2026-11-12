//! Typed physical call ABI lowering. Argument locals are reserved before bytes
//! are emitted, so data/table relocation offsets never move after publication.

use std::ops::Deref;
use std::rc::Rc;

use wasm_encoder::{Instruction, ValType};

#[derive(Clone, Debug)]
enum CallExecutionAbi {
    Boundary,
    Attached { arguments: Box<[(usize, u32)]> },
}

#[derive(Clone, Debug, Default)]
pub(crate) struct WasmExecutionCallAbis {
    calls: Vec<Option<CallExecutionAbi>>,
    scratch: Vec<(ValType, u32)>,
}

impl WasmExecutionCallAbis {
    pub(crate) fn register_boundary(&mut self, index: u32) {
        self.register(index, CallExecutionAbi::Boundary);
    }

    pub(crate) fn register_attached(&mut self, index: u32, params: &[ValType]) {
        let mut counts = vec![0u32; self.scratch.len()];
        let mut arguments = Vec::with_capacity(params.len());
        for &ty in params {
            let group = self
                .scratch
                .iter()
                .position(|&(known, _)| known == ty)
                .unwrap_or_else(|| {
                    self.scratch.push((ty, 0));
                    counts.push(0);
                    self.scratch.len() - 1
                });
            let ordinal = counts[group];
            counts[group] += 1;
            self.scratch[group].1 = self.scratch[group].1.max(counts[group]);
            arguments.push((group, ordinal));
        }
        self.register(
            index,
            CallExecutionAbi::Attached {
                arguments: arguments.into(),
            },
        );
    }

    fn register(&mut self, index: u32, abi: CallExecutionAbi) {
        let index = index as usize;
        self.calls
            .resize_with(self.calls.len().max(index + 1), || None);
        assert!(
            self.calls[index].is_none(),
            "duplicate WASM call ABI at {index}"
        );
        self.calls[index] = Some(abi);
    }
}

/// An instruction sink with a single attached execution context at local zero.
/// The stack emitters retain logical operands; this sink converts them to the
/// physical ABI using bounded, typed locals, without runtime attachment or
/// runtime allocation. The emitted local moves are part of the WASM cost.
pub(crate) struct Function {
    body: wasm_encoder::Function,
    locals: Vec<ValType>,
    execution: Option<(Rc<WasmExecutionCallAbis>, Vec<u32>)>,
    started: bool,
}

impl Function {
    #[cfg(test)]
    pub(crate) fn new(locals: impl IntoIterator<Item = (u32, ValType)>) -> Self {
        Self::new_with_locals_types(
            locals
                .into_iter()
                .flat_map(|(count, ty)| std::iter::repeat_n(ty, count as usize)),
        )
    }

    pub(crate) fn new_with_locals_types(locals: impl IntoIterator<Item = ValType>) -> Self {
        let locals: Vec<_> = locals.into_iter().collect();
        Self {
            body: wasm_encoder::Function::new_with_locals_types(locals.clone()),
            locals,
            execution: None,
            started: false,
        }
    }

    pub(crate) fn attach_context(
        &mut self,
        physical_params: usize,
        abis: Rc<WasmExecutionCallAbis>,
    ) {
        assert!(
            !self.started && self.execution.is_none(),
            "WASM context must precede emission"
        );
        let mut groups = Vec::with_capacity(abis.scratch.len());
        for &(ty, count) in &abis.scratch {
            groups.push(
                u32::try_from(physical_params + self.locals.len()).expect("WASM local index"),
            );
            self.locals.extend(std::iter::repeat_n(ty, count as usize));
        }
        self.body = wasm_encoder::Function::new_with_locals_types(self.locals.clone());
        self.execution = Some((abis, groups));
    }

    pub(crate) fn instruction(&mut self, instruction: &Instruction<'_>) -> &mut Self {
        self.started = true;
        self.body.instruction(instruction);
        self
    }

    pub(crate) fn raw(&mut self, bytes: impl IntoIterator<Item = u8>) -> &mut Self {
        self.started = true;
        self.body.raw(bytes);
        self
    }

    pub(super) fn emit_call_context(&mut self, callee: u32) {
        let Some((abis, groups)) = &self.execution else {
            return;
        };
        let abi = abis
            .calls
            .get(callee as usize)
            .and_then(Option::as_ref)
            .unwrap_or_else(|| {
                panic!("missing typed WASM execution call ABI for function {callee}")
            });
        let CallExecutionAbi::Attached { arguments } = abi else {
            return;
        };
        for &(group, ordinal) in arguments.iter().rev() {
            self.body
                .instruction(&Instruction::LocalSet(groups[group] + ordinal));
        }
        self.body.instruction(&Instruction::LocalGet(0));
        for &(group, ordinal) in arguments.iter() {
            self.body
                .instruction(&Instruction::LocalGet(groups[group] + ordinal));
        }
        self.started = true;
    }
}

impl Deref for Function {
    type Target = wasm_encoder::Function;
    fn deref(&self) -> &Self::Target {
        &self.body
    }
}
