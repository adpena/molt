use wasm_encoder::{
    CodeSection, ConstExpr, ElementSection, EntityType, ExportKind, ExportSection, Function,
    FunctionSection, ImportSection, Instruction, MemorySection, MemoryType, Module, RefType,
    TableSection, TableType, TypeSection, ValType,
};
use crate::{SimpleIR, FunctionIR};
use std::collections::HashMap;

pub struct WasmBackend {
    module: Module,
    types: TypeSection,
    funcs: FunctionSection,
    codes: CodeSection,
    exports: ExportSection,
    imports: ImportSection,
    memories: MemorySection,
    tables: TableSection,
    elements: ElementSection,
    func_count: u32,
}

impl Default for WasmBackend {
    fn default() -> Self {
        Self::new()
    }
}

impl WasmBackend {
    pub fn new() -> Self {
        Self {
            module: Module::new(),
            types: TypeSection::new(),
            funcs: FunctionSection::new(),
            codes: CodeSection::new(),
            exports: ExportSection::new(),
            imports: ImportSection::new(),
            memories: MemorySection::new(),
            tables: TableSection::new(),
            elements: ElementSection::new(),
            func_count: 0,
        }
    }

    pub fn compile(mut self, ir: SimpleIR) -> Vec<u8> {
        // Type 0: () -> i64 (User functions)
        self.types.function(std::iter::empty::<ValType>(), std::iter::once(ValType::I64));
        // Type 1: (i64) -> () (print_int)
        self.types.function(std::iter::once(ValType::I64), std::iter::empty::<ValType>());
        // Type 2: (i64) -> i64 (alloc, sleep, poll)
        self.types.function(std::iter::once(ValType::I64), std::iter::once(ValType::I64));
        
        // Host Imports
        self.imports.import("molt", "print_int", EntityType::Function(1));
        self.imports.import("molt", "alloc", EntityType::Function(2));
        self.imports.import("molt", "async_sleep", EntityType::Function(2));
        self.imports.import("molt", "block_on", EntityType::Function(2));
        
        self.func_count = 4; 

        // Memory & Table
        self.memories.memory(MemoryType {
            minimum: 1,
            maximum: None,
            memory64: false,
            shared: false,
        });
        self.exports.export("molt_memory", ExportKind::Memory, 0);
        
        self.tables.table(TableType {
            element_type: RefType::FUNCREF,
            minimum: 20,
            maximum: None,
        });
        self.exports.export("molt_table", ExportKind::Table, 0);

        // Function indices for table
        let mut table_indices = vec![2]; // async_sleep at index 0 of table
        let mut func_to_table_idx = HashMap::new();
        func_to_table_idx.insert("molt_async_sleep".to_string(), 0);
        
        let user_func_start = self.func_count;
        for (i, func_ir) in ir.functions.iter().enumerate() {
            let idx = (i + 1) as u32; 
            func_to_table_idx.insert(func_ir.name.clone(), idx);
            table_indices.push(user_func_start + i as u32);
        }
        
        self.elements.active(
            None,
            &ConstExpr::i32_const(0),
            wasm_encoder::Elements::Functions(&table_indices),
        );

        for func_ir in ir.functions {
            let type_idx = if func_ir.name.ends_with("_poll") { 2 } else { 0 };
            self.compile_func(func_ir, type_idx, &func_to_table_idx); 
        }

        self.module.section(&self.types);
        self.module.section(&self.imports);
        self.module.section(&self.funcs);
        self.module.section(&self.tables);
        self.module.section(&self.memories);
        self.module.section(&self.exports);
        self.module.section(&self.elements);
        self.module.section(&self.codes);
        
        self.module.finish()
    }

    fn compile_func(&mut self, func_ir: FunctionIR, type_idx: u32, func_map: &HashMap<String, u32>) {
        self.funcs.function(type_idx);
        self.exports.export(&func_ir.name, ExportKind::Func, self.func_count);
        self.func_count += 1;

        let mut locals = HashMap::new(); 
        let mut local_count = 0;
        let mut local_types = Vec::new();

        if type_idx == 2 {
            locals.insert("self_param".to_string(), 0);
            local_count = 1;
        }

        for op in &func_ir.ops {
            if let Some(out) = &op.out {
                if !locals.contains_key(out) {
                    locals.insert(out.clone(), local_count);
                    local_types.push(ValType::I64);
                    local_count += 1;
                }
            }
        }

        let mut func = Function::new_with_locals_types(local_types);

        for op in func_ir.ops {
            match op.kind.as_str() {
                "const" => {
                    let val = op.value.unwrap();
                    func.instruction(&Instruction::I64Const(val));
                    let local_idx = locals[op.out.as_ref().unwrap()];
                    func.instruction(&Instruction::LocalSet(local_idx));
                }
                "add" => {
                    let args = op.args.as_ref().unwrap();
                    let lhs = locals[&args[0]];
                    let rhs = locals[&args[1]];
                    func.instruction(&Instruction::LocalGet(lhs));
                    func.instruction(&Instruction::LocalGet(rhs));
                    func.instruction(&Instruction::I64Add);
                    let res = locals[op.out.as_ref().unwrap()];
                    func.instruction(&Instruction::LocalSet(res));
                }
                "print" => {
                    let args = op.args.as_ref().unwrap();
                    if let Some(&idx) = locals.get(&args[0]) {
                        func.instruction(&Instruction::LocalGet(idx));
                        func.instruction(&Instruction::Call(0)); 
                    }
                }
                "alloc" => {
                    func.instruction(&Instruction::I64Const(op.value.unwrap()));
                    func.instruction(&Instruction::Call(1)); 
                    func.instruction(&Instruction::LocalSet(locals[op.out.as_ref().unwrap()]));
                }
                "store" => {
                    let args = op.args.as_ref().unwrap();
                    func.instruction(&Instruction::LocalGet(locals[&args[0]]));
                    func.instruction(&Instruction::I32WrapI64);
                    func.instruction(&Instruction::LocalGet(locals[&args[1]]));
                    func.instruction(&Instruction::I64Store(wasm_encoder::MemArg {
                        align: 3,
                        offset: op.value.unwrap() as u64,
                        memory_index: 0,
                    }));
                }
                "load" => {
                    let args = op.args.as_ref().unwrap();
                    func.instruction(&Instruction::LocalGet(locals[&args[0]]));
                    func.instruction(&Instruction::I32WrapI64);
                    func.instruction(&Instruction::I64Load(wasm_encoder::MemArg {
                        align: 3,
                        offset: op.value.unwrap() as u64,
                        memory_index: 0,
                    }));
                    func.instruction(&Instruction::LocalSet(locals[op.out.as_ref().unwrap()]));
                }
                "guarded_load" => {
                    let args = op.args.as_ref().unwrap();
                    func.instruction(&Instruction::LocalGet(locals[&args[0]]));
                    func.instruction(&Instruction::I32WrapI64);
                    func.instruction(&Instruction::I64Load(wasm_encoder::MemArg {
                        align: 3,
                        offset: op.value.unwrap() as u64,
                        memory_index: 0,
                    }));
                    func.instruction(&Instruction::LocalSet(locals[op.out.as_ref().unwrap()]));
                }
                "state_switch" => {}
                "state_transition" => {
                    let args = op.args.as_ref().unwrap();
                    let future = locals[&args[0]];
                    func.instruction(&Instruction::LocalGet(0)); 
                    func.instruction(&Instruction::I32WrapI64);
                    func.instruction(&Instruction::I32Const(op.value.unwrap() as i32));
                    func.instruction(&Instruction::I32Store(wasm_encoder::MemArg {
                        align: 2,
                        offset: 16, // state field
                        memory_index: 0,
                    }));
                    func.instruction(&Instruction::LocalGet(future));
                    func.instruction(&Instruction::LocalGet(future));
                    func.instruction(&Instruction::I32WrapI64);
                    func.instruction(&Instruction::I32Load(wasm_encoder::MemArg {
                        align: 2,
                        offset: 8, 
                        memory_index: 0,
                    }));
                    func.instruction(&Instruction::CallIndirect { ty: 2, table: 0 });
                    func.instruction(&Instruction::LocalSet(locals[op.out.as_ref().unwrap()]));
                }
                "call_async" => {
                    func.instruction(&Instruction::I64Const(0));
                    func.instruction(&Instruction::Call(1)); 
                    let res = locals[op.out.as_ref().unwrap()];
                    func.instruction(&Instruction::LocalSet(res));
                    func.instruction(&Instruction::LocalGet(res));
                    func.instruction(&Instruction::I32WrapI64);
                    let table_idx = func_map[op.s_value.as_ref().unwrap()];
                    func.instruction(&Instruction::I32Const(table_idx as i32));
                    func.instruction(&Instruction::I32Store(wasm_encoder::MemArg {
                        align: 2,
                        offset: 8,
                        memory_index: 0,
                    }));
                }
                "alloc_future" => {
                    func.instruction(&Instruction::I64Const(0));
                    func.instruction(&Instruction::Call(1)); 
                    let res = locals[op.out.as_ref().unwrap()];
                    func.instruction(&Instruction::LocalSet(res));
                    func.instruction(&Instruction::LocalGet(res));
                    func.instruction(&Instruction::I32WrapI64);
                    let table_idx = func_map[op.s_value.as_ref().unwrap()];
                    func.instruction(&Instruction::I32Const(table_idx as i32)); 
                    func.instruction(&Instruction::I32Store(wasm_encoder::MemArg {
                        align: 2,
                        offset: 8,
                        memory_index: 0,
                    }));
                }
                "block_on" => {
                    let args = op.args.as_ref().unwrap();
                    func.instruction(&Instruction::LocalGet(locals[&args[0]]));
                    func.instruction(&Instruction::Call(3)); 
                    func.instruction(&Instruction::LocalSet(locals[op.out.as_ref().unwrap()]));
                }
                "ret" => {
                    func.instruction(&Instruction::LocalGet(locals[op.var.as_ref().unwrap()]));
                    func.instruction(&Instruction::End);
                }
                "ret_void" => {
                    if type_idx == 0 || type_idx == 2 {
                        func.instruction(&Instruction::I64Const(0));
                    }
                    func.instruction(&Instruction::End);
                }
                _ => {}
            }
        }
        self.codes.function(&func);
    }
}
