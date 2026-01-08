use crate::{FunctionIR, OpIR, SimpleIR};
use std::collections::HashMap;
use wasm_encoder::{
    BlockType, CodeSection, ConstExpr, DataSection, ElementSection, EntityType, ExportKind,
    ExportSection, Function, FunctionSection, ImportSection, Instruction, MemorySection,
    MemoryType, Module, RefType, TableSection, TableType, TypeSection, ValType,
};

const QNAN: u64 = 0x7ff8_0000_0000_0000;
const TAG_INT: u64 = 0x0001_0000_0000_0000;
const TAG_BOOL: u64 = 0x0002_0000_0000_0000;
const TAG_NONE: u64 = 0x0003_0000_0000_0000;
const TAG_PTR: u64 = 0x0004_0000_0000_0000;
const TAG_PENDING: u64 = 0x0005_0000_0000_0000;
const POINTER_MASK: u64 = 0x0000_FFFF_FFFF_FFFF;
const INT_MASK: u64 = (1 << 47) - 1;

fn box_int(val: i64) -> i64 {
    let masked = (val as u64) & POINTER_MASK;
    (QNAN | TAG_INT | masked) as i64
}

fn box_float(val: f64) -> i64 {
    val.to_bits() as i64
}

fn box_bool(val: i64) -> i64 {
    let bit = if val != 0 { 1u64 } else { 0u64 };
    (QNAN | TAG_BOOL | bit) as i64
}

fn box_none() -> i64 {
    (QNAN | TAG_NONE) as i64
}

fn box_pending() -> i64 {
    (QNAN | TAG_PENDING) as i64
}

pub struct WasmBackend {
    module: Module,
    types: TypeSection,
    funcs: FunctionSection,
    codes: CodeSection,
    exports: ExportSection,
    imports: ImportSection,
    memories: MemorySection,
    data: DataSection,
    tables: TableSection,
    elements: ElementSection,
    func_count: u32,
    import_ids: HashMap<String, u32>,
    data_offset: u32,
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
            data: DataSection::new(),
            tables: TableSection::new(),
            elements: ElementSection::new(),
            func_count: 0,
            import_ids: HashMap::new(),
            data_offset: 0,
        }
    }

    pub fn compile(mut self, ir: SimpleIR) -> Vec<u8> {
        // Type 0: () -> i64 (User functions)
        self.types
            .function(std::iter::empty::<ValType>(), std::iter::once(ValType::I64));
        // Type 1: (i64) -> () (print_obj)
        self.types
            .function(std::iter::once(ValType::I64), std::iter::empty::<ValType>());
        // Type 2: (i64) -> i64 (alloc, sleep, block_on, is_truthy)
        self.types
            .function(std::iter::once(ValType::I64), std::iter::once(ValType::I64));
        // Type 3: (i64, i64) -> i64 (add/sub/mul/lt/list_append/list_pop)
        self.types.function(
            std::iter::repeat_n(ValType::I64, 2),
            std::iter::once(ValType::I64),
        );
        // Type 4: (i64, i64, i64) -> i32 (parse_scalar)
        self.types.function(
            std::iter::repeat_n(ValType::I64, 3),
            std::iter::once(ValType::I32),
        );
        // Type 5: (i64, i64, i64) -> i64 (stream_send, ws_send, slice, slice_new, dict_get)
        self.types.function(
            std::iter::repeat_n(ValType::I64, 3),
            std::iter::once(ValType::I64),
        );
        // Type 6: (i64, i64) -> () (list_builder_append)
        self.types
            .function(std::iter::repeat_n(ValType::I64, 2), std::iter::empty());
        // Type 7: (i64, i64, i64, i64) -> i64 (dict_pop)
        self.types.function(
            std::iter::repeat_n(ValType::I64, 4),
            std::iter::once(ValType::I64),
        );
        // Type 8: () -> () (print_newline)
        self.types
            .function(std::iter::empty::<ValType>(), std::iter::empty());
        // Type 9: (i64, i64, i64, i64, i64, i64) -> i64 (string_count_slice)
        self.types.function(
            std::iter::repeat_n(ValType::I64, 6),
            std::iter::once(ValType::I64),
        );

        let mut import_idx = 0;
        let mut add_import = |name: &str, ty: u32, ids: &mut HashMap<String, u32>| {
            self.imports
                .import("molt_runtime", name, EntityType::Function(ty));
            ids.insert(name.to_string(), import_idx);
            import_idx += 1;
        };

        // Host Imports (aligned with wit/molt-runtime.wit)
        add_import("print_obj", 1, &mut self.import_ids);
        add_import("print_newline", 8, &mut self.import_ids);
        add_import("alloc", 2, &mut self.import_ids);
        add_import("async_sleep", 2, &mut self.import_ids);
        add_import("block_on", 2, &mut self.import_ids);
        add_import("chan_new", 2, &mut self.import_ids);
        add_import("chan_send", 3, &mut self.import_ids);
        add_import("chan_recv", 2, &mut self.import_ids);
        add_import("add", 3, &mut self.import_ids);
        add_import("vec_sum_int", 3, &mut self.import_ids);
        add_import("vec_sum_int_trusted", 3, &mut self.import_ids);
        add_import("vec_sum_int_range", 5, &mut self.import_ids);
        add_import("vec_sum_int_range_trusted", 5, &mut self.import_ids);
        add_import("vec_prod_int", 3, &mut self.import_ids);
        add_import("vec_prod_int_trusted", 3, &mut self.import_ids);
        add_import("vec_prod_int_range", 5, &mut self.import_ids);
        add_import("vec_prod_int_range_trusted", 5, &mut self.import_ids);
        add_import("vec_min_int", 3, &mut self.import_ids);
        add_import("vec_min_int_trusted", 3, &mut self.import_ids);
        add_import("vec_min_int_range", 5, &mut self.import_ids);
        add_import("vec_min_int_range_trusted", 5, &mut self.import_ids);
        add_import("vec_max_int", 3, &mut self.import_ids);
        add_import("vec_max_int_trusted", 3, &mut self.import_ids);
        add_import("vec_max_int_range", 5, &mut self.import_ids);
        add_import("vec_max_int_range_trusted", 5, &mut self.import_ids);
        add_import("sub", 3, &mut self.import_ids);
        add_import("mul", 3, &mut self.import_ids);
        add_import("bit_or", 3, &mut self.import_ids);
        add_import("bit_and", 3, &mut self.import_ids);
        add_import("bit_xor", 3, &mut self.import_ids);
        add_import("lshift", 3, &mut self.import_ids);
        add_import("rshift", 3, &mut self.import_ids);
        add_import("matmul", 3, &mut self.import_ids);
        add_import("div", 3, &mut self.import_ids);
        add_import("floordiv", 3, &mut self.import_ids);
        add_import("mod", 3, &mut self.import_ids);
        add_import("pow", 3, &mut self.import_ids);
        add_import("pow_mod", 5, &mut self.import_ids);
        add_import("round", 5, &mut self.import_ids);
        add_import("trunc", 2, &mut self.import_ids);
        add_import("lt", 3, &mut self.import_ids);
        add_import("le", 3, &mut self.import_ids);
        add_import("gt", 3, &mut self.import_ids);
        add_import("ge", 3, &mut self.import_ids);
        add_import("eq", 3, &mut self.import_ids);
        add_import("is", 3, &mut self.import_ids);
        add_import("closure_load", 3, &mut self.import_ids);
        add_import("closure_store", 5, &mut self.import_ids);
        add_import("not", 2, &mut self.import_ids);
        add_import("contains", 3, &mut self.import_ids);
        add_import("guard_type", 3, &mut self.import_ids);
        add_import("handle_resolve", 2, &mut self.import_ids);
        add_import("get_attr_generic", 5, &mut self.import_ids);
        add_import("get_attr_object", 5, &mut self.import_ids);
        add_import("set_attr_generic", 7, &mut self.import_ids);
        add_import("set_attr_object", 7, &mut self.import_ids);
        add_import("del_attr_generic", 5, &mut self.import_ids);
        add_import("del_attr_object", 5, &mut self.import_ids);
        add_import("object_field_get", 3, &mut self.import_ids);
        add_import("object_field_set", 5, &mut self.import_ids);
        add_import("object_field_init", 5, &mut self.import_ids);
        add_import("module_new", 2, &mut self.import_ids);
        add_import("module_cache_get", 2, &mut self.import_ids);
        add_import("module_cache_set", 3, &mut self.import_ids);
        add_import("module_get_attr", 3, &mut self.import_ids);
        add_import("module_set_attr", 5, &mut self.import_ids);
        add_import("get_attr_name", 3, &mut self.import_ids);
        add_import("get_attr_name_default", 5, &mut self.import_ids);
        add_import("has_attr_name", 3, &mut self.import_ids);
        add_import("set_attr_name", 5, &mut self.import_ids);
        add_import("del_attr_name", 3, &mut self.import_ids);
        add_import("is_truthy", 2, &mut self.import_ids);
        add_import("json_parse_scalar", 4, &mut self.import_ids);
        add_import("msgpack_parse_scalar", 4, &mut self.import_ids);
        add_import("cbor_parse_scalar", 4, &mut self.import_ids);
        add_import("string_from_bytes", 4, &mut self.import_ids);
        add_import("bytes_from_bytes", 4, &mut self.import_ids);
        add_import("bigint_from_str", 3, &mut self.import_ids);
        add_import("str_from_obj", 2, &mut self.import_ids);
        add_import("repr_from_obj", 2, &mut self.import_ids);
        add_import("ascii_from_obj", 2, &mut self.import_ids);
        add_import("int_from_obj", 5, &mut self.import_ids);
        add_import("float_from_obj", 2, &mut self.import_ids);
        add_import("memoryview_new", 2, &mut self.import_ids);
        add_import("memoryview_tobytes", 2, &mut self.import_ids);
        add_import("intarray_from_seq", 2, &mut self.import_ids);
        add_import("len", 2, &mut self.import_ids);
        add_import("slice", 5, &mut self.import_ids);
        add_import("slice_new", 5, &mut self.import_ids);
        add_import("range_new", 5, &mut self.import_ids);
        add_import("list_builder_new", 2, &mut self.import_ids);
        add_import("list_builder_append", 6, &mut self.import_ids);
        add_import("list_builder_finish", 2, &mut self.import_ids);
        add_import("tuple_builder_finish", 2, &mut self.import_ids);
        add_import("list_append", 3, &mut self.import_ids);
        add_import("list_pop", 3, &mut self.import_ids);
        add_import("list_extend", 3, &mut self.import_ids);
        add_import("list_insert", 5, &mut self.import_ids);
        add_import("list_remove", 3, &mut self.import_ids);
        add_import("list_count", 3, &mut self.import_ids);
        add_import("list_index", 3, &mut self.import_ids);
        add_import("tuple_from_list", 2, &mut self.import_ids);
        add_import("dict_new", 2, &mut self.import_ids);
        add_import("dict_set", 5, &mut self.import_ids);
        add_import("dict_get", 5, &mut self.import_ids);
        add_import("dict_pop", 7, &mut self.import_ids);
        add_import("dict_keys", 2, &mut self.import_ids);
        add_import("dict_values", 2, &mut self.import_ids);
        add_import("dict_items", 2, &mut self.import_ids);
        add_import("set_new", 2, &mut self.import_ids);
        add_import("set_add", 3, &mut self.import_ids);
        add_import("set_discard", 3, &mut self.import_ids);
        add_import("set_remove", 3, &mut self.import_ids);
        add_import("set_pop", 2, &mut self.import_ids);
        add_import("set_update", 3, &mut self.import_ids);
        add_import("set_intersection_update", 3, &mut self.import_ids);
        add_import("set_difference_update", 3, &mut self.import_ids);
        add_import("set_symdiff_update", 3, &mut self.import_ids);
        add_import("frozenset_new", 2, &mut self.import_ids);
        add_import("frozenset_add", 3, &mut self.import_ids);
        add_import("tuple_count", 3, &mut self.import_ids);
        add_import("tuple_index", 3, &mut self.import_ids);
        add_import("iter", 2, &mut self.import_ids);
        add_import("aiter", 2, &mut self.import_ids);
        add_import("iter_next", 2, &mut self.import_ids);
        add_import("anext", 2, &mut self.import_ids);
        add_import("generator_send", 3, &mut self.import_ids);
        add_import("generator_throw", 3, &mut self.import_ids);
        add_import("generator_close", 2, &mut self.import_ids);
        add_import("is_generator", 2, &mut self.import_ids);
        add_import("index", 3, &mut self.import_ids);
        add_import("store_index", 5, &mut self.import_ids);
        add_import("bytes_find", 3, &mut self.import_ids);
        add_import("bytearray_find", 3, &mut self.import_ids);
        add_import("string_find", 3, &mut self.import_ids);
        add_import("string_format", 3, &mut self.import_ids);
        add_import("string_startswith", 3, &mut self.import_ids);
        add_import("string_endswith", 3, &mut self.import_ids);
        add_import("string_count", 3, &mut self.import_ids);
        add_import("string_count_slice", 9, &mut self.import_ids);
        add_import("env_get", 3, &mut self.import_ids);
        add_import("string_join", 3, &mut self.import_ids);
        add_import("string_split", 3, &mut self.import_ids);
        add_import("bytes_split", 3, &mut self.import_ids);
        add_import("bytearray_split", 3, &mut self.import_ids);
        add_import("string_replace", 5, &mut self.import_ids);
        add_import("bytes_replace", 5, &mut self.import_ids);
        add_import("bytearray_replace", 5, &mut self.import_ids);
        add_import("bytes_from_obj", 2, &mut self.import_ids);
        add_import("bytearray_from_obj", 2, &mut self.import_ids);
        add_import("buffer2d_new", 5, &mut self.import_ids);
        add_import("buffer2d_get", 5, &mut self.import_ids);
        add_import("buffer2d_set", 7, &mut self.import_ids);
        add_import("buffer2d_matmul", 3, &mut self.import_ids);
        add_import("dataclass_new", 7, &mut self.import_ids);
        add_import("dataclass_get", 3, &mut self.import_ids);
        add_import("dataclass_set", 5, &mut self.import_ids);
        add_import("dataclass_set_class", 3, &mut self.import_ids);
        add_import("class_new", 2, &mut self.import_ids);
        add_import("class_set_base", 3, &mut self.import_ids);
        add_import("class_apply_set_name", 2, &mut self.import_ids);
        add_import("super_new", 3, &mut self.import_ids);
        add_import("builtin_type", 2, &mut self.import_ids);
        add_import("type_of", 2, &mut self.import_ids);
        add_import("isinstance", 3, &mut self.import_ids);
        add_import("issubclass", 3, &mut self.import_ids);
        add_import("object_new", 0, &mut self.import_ids);
        add_import("func_new", 3, &mut self.import_ids);
        add_import("bound_method_new", 3, &mut self.import_ids);
        add_import("classmethod_new", 2, &mut self.import_ids);
        add_import("staticmethod_new", 2, &mut self.import_ids);
        add_import("property_new", 5, &mut self.import_ids);
        add_import("object_set_class", 3, &mut self.import_ids);
        add_import("stream_new", 2, &mut self.import_ids);
        add_import("stream_send", 5, &mut self.import_ids);
        add_import("stream_recv", 2, &mut self.import_ids);
        add_import("stream_close", 1, &mut self.import_ids);
        add_import("ws_connect", 4, &mut self.import_ids);
        add_import("ws_pair", 4, &mut self.import_ids);
        add_import("ws_send", 5, &mut self.import_ids);
        add_import("ws_recv", 2, &mut self.import_ids);
        add_import("ws_close", 1, &mut self.import_ids);
        add_import("context_null", 2, &mut self.import_ids);
        add_import("context_enter", 2, &mut self.import_ids);
        add_import("context_exit", 3, &mut self.import_ids);
        add_import("context_unwind", 2, &mut self.import_ids);
        add_import("context_depth", 0, &mut self.import_ids);
        add_import("context_unwind_to", 3, &mut self.import_ids);
        add_import("context_closing", 2, &mut self.import_ids);
        add_import("exception_push", 0, &mut self.import_ids);
        add_import("exception_pop", 0, &mut self.import_ids);
        add_import("exception_last", 0, &mut self.import_ids);
        add_import("exception_new", 3, &mut self.import_ids);
        add_import("exception_clear", 0, &mut self.import_ids);
        add_import("exception_pending", 0, &mut self.import_ids);
        add_import("exception_kind", 2, &mut self.import_ids);
        add_import("exception_message", 2, &mut self.import_ids);
        add_import("exception_set_cause", 3, &mut self.import_ids);
        add_import("exception_context_set", 2, &mut self.import_ids);
        add_import("raise", 2, &mut self.import_ids);
        add_import("bridge_unavailable", 2, &mut self.import_ids);
        add_import("file_open", 3, &mut self.import_ids);
        add_import("file_read", 3, &mut self.import_ids);
        add_import("file_write", 3, &mut self.import_ids);
        add_import("file_close", 2, &mut self.import_ids);

        self.func_count = import_idx;

        let mut user_type_map: HashMap<usize, u32> = HashMap::new();
        user_type_map.insert(0, 0);
        user_type_map.insert(1, 2);
        let mut next_type_idx = 10u32;
        for func_ir in &ir.functions {
            if func_ir.name.ends_with("_poll") {
                continue;
            }
            let arity = func_ir.params.len();
            if let std::collections::hash_map::Entry::Vacant(entry) = user_type_map.entry(arity) {
                self.types.function(
                    std::iter::repeat_n(ValType::I64, arity),
                    std::iter::once(ValType::I64),
                );
                entry.insert(next_type_idx);
                next_type_idx += 1;
            }
        }

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
        let mut table_indices = vec![self.import_ids["async_sleep"]]; // async_sleep at index 0 of table
        let mut func_to_table_idx = HashMap::new();
        let mut func_to_index = HashMap::new();
        func_to_table_idx.insert("molt_async_sleep".to_string(), 0);

        let user_func_start = self.func_count;
        for (i, func_ir) in ir.functions.iter().enumerate() {
            let idx = (i + 1) as u32;
            func_to_table_idx.insert(func_ir.name.clone(), idx);
            func_to_index.insert(func_ir.name.clone(), user_func_start + i as u32);
            table_indices.push(user_func_start + i as u32);
        }

        self.elements.active(
            None,
            &ConstExpr::i32_const(0),
            wasm_encoder::Elements::Functions(&table_indices),
        );

        let import_ids = self.import_ids.clone();
        for func_ir in ir.functions {
            let type_idx = if func_ir.name.ends_with("_poll") {
                2
            } else {
                *user_type_map.get(&func_ir.params.len()).unwrap_or(&0)
            };
            self.compile_func(
                func_ir,
                type_idx,
                &func_to_table_idx,
                &func_to_index,
                &import_ids,
                &user_type_map,
            );
        }

        self.module.section(&self.types);
        self.module.section(&self.imports);
        self.module.section(&self.funcs);
        self.module.section(&self.tables);
        self.module.section(&self.memories);
        self.module.section(&self.exports);
        self.module.section(&self.elements);
        self.module.section(&self.codes);
        self.module.section(&self.data);

        self.module.finish()
    }

    fn compile_func(
        &mut self,
        func_ir: FunctionIR,
        type_idx: u32,
        func_map: &HashMap<String, u32>,
        func_indices: &HashMap<String, u32>,
        import_ids: &HashMap<String, u32>,
        user_type_map: &HashMap<usize, u32>,
    ) {
        self.funcs.function(type_idx);
        self.exports
            .export(&func_ir.name, ExportKind::Func, self.func_count);
        self.func_count += 1;

        let mut locals = HashMap::new();
        let mut local_count = 0;
        let mut local_types = Vec::new();

        for (idx, name) in func_ir.params.iter().enumerate() {
            locals.insert(name.clone(), idx as u32);
            local_count += 1;
        }

        if type_idx == 2 {
            locals.entry("self_param".to_string()).or_insert(0);
            locals.entry("self".to_string()).or_insert(0);
            if local_count == 0 {
                local_count = 1;
            }
        }

        for op in &func_ir.ops {
            if let Some(out) = &op.out {
                if let std::collections::hash_map::Entry::Vacant(entry) = locals.entry(out.clone())
                {
                    entry.insert(local_count);
                    local_types.push(ValType::I64);
                    local_count += 1;
                }
                if op.kind == "const_str" || op.kind == "const_bytes" || op.kind == "const_bigint"
                {
                    let ptr_name = format!("{out}_ptr");
                    if let std::collections::hash_map::Entry::Vacant(entry) = locals.entry(ptr_name)
                    {
                        entry.insert(local_count);
                        local_types.push(ValType::I64);
                        local_count += 1;
                    }
                    let len_name = format!("{out}_len");
                    if let std::collections::hash_map::Entry::Vacant(entry) = locals.entry(len_name)
                    {
                        entry.insert(local_count);
                        local_types.push(ValType::I64);
                        local_count += 1;
                    }
                }
            }
        }

        let stateful = func_ir.ops.iter().any(|op| {
            matches!(
                op.kind.as_str(),
                "state_switch"
                    | "state_transition"
                    | "state_yield"
                    | "chan_send_yield"
                    | "chan_recv_yield"
            )
        });
        let state_local = if stateful {
            let idx = local_count;
            local_types.push(ValType::I64);
            Some(idx)
        } else {
            None
        };

        let mut func = Function::new_with_locals_types(local_types);
        #[derive(Clone, Copy)]
        enum ControlKind {
            Block,
            Loop,
            If,
            Try,
        }
        let mut control_stack: Vec<ControlKind> = Vec::new();
        let mut try_stack: Vec<usize> = Vec::new();

        let mut emit_ops = |func: &mut Function,
                            ops: &[OpIR],
                            control_stack: &mut Vec<ControlKind>,
                            try_stack: &mut Vec<usize>| {
            for op in ops {
                match op.kind.as_str() {
                    "const" => {
                        let val = op.value.unwrap();
                        func.instruction(&Instruction::I64Const(box_int(val)));
                        let local_idx = locals[op.out.as_ref().unwrap()];
                        func.instruction(&Instruction::LocalSet(local_idx));
                    }
                    "const_bool" => {
                        let val = op.value.unwrap();
                        func.instruction(&Instruction::I64Const(box_bool(val)));
                        let local_idx = locals[op.out.as_ref().unwrap()];
                        func.instruction(&Instruction::LocalSet(local_idx));
                    }
                    "const_float" => {
                        let val = op.f_value.expect("Float value not found");
                        func.instruction(&Instruction::I64Const(box_float(val)));
                        let local_idx = locals[op.out.as_ref().unwrap()];
                        func.instruction(&Instruction::LocalSet(local_idx));
                    }
                    "const_none" => {
                        func.instruction(&Instruction::I64Const(box_none()));
                        let local_idx = locals[op.out.as_ref().unwrap()];
                        func.instruction(&Instruction::LocalSet(local_idx));
                    }
                    "const_str" => {
                        let s = op.s_value.as_ref().unwrap();
                        let out_name = op.out.as_ref().unwrap();
                        let bytes = s.as_bytes();
                        let offset = self.data_offset;
                        self.data.active(
                            0,
                            &ConstExpr::i32_const(offset as i32),
                            bytes.iter().copied(),
                        );
                        self.data_offset = (self.data_offset + bytes.len() as u32 + 7) & !7;

                        let ptr_local = locals[&format!("{out_name}_ptr")];
                        let len_local = locals[&format!("{out_name}_len")];
                        func.instruction(&Instruction::I64Const(offset as i64));
                        func.instruction(&Instruction::LocalSet(ptr_local));
                        func.instruction(&Instruction::I64Const(bytes.len() as i64));
                        func.instruction(&Instruction::LocalSet(len_local));

                        func.instruction(&Instruction::I64Const(8));
                        func.instruction(&Instruction::Call(import_ids["alloc"]));
                        let out_local = locals[out_name];
                        func.instruction(&Instruction::LocalSet(out_local));

                        func.instruction(&Instruction::LocalGet(ptr_local));
                        func.instruction(&Instruction::LocalGet(len_local));
                        func.instruction(&Instruction::LocalGet(out_local));
                        func.instruction(&Instruction::Call(import_ids["string_from_bytes"]));
                        func.instruction(&Instruction::Drop);

                        func.instruction(&Instruction::LocalGet(out_local));
                        func.instruction(&Instruction::I32WrapI64);
                        func.instruction(&Instruction::I64Load(wasm_encoder::MemArg {
                            align: 3,
                            offset: 0,
                            memory_index: 0,
                        }));
                        func.instruction(&Instruction::LocalSet(out_local));
                    }
                    "const_bigint" => {
                        let s = op.s_value.as_ref().unwrap();
                        let out_name = op.out.as_ref().unwrap();
                        let bytes = s.as_bytes();
                        let offset = self.data_offset;
                        self.data.active(
                            0,
                            &ConstExpr::i32_const(offset as i32),
                            bytes.iter().copied(),
                        );
                        self.data_offset = (self.data_offset + bytes.len() as u32 + 7) & !7;

                        let ptr_local = locals[&format!("{out_name}_ptr")];
                        let len_local = locals[&format!("{out_name}_len")];
                        func.instruction(&Instruction::I64Const(offset as i64));
                        func.instruction(&Instruction::LocalSet(ptr_local));
                        func.instruction(&Instruction::I64Const(bytes.len() as i64));
                        func.instruction(&Instruction::LocalSet(len_local));

                        func.instruction(&Instruction::LocalGet(ptr_local));
                        func.instruction(&Instruction::LocalGet(len_local));
                        func.instruction(&Instruction::Call(import_ids["bigint_from_str"]));
                        let out_local = locals[out_name];
                        func.instruction(&Instruction::LocalSet(out_local));
                    }
                    "const_bytes" => {
                        let bytes = op.bytes.as_ref().expect("Bytes not found");
                        let out_name = op.out.as_ref().unwrap();
                        let offset = self.data_offset;
                        self.data.active(
                            0,
                            &ConstExpr::i32_const(offset as i32),
                            bytes.iter().copied(),
                        );
                        self.data_offset = (self.data_offset + bytes.len() as u32 + 7) & !7;

                        let ptr_local = locals[&format!("{out_name}_ptr")];
                        let len_local = locals[&format!("{out_name}_len")];
                        func.instruction(&Instruction::I64Const(offset as i64));
                        func.instruction(&Instruction::LocalSet(ptr_local));
                        func.instruction(&Instruction::I64Const(bytes.len() as i64));
                        func.instruction(&Instruction::LocalSet(len_local));

                        func.instruction(&Instruction::I64Const(8));
                        func.instruction(&Instruction::Call(import_ids["alloc"]));
                        let out_local = locals[out_name];
                        func.instruction(&Instruction::LocalSet(out_local));

                        func.instruction(&Instruction::LocalGet(ptr_local));
                        func.instruction(&Instruction::LocalGet(len_local));
                        func.instruction(&Instruction::LocalGet(out_local));
                        func.instruction(&Instruction::Call(import_ids["bytes_from_bytes"]));
                        func.instruction(&Instruction::Drop);

                        func.instruction(&Instruction::LocalGet(out_local));
                        func.instruction(&Instruction::I32WrapI64);
                        func.instruction(&Instruction::I64Load(wasm_encoder::MemArg {
                            align: 3,
                            offset: 0,
                            memory_index: 0,
                        }));
                        func.instruction(&Instruction::LocalSet(out_local));
                    }
                    "add" => {
                        let args = op.args.as_ref().unwrap();
                        let lhs = locals[&args[0]];
                        let rhs = locals[&args[1]];
                        func.instruction(&Instruction::LocalGet(lhs));
                        func.instruction(&Instruction::LocalGet(rhs));
                        func.instruction(&Instruction::Call(import_ids["add"]));
                        let res = locals[op.out.as_ref().unwrap()];
                        func.instruction(&Instruction::LocalSet(res));
                    }
                    "vec_sum_int" => {
                        let args = op.args.as_ref().unwrap();
                        let seq = locals[&args[0]];
                        let acc = locals[&args[1]];
                        func.instruction(&Instruction::LocalGet(seq));
                        func.instruction(&Instruction::LocalGet(acc));
                        func.instruction(&Instruction::Call(import_ids["vec_sum_int"]));
                        let res = locals[op.out.as_ref().unwrap()];
                        func.instruction(&Instruction::LocalSet(res));
                    }
                    "vec_sum_int_trusted" => {
                        let args = op.args.as_ref().unwrap();
                        let seq = locals[&args[0]];
                        let acc = locals[&args[1]];
                        func.instruction(&Instruction::LocalGet(seq));
                        func.instruction(&Instruction::LocalGet(acc));
                        func.instruction(&Instruction::Call(import_ids["vec_sum_int_trusted"]));
                        let res = locals[op.out.as_ref().unwrap()];
                        func.instruction(&Instruction::LocalSet(res));
                    }
                    "vec_sum_int_range" => {
                        let args = op.args.as_ref().unwrap();
                        let seq = locals[&args[0]];
                        let acc = locals[&args[1]];
                        let start = locals[&args[2]];
                        func.instruction(&Instruction::LocalGet(seq));
                        func.instruction(&Instruction::LocalGet(acc));
                        func.instruction(&Instruction::LocalGet(start));
                        func.instruction(&Instruction::Call(import_ids["vec_sum_int_range"]));
                        let res = locals[op.out.as_ref().unwrap()];
                        func.instruction(&Instruction::LocalSet(res));
                    }
                    "vec_sum_int_range_trusted" => {
                        let args = op.args.as_ref().unwrap();
                        let seq = locals[&args[0]];
                        let acc = locals[&args[1]];
                        let start = locals[&args[2]];
                        func.instruction(&Instruction::LocalGet(seq));
                        func.instruction(&Instruction::LocalGet(acc));
                        func.instruction(&Instruction::LocalGet(start));
                        func.instruction(&Instruction::Call(
                            import_ids["vec_sum_int_range_trusted"],
                        ));
                        let res = locals[op.out.as_ref().unwrap()];
                        func.instruction(&Instruction::LocalSet(res));
                    }
                    "vec_prod_int" => {
                        let args = op.args.as_ref().unwrap();
                        let seq = locals[&args[0]];
                        let acc = locals[&args[1]];
                        func.instruction(&Instruction::LocalGet(seq));
                        func.instruction(&Instruction::LocalGet(acc));
                        func.instruction(&Instruction::Call(import_ids["vec_prod_int"]));
                        let res = locals[op.out.as_ref().unwrap()];
                        func.instruction(&Instruction::LocalSet(res));
                    }
                    "vec_prod_int_trusted" => {
                        let args = op.args.as_ref().unwrap();
                        let seq = locals[&args[0]];
                        let acc = locals[&args[1]];
                        func.instruction(&Instruction::LocalGet(seq));
                        func.instruction(&Instruction::LocalGet(acc));
                        func.instruction(&Instruction::Call(import_ids["vec_prod_int_trusted"]));
                        let res = locals[op.out.as_ref().unwrap()];
                        func.instruction(&Instruction::LocalSet(res));
                    }
                    "vec_prod_int_range" => {
                        let args = op.args.as_ref().unwrap();
                        let seq = locals[&args[0]];
                        let acc = locals[&args[1]];
                        let start = locals[&args[2]];
                        func.instruction(&Instruction::LocalGet(seq));
                        func.instruction(&Instruction::LocalGet(acc));
                        func.instruction(&Instruction::LocalGet(start));
                        func.instruction(&Instruction::Call(import_ids["vec_prod_int_range"]));
                        let res = locals[op.out.as_ref().unwrap()];
                        func.instruction(&Instruction::LocalSet(res));
                    }
                    "vec_prod_int_range_trusted" => {
                        let args = op.args.as_ref().unwrap();
                        let seq = locals[&args[0]];
                        let acc = locals[&args[1]];
                        let start = locals[&args[2]];
                        func.instruction(&Instruction::LocalGet(seq));
                        func.instruction(&Instruction::LocalGet(acc));
                        func.instruction(&Instruction::LocalGet(start));
                        func.instruction(&Instruction::Call(
                            import_ids["vec_prod_int_range_trusted"],
                        ));
                        let res = locals[op.out.as_ref().unwrap()];
                        func.instruction(&Instruction::LocalSet(res));
                    }
                    "vec_min_int" => {
                        let args = op.args.as_ref().unwrap();
                        let seq = locals[&args[0]];
                        let acc = locals[&args[1]];
                        func.instruction(&Instruction::LocalGet(seq));
                        func.instruction(&Instruction::LocalGet(acc));
                        func.instruction(&Instruction::Call(import_ids["vec_min_int"]));
                        let res = locals[op.out.as_ref().unwrap()];
                        func.instruction(&Instruction::LocalSet(res));
                    }
                    "vec_min_int_trusted" => {
                        let args = op.args.as_ref().unwrap();
                        let seq = locals[&args[0]];
                        let acc = locals[&args[1]];
                        func.instruction(&Instruction::LocalGet(seq));
                        func.instruction(&Instruction::LocalGet(acc));
                        func.instruction(&Instruction::Call(import_ids["vec_min_int_trusted"]));
                        let res = locals[op.out.as_ref().unwrap()];
                        func.instruction(&Instruction::LocalSet(res));
                    }
                    "vec_min_int_range" => {
                        let args = op.args.as_ref().unwrap();
                        let seq = locals[&args[0]];
                        let acc = locals[&args[1]];
                        let start = locals[&args[2]];
                        func.instruction(&Instruction::LocalGet(seq));
                        func.instruction(&Instruction::LocalGet(acc));
                        func.instruction(&Instruction::LocalGet(start));
                        func.instruction(&Instruction::Call(import_ids["vec_min_int_range"]));
                        let res = locals[op.out.as_ref().unwrap()];
                        func.instruction(&Instruction::LocalSet(res));
                    }
                    "vec_min_int_range_trusted" => {
                        let args = op.args.as_ref().unwrap();
                        let seq = locals[&args[0]];
                        let acc = locals[&args[1]];
                        let start = locals[&args[2]];
                        func.instruction(&Instruction::LocalGet(seq));
                        func.instruction(&Instruction::LocalGet(acc));
                        func.instruction(&Instruction::LocalGet(start));
                        func.instruction(&Instruction::Call(
                            import_ids["vec_min_int_range_trusted"],
                        ));
                        let res = locals[op.out.as_ref().unwrap()];
                        func.instruction(&Instruction::LocalSet(res));
                    }
                    "vec_max_int" => {
                        let args = op.args.as_ref().unwrap();
                        let seq = locals[&args[0]];
                        let acc = locals[&args[1]];
                        func.instruction(&Instruction::LocalGet(seq));
                        func.instruction(&Instruction::LocalGet(acc));
                        func.instruction(&Instruction::Call(import_ids["vec_max_int"]));
                        let res = locals[op.out.as_ref().unwrap()];
                        func.instruction(&Instruction::LocalSet(res));
                    }
                    "vec_max_int_trusted" => {
                        let args = op.args.as_ref().unwrap();
                        let seq = locals[&args[0]];
                        let acc = locals[&args[1]];
                        func.instruction(&Instruction::LocalGet(seq));
                        func.instruction(&Instruction::LocalGet(acc));
                        func.instruction(&Instruction::Call(import_ids["vec_max_int_trusted"]));
                        let res = locals[op.out.as_ref().unwrap()];
                        func.instruction(&Instruction::LocalSet(res));
                    }
                    "vec_max_int_range" => {
                        let args = op.args.as_ref().unwrap();
                        let seq = locals[&args[0]];
                        let acc = locals[&args[1]];
                        let start = locals[&args[2]];
                        func.instruction(&Instruction::LocalGet(seq));
                        func.instruction(&Instruction::LocalGet(acc));
                        func.instruction(&Instruction::LocalGet(start));
                        func.instruction(&Instruction::Call(import_ids["vec_max_int_range"]));
                        let res = locals[op.out.as_ref().unwrap()];
                        func.instruction(&Instruction::LocalSet(res));
                    }
                    "vec_max_int_range_trusted" => {
                        let args = op.args.as_ref().unwrap();
                        let seq = locals[&args[0]];
                        let acc = locals[&args[1]];
                        let start = locals[&args[2]];
                        func.instruction(&Instruction::LocalGet(seq));
                        func.instruction(&Instruction::LocalGet(acc));
                        func.instruction(&Instruction::LocalGet(start));
                        func.instruction(&Instruction::Call(
                            import_ids["vec_max_int_range_trusted"],
                        ));
                        let res = locals[op.out.as_ref().unwrap()];
                        func.instruction(&Instruction::LocalSet(res));
                    }
                    "sub" => {
                        let args = op.args.as_ref().unwrap();
                        let lhs = locals[&args[0]];
                        let rhs = locals[&args[1]];
                        func.instruction(&Instruction::LocalGet(lhs));
                        func.instruction(&Instruction::LocalGet(rhs));
                        func.instruction(&Instruction::Call(import_ids["sub"]));
                        let res = locals[op.out.as_ref().unwrap()];
                        func.instruction(&Instruction::LocalSet(res));
                    }
                    "mul" => {
                        let args = op.args.as_ref().unwrap();
                        let lhs = locals[&args[0]];
                        let rhs = locals[&args[1]];
                        func.instruction(&Instruction::LocalGet(lhs));
                        func.instruction(&Instruction::LocalGet(rhs));
                        func.instruction(&Instruction::Call(import_ids["mul"]));
                        let res = locals[op.out.as_ref().unwrap()];
                        func.instruction(&Instruction::LocalSet(res));
                    }
                    "bit_or" => {
                        let args = op.args.as_ref().unwrap();
                        let lhs = locals[&args[0]];
                        let rhs = locals[&args[1]];
                        func.instruction(&Instruction::LocalGet(lhs));
                        func.instruction(&Instruction::LocalGet(rhs));
                        func.instruction(&Instruction::Call(import_ids["bit_or"]));
                        let res = locals[op.out.as_ref().unwrap()];
                        func.instruction(&Instruction::LocalSet(res));
                    }
                    "bit_and" => {
                        let args = op.args.as_ref().unwrap();
                        let lhs = locals[&args[0]];
                        let rhs = locals[&args[1]];
                        func.instruction(&Instruction::LocalGet(lhs));
                        func.instruction(&Instruction::LocalGet(rhs));
                        func.instruction(&Instruction::Call(import_ids["bit_and"]));
                        let res = locals[op.out.as_ref().unwrap()];
                        func.instruction(&Instruction::LocalSet(res));
                    }
                    "bit_xor" => {
                        let args = op.args.as_ref().unwrap();
                        let lhs = locals[&args[0]];
                        let rhs = locals[&args[1]];
                        func.instruction(&Instruction::LocalGet(lhs));
                        func.instruction(&Instruction::LocalGet(rhs));
                        func.instruction(&Instruction::Call(import_ids["bit_xor"]));
                        let res = locals[op.out.as_ref().unwrap()];
                        func.instruction(&Instruction::LocalSet(res));
                    }
                    "lshift" => {
                        let args = op.args.as_ref().unwrap();
                        let lhs = locals[&args[0]];
                        let rhs = locals[&args[1]];
                        func.instruction(&Instruction::LocalGet(lhs));
                        func.instruction(&Instruction::LocalGet(rhs));
                        func.instruction(&Instruction::Call(import_ids["lshift"]));
                        let res = locals[op.out.as_ref().unwrap()];
                        func.instruction(&Instruction::LocalSet(res));
                    }
                    "rshift" => {
                        let args = op.args.as_ref().unwrap();
                        let lhs = locals[&args[0]];
                        let rhs = locals[&args[1]];
                        func.instruction(&Instruction::LocalGet(lhs));
                        func.instruction(&Instruction::LocalGet(rhs));
                        func.instruction(&Instruction::Call(import_ids["rshift"]));
                        let res = locals[op.out.as_ref().unwrap()];
                        func.instruction(&Instruction::LocalSet(res));
                    }
                    "matmul" => {
                        let args = op.args.as_ref().unwrap();
                        let lhs = locals[&args[0]];
                        let rhs = locals[&args[1]];
                        func.instruction(&Instruction::LocalGet(lhs));
                        func.instruction(&Instruction::LocalGet(rhs));
                        func.instruction(&Instruction::Call(import_ids["matmul"]));
                        let res = locals[op.out.as_ref().unwrap()];
                        func.instruction(&Instruction::LocalSet(res));
                    }
                    "div" => {
                        let args = op.args.as_ref().unwrap();
                        let lhs = locals[&args[0]];
                        let rhs = locals[&args[1]];
                        func.instruction(&Instruction::LocalGet(lhs));
                        func.instruction(&Instruction::LocalGet(rhs));
                        func.instruction(&Instruction::Call(import_ids["div"]));
                        let res = locals[op.out.as_ref().unwrap()];
                        func.instruction(&Instruction::LocalSet(res));
                    }
                    "floordiv" => {
                        let args = op.args.as_ref().unwrap();
                        let lhs = locals[&args[0]];
                        let rhs = locals[&args[1]];
                        func.instruction(&Instruction::LocalGet(lhs));
                        func.instruction(&Instruction::LocalGet(rhs));
                        func.instruction(&Instruction::Call(import_ids["floordiv"]));
                        let res = locals[op.out.as_ref().unwrap()];
                        func.instruction(&Instruction::LocalSet(res));
                    }
                    "mod" => {
                        let args = op.args.as_ref().unwrap();
                        let lhs = locals[&args[0]];
                        let rhs = locals[&args[1]];
                        func.instruction(&Instruction::LocalGet(lhs));
                        func.instruction(&Instruction::LocalGet(rhs));
                        func.instruction(&Instruction::Call(import_ids["mod"]));
                        let res = locals[op.out.as_ref().unwrap()];
                        func.instruction(&Instruction::LocalSet(res));
                    }
                    "pow" => {
                        let args = op.args.as_ref().unwrap();
                        let lhs = locals[&args[0]];
                        let rhs = locals[&args[1]];
                        func.instruction(&Instruction::LocalGet(lhs));
                        func.instruction(&Instruction::LocalGet(rhs));
                        func.instruction(&Instruction::Call(import_ids["pow"]));
                        let res = locals[op.out.as_ref().unwrap()];
                        func.instruction(&Instruction::LocalSet(res));
                    }
                    "pow_mod" => {
                        let args = op.args.as_ref().unwrap();
                        let lhs = locals[&args[0]];
                        let rhs = locals[&args[1]];
                        let modulus = locals[&args[2]];
                        func.instruction(&Instruction::LocalGet(lhs));
                        func.instruction(&Instruction::LocalGet(rhs));
                        func.instruction(&Instruction::LocalGet(modulus));
                        func.instruction(&Instruction::Call(import_ids["pow_mod"]));
                        let res = locals[op.out.as_ref().unwrap()];
                        func.instruction(&Instruction::LocalSet(res));
                    }
                    "round" => {
                        let args = op.args.as_ref().unwrap();
                        let val = locals[&args[0]];
                        let ndigits = locals[&args[1]];
                        let has_ndigits = locals[&args[2]];
                        func.instruction(&Instruction::LocalGet(val));
                        func.instruction(&Instruction::LocalGet(ndigits));
                        func.instruction(&Instruction::LocalGet(has_ndigits));
                        func.instruction(&Instruction::Call(import_ids["round"]));
                        let res = locals[op.out.as_ref().unwrap()];
                        func.instruction(&Instruction::LocalSet(res));
                    }
                    "trunc" => {
                        let args = op.args.as_ref().unwrap();
                        let val = locals[&args[0]];
                        func.instruction(&Instruction::LocalGet(val));
                        func.instruction(&Instruction::Call(import_ids["trunc"]));
                        let res = locals[op.out.as_ref().unwrap()];
                        func.instruction(&Instruction::LocalSet(res));
                    }
                    "lt" => {
                        let args = op.args.as_ref().unwrap();
                        let lhs = locals[&args[0]];
                        let rhs = locals[&args[1]];
                        func.instruction(&Instruction::LocalGet(lhs));
                        func.instruction(&Instruction::LocalGet(rhs));
                        func.instruction(&Instruction::Call(import_ids["lt"]));
                        let res = locals[op.out.as_ref().unwrap()];
                        func.instruction(&Instruction::LocalSet(res));
                    }
                    "le" => {
                        let args = op.args.as_ref().unwrap();
                        let lhs = locals[&args[0]];
                        let rhs = locals[&args[1]];
                        func.instruction(&Instruction::LocalGet(lhs));
                        func.instruction(&Instruction::LocalGet(rhs));
                        func.instruction(&Instruction::Call(import_ids["le"]));
                        let res = locals[op.out.as_ref().unwrap()];
                        func.instruction(&Instruction::LocalSet(res));
                    }
                    "gt" => {
                        let args = op.args.as_ref().unwrap();
                        let lhs = locals[&args[0]];
                        let rhs = locals[&args[1]];
                        func.instruction(&Instruction::LocalGet(lhs));
                        func.instruction(&Instruction::LocalGet(rhs));
                        func.instruction(&Instruction::Call(import_ids["gt"]));
                        let res = locals[op.out.as_ref().unwrap()];
                        func.instruction(&Instruction::LocalSet(res));
                    }
                    "ge" => {
                        let args = op.args.as_ref().unwrap();
                        let lhs = locals[&args[0]];
                        let rhs = locals[&args[1]];
                        func.instruction(&Instruction::LocalGet(lhs));
                        func.instruction(&Instruction::LocalGet(rhs));
                        func.instruction(&Instruction::Call(import_ids["ge"]));
                        let res = locals[op.out.as_ref().unwrap()];
                        func.instruction(&Instruction::LocalSet(res));
                    }
                    "eq" => {
                        let args = op.args.as_ref().unwrap();
                        let lhs = locals[&args[0]];
                        let rhs = locals[&args[1]];
                        func.instruction(&Instruction::LocalGet(lhs));
                        func.instruction(&Instruction::LocalGet(rhs));
                        func.instruction(&Instruction::Call(import_ids["eq"]));
                        let res = locals[op.out.as_ref().unwrap()];
                        func.instruction(&Instruction::LocalSet(res));
                    }
                    "is" => {
                        let args = op.args.as_ref().unwrap();
                        let lhs = locals[&args[0]];
                        let rhs = locals[&args[1]];
                        func.instruction(&Instruction::LocalGet(lhs));
                        func.instruction(&Instruction::LocalGet(rhs));
                        func.instruction(&Instruction::Call(import_ids["is"]));
                        let res = locals[op.out.as_ref().unwrap()];
                        func.instruction(&Instruction::LocalSet(res));
                    }
                    "not" => {
                        let args = op.args.as_ref().unwrap();
                        let val = locals[&args[0]];
                        func.instruction(&Instruction::LocalGet(val));
                        func.instruction(&Instruction::Call(import_ids["not"]));
                        let res = locals[op.out.as_ref().unwrap()];
                        func.instruction(&Instruction::LocalSet(res));
                    }
                    "and" => {
                        let args = op.args.as_ref().unwrap();
                        let lhs = locals[&args[0]];
                        let rhs = locals[&args[1]];
                        func.instruction(&Instruction::LocalGet(lhs));
                        func.instruction(&Instruction::Call(import_ids["is_truthy"]));
                        func.instruction(&Instruction::LocalGet(rhs));
                        func.instruction(&Instruction::Call(import_ids["is_truthy"]));
                        func.instruction(&Instruction::I64And);
                        func.instruction(&Instruction::I64Const((QNAN | TAG_BOOL) as i64));
                        func.instruction(&Instruction::I64Or);
                        let res = locals[op.out.as_ref().unwrap()];
                        func.instruction(&Instruction::LocalSet(res));
                    }
                    "or" => {
                        let args = op.args.as_ref().unwrap();
                        let lhs = locals[&args[0]];
                        let rhs = locals[&args[1]];
                        func.instruction(&Instruction::LocalGet(lhs));
                        func.instruction(&Instruction::Call(import_ids["is_truthy"]));
                        func.instruction(&Instruction::LocalGet(rhs));
                        func.instruction(&Instruction::Call(import_ids["is_truthy"]));
                        func.instruction(&Instruction::I64Or);
                        func.instruction(&Instruction::I64Const((QNAN | TAG_BOOL) as i64));
                        func.instruction(&Instruction::I64Or);
                        let res = locals[op.out.as_ref().unwrap()];
                        func.instruction(&Instruction::LocalSet(res));
                    }
                    "contains" => {
                        let args = op.args.as_ref().unwrap();
                        let container = locals[&args[0]];
                        let item = locals[&args[1]];
                        func.instruction(&Instruction::LocalGet(container));
                        func.instruction(&Instruction::LocalGet(item));
                        func.instruction(&Instruction::Call(import_ids["contains"]));
                        let res = locals[op.out.as_ref().unwrap()];
                        func.instruction(&Instruction::LocalSet(res));
                    }
                    "guard_type" => {
                        let args = op.args.as_ref().unwrap();
                        let val = locals[&args[0]];
                        let expected = locals[&args[1]];
                        func.instruction(&Instruction::LocalGet(val));
                        func.instruction(&Instruction::LocalGet(expected));
                        func.instruction(&Instruction::Call(import_ids["guard_type"]));
                        if let Some(out) = op.out.as_ref() {
                            let res = locals[out];
                            func.instruction(&Instruction::LocalSet(res));
                        } else {
                            func.instruction(&Instruction::Drop);
                        }
                    }
                    "print" => {
                        let args = op.args.as_ref().unwrap();
                        if let Some(&idx) = locals.get(&args[0]) {
                            func.instruction(&Instruction::LocalGet(idx));
                            func.instruction(&Instruction::Call(import_ids["print_obj"]));
                        }
                    }
                    "print_newline" => {
                        func.instruction(&Instruction::Call(import_ids["print_newline"]));
                    }
                    "alloc" => {
                        func.instruction(&Instruction::I64Const(op.value.unwrap()));
                        func.instruction(&Instruction::Call(import_ids["alloc"]));
                        func.instruction(&Instruction::LocalSet(locals[op.out.as_ref().unwrap()]));
                    }
                    "json_parse" => {
                        let args = op.args.as_ref().unwrap();
                        let arg_name = &args[0];
                        let ptr = locals
                            .get(&format!("{arg_name}_ptr"))
                            .copied()
                            .unwrap_or(locals[arg_name]);
                        let len = locals[&format!("{arg_name}_len")];

                        func.instruction(&Instruction::I64Const(8));
                        func.instruction(&Instruction::Call(import_ids["alloc"]));
                        let out_ptr = locals[op.out.as_ref().unwrap()];
                        func.instruction(&Instruction::LocalSet(out_ptr));

                        func.instruction(&Instruction::LocalGet(ptr));
                        func.instruction(&Instruction::LocalGet(len));
                        func.instruction(&Instruction::LocalGet(out_ptr));
                        func.instruction(&Instruction::Call(import_ids["json_parse_scalar"]));
                        func.instruction(&Instruction::Drop);

                        func.instruction(&Instruction::LocalGet(out_ptr));
                        func.instruction(&Instruction::I32WrapI64);
                        func.instruction(&Instruction::I64Load(wasm_encoder::MemArg {
                            align: 3,
                            offset: 0,
                            memory_index: 0,
                        }));
                        func.instruction(&Instruction::LocalSet(out_ptr));
                    }
                    "msgpack_parse" => {
                        let args = op.args.as_ref().unwrap();
                        let arg_name = &args[0];
                        let ptr = locals
                            .get(&format!("{arg_name}_ptr"))
                            .copied()
                            .unwrap_or(locals[arg_name]);
                        let len = locals[&format!("{arg_name}_len")];

                        func.instruction(&Instruction::I64Const(8));
                        func.instruction(&Instruction::Call(import_ids["alloc"]));
                        let out_ptr = locals[op.out.as_ref().unwrap()];
                        func.instruction(&Instruction::LocalSet(out_ptr));

                        func.instruction(&Instruction::LocalGet(ptr));
                        func.instruction(&Instruction::LocalGet(len));
                        func.instruction(&Instruction::LocalGet(out_ptr));
                        func.instruction(&Instruction::Call(import_ids["msgpack_parse_scalar"]));
                        func.instruction(&Instruction::Drop);

                        func.instruction(&Instruction::LocalGet(out_ptr));
                        func.instruction(&Instruction::I32WrapI64);
                        func.instruction(&Instruction::I64Load(wasm_encoder::MemArg {
                            align: 3,
                            offset: 0,
                            memory_index: 0,
                        }));
                        func.instruction(&Instruction::LocalSet(out_ptr));
                    }
                    "cbor_parse" => {
                        let args = op.args.as_ref().unwrap();
                        let arg_name = &args[0];
                        let ptr = locals
                            .get(&format!("{arg_name}_ptr"))
                            .copied()
                            .unwrap_or(locals[arg_name]);
                        let len = locals[&format!("{arg_name}_len")];

                        func.instruction(&Instruction::I64Const(8));
                        func.instruction(&Instruction::Call(import_ids["alloc"]));
                        let out_ptr = locals[op.out.as_ref().unwrap()];
                        func.instruction(&Instruction::LocalSet(out_ptr));

                        func.instruction(&Instruction::LocalGet(ptr));
                        func.instruction(&Instruction::LocalGet(len));
                        func.instruction(&Instruction::LocalGet(out_ptr));
                        func.instruction(&Instruction::Call(import_ids["cbor_parse_scalar"]));
                        func.instruction(&Instruction::Drop);

                        func.instruction(&Instruction::LocalGet(out_ptr));
                        func.instruction(&Instruction::I32WrapI64);
                        func.instruction(&Instruction::I64Load(wasm_encoder::MemArg {
                            align: 3,
                            offset: 0,
                            memory_index: 0,
                        }));
                        func.instruction(&Instruction::LocalSet(out_ptr));
                    }
                    "len" => {
                        let args = op.args.as_ref().unwrap();
                        let arg = locals[&args[0]];
                        func.instruction(&Instruction::LocalGet(arg));
                        func.instruction(&Instruction::Call(import_ids["len"]));
                        let res = locals[op.out.as_ref().unwrap()];
                        func.instruction(&Instruction::LocalSet(res));
                    }
                    "list_new" => {
                        let args = op.args.as_ref().unwrap();
                        let out = locals[op.out.as_ref().unwrap()];
                        func.instruction(&Instruction::I64Const(args.len() as i64));
                        func.instruction(&Instruction::Call(import_ids["list_builder_new"]));
                        func.instruction(&Instruction::LocalSet(out));
                        for name in args {
                            let val = locals[name];
                            func.instruction(&Instruction::LocalGet(out));
                            func.instruction(&Instruction::LocalGet(val));
                            func.instruction(&Instruction::Call(import_ids["list_builder_append"]));
                        }
                        func.instruction(&Instruction::LocalGet(out));
                        func.instruction(&Instruction::Call(import_ids["list_builder_finish"]));
                        func.instruction(&Instruction::LocalSet(out));
                    }
                    "range_new" => {
                        let args = op.args.as_ref().unwrap();
                        let out = locals[op.out.as_ref().unwrap()];
                        let start = locals[&args[0]];
                        let stop = locals[&args[1]];
                        let step = locals[&args[2]];
                        func.instruction(&Instruction::LocalGet(start));
                        func.instruction(&Instruction::LocalGet(stop));
                        func.instruction(&Instruction::LocalGet(step));
                        func.instruction(&Instruction::Call(import_ids["range_new"]));
                        func.instruction(&Instruction::LocalSet(out));
                    }
                    "tuple_new" => {
                        let args = op.args.as_ref().unwrap();
                        let out = locals[op.out.as_ref().unwrap()];
                        func.instruction(&Instruction::I64Const(args.len() as i64));
                        func.instruction(&Instruction::Call(import_ids["list_builder_new"]));
                        func.instruction(&Instruction::LocalSet(out));
                        for name in args {
                            let val = locals[name];
                            func.instruction(&Instruction::LocalGet(out));
                            func.instruction(&Instruction::LocalGet(val));
                            func.instruction(&Instruction::Call(import_ids["list_builder_append"]));
                        }
                        func.instruction(&Instruction::LocalGet(out));
                        func.instruction(&Instruction::Call(import_ids["tuple_builder_finish"]));
                        func.instruction(&Instruction::LocalSet(out));
                    }
                    "list_append" => {
                        let args = op.args.as_ref().unwrap();
                        let list = locals[&args[0]];
                        let val = locals[&args[1]];
                        func.instruction(&Instruction::LocalGet(list));
                        func.instruction(&Instruction::LocalGet(val));
                        func.instruction(&Instruction::Call(import_ids["list_append"]));
                        let res = locals[op.out.as_ref().unwrap()];
                        func.instruction(&Instruction::LocalSet(res));
                    }
                    "list_pop" => {
                        let args = op.args.as_ref().unwrap();
                        let list = locals[&args[0]];
                        let idx = locals[&args[1]];
                        func.instruction(&Instruction::LocalGet(list));
                        func.instruction(&Instruction::LocalGet(idx));
                        func.instruction(&Instruction::Call(import_ids["list_pop"]));
                        let res = locals[op.out.as_ref().unwrap()];
                        func.instruction(&Instruction::LocalSet(res));
                    }
                    "list_extend" => {
                        let args = op.args.as_ref().unwrap();
                        let list = locals[&args[0]];
                        let other = locals[&args[1]];
                        func.instruction(&Instruction::LocalGet(list));
                        func.instruction(&Instruction::LocalGet(other));
                        func.instruction(&Instruction::Call(import_ids["list_extend"]));
                        let res = locals[op.out.as_ref().unwrap()];
                        func.instruction(&Instruction::LocalSet(res));
                    }
                    "list_insert" => {
                        let args = op.args.as_ref().unwrap();
                        let list = locals[&args[0]];
                        let idx = locals[&args[1]];
                        let val = locals[&args[2]];
                        func.instruction(&Instruction::LocalGet(list));
                        func.instruction(&Instruction::LocalGet(idx));
                        func.instruction(&Instruction::LocalGet(val));
                        func.instruction(&Instruction::Call(import_ids["list_insert"]));
                        let res = locals[op.out.as_ref().unwrap()];
                        func.instruction(&Instruction::LocalSet(res));
                    }
                    "list_remove" => {
                        let args = op.args.as_ref().unwrap();
                        let list = locals[&args[0]];
                        let val = locals[&args[1]];
                        func.instruction(&Instruction::LocalGet(list));
                        func.instruction(&Instruction::LocalGet(val));
                        func.instruction(&Instruction::Call(import_ids["list_remove"]));
                        let res = locals[op.out.as_ref().unwrap()];
                        func.instruction(&Instruction::LocalSet(res));
                    }
                    "list_count" => {
                        let args = op.args.as_ref().unwrap();
                        let list = locals[&args[0]];
                        let val = locals[&args[1]];
                        func.instruction(&Instruction::LocalGet(list));
                        func.instruction(&Instruction::LocalGet(val));
                        func.instruction(&Instruction::Call(import_ids["list_count"]));
                        let res = locals[op.out.as_ref().unwrap()];
                        func.instruction(&Instruction::LocalSet(res));
                    }
                    "list_index" => {
                        let args = op.args.as_ref().unwrap();
                        let list = locals[&args[0]];
                        let val = locals[&args[1]];
                        func.instruction(&Instruction::LocalGet(list));
                        func.instruction(&Instruction::LocalGet(val));
                        func.instruction(&Instruction::Call(import_ids["list_index"]));
                        let res = locals[op.out.as_ref().unwrap()];
                        func.instruction(&Instruction::LocalSet(res));
                    }
                    "tuple_from_list" => {
                        let args = op.args.as_ref().unwrap();
                        let list = locals[&args[0]];
                        func.instruction(&Instruction::LocalGet(list));
                        func.instruction(&Instruction::Call(import_ids["tuple_from_list"]));
                        let res = locals[op.out.as_ref().unwrap()];
                        func.instruction(&Instruction::LocalSet(res));
                    }
                    "dict_new" => {
                        let args = op.args.as_ref().unwrap();
                        let out = locals[op.out.as_ref().unwrap()];
                        func.instruction(&Instruction::I64Const((args.len() / 2) as i64));
                        func.instruction(&Instruction::Call(import_ids["dict_new"]));
                        func.instruction(&Instruction::LocalSet(out));
                        for pair in args.chunks(2) {
                            let key = locals[&pair[0]];
                            let val = locals[&pair[1]];
                            func.instruction(&Instruction::LocalGet(out));
                            func.instruction(&Instruction::LocalGet(key));
                            func.instruction(&Instruction::LocalGet(val));
                            func.instruction(&Instruction::Call(import_ids["dict_set"]));
                            func.instruction(&Instruction::LocalSet(out));
                        }
                    }
                    "set_new" => {
                        let args = op.args.as_ref().unwrap();
                        let out = locals[op.out.as_ref().unwrap()];
                        func.instruction(&Instruction::I64Const(args.len() as i64));
                        func.instruction(&Instruction::Call(import_ids["set_new"]));
                        func.instruction(&Instruction::LocalSet(out));
                        for name in args {
                            let val = locals[name];
                            func.instruction(&Instruction::LocalGet(out));
                            func.instruction(&Instruction::LocalGet(val));
                            func.instruction(&Instruction::Call(import_ids["set_add"]));
                            func.instruction(&Instruction::Drop);
                        }
                    }
                    "frozenset_new" => {
                        let args = op.args.as_ref().unwrap();
                        let out = locals[op.out.as_ref().unwrap()];
                        func.instruction(&Instruction::I64Const(args.len() as i64));
                        func.instruction(&Instruction::Call(import_ids["frozenset_new"]));
                        func.instruction(&Instruction::LocalSet(out));
                        for name in args {
                            let val = locals[name];
                            func.instruction(&Instruction::LocalGet(out));
                            func.instruction(&Instruction::LocalGet(val));
                            func.instruction(&Instruction::Call(import_ids["frozenset_add"]));
                            func.instruction(&Instruction::Drop);
                        }
                    }
                    "dict_get" => {
                        let args = op.args.as_ref().unwrap();
                        let dict = locals[&args[0]];
                        let key = locals[&args[1]];
                        let default = locals[&args[2]];
                        func.instruction(&Instruction::LocalGet(dict));
                        func.instruction(&Instruction::LocalGet(key));
                        func.instruction(&Instruction::LocalGet(default));
                        func.instruction(&Instruction::Call(import_ids["dict_get"]));
                        let res = locals[op.out.as_ref().unwrap()];
                        func.instruction(&Instruction::LocalSet(res));
                    }
                    "dict_pop" => {
                        let args = op.args.as_ref().unwrap();
                        let dict = locals[&args[0]];
                        let key = locals[&args[1]];
                        let default = locals[&args[2]];
                        let has_default = locals[&args[3]];
                        func.instruction(&Instruction::LocalGet(dict));
                        func.instruction(&Instruction::LocalGet(key));
                        func.instruction(&Instruction::LocalGet(default));
                        func.instruction(&Instruction::LocalGet(has_default));
                        func.instruction(&Instruction::Call(import_ids["dict_pop"]));
                        let res = locals[op.out.as_ref().unwrap()];
                        func.instruction(&Instruction::LocalSet(res));
                    }
                    "set_add" => {
                        let args = op.args.as_ref().unwrap();
                        let set_bits = locals[&args[0]];
                        let key = locals[&args[1]];
                        func.instruction(&Instruction::LocalGet(set_bits));
                        func.instruction(&Instruction::LocalGet(key));
                        func.instruction(&Instruction::Call(import_ids["set_add"]));
                        let res = locals[op.out.as_ref().unwrap()];
                        func.instruction(&Instruction::LocalSet(res));
                    }
                    "frozenset_add" => {
                        let args = op.args.as_ref().unwrap();
                        let set_bits = locals[&args[0]];
                        let key = locals[&args[1]];
                        func.instruction(&Instruction::LocalGet(set_bits));
                        func.instruction(&Instruction::LocalGet(key));
                        func.instruction(&Instruction::Call(import_ids["frozenset_add"]));
                        let res = locals[op.out.as_ref().unwrap()];
                        func.instruction(&Instruction::LocalSet(res));
                    }
                    "set_discard" => {
                        let args = op.args.as_ref().unwrap();
                        let set_bits = locals[&args[0]];
                        let key = locals[&args[1]];
                        func.instruction(&Instruction::LocalGet(set_bits));
                        func.instruction(&Instruction::LocalGet(key));
                        func.instruction(&Instruction::Call(import_ids["set_discard"]));
                        let res = locals[op.out.as_ref().unwrap()];
                        func.instruction(&Instruction::LocalSet(res));
                    }
                    "set_remove" => {
                        let args = op.args.as_ref().unwrap();
                        let set_bits = locals[&args[0]];
                        let key = locals[&args[1]];
                        func.instruction(&Instruction::LocalGet(set_bits));
                        func.instruction(&Instruction::LocalGet(key));
                        func.instruction(&Instruction::Call(import_ids["set_remove"]));
                        let res = locals[op.out.as_ref().unwrap()];
                        func.instruction(&Instruction::LocalSet(res));
                    }
                    "set_pop" => {
                        let args = op.args.as_ref().unwrap();
                        let set_bits = locals[&args[0]];
                        func.instruction(&Instruction::LocalGet(set_bits));
                        func.instruction(&Instruction::Call(import_ids["set_pop"]));
                        let res = locals[op.out.as_ref().unwrap()];
                        func.instruction(&Instruction::LocalSet(res));
                    }
                    "set_update" => {
                        let args = op.args.as_ref().unwrap();
                        let set_bits = locals[&args[0]];
                        let other = locals[&args[1]];
                        func.instruction(&Instruction::LocalGet(set_bits));
                        func.instruction(&Instruction::LocalGet(other));
                        func.instruction(&Instruction::Call(import_ids["set_update"]));
                        let res = locals[op.out.as_ref().unwrap()];
                        func.instruction(&Instruction::LocalSet(res));
                    }
                    "set_intersection_update" => {
                        let args = op.args.as_ref().unwrap();
                        let set_bits = locals[&args[0]];
                        let other = locals[&args[1]];
                        func.instruction(&Instruction::LocalGet(set_bits));
                        func.instruction(&Instruction::LocalGet(other));
                        func.instruction(&Instruction::Call(
                            import_ids["set_intersection_update"],
                        ));
                        let res = locals[op.out.as_ref().unwrap()];
                        func.instruction(&Instruction::LocalSet(res));
                    }
                    "set_difference_update" => {
                        let args = op.args.as_ref().unwrap();
                        let set_bits = locals[&args[0]];
                        let other = locals[&args[1]];
                        func.instruction(&Instruction::LocalGet(set_bits));
                        func.instruction(&Instruction::LocalGet(other));
                        func.instruction(&Instruction::Call(
                            import_ids["set_difference_update"],
                        ));
                        let res = locals[op.out.as_ref().unwrap()];
                        func.instruction(&Instruction::LocalSet(res));
                    }
                    "set_symdiff_update" => {
                        let args = op.args.as_ref().unwrap();
                        let set_bits = locals[&args[0]];
                        let other = locals[&args[1]];
                        func.instruction(&Instruction::LocalGet(set_bits));
                        func.instruction(&Instruction::LocalGet(other));
                        func.instruction(&Instruction::Call(
                            import_ids["set_symdiff_update"],
                        ));
                        let res = locals[op.out.as_ref().unwrap()];
                        func.instruction(&Instruction::LocalSet(res));
                    }
                    "dict_keys" => {
                        let args = op.args.as_ref().unwrap();
                        let dict = locals[&args[0]];
                        func.instruction(&Instruction::LocalGet(dict));
                        func.instruction(&Instruction::Call(import_ids["dict_keys"]));
                        let res = locals[op.out.as_ref().unwrap()];
                        func.instruction(&Instruction::LocalSet(res));
                    }
                    "dict_values" => {
                        let args = op.args.as_ref().unwrap();
                        let dict = locals[&args[0]];
                        func.instruction(&Instruction::LocalGet(dict));
                        func.instruction(&Instruction::Call(import_ids["dict_values"]));
                        let res = locals[op.out.as_ref().unwrap()];
                        func.instruction(&Instruction::LocalSet(res));
                    }
                    "dict_items" => {
                        let args = op.args.as_ref().unwrap();
                        let dict = locals[&args[0]];
                        func.instruction(&Instruction::LocalGet(dict));
                        func.instruction(&Instruction::Call(import_ids["dict_items"]));
                        let res = locals[op.out.as_ref().unwrap()];
                        func.instruction(&Instruction::LocalSet(res));
                    }
                    "tuple_count" => {
                        let args = op.args.as_ref().unwrap();
                        let tuple = locals[&args[0]];
                        let val = locals[&args[1]];
                        func.instruction(&Instruction::LocalGet(tuple));
                        func.instruction(&Instruction::LocalGet(val));
                        func.instruction(&Instruction::Call(import_ids["tuple_count"]));
                        let res = locals[op.out.as_ref().unwrap()];
                        func.instruction(&Instruction::LocalSet(res));
                    }
                    "tuple_index" => {
                        let args = op.args.as_ref().unwrap();
                        let tuple = locals[&args[0]];
                        let val = locals[&args[1]];
                        func.instruction(&Instruction::LocalGet(tuple));
                        func.instruction(&Instruction::LocalGet(val));
                        func.instruction(&Instruction::Call(import_ids["tuple_index"]));
                        let res = locals[op.out.as_ref().unwrap()];
                        func.instruction(&Instruction::LocalSet(res));
                    }
                    "iter" => {
                        let args = op.args.as_ref().unwrap();
                        let obj = locals[&args[0]];
                        func.instruction(&Instruction::LocalGet(obj));
                        func.instruction(&Instruction::Call(import_ids["iter"]));
                        let res = locals[op.out.as_ref().unwrap()];
                        func.instruction(&Instruction::LocalSet(res));
                    }
                    "aiter" => {
                        let args = op.args.as_ref().unwrap();
                        let obj = locals[&args[0]];
                        func.instruction(&Instruction::LocalGet(obj));
                        func.instruction(&Instruction::Call(import_ids["aiter"]));
                        let res = locals[op.out.as_ref().unwrap()];
                        func.instruction(&Instruction::LocalSet(res));
                    }
                    "iter_next" => {
                        let args = op.args.as_ref().unwrap();
                        let iter = locals[&args[0]];
                        func.instruction(&Instruction::LocalGet(iter));
                        func.instruction(&Instruction::Call(import_ids["iter_next"]));
                        let res = locals[op.out.as_ref().unwrap()];
                        func.instruction(&Instruction::LocalSet(res));
                    }
                    "anext" => {
                        let args = op.args.as_ref().unwrap();
                        let iter = locals[&args[0]];
                        func.instruction(&Instruction::LocalGet(iter));
                        func.instruction(&Instruction::Call(import_ids["anext"]));
                        let res = locals[op.out.as_ref().unwrap()];
                        func.instruction(&Instruction::LocalSet(res));
                    }
                    "gen_send" => {
                        let args = op.args.as_ref().unwrap();
                        let gen = locals[&args[0]];
                        let val = locals[&args[1]];
                        func.instruction(&Instruction::LocalGet(gen));
                        func.instruction(&Instruction::LocalGet(val));
                        func.instruction(&Instruction::Call(import_ids["generator_send"]));
                        let res = locals[op.out.as_ref().unwrap()];
                        func.instruction(&Instruction::LocalSet(res));
                    }
                    "gen_throw" => {
                        let args = op.args.as_ref().unwrap();
                        let gen = locals[&args[0]];
                        let val = locals[&args[1]];
                        func.instruction(&Instruction::LocalGet(gen));
                        func.instruction(&Instruction::LocalGet(val));
                        func.instruction(&Instruction::Call(import_ids["generator_throw"]));
                        let res = locals[op.out.as_ref().unwrap()];
                        func.instruction(&Instruction::LocalSet(res));
                    }
                    "gen_close" => {
                        let args = op.args.as_ref().unwrap();
                        let gen = locals[&args[0]];
                        func.instruction(&Instruction::LocalGet(gen));
                        func.instruction(&Instruction::Call(import_ids["generator_close"]));
                        let res = locals[op.out.as_ref().unwrap()];
                        func.instruction(&Instruction::LocalSet(res));
                    }
                    "is_generator" => {
                        let args = op.args.as_ref().unwrap();
                        let obj = locals[&args[0]];
                        func.instruction(&Instruction::LocalGet(obj));
                        func.instruction(&Instruction::Call(import_ids["is_generator"]));
                        let res = locals[op.out.as_ref().unwrap()];
                        func.instruction(&Instruction::LocalSet(res));
                    }
                    "index" => {
                        let args = op.args.as_ref().unwrap();
                        let obj = locals[&args[0]];
                        let idx = locals[&args[1]];
                        func.instruction(&Instruction::LocalGet(obj));
                        func.instruction(&Instruction::LocalGet(idx));
                        func.instruction(&Instruction::Call(import_ids["index"]));
                        let res = locals[op.out.as_ref().unwrap()];
                        func.instruction(&Instruction::LocalSet(res));
                    }
                    "store_index" => {
                        let args = op.args.as_ref().unwrap();
                        let obj = locals[&args[0]];
                        let idx = locals[&args[1]];
                        let val = locals[&args[2]];
                        func.instruction(&Instruction::LocalGet(obj));
                        func.instruction(&Instruction::LocalGet(idx));
                        func.instruction(&Instruction::LocalGet(val));
                        func.instruction(&Instruction::Call(import_ids["store_index"]));
                        if let Some(out) = op.out.as_ref() {
                            let res = locals[out];
                            func.instruction(&Instruction::LocalSet(res));
                        } else {
                            func.instruction(&Instruction::Drop);
                        }
                    }
                    "slice" => {
                        let args = op.args.as_ref().unwrap();
                        let obj = locals[&args[0]];
                        let start = locals[&args[1]];
                        let end = locals[&args[2]];
                        func.instruction(&Instruction::LocalGet(obj));
                        func.instruction(&Instruction::LocalGet(start));
                        func.instruction(&Instruction::LocalGet(end));
                        func.instruction(&Instruction::Call(import_ids["slice"]));
                        let res = locals[op.out.as_ref().unwrap()];
                        func.instruction(&Instruction::LocalSet(res));
                    }
                    "slice_new" => {
                        let args = op.args.as_ref().unwrap();
                        let start = locals[&args[0]];
                        let stop = locals[&args[1]];
                        let step = locals[&args[2]];
                        func.instruction(&Instruction::LocalGet(start));
                        func.instruction(&Instruction::LocalGet(stop));
                        func.instruction(&Instruction::LocalGet(step));
                        func.instruction(&Instruction::Call(import_ids["slice_new"]));
                        let res = locals[op.out.as_ref().unwrap()];
                        func.instruction(&Instruction::LocalSet(res));
                    }
                    "bytes_find" => {
                        let args = op.args.as_ref().unwrap();
                        let hay = locals[&args[0]];
                        let needle = locals[&args[1]];
                        func.instruction(&Instruction::LocalGet(hay));
                        func.instruction(&Instruction::LocalGet(needle));
                        func.instruction(&Instruction::Call(import_ids["bytes_find"]));
                        let res = locals[op.out.as_ref().unwrap()];
                        func.instruction(&Instruction::LocalSet(res));
                    }
                    "bytearray_find" => {
                        let args = op.args.as_ref().unwrap();
                        let hay = locals[&args[0]];
                        let needle = locals[&args[1]];
                        func.instruction(&Instruction::LocalGet(hay));
                        func.instruction(&Instruction::LocalGet(needle));
                        func.instruction(&Instruction::Call(import_ids["bytearray_find"]));
                        let res = locals[op.out.as_ref().unwrap()];
                        func.instruction(&Instruction::LocalSet(res));
                    }
                    "string_find" => {
                        let args = op.args.as_ref().unwrap();
                        let hay = locals[&args[0]];
                        let needle = locals[&args[1]];
                        func.instruction(&Instruction::LocalGet(hay));
                        func.instruction(&Instruction::LocalGet(needle));
                        func.instruction(&Instruction::Call(import_ids["string_find"]));
                        let res = locals[op.out.as_ref().unwrap()];
                        func.instruction(&Instruction::LocalSet(res));
                    }
                    "string_format" => {
                        let args = op.args.as_ref().unwrap();
                        let val = locals[&args[0]];
                        let spec = locals[&args[1]];
                        func.instruction(&Instruction::LocalGet(val));
                        func.instruction(&Instruction::LocalGet(spec));
                        func.instruction(&Instruction::Call(import_ids["string_format"]));
                        let res = locals[op.out.as_ref().unwrap()];
                        func.instruction(&Instruction::LocalSet(res));
                    }
                    "string_startswith" => {
                        let args = op.args.as_ref().unwrap();
                        let hay = locals[&args[0]];
                        let needle = locals[&args[1]];
                        func.instruction(&Instruction::LocalGet(hay));
                        func.instruction(&Instruction::LocalGet(needle));
                        func.instruction(&Instruction::Call(import_ids["string_startswith"]));
                        let res = locals[op.out.as_ref().unwrap()];
                        func.instruction(&Instruction::LocalSet(res));
                    }
                    "string_endswith" => {
                        let args = op.args.as_ref().unwrap();
                        let hay = locals[&args[0]];
                        let needle = locals[&args[1]];
                        func.instruction(&Instruction::LocalGet(hay));
                        func.instruction(&Instruction::LocalGet(needle));
                        func.instruction(&Instruction::Call(import_ids["string_endswith"]));
                        let res = locals[op.out.as_ref().unwrap()];
                        func.instruction(&Instruction::LocalSet(res));
                    }
                    "string_count" => {
                        let args = op.args.as_ref().unwrap();
                        let hay = locals[&args[0]];
                        let needle = locals[&args[1]];
                        func.instruction(&Instruction::LocalGet(hay));
                        func.instruction(&Instruction::LocalGet(needle));
                        func.instruction(&Instruction::Call(import_ids["string_count"]));
                        let res = locals[op.out.as_ref().unwrap()];
                        func.instruction(&Instruction::LocalSet(res));
                    }
                    "string_count_slice" => {
                        let args = op.args.as_ref().unwrap();
                        let hay = locals[&args[0]];
                        let needle = locals[&args[1]];
                        let start = locals[&args[2]];
                        let end = locals[&args[3]];
                        let has_start = locals[&args[4]];
                        let has_end = locals[&args[5]];
                        func.instruction(&Instruction::LocalGet(hay));
                        func.instruction(&Instruction::LocalGet(needle));
                        func.instruction(&Instruction::LocalGet(start));
                        func.instruction(&Instruction::LocalGet(end));
                        func.instruction(&Instruction::LocalGet(has_start));
                        func.instruction(&Instruction::LocalGet(has_end));
                        func.instruction(&Instruction::Call(import_ids["string_count_slice"]));
                        let res = locals[op.out.as_ref().unwrap()];
                        func.instruction(&Instruction::LocalSet(res));
                    }
                    "env_get" => {
                        let args = op.args.as_ref().unwrap();
                        let key = locals[&args[0]];
                        let default = locals[&args[1]];
                        func.instruction(&Instruction::LocalGet(key));
                        func.instruction(&Instruction::LocalGet(default));
                        func.instruction(&Instruction::Call(import_ids["env_get"]));
                        let res = locals[op.out.as_ref().unwrap()];
                        func.instruction(&Instruction::LocalSet(res));
                    }
                    "string_join" => {
                        let args = op.args.as_ref().unwrap();
                        let sep = locals[&args[0]];
                        let items = locals[&args[1]];
                        func.instruction(&Instruction::LocalGet(sep));
                        func.instruction(&Instruction::LocalGet(items));
                        func.instruction(&Instruction::Call(import_ids["string_join"]));
                        let res = locals[op.out.as_ref().unwrap()];
                        func.instruction(&Instruction::LocalSet(res));
                    }
                    "string_split" => {
                        let args = op.args.as_ref().unwrap();
                        let hay = locals[&args[0]];
                        let needle = locals[&args[1]];
                        func.instruction(&Instruction::LocalGet(hay));
                        func.instruction(&Instruction::LocalGet(needle));
                        func.instruction(&Instruction::Call(import_ids["string_split"]));
                        let res = locals[op.out.as_ref().unwrap()];
                        func.instruction(&Instruction::LocalSet(res));
                    }
                    "bytes_split" => {
                        let args = op.args.as_ref().unwrap();
                        let hay = locals[&args[0]];
                        let needle = locals[&args[1]];
                        func.instruction(&Instruction::LocalGet(hay));
                        func.instruction(&Instruction::LocalGet(needle));
                        func.instruction(&Instruction::Call(import_ids["bytes_split"]));
                        let res = locals[op.out.as_ref().unwrap()];
                        func.instruction(&Instruction::LocalSet(res));
                    }
                    "bytearray_split" => {
                        let args = op.args.as_ref().unwrap();
                        let hay = locals[&args[0]];
                        let needle = locals[&args[1]];
                        func.instruction(&Instruction::LocalGet(hay));
                        func.instruction(&Instruction::LocalGet(needle));
                        func.instruction(&Instruction::Call(import_ids["bytearray_split"]));
                        let res = locals[op.out.as_ref().unwrap()];
                        func.instruction(&Instruction::LocalSet(res));
                    }
                    "bytes_replace" => {
                        let args = op.args.as_ref().unwrap();
                        let hay = locals[&args[0]];
                        let needle = locals[&args[1]];
                        let replacement = locals[&args[2]];
                        func.instruction(&Instruction::LocalGet(hay));
                        func.instruction(&Instruction::LocalGet(needle));
                        func.instruction(&Instruction::LocalGet(replacement));
                        func.instruction(&Instruction::Call(import_ids["bytes_replace"]));
                        let res = locals[op.out.as_ref().unwrap()];
                        func.instruction(&Instruction::LocalSet(res));
                    }
                    "string_replace" => {
                        let args = op.args.as_ref().unwrap();
                        let hay = locals[&args[0]];
                        let needle = locals[&args[1]];
                        let replacement = locals[&args[2]];
                        func.instruction(&Instruction::LocalGet(hay));
                        func.instruction(&Instruction::LocalGet(needle));
                        func.instruction(&Instruction::LocalGet(replacement));
                        func.instruction(&Instruction::Call(import_ids["string_replace"]));
                        let res = locals[op.out.as_ref().unwrap()];
                        func.instruction(&Instruction::LocalSet(res));
                    }
                    "bytearray_replace" => {
                        let args = op.args.as_ref().unwrap();
                        let hay = locals[&args[0]];
                        let needle = locals[&args[1]];
                        let replacement = locals[&args[2]];
                        func.instruction(&Instruction::LocalGet(hay));
                        func.instruction(&Instruction::LocalGet(needle));
                        func.instruction(&Instruction::LocalGet(replacement));
                        func.instruction(&Instruction::Call(import_ids["bytearray_replace"]));
                        let res = locals[op.out.as_ref().unwrap()];
                        func.instruction(&Instruction::LocalSet(res));
                    }
                    "bytes_from_obj" => {
                        let args = op.args.as_ref().unwrap();
                        let src = locals[&args[0]];
                        func.instruction(&Instruction::LocalGet(src));
                        func.instruction(&Instruction::Call(import_ids["bytes_from_obj"]));
                        let res = locals[op.out.as_ref().unwrap()];
                        func.instruction(&Instruction::LocalSet(res));
                    }
                    "bytearray_from_obj" => {
                        let args = op.args.as_ref().unwrap();
                        let src = locals[&args[0]];
                        func.instruction(&Instruction::LocalGet(src));
                        func.instruction(&Instruction::Call(import_ids["bytearray_from_obj"]));
                        let res = locals[op.out.as_ref().unwrap()];
                        func.instruction(&Instruction::LocalSet(res));
                    }
                    "float_from_obj" => {
                        let args = op.args.as_ref().unwrap();
                        let src = locals[&args[0]];
                        func.instruction(&Instruction::LocalGet(src));
                        func.instruction(&Instruction::Call(import_ids["float_from_obj"]));
                        let res = locals[op.out.as_ref().unwrap()];
                        func.instruction(&Instruction::LocalSet(res));
                    }
                    "int_from_obj" => {
                        let args = op.args.as_ref().unwrap();
                        let val = locals[&args[0]];
                        let base = locals[&args[1]];
                        let has_base = locals[&args[2]];
                        func.instruction(&Instruction::LocalGet(val));
                        func.instruction(&Instruction::LocalGet(base));
                        func.instruction(&Instruction::LocalGet(has_base));
                        func.instruction(&Instruction::Call(import_ids["int_from_obj"]));
                        let res = locals[op.out.as_ref().unwrap()];
                        func.instruction(&Instruction::LocalSet(res));
                    }
                    "intarray_from_seq" => {
                        let args = op.args.as_ref().unwrap();
                        let src = locals[&args[0]];
                        func.instruction(&Instruction::LocalGet(src));
                        func.instruction(&Instruction::Call(import_ids["intarray_from_seq"]));
                        let res = locals[op.out.as_ref().unwrap()];
                        func.instruction(&Instruction::LocalSet(res));
                    }
                    "memoryview_new" => {
                        let args = op.args.as_ref().unwrap();
                        let src = locals[&args[0]];
                        func.instruction(&Instruction::LocalGet(src));
                        func.instruction(&Instruction::Call(import_ids["memoryview_new"]));
                        let res = locals[op.out.as_ref().unwrap()];
                        func.instruction(&Instruction::LocalSet(res));
                    }
                    "memoryview_tobytes" => {
                        let args = op.args.as_ref().unwrap();
                        let src = locals[&args[0]];
                        func.instruction(&Instruction::LocalGet(src));
                        func.instruction(&Instruction::Call(import_ids["memoryview_tobytes"]));
                        let res = locals[op.out.as_ref().unwrap()];
                        func.instruction(&Instruction::LocalSet(res));
                    }
                    "buffer2d_new" => {
                        let args = op.args.as_ref().unwrap();
                        let rows = locals[&args[0]];
                        let cols = locals[&args[1]];
                        let init = locals[&args[2]];
                        func.instruction(&Instruction::LocalGet(rows));
                        func.instruction(&Instruction::LocalGet(cols));
                        func.instruction(&Instruction::LocalGet(init));
                        func.instruction(&Instruction::Call(import_ids["buffer2d_new"]));
                        let res = locals[op.out.as_ref().unwrap()];
                        func.instruction(&Instruction::LocalSet(res));
                    }
                    "buffer2d_get" => {
                        let args = op.args.as_ref().unwrap();
                        let buf = locals[&args[0]];
                        let row = locals[&args[1]];
                        let col = locals[&args[2]];
                        func.instruction(&Instruction::LocalGet(buf));
                        func.instruction(&Instruction::LocalGet(row));
                        func.instruction(&Instruction::LocalGet(col));
                        func.instruction(&Instruction::Call(import_ids["buffer2d_get"]));
                        let res = locals[op.out.as_ref().unwrap()];
                        func.instruction(&Instruction::LocalSet(res));
                    }
                    "buffer2d_set" => {
                        let args = op.args.as_ref().unwrap();
                        let buf = locals[&args[0]];
                        let row = locals[&args[1]];
                        let col = locals[&args[2]];
                        let val = locals[&args[3]];
                        func.instruction(&Instruction::LocalGet(buf));
                        func.instruction(&Instruction::LocalGet(row));
                        func.instruction(&Instruction::LocalGet(col));
                        func.instruction(&Instruction::LocalGet(val));
                        func.instruction(&Instruction::Call(import_ids["buffer2d_set"]));
                        let res = locals[op.out.as_ref().unwrap()];
                        func.instruction(&Instruction::LocalSet(res));
                    }
                    "buffer2d_matmul" => {
                        let args = op.args.as_ref().unwrap();
                        let lhs = locals[&args[0]];
                        let rhs = locals[&args[1]];
                        func.instruction(&Instruction::LocalGet(lhs));
                        func.instruction(&Instruction::LocalGet(rhs));
                        func.instruction(&Instruction::Call(import_ids["buffer2d_matmul"]));
                        let res = locals[op.out.as_ref().unwrap()];
                        func.instruction(&Instruction::LocalSet(res));
                    }
                    "str_from_obj" => {
                        let args = op.args.as_ref().unwrap();
                        let src = locals[&args[0]];
                        func.instruction(&Instruction::LocalGet(src));
                        func.instruction(&Instruction::Call(import_ids["str_from_obj"]));
                        let res = locals[op.out.as_ref().unwrap()];
                        func.instruction(&Instruction::LocalSet(res));
                    }
                    "repr_from_obj" => {
                        let args = op.args.as_ref().unwrap();
                        let src = locals[&args[0]];
                        func.instruction(&Instruction::LocalGet(src));
                        func.instruction(&Instruction::Call(import_ids["repr_from_obj"]));
                        let res = locals[op.out.as_ref().unwrap()];
                        func.instruction(&Instruction::LocalSet(res));
                    }
                    "ascii_from_obj" => {
                        let args = op.args.as_ref().unwrap();
                        let src = locals[&args[0]];
                        func.instruction(&Instruction::LocalGet(src));
                        func.instruction(&Instruction::Call(import_ids["ascii_from_obj"]));
                        let res = locals[op.out.as_ref().unwrap()];
                        func.instruction(&Instruction::LocalSet(res));
                    }
                    "dataclass_new" => {
                        let args = op.args.as_ref().unwrap();
                        let name = locals[&args[0]];
                        let fields = locals[&args[1]];
                        let values = locals[&args[2]];
                        let flags = locals[&args[3]];
                        func.instruction(&Instruction::LocalGet(name));
                        func.instruction(&Instruction::LocalGet(fields));
                        func.instruction(&Instruction::LocalGet(values));
                        func.instruction(&Instruction::LocalGet(flags));
                        func.instruction(&Instruction::Call(import_ids["dataclass_new"]));
                        let res = locals[op.out.as_ref().unwrap()];
                        func.instruction(&Instruction::LocalSet(res));
                    }
                    "dataclass_get" => {
                        let args = op.args.as_ref().unwrap();
                        let obj = locals[&args[0]];
                        let idx = locals[&args[1]];
                        func.instruction(&Instruction::LocalGet(obj));
                        func.instruction(&Instruction::LocalGet(idx));
                        func.instruction(&Instruction::Call(import_ids["dataclass_get"]));
                        let res = locals[op.out.as_ref().unwrap()];
                        func.instruction(&Instruction::LocalSet(res));
                    }
                    "dataclass_set" => {
                        let args = op.args.as_ref().unwrap();
                        let obj = locals[&args[0]];
                        let idx = locals[&args[1]];
                        let val = locals[&args[2]];
                        func.instruction(&Instruction::LocalGet(obj));
                        func.instruction(&Instruction::LocalGet(idx));
                        func.instruction(&Instruction::LocalGet(val));
                        func.instruction(&Instruction::Call(import_ids["dataclass_set"]));
                        let res = locals[op.out.as_ref().unwrap()];
                        func.instruction(&Instruction::LocalSet(res));
                    }
                    "dataclass_set_class" => {
                        let args = op.args.as_ref().unwrap();
                        let obj = locals[&args[0]];
                        let class_obj = locals[&args[1]];
                        func.instruction(&Instruction::LocalGet(obj));
                        func.instruction(&Instruction::LocalGet(class_obj));
                        func.instruction(&Instruction::Call(import_ids["dataclass_set_class"]));
                        let res = locals[op.out.as_ref().unwrap()];
                        func.instruction(&Instruction::LocalSet(res));
                    }
                    "class_new" => {
                        let args = op.args.as_ref().unwrap();
                        let name = locals[&args[0]];
                        func.instruction(&Instruction::LocalGet(name));
                        func.instruction(&Instruction::Call(import_ids["class_new"]));
                        let res = locals[op.out.as_ref().unwrap()];
                        func.instruction(&Instruction::LocalSet(res));
                    }
                    "class_set_base" => {
                        let args = op.args.as_ref().unwrap();
                        let class_bits = locals[&args[0]];
                        let base_bits = locals[&args[1]];
                        func.instruction(&Instruction::LocalGet(class_bits));
                        func.instruction(&Instruction::LocalGet(base_bits));
                        func.instruction(&Instruction::Call(import_ids["class_set_base"]));
                        let res = locals[op.out.as_ref().unwrap()];
                        func.instruction(&Instruction::LocalSet(res));
                    }
                    "class_apply_set_name" => {
                        let args = op.args.as_ref().unwrap();
                        let class_bits = locals[&args[0]];
                        func.instruction(&Instruction::LocalGet(class_bits));
                        func.instruction(&Instruction::Call(import_ids["class_apply_set_name"]));
                        let res = locals[op.out.as_ref().unwrap()];
                        func.instruction(&Instruction::LocalSet(res));
                    }
                    "super_new" => {
                        let args = op.args.as_ref().unwrap();
                        let type_bits = locals[&args[0]];
                        let obj_bits = locals[&args[1]];
                        func.instruction(&Instruction::LocalGet(type_bits));
                        func.instruction(&Instruction::LocalGet(obj_bits));
                        func.instruction(&Instruction::Call(import_ids["super_new"]));
                        let res = locals[op.out.as_ref().unwrap()];
                        func.instruction(&Instruction::LocalSet(res));
                    }
                    "builtin_type" => {
                        let args = op.args.as_ref().unwrap();
                        let tag = locals[&args[0]];
                        func.instruction(&Instruction::LocalGet(tag));
                        func.instruction(&Instruction::Call(import_ids["builtin_type"]));
                        let res = locals[op.out.as_ref().unwrap()];
                        func.instruction(&Instruction::LocalSet(res));
                    }
                    "type_of" => {
                        let args = op.args.as_ref().unwrap();
                        let obj = locals[&args[0]];
                        func.instruction(&Instruction::LocalGet(obj));
                        func.instruction(&Instruction::Call(import_ids["type_of"]));
                        let res = locals[op.out.as_ref().unwrap()];
                        func.instruction(&Instruction::LocalSet(res));
                    }
                    "isinstance" => {
                        let args = op.args.as_ref().unwrap();
                        let obj = locals[&args[0]];
                        let cls = locals[&args[1]];
                        func.instruction(&Instruction::LocalGet(obj));
                        func.instruction(&Instruction::LocalGet(cls));
                        func.instruction(&Instruction::Call(import_ids["isinstance"]));
                        let res = locals[op.out.as_ref().unwrap()];
                        func.instruction(&Instruction::LocalSet(res));
                    }
                    "issubclass" => {
                        let args = op.args.as_ref().unwrap();
                        let sub = locals[&args[0]];
                        let cls = locals[&args[1]];
                        func.instruction(&Instruction::LocalGet(sub));
                        func.instruction(&Instruction::LocalGet(cls));
                        func.instruction(&Instruction::Call(import_ids["issubclass"]));
                        let res = locals[op.out.as_ref().unwrap()];
                        func.instruction(&Instruction::LocalSet(res));
                    }
                    "object_new" => {
                        func.instruction(&Instruction::Call(import_ids["object_new"]));
                        let res = locals[op.out.as_ref().unwrap()];
                        func.instruction(&Instruction::LocalSet(res));
                    }
                    "classmethod_new" => {
                        let args = op.args.as_ref().unwrap();
                        let func_bits = locals[&args[0]];
                        func.instruction(&Instruction::LocalGet(func_bits));
                        func.instruction(&Instruction::Call(import_ids["classmethod_new"]));
                        let res = locals[op.out.as_ref().unwrap()];
                        func.instruction(&Instruction::LocalSet(res));
                    }
                    "staticmethod_new" => {
                        let args = op.args.as_ref().unwrap();
                        let func_bits = locals[&args[0]];
                        func.instruction(&Instruction::LocalGet(func_bits));
                        func.instruction(&Instruction::Call(import_ids["staticmethod_new"]));
                        let res = locals[op.out.as_ref().unwrap()];
                        func.instruction(&Instruction::LocalSet(res));
                    }
                    "property_new" => {
                        let args = op.args.as_ref().unwrap();
                        let getter = locals[&args[0]];
                        let setter = locals[&args[1]];
                        let deleter = locals[&args[2]];
                        func.instruction(&Instruction::LocalGet(getter));
                        func.instruction(&Instruction::LocalGet(setter));
                        func.instruction(&Instruction::LocalGet(deleter));
                        func.instruction(&Instruction::Call(import_ids["property_new"]));
                        let res = locals[op.out.as_ref().unwrap()];
                        func.instruction(&Instruction::LocalSet(res));
                    }
                    "object_set_class" => {
                        let args = op.args.as_ref().unwrap();
                        let obj = locals[&args[0]];
                        let class_obj = locals[&args[1]];
                        func.instruction(&Instruction::LocalGet(obj));
                        func.instruction(&Instruction::Call(import_ids["handle_resolve"]));
                        func.instruction(&Instruction::LocalGet(class_obj));
                        func.instruction(&Instruction::Call(import_ids["object_set_class"]));
                        let res = locals[op.out.as_ref().unwrap()];
                        func.instruction(&Instruction::LocalSet(res));
                    }
                    "get_attr_generic_ptr" => {
                        let args = op.args.as_ref().unwrap();
                        let obj = locals[&args[0]];
                        let attr = op.s_value.as_ref().unwrap();
                        let bytes = attr.as_bytes();
                        let offset = self.data_offset;
                        self.data.active(
                            0,
                            &ConstExpr::i32_const(offset as i32),
                            bytes.iter().copied(),
                        );
                        self.data_offset = (self.data_offset + bytes.len() as u32 + 7) & !7;
                        func.instruction(&Instruction::LocalGet(obj));
                        func.instruction(&Instruction::Call(import_ids["handle_resolve"]));
                        func.instruction(&Instruction::I64Const(offset as i64));
                        func.instruction(&Instruction::I64Const(bytes.len() as i64));
                        func.instruction(&Instruction::Call(import_ids["get_attr_generic"]));
                        let res = locals[op.out.as_ref().unwrap()];
                        func.instruction(&Instruction::LocalSet(res));
                    }
                    "get_attr_generic_obj" => {
                        let args = op.args.as_ref().unwrap();
                        let obj = locals[&args[0]];
                        let attr = op.s_value.as_ref().unwrap();
                        let bytes = attr.as_bytes();
                        let offset = self.data_offset;
                        self.data.active(
                            0,
                            &ConstExpr::i32_const(offset as i32),
                            bytes.iter().copied(),
                        );
                        self.data_offset = (self.data_offset + bytes.len() as u32 + 7) & !7;
                        func.instruction(&Instruction::LocalGet(obj));
                        func.instruction(&Instruction::I64Const(offset as i64));
                        func.instruction(&Instruction::I64Const(bytes.len() as i64));
                        func.instruction(&Instruction::Call(import_ids["get_attr_object"]));
                        let res = locals[op.out.as_ref().unwrap()];
                        func.instruction(&Instruction::LocalSet(res));
                    }
                    "set_attr_generic_ptr" => {
                        let args = op.args.as_ref().unwrap();
                        let obj = locals[&args[0]];
                        let val = locals[&args[1]];
                        let attr = op.s_value.as_ref().unwrap();
                        let bytes = attr.as_bytes();
                        let offset = self.data_offset;
                        self.data.active(
                            0,
                            &ConstExpr::i32_const(offset as i32),
                            bytes.iter().copied(),
                        );
                        self.data_offset = (self.data_offset + bytes.len() as u32 + 7) & !7;
                        func.instruction(&Instruction::LocalGet(obj));
                        func.instruction(&Instruction::Call(import_ids["handle_resolve"]));
                        func.instruction(&Instruction::I64Const(offset as i64));
                        func.instruction(&Instruction::I64Const(bytes.len() as i64));
                        func.instruction(&Instruction::LocalGet(val));
                        func.instruction(&Instruction::Call(import_ids["set_attr_generic"]));
                        let res = locals[op.out.as_ref().unwrap()];
                        func.instruction(&Instruction::LocalSet(res));
                    }
                    "set_attr_generic_obj" => {
                        let args = op.args.as_ref().unwrap();
                        let obj = locals[&args[0]];
                        let val = locals[&args[1]];
                        let attr = op.s_value.as_ref().unwrap();
                        let bytes = attr.as_bytes();
                        let offset = self.data_offset;
                        self.data.active(
                            0,
                            &ConstExpr::i32_const(offset as i32),
                            bytes.iter().copied(),
                        );
                        self.data_offset = (self.data_offset + bytes.len() as u32 + 7) & !7;
                        func.instruction(&Instruction::LocalGet(obj));
                        func.instruction(&Instruction::I64Const(offset as i64));
                        func.instruction(&Instruction::I64Const(bytes.len() as i64));
                        func.instruction(&Instruction::LocalGet(val));
                        func.instruction(&Instruction::Call(import_ids["set_attr_object"]));
                        let res = locals[op.out.as_ref().unwrap()];
                        func.instruction(&Instruction::LocalSet(res));
                    }
                    "del_attr_generic_ptr" => {
                        let args = op.args.as_ref().unwrap();
                        let obj = locals[&args[0]];
                        let attr = op.s_value.as_ref().unwrap();
                        let bytes = attr.as_bytes();
                        let offset = self.data_offset;
                        self.data.active(
                            0,
                            &ConstExpr::i32_const(offset as i32),
                            bytes.iter().copied(),
                        );
                        self.data_offset = (self.data_offset + bytes.len() as u32 + 7) & !7;
                        func.instruction(&Instruction::LocalGet(obj));
                        func.instruction(&Instruction::Call(import_ids["handle_resolve"]));
                        func.instruction(&Instruction::I64Const(offset as i64));
                        func.instruction(&Instruction::I64Const(bytes.len() as i64));
                        func.instruction(&Instruction::Call(import_ids["del_attr_generic"]));
                        let res = locals[op.out.as_ref().unwrap()];
                        func.instruction(&Instruction::LocalSet(res));
                    }
                    "del_attr_generic_obj" => {
                        let args = op.args.as_ref().unwrap();
                        let obj = locals[&args[0]];
                        let attr = op.s_value.as_ref().unwrap();
                        let bytes = attr.as_bytes();
                        let offset = self.data_offset;
                        self.data.active(
                            0,
                            &ConstExpr::i32_const(offset as i32),
                            bytes.iter().copied(),
                        );
                        self.data_offset = (self.data_offset + bytes.len() as u32 + 7) & !7;
                        func.instruction(&Instruction::LocalGet(obj));
                        func.instruction(&Instruction::I64Const(offset as i64));
                        func.instruction(&Instruction::I64Const(bytes.len() as i64));
                        func.instruction(&Instruction::Call(import_ids["del_attr_object"]));
                        let res = locals[op.out.as_ref().unwrap()];
                        func.instruction(&Instruction::LocalSet(res));
                    }
                    "get_attr_name" => {
                        let args = op.args.as_ref().unwrap();
                        let obj = locals[&args[0]];
                        let name = locals[&args[1]];
                        func.instruction(&Instruction::LocalGet(obj));
                        func.instruction(&Instruction::LocalGet(name));
                        func.instruction(&Instruction::Call(import_ids["get_attr_name"]));
                        let res = locals[op.out.as_ref().unwrap()];
                        func.instruction(&Instruction::LocalSet(res));
                    }
                    "get_attr_name_default" => {
                        let args = op.args.as_ref().unwrap();
                        let obj = locals[&args[0]];
                        let name = locals[&args[1]];
                        let default_val = locals[&args[2]];
                        func.instruction(&Instruction::LocalGet(obj));
                        func.instruction(&Instruction::LocalGet(name));
                        func.instruction(&Instruction::LocalGet(default_val));
                        func.instruction(&Instruction::Call(import_ids["get_attr_name_default"]));
                        let res = locals[op.out.as_ref().unwrap()];
                        func.instruction(&Instruction::LocalSet(res));
                    }
                    "has_attr_name" => {
                        let args = op.args.as_ref().unwrap();
                        let obj = locals[&args[0]];
                        let name = locals[&args[1]];
                        func.instruction(&Instruction::LocalGet(obj));
                        func.instruction(&Instruction::LocalGet(name));
                        func.instruction(&Instruction::Call(import_ids["has_attr_name"]));
                        let res = locals[op.out.as_ref().unwrap()];
                        func.instruction(&Instruction::LocalSet(res));
                    }
                    "set_attr_name" => {
                        let args = op.args.as_ref().unwrap();
                        let obj = locals[&args[0]];
                        let name = locals[&args[1]];
                        let val = locals[&args[2]];
                        func.instruction(&Instruction::LocalGet(obj));
                        func.instruction(&Instruction::LocalGet(name));
                        func.instruction(&Instruction::LocalGet(val));
                        func.instruction(&Instruction::Call(import_ids["set_attr_name"]));
                        let res = locals[op.out.as_ref().unwrap()];
                        func.instruction(&Instruction::LocalSet(res));
                    }
                    "del_attr_name" => {
                        let args = op.args.as_ref().unwrap();
                        let obj = locals[&args[0]];
                        let name = locals[&args[1]];
                        func.instruction(&Instruction::LocalGet(obj));
                        func.instruction(&Instruction::LocalGet(name));
                        func.instruction(&Instruction::Call(import_ids["del_attr_name"]));
                        let res = locals[op.out.as_ref().unwrap()];
                        func.instruction(&Instruction::LocalSet(res));
                    }
                    "store" => {
                        let args = op.args.as_ref().unwrap();
                        func.instruction(&Instruction::LocalGet(locals[&args[0]]));
                        func.instruction(&Instruction::Call(import_ids["handle_resolve"]));
                        func.instruction(&Instruction::I64Const(op.value.unwrap()));
                        func.instruction(&Instruction::LocalGet(locals[&args[1]]));
                        func.instruction(&Instruction::Call(import_ids["object_field_set"]));
                        if let Some(out) = op.out.as_ref() {
                            if out != "none" {
                                func.instruction(&Instruction::LocalSet(locals[out]));
                            } else {
                                func.instruction(&Instruction::Drop);
                            }
                        } else {
                            func.instruction(&Instruction::Drop);
                        }
                    }
                    "store_init" => {
                        let args = op.args.as_ref().unwrap();
                        func.instruction(&Instruction::LocalGet(locals[&args[0]]));
                        func.instruction(&Instruction::Call(import_ids["handle_resolve"]));
                        func.instruction(&Instruction::I64Const(op.value.unwrap()));
                        func.instruction(&Instruction::LocalGet(locals[&args[1]]));
                        func.instruction(&Instruction::Call(import_ids["object_field_init"]));
                        if let Some(out) = op.out.as_ref() {
                            if out != "none" {
                                func.instruction(&Instruction::LocalSet(locals[out]));
                            } else {
                                func.instruction(&Instruction::Drop);
                            }
                        } else {
                            func.instruction(&Instruction::Drop);
                        }
                    }
                    "load" => {
                        let args = op.args.as_ref().unwrap();
                        func.instruction(&Instruction::LocalGet(locals[&args[0]]));
                        func.instruction(&Instruction::Call(import_ids["handle_resolve"]));
                        func.instruction(&Instruction::I64Const(op.value.unwrap()));
                        func.instruction(&Instruction::Call(import_ids["object_field_get"]));
                        func.instruction(&Instruction::LocalSet(locals[op.out.as_ref().unwrap()]));
                    }
                    "closure_load" => {
                        let args = op.args.as_ref().unwrap();
                        func.instruction(&Instruction::LocalGet(locals[&args[0]]));
                        func.instruction(&Instruction::Call(import_ids["handle_resolve"]));
                        func.instruction(&Instruction::I64Const(op.value.unwrap()));
                        func.instruction(&Instruction::Call(import_ids["closure_load"]));
                        func.instruction(&Instruction::LocalSet(locals[op.out.as_ref().unwrap()]));
                    }
                    "closure_store" => {
                        let args = op.args.as_ref().unwrap();
                        func.instruction(&Instruction::LocalGet(locals[&args[0]]));
                        func.instruction(&Instruction::Call(import_ids["handle_resolve"]));
                        func.instruction(&Instruction::I64Const(op.value.unwrap()));
                        func.instruction(&Instruction::LocalGet(locals[&args[1]]));
                        func.instruction(&Instruction::Call(import_ids["closure_store"]));
                        if let Some(out) = op.out.as_ref() {
                            func.instruction(&Instruction::LocalSet(locals[out]));
                        } else {
                            func.instruction(&Instruction::Drop);
                        }
                    }
                    "guarded_load" => {
                        let args = op.args.as_ref().unwrap();
                        func.instruction(&Instruction::LocalGet(locals[&args[0]]));
                        func.instruction(&Instruction::Call(import_ids["handle_resolve"]));
                        func.instruction(&Instruction::I64Const(op.value.unwrap()));
                        func.instruction(&Instruction::Call(import_ids["object_field_get"]));
                        func.instruction(&Instruction::LocalSet(locals[op.out.as_ref().unwrap()]));
                    }
                    "state_switch" => {}
                    "state_transition" => {
                        let args = op.args.as_ref().unwrap();
                        let future = locals[&args[0]];
                        let slot_bits = args.get(1).map(|name| locals[name]);
                        func.instruction(&Instruction::LocalGet(0));
                        func.instruction(&Instruction::I32WrapI64);
                        func.instruction(&Instruction::I32Const(-16));
                        func.instruction(&Instruction::I32Add);
                        func.instruction(&Instruction::I64Const(op.value.unwrap()));
                        func.instruction(&Instruction::I64Store(wasm_encoder::MemArg {
                            align: 3,
                            offset: 0,
                            memory_index: 0,
                        }));
                        func.instruction(&Instruction::LocalGet(future));
                        func.instruction(&Instruction::LocalGet(future));
                        func.instruction(&Instruction::I32WrapI64);
                        func.instruction(&Instruction::I32Const(-24));
                        func.instruction(&Instruction::I32Add);
                        func.instruction(&Instruction::I32Load(wasm_encoder::MemArg {
                            align: 2,
                            offset: 0,
                            memory_index: 0,
                        }));
                        func.instruction(&Instruction::CallIndirect { ty: 2, table: 0 });
                        let out = locals[op.out.as_ref().unwrap()];
                        func.instruction(&Instruction::LocalSet(out));
                        if let Some(slot) = slot_bits {
                            func.instruction(&Instruction::LocalGet(0));
                            func.instruction(&Instruction::LocalGet(slot));
                            func.instruction(&Instruction::I64Const(INT_MASK as i64));
                            func.instruction(&Instruction::I64And);
                            func.instruction(&Instruction::LocalGet(out));
                            func.instruction(&Instruction::Call(import_ids["closure_store"]));
                            func.instruction(&Instruction::Drop);
                        }
                        func.instruction(&Instruction::LocalGet(out));
                        func.instruction(&Instruction::I64Const(box_pending()));
                        func.instruction(&Instruction::I64Eq);
                        func.instruction(&Instruction::If(BlockType::Empty));
                        func.instruction(&Instruction::I64Const(box_pending()));
                        func.instruction(&Instruction::Return);
                        func.instruction(&Instruction::End);
                    }
                    "call_async" => {
                        func.instruction(&Instruction::I64Const(0));
                        func.instruction(&Instruction::Call(import_ids["alloc"]));
                        let res = locals[op.out.as_ref().unwrap()];
                        func.instruction(&Instruction::LocalSet(res));
                        func.instruction(&Instruction::LocalGet(res));
                        func.instruction(&Instruction::I32WrapI64);
                        func.instruction(&Instruction::I32Const(-24));
                        func.instruction(&Instruction::I32Add);
                        let table_idx = func_map[op.s_value.as_ref().unwrap()];
                        func.instruction(&Instruction::I32Const(table_idx as i32));
                        func.instruction(&Instruction::I32Store(wasm_encoder::MemArg {
                            align: 2,
                            offset: 0,
                            memory_index: 0,
                        }));
                        func.instruction(&Instruction::LocalGet(res));
                        func.instruction(&Instruction::I32WrapI64);
                        func.instruction(&Instruction::I32Const(-16));
                        func.instruction(&Instruction::I32Add);
                        func.instruction(&Instruction::I64Const(0));
                        func.instruction(&Instruction::I64Store(wasm_encoder::MemArg {
                            align: 3,
                            offset: 0,
                            memory_index: 0,
                        }));
                    }
                    "call" => {
                        let target_name = op.s_value.as_ref().unwrap();
                        let args_names = op.args.as_ref().unwrap();
                        let out = locals[op.out.as_ref().unwrap()];
                        if args_names.is_empty() {
                            let func_idx = *func_indices
                                .get(target_name)
                                .expect("call target not found");
                            func.instruction(&Instruction::Call(func_idx));
                            func.instruction(&Instruction::LocalSet(out));
                        } else {
                            let func_idx = *func_indices
                                .get(target_name)
                                .expect("call target not found");
                            for arg_name in args_names {
                                let arg = locals[arg_name];
                                func.instruction(&Instruction::LocalGet(arg));
                            }
                            func.instruction(&Instruction::Call(func_idx));
                            func.instruction(&Instruction::LocalSet(out));
                        }
                    }
                    "func_new" => {
                        let func_name = op.s_value.as_ref().unwrap();
                        let arity = op.value.unwrap_or(0);
                        let table_idx = func_map[func_name] as i64;
                        func.instruction(&Instruction::I64Const(table_idx));
                        func.instruction(&Instruction::I64Const(arity));
                        func.instruction(&Instruction::Call(import_ids["func_new"]));
                        let res = locals[op.out.as_ref().unwrap()];
                        func.instruction(&Instruction::LocalSet(res));
                    }
                    "bound_method_new" => {
                        let args = op.args.as_ref().unwrap();
                        let func_bits = locals[&args[0]];
                        let self_bits = locals[&args[1]];
                        func.instruction(&Instruction::LocalGet(func_bits));
                        func.instruction(&Instruction::LocalGet(self_bits));
                        func.instruction(&Instruction::Call(import_ids["bound_method_new"]));
                        let res = locals[op.out.as_ref().unwrap()];
                        func.instruction(&Instruction::LocalSet(res));
                    }
                    "call_func" => {
                        let args_names = op.args.as_ref().unwrap();
                        let func_bits = locals[&args_names[0]];
                        let out = locals[op.out.as_ref().unwrap()];
                        let arity = args_names.len().saturating_sub(1);
                        let call_type = *user_type_map.get(&arity).expect("call_func type missing");
                        for arg_name in &args_names[1..] {
                            let arg = locals[arg_name];
                            func.instruction(&Instruction::LocalGet(arg));
                        }
                        func.instruction(&Instruction::LocalGet(func_bits));
                        func.instruction(&Instruction::Call(import_ids["handle_resolve"]));
                        func.instruction(&Instruction::I32WrapI64);
                        func.instruction(&Instruction::I64Load(wasm_encoder::MemArg {
                            align: 3,
                            offset: 0,
                            memory_index: 0,
                        }));
                        func.instruction(&Instruction::I32WrapI64);
                        func.instruction(&Instruction::CallIndirect {
                            ty: call_type,
                            table: 0,
                        });
                        func.instruction(&Instruction::LocalSet(out));
                    }
                    "call_method" => {
                        let args_names = op.args.as_ref().unwrap();
                        let method_bits = locals[&args_names[0]];
                        let out = locals[op.out.as_ref().unwrap()];
                        let extra_arity = args_names.len().saturating_sub(1);
                        let call_type = *user_type_map
                            .get(&(extra_arity + 1))
                            .expect("call_method type missing");

                        func.instruction(&Instruction::LocalGet(method_bits));
                        func.instruction(&Instruction::Call(import_ids["handle_resolve"]));
                        func.instruction(&Instruction::I32WrapI64);
                        func.instruction(&Instruction::I32Const(8));
                        func.instruction(&Instruction::I32Add);
                        func.instruction(&Instruction::I64Load(wasm_encoder::MemArg {
                            align: 3,
                            offset: 0,
                            memory_index: 0,
                        }));
                        for arg_name in &args_names[1..] {
                            let arg = locals[arg_name];
                            func.instruction(&Instruction::LocalGet(arg));
                        }
                        func.instruction(&Instruction::LocalGet(method_bits));
                        func.instruction(&Instruction::Call(import_ids["handle_resolve"]));
                        func.instruction(&Instruction::I32WrapI64);
                        func.instruction(&Instruction::I64Load(wasm_encoder::MemArg {
                            align: 3,
                            offset: 0,
                            memory_index: 0,
                        }));
                        func.instruction(&Instruction::Call(import_ids["handle_resolve"]));
                        func.instruction(&Instruction::I32WrapI64);
                        func.instruction(&Instruction::I64Load(wasm_encoder::MemArg {
                            align: 3,
                            offset: 0,
                            memory_index: 0,
                        }));
                        func.instruction(&Instruction::I32WrapI64);
                        func.instruction(&Instruction::CallIndirect {
                            ty: call_type,
                            table: 0,
                        });
                        func.instruction(&Instruction::LocalSet(out));
                    }
                    "chan_new" => {
                        let args = op.args.as_ref().unwrap();
                        let cap = locals[&args[0]];
                        func.instruction(&Instruction::LocalGet(cap));
                        func.instruction(&Instruction::Call(import_ids["chan_new"]));
                        func.instruction(&Instruction::LocalSet(locals[op.out.as_ref().unwrap()]));
                    }
                    "module_new" => {
                        let args = op.args.as_ref().unwrap();
                        let name = locals[&args[0]];
                        func.instruction(&Instruction::LocalGet(name));
                        func.instruction(&Instruction::Call(import_ids["module_new"]));
                        let res = locals[op.out.as_ref().unwrap()];
                        func.instruction(&Instruction::LocalSet(res));
                    }
                    "module_cache_get" => {
                        let args = op.args.as_ref().unwrap();
                        let name = locals[&args[0]];
                        func.instruction(&Instruction::LocalGet(name));
                        func.instruction(&Instruction::Call(import_ids["module_cache_get"]));
                        let res = locals[op.out.as_ref().unwrap()];
                        func.instruction(&Instruction::LocalSet(res));
                    }
                    "module_cache_set" => {
                        let args = op.args.as_ref().unwrap();
                        let name = locals[&args[0]];
                        let module = locals[&args[1]];
                        func.instruction(&Instruction::LocalGet(name));
                        func.instruction(&Instruction::LocalGet(module));
                        func.instruction(&Instruction::Call(import_ids["module_cache_set"]));
                        let res = locals[op.out.as_ref().unwrap()];
                        func.instruction(&Instruction::LocalSet(res));
                    }
                    "module_get_attr" => {
                        let args = op.args.as_ref().unwrap();
                        let module = locals[&args[0]];
                        let name = locals[&args[1]];
                        func.instruction(&Instruction::LocalGet(module));
                        func.instruction(&Instruction::LocalGet(name));
                        func.instruction(&Instruction::Call(import_ids["module_get_attr"]));
                        let res = locals[op.out.as_ref().unwrap()];
                        func.instruction(&Instruction::LocalSet(res));
                    }
                    "module_set_attr" => {
                        let args = op.args.as_ref().unwrap();
                        let module = locals[&args[0]];
                        let name = locals[&args[1]];
                        let val = locals[&args[2]];
                        func.instruction(&Instruction::LocalGet(module));
                        func.instruction(&Instruction::LocalGet(name));
                        func.instruction(&Instruction::LocalGet(val));
                        func.instruction(&Instruction::Call(import_ids["module_set_attr"]));
                        let res = locals[op.out.as_ref().unwrap()];
                        func.instruction(&Instruction::LocalSet(res));
                    }
                    "alloc_future" => {
                        let total = op.value.unwrap_or(0) + 24;
                        func.instruction(&Instruction::I64Const(total));
                        func.instruction(&Instruction::Call(import_ids["alloc"]));
                        func.instruction(&Instruction::I64Const(24));
                        func.instruction(&Instruction::I64Add);
                        let res = locals[op.out.as_ref().unwrap()];
                        func.instruction(&Instruction::LocalSet(res));
                        func.instruction(&Instruction::LocalGet(res));
                        func.instruction(&Instruction::I32WrapI64);
                        func.instruction(&Instruction::I32Const(-24));
                        func.instruction(&Instruction::I32Add);
                        let table_idx = func_map[op.s_value.as_ref().unwrap()];
                        func.instruction(&Instruction::I32Const(table_idx as i32));
                        func.instruction(&Instruction::I32Store(wasm_encoder::MemArg {
                            align: 2,
                            offset: 0,
                            memory_index: 0,
                        }));
                        func.instruction(&Instruction::LocalGet(res));
                        func.instruction(&Instruction::I32WrapI64);
                        func.instruction(&Instruction::I32Const(-16));
                        func.instruction(&Instruction::I32Add);
                        func.instruction(&Instruction::I64Const(0));
                        func.instruction(&Instruction::I64Store(wasm_encoder::MemArg {
                            align: 3,
                            offset: 0,
                            memory_index: 0,
                        }));
                        if let Some(args) = op.args.as_ref() {
                            for (i, name) in args.iter().enumerate() {
                                let arg_local = locals[name];
                                func.instruction(&Instruction::LocalGet(res));
                                func.instruction(&Instruction::I32WrapI64);
                                func.instruction(&Instruction::I32Const((i as i32) * 8));
                                func.instruction(&Instruction::I32Add);
                                func.instruction(&Instruction::LocalGet(arg_local));
                                func.instruction(&Instruction::I64Store(wasm_encoder::MemArg {
                                    align: 3,
                                    offset: 0,
                                    memory_index: 0,
                                }));
                            }
                        }
                    }
                    "alloc_generator" => {
                        let total = op.value.unwrap_or(0) + 24;
                        func.instruction(&Instruction::I64Const(total));
                        func.instruction(&Instruction::Call(import_ids["alloc"]));
                        func.instruction(&Instruction::I64Const(24));
                        func.instruction(&Instruction::I64Add);
                        let res = locals[op.out.as_ref().unwrap()];
                        func.instruction(&Instruction::LocalSet(res));
                        func.instruction(&Instruction::LocalGet(res));
                        func.instruction(&Instruction::I32WrapI64);
                        func.instruction(&Instruction::I32Const(-24));
                        func.instruction(&Instruction::I32Add);
                        let table_idx = func_map[op.s_value.as_ref().unwrap()];
                        func.instruction(&Instruction::I32Const(table_idx as i32));
                        func.instruction(&Instruction::I32Store(wasm_encoder::MemArg {
                            align: 2,
                            offset: 0,
                            memory_index: 0,
                        }));
                        func.instruction(&Instruction::LocalGet(res));
                        func.instruction(&Instruction::I32WrapI64);
                        func.instruction(&Instruction::I32Const(-16));
                        func.instruction(&Instruction::I32Add);
                        func.instruction(&Instruction::I64Const(0));
                        func.instruction(&Instruction::I64Store(wasm_encoder::MemArg {
                            align: 3,
                            offset: 0,
                            memory_index: 0,
                        }));
                        func.instruction(&Instruction::LocalGet(res));
                        func.instruction(&Instruction::I32WrapI64);
                        func.instruction(&Instruction::I32Const(0));
                        func.instruction(&Instruction::I32Add);
                        func.instruction(&Instruction::I64Const(box_none()));
                        func.instruction(&Instruction::I64Store(wasm_encoder::MemArg {
                            align: 3,
                            offset: 0,
                            memory_index: 0,
                        }));
                        func.instruction(&Instruction::LocalGet(res));
                        func.instruction(&Instruction::I32WrapI64);
                        func.instruction(&Instruction::I32Const(8));
                        func.instruction(&Instruction::I32Add);
                        func.instruction(&Instruction::I64Const(box_none()));
                        func.instruction(&Instruction::I64Store(wasm_encoder::MemArg {
                            align: 3,
                            offset: 0,
                            memory_index: 0,
                        }));
                        func.instruction(&Instruction::LocalGet(res));
                        func.instruction(&Instruction::I32WrapI64);
                        func.instruction(&Instruction::I32Const(16));
                        func.instruction(&Instruction::I32Add);
                        func.instruction(&Instruction::I64Const(box_bool(0)));
                        func.instruction(&Instruction::I64Store(wasm_encoder::MemArg {
                            align: 3,
                            offset: 0,
                            memory_index: 0,
                        }));
                        func.instruction(&Instruction::LocalGet(res));
                        func.instruction(&Instruction::I32WrapI64);
                        func.instruction(&Instruction::I32Const(24));
                        func.instruction(&Instruction::I32Add);
                        func.instruction(&Instruction::I64Const(box_int(1)));
                        func.instruction(&Instruction::I64Store(wasm_encoder::MemArg {
                            align: 3,
                            offset: 0,
                            memory_index: 0,
                        }));
                        if let Some(args) = op.args.as_ref() {
                            for (i, name) in args.iter().enumerate() {
                                let arg_local = locals[name];
                                func.instruction(&Instruction::LocalGet(res));
                                func.instruction(&Instruction::I32WrapI64);
                                func.instruction(&Instruction::I32Const(32 + (i as i32) * 8));
                                func.instruction(&Instruction::I32Add);
                                func.instruction(&Instruction::LocalGet(arg_local));
                                func.instruction(&Instruction::I64Store(wasm_encoder::MemArg {
                                    align: 3,
                                    offset: 0,
                                    memory_index: 0,
                                }));
                            }
                        }
                        func.instruction(&Instruction::LocalGet(res));
                        func.instruction(&Instruction::I64Const((QNAN | TAG_PTR) as i64));
                        func.instruction(&Instruction::I64Or);
                        func.instruction(&Instruction::LocalSet(res));
                    }
                    "state_yield" => {
                        let args = op.args.as_ref().unwrap();
                        func.instruction(&Instruction::LocalGet(0));
                        func.instruction(&Instruction::I32WrapI64);
                        func.instruction(&Instruction::I32Const(-16));
                        func.instruction(&Instruction::I32Add);
                        func.instruction(&Instruction::I64Const(op.value.unwrap()));
                        func.instruction(&Instruction::I64Store(wasm_encoder::MemArg {
                            align: 3,
                            offset: 0,
                            memory_index: 0,
                        }));
                        let pair = locals[&args[0]];
                        if let Some(out) = op.out.as_ref() {
                            func.instruction(&Instruction::LocalGet(pair));
                            func.instruction(&Instruction::LocalSet(locals[out]));
                            func.instruction(&Instruction::LocalGet(locals[out]));
                        } else {
                            func.instruction(&Instruction::LocalGet(pair));
                        }
                        func.instruction(&Instruction::Return);
                    }
                    "context_null" => {
                        let args = op.args.as_ref().unwrap();
                        let payload = locals[&args[0]];
                        func.instruction(&Instruction::LocalGet(payload));
                        func.instruction(&Instruction::Call(import_ids["context_null"]));
                        func.instruction(&Instruction::LocalSet(locals[op.out.as_ref().unwrap()]));
                    }
                    "context_enter" => {
                        let args = op.args.as_ref().unwrap();
                        let ctx = locals[&args[0]];
                        func.instruction(&Instruction::LocalGet(ctx));
                        func.instruction(&Instruction::Call(import_ids["context_enter"]));
                        func.instruction(&Instruction::LocalSet(locals[op.out.as_ref().unwrap()]));
                    }
                    "context_exit" => {
                        let args = op.args.as_ref().unwrap();
                        let ctx = locals[&args[0]];
                        let exc = locals[&args[1]];
                        func.instruction(&Instruction::LocalGet(ctx));
                        func.instruction(&Instruction::LocalGet(exc));
                        func.instruction(&Instruction::Call(import_ids["context_exit"]));
                        func.instruction(&Instruction::LocalSet(locals[op.out.as_ref().unwrap()]));
                    }
                    "context_unwind" => {
                        let args = op.args.as_ref().unwrap();
                        let exc = locals[&args[0]];
                        func.instruction(&Instruction::LocalGet(exc));
                        func.instruction(&Instruction::Call(import_ids["context_unwind"]));
                        func.instruction(&Instruction::LocalSet(locals[op.out.as_ref().unwrap()]));
                    }
                    "context_depth" => {
                        func.instruction(&Instruction::Call(import_ids["context_depth"]));
                        func.instruction(&Instruction::LocalSet(locals[op.out.as_ref().unwrap()]));
                    }
                    "context_unwind_to" => {
                        let args = op.args.as_ref().unwrap();
                        let depth = locals[&args[0]];
                        let exc = locals[&args[1]];
                        func.instruction(&Instruction::LocalGet(depth));
                        func.instruction(&Instruction::LocalGet(exc));
                        func.instruction(&Instruction::Call(import_ids["context_unwind_to"]));
                        func.instruction(&Instruction::LocalSet(locals[op.out.as_ref().unwrap()]));
                    }
                    "context_closing" => {
                        let args = op.args.as_ref().unwrap();
                        let payload = locals[&args[0]];
                        func.instruction(&Instruction::LocalGet(payload));
                        func.instruction(&Instruction::Call(import_ids["context_closing"]));
                        func.instruction(&Instruction::LocalSet(locals[op.out.as_ref().unwrap()]));
                    }
                    "exception_push" => {
                        func.instruction(&Instruction::Call(import_ids["exception_push"]));
                        func.instruction(&Instruction::LocalSet(locals[op.out.as_ref().unwrap()]));
                    }
                    "exception_pop" => {
                        func.instruction(&Instruction::Call(import_ids["exception_pop"]));
                        func.instruction(&Instruction::LocalSet(locals[op.out.as_ref().unwrap()]));
                    }
                    "exception_last" => {
                        func.instruction(&Instruction::Call(import_ids["exception_last"]));
                        func.instruction(&Instruction::LocalSet(locals[op.out.as_ref().unwrap()]));
                    }
                    "exception_new" => {
                        let args = op.args.as_ref().unwrap();
                        let kind = locals[&args[0]];
                        let msg = locals[&args[1]];
                        func.instruction(&Instruction::LocalGet(kind));
                        func.instruction(&Instruction::LocalGet(msg));
                        func.instruction(&Instruction::Call(import_ids["exception_new"]));
                        func.instruction(&Instruction::LocalSet(locals[op.out.as_ref().unwrap()]));
                    }
                    "exception_clear" => {
                        func.instruction(&Instruction::Call(import_ids["exception_clear"]));
                        func.instruction(&Instruction::LocalSet(locals[op.out.as_ref().unwrap()]));
                    }
                    "exception_kind" => {
                        let args = op.args.as_ref().unwrap();
                        let exc = locals[&args[0]];
                        func.instruction(&Instruction::LocalGet(exc));
                        func.instruction(&Instruction::Call(import_ids["exception_kind"]));
                        func.instruction(&Instruction::LocalSet(locals[op.out.as_ref().unwrap()]));
                    }
                    "exception_message" => {
                        let args = op.args.as_ref().unwrap();
                        let exc = locals[&args[0]];
                        func.instruction(&Instruction::LocalGet(exc));
                        func.instruction(&Instruction::Call(import_ids["exception_message"]));
                        func.instruction(&Instruction::LocalSet(locals[op.out.as_ref().unwrap()]));
                    }
                    "exception_set_cause" => {
                        let args = op.args.as_ref().unwrap();
                        let exc = locals[&args[0]];
                        let cause = locals[&args[1]];
                        func.instruction(&Instruction::LocalGet(exc));
                        func.instruction(&Instruction::LocalGet(cause));
                        func.instruction(&Instruction::Call(import_ids["exception_set_cause"]));
                        func.instruction(&Instruction::LocalSet(locals[op.out.as_ref().unwrap()]));
                    }
                    "exception_context_set" => {
                        let args = op.args.as_ref().unwrap();
                        let exc = locals[&args[0]];
                        func.instruction(&Instruction::LocalGet(exc));
                        func.instruction(&Instruction::Call(import_ids["exception_context_set"]));
                        func.instruction(&Instruction::LocalSet(locals[op.out.as_ref().unwrap()]));
                    }
                    "raise" => {
                        let args = op.args.as_ref().unwrap();
                        let exc = locals[&args[0]];
                        func.instruction(&Instruction::LocalGet(exc));
                        func.instruction(&Instruction::Call(import_ids["raise"]));
                        func.instruction(&Instruction::LocalSet(locals[op.out.as_ref().unwrap()]));
                    }
                    "bridge_unavailable" => {
                        let args = op.args.as_ref().unwrap();
                        let msg = locals[&args[0]];
                        func.instruction(&Instruction::LocalGet(msg));
                        func.instruction(&Instruction::Call(import_ids["bridge_unavailable"]));
                        func.instruction(&Instruction::LocalSet(locals[op.out.as_ref().unwrap()]));
                    }
                    "file_open" => {
                        let args = op.args.as_ref().unwrap();
                        let path = locals[&args[0]];
                        let mode = locals[&args[1]];
                        func.instruction(&Instruction::LocalGet(path));
                        func.instruction(&Instruction::LocalGet(mode));
                        func.instruction(&Instruction::Call(import_ids["file_open"]));
                        func.instruction(&Instruction::LocalSet(locals[op.out.as_ref().unwrap()]));
                    }
                    "file_read" => {
                        let args = op.args.as_ref().unwrap();
                        let handle = locals[&args[0]];
                        let size = locals[&args[1]];
                        func.instruction(&Instruction::LocalGet(handle));
                        func.instruction(&Instruction::LocalGet(size));
                        func.instruction(&Instruction::Call(import_ids["file_read"]));
                        func.instruction(&Instruction::LocalSet(locals[op.out.as_ref().unwrap()]));
                    }
                    "file_write" => {
                        let args = op.args.as_ref().unwrap();
                        let handle = locals[&args[0]];
                        let data = locals[&args[1]];
                        func.instruction(&Instruction::LocalGet(handle));
                        func.instruction(&Instruction::LocalGet(data));
                        func.instruction(&Instruction::Call(import_ids["file_write"]));
                        func.instruction(&Instruction::LocalSet(locals[op.out.as_ref().unwrap()]));
                    }
                    "file_close" => {
                        let args = op.args.as_ref().unwrap();
                        let handle = locals[&args[0]];
                        func.instruction(&Instruction::LocalGet(handle));
                        func.instruction(&Instruction::Call(import_ids["file_close"]));
                        func.instruction(&Instruction::LocalSet(locals[op.out.as_ref().unwrap()]));
                    }
                    "block_on" => {
                        let args = op.args.as_ref().unwrap();
                        func.instruction(&Instruction::LocalGet(locals[&args[0]]));
                        func.instruction(&Instruction::Call(import_ids["block_on"]));
                        func.instruction(&Instruction::LocalSet(locals[op.out.as_ref().unwrap()]));
                    }
                    "ret" => {
                        func.instruction(&Instruction::LocalGet(locals[op.var.as_ref().unwrap()]));
                        func.instruction(&Instruction::Return);
                    }
                    "ret_void" => {
                        if type_idx == 0 || type_idx == 2 {
                            func.instruction(&Instruction::I64Const(0));
                        }
                        func.instruction(&Instruction::Return);
                    }
                    "if" => {
                        let args = op.args.as_ref().unwrap();
                        let cond = locals[&args[0]];
                        func.instruction(&Instruction::LocalGet(cond));
                        func.instruction(&Instruction::Call(import_ids["is_truthy"]));
                        func.instruction(&Instruction::I64Const(0));
                        func.instruction(&Instruction::I64Ne);
                        func.instruction(&Instruction::If(BlockType::Empty));
                        control_stack.push(ControlKind::If);
                    }
                    "else" => {
                        func.instruction(&Instruction::Else);
                    }
                    "end_if" => {
                        func.instruction(&Instruction::End);
                        control_stack.pop();
                    }
                    "loop_start" => {
                        func.instruction(&Instruction::Block(BlockType::Empty));
                        func.instruction(&Instruction::Loop(BlockType::Empty));
                        control_stack.push(ControlKind::Block);
                        control_stack.push(ControlKind::Loop);
                    }
                    "loop_index_start" => {
                        let args = op.args.as_ref().unwrap();
                        let start = locals[&args[0]];
                        let out = locals[op.out.as_ref().unwrap()];
                        func.instruction(&Instruction::LocalGet(start));
                        func.instruction(&Instruction::LocalSet(out));
                        func.instruction(&Instruction::Block(BlockType::Empty));
                        func.instruction(&Instruction::Loop(BlockType::Empty));
                        control_stack.push(ControlKind::Block);
                        control_stack.push(ControlKind::Loop);
                    }
                    "loop_index_next" => {
                        let args = op.args.as_ref().unwrap();
                        let next_idx = locals[&args[0]];
                        let out = locals[op.out.as_ref().unwrap()];
                        func.instruction(&Instruction::LocalGet(next_idx));
                        func.instruction(&Instruction::LocalSet(out));
                    }
                    "loop_break_if_true" => {
                        let args = op.args.as_ref().unwrap();
                        let cond = locals[&args[0]];
                        func.instruction(&Instruction::LocalGet(cond));
                        func.instruction(&Instruction::Call(import_ids["is_truthy"]));
                        func.instruction(&Instruction::I64Const(0));
                        func.instruction(&Instruction::I64Ne);
                        func.instruction(&Instruction::BrIf(1));
                    }
                    "loop_break_if_false" => {
                        let args = op.args.as_ref().unwrap();
                        let cond = locals[&args[0]];
                        func.instruction(&Instruction::LocalGet(cond));
                        func.instruction(&Instruction::Call(import_ids["is_truthy"]));
                        func.instruction(&Instruction::I64Const(0));
                        func.instruction(&Instruction::I64Eq);
                        func.instruction(&Instruction::BrIf(1));
                    }
                    "loop_continue" => {
                        func.instruction(&Instruction::Br(0));
                    }
                    "loop_end" => {
                        func.instruction(&Instruction::End);
                        func.instruction(&Instruction::End);
                        control_stack.pop();
                        control_stack.pop();
                    }
                    "try_start" => {
                        func.instruction(&Instruction::Block(BlockType::Empty));
                        control_stack.push(ControlKind::Try);
                        try_stack.push(control_stack.len() - 1);
                    }
                    "try_end" => {
                        func.instruction(&Instruction::End);
                        control_stack.pop();
                        try_stack.pop();
                    }
                    "check_exception" => {
                        if let Some(&try_index) = try_stack.last() {
                            func.instruction(&Instruction::Call(import_ids["exception_pending"]));
                            func.instruction(&Instruction::I64Const(0));
                            func.instruction(&Instruction::I64Ne);
                            let depth = control_stack.len().saturating_sub(1 + try_index);
                            func.instruction(&Instruction::BrIf(depth as u32));
                        }
                    }
                    _ => {}
                }
            }
        };

        if stateful {
            let state_local = state_local.expect("state local missing for stateful wasm");
            let self_param = *locals
                .get("self_param")
                .expect("self_param missing for stateful wasm");
            let op_count = func_ir.ops.len();
            let mut label_to_index: HashMap<i64, usize> = HashMap::new();
            for (idx, op) in func_ir.ops.iter().enumerate() {
                if op.kind == "label" || op.kind == "state_label" {
                    if let Some(label_id) = op.value {
                        label_to_index.insert(label_id, idx);
                    }
                }
            }

            let mut else_for_if: HashMap<usize, usize> = HashMap::new();
            let mut end_for_if: HashMap<usize, usize> = HashMap::new();
            let mut end_for_else: HashMap<usize, usize> = HashMap::new();
            let mut if_stack: Vec<usize> = Vec::new();
            for (idx, op) in func_ir.ops.iter().enumerate() {
                match op.kind.as_str() {
                    "if" => if_stack.push(idx),
                    "else" => {
                        if let Some(if_idx) = if_stack.last().copied() {
                            else_for_if.insert(if_idx, idx);
                        }
                    }
                    "end_if" => {
                        if let Some(if_idx) = if_stack.pop() {
                            end_for_if.insert(if_idx, idx);
                            if let Some(else_idx) = else_for_if.get(&if_idx).copied() {
                                end_for_else.insert(else_idx, idx);
                            }
                        }
                    }
                    _ => {}
                }
            }

            let mut loop_end_for_start: HashMap<usize, usize> = HashMap::new();
            let mut loop_stack: Vec<usize> = Vec::new();
            for (idx, op) in func_ir.ops.iter().enumerate() {
                match op.kind.as_str() {
                    "loop_start" | "loop_index_start" => loop_stack.push(idx),
                    "loop_end" => {
                        if let Some(start_idx) = loop_stack.pop() {
                            loop_end_for_start.insert(start_idx, idx);
                        }
                    }
                    _ => {}
                }
            }
            let mut loop_continue_target: HashMap<usize, usize> = HashMap::new();
            let mut loop_break_target: HashMap<usize, usize> = HashMap::new();
            let mut loop_scan: Vec<usize> = Vec::new();
            for (idx, op) in func_ir.ops.iter().enumerate() {
                match op.kind.as_str() {
                    "loop_start" | "loop_index_start" => loop_scan.push(idx),
                    "loop_end" => {
                        loop_scan.pop();
                    }
                    "loop_continue" => {
                        if let Some(start_idx) = loop_scan.last().copied() {
                            loop_continue_target.insert(idx, start_idx);
                        }
                    }
                    "loop_break_if_true" | "loop_break_if_false" => {
                        if let Some(start_idx) = loop_scan.last().copied() {
                            if let Some(end_idx) = loop_end_for_start.get(&start_idx).copied() {
                                loop_break_target.insert(idx, end_idx);
                            }
                        }
                    }
                    _ => {}
                }
            }

            let mut state_map: HashMap<i64, usize> = HashMap::new();
            state_map.insert(0, 0);
            for (idx, op) in func_ir.ops.iter().enumerate() {
                if matches!(op.kind.as_str(), "state_yield" | "state_label") {
                    if let Some(state_id) = op.value {
                        state_map.insert(state_id, idx + 1);
                    }
                }
            }

            let dispatch_depths: Vec<u32> = (0..op_count)
                .map(|idx| (op_count - 1 - idx) as u32)
                .collect();

            func.instruction(&Instruction::LocalGet(self_param));
            func.instruction(&Instruction::I32WrapI64);
            func.instruction(&Instruction::I32Const(-16));
            func.instruction(&Instruction::I32Add);
            func.instruction(&Instruction::I64Load(wasm_encoder::MemArg {
                align: 3,
                offset: 0,
                memory_index: 0,
            }));
            func.instruction(&Instruction::LocalSet(state_local));
            for (state_id, target_idx) in &state_map {
                func.instruction(&Instruction::LocalGet(state_local));
                func.instruction(&Instruction::I64Const(*state_id));
                func.instruction(&Instruction::I64Eq);
                func.instruction(&Instruction::If(BlockType::Empty));
                func.instruction(&Instruction::I64Const(*target_idx as i64));
                func.instruction(&Instruction::LocalSet(state_local));
                func.instruction(&Instruction::End);
            }

            func.instruction(&Instruction::Loop(BlockType::Empty));
            for _ in (0..op_count).rev() {
                func.instruction(&Instruction::Block(BlockType::Empty));
            }

            func.instruction(&Instruction::LocalGet(state_local));
            func.instruction(&Instruction::I32WrapI64);
            let targets: Vec<u32> = (0..op_count).map(|idx| idx as u32).collect();
            func.instruction(&Instruction::BrTable(targets.into(), op_count as u32));
            func.instruction(&Instruction::End);

            let mut scratch_control: Vec<ControlKind> = Vec::new();
            let mut scratch_try: Vec<usize> = Vec::new();

            for (idx, op) in func_ir.ops.iter().enumerate() {
                let depth = dispatch_depths[idx];
                match op.kind.as_str() {
                    "state_switch" => {
                        func.instruction(&Instruction::I64Const((idx + 1) as i64));
                        func.instruction(&Instruction::LocalSet(state_local));
                        func.instruction(&Instruction::Br(depth));
                    }
                    "state_transition" => {
                        let args = op.args.as_ref().unwrap();
                        let future = locals[&args[0]];
                        let (slot_bits, pending_state) = if args.len() == 2 {
                            (None, locals[&args[1]])
                        } else {
                            (Some(locals[&args[1]]), locals[&args[2]])
                        };
                        let next_state_id = op.value.unwrap();
                        func.instruction(&Instruction::I64Const((idx + 1) as i64));
                        func.instruction(&Instruction::LocalSet(state_local));
                        func.instruction(&Instruction::LocalGet(self_param));
                        func.instruction(&Instruction::I32WrapI64);
                        func.instruction(&Instruction::I32Const(-16));
                        func.instruction(&Instruction::I32Add);
                        func.instruction(&Instruction::LocalGet(pending_state));
                        func.instruction(&Instruction::I64Const(INT_MASK as i64));
                        func.instruction(&Instruction::I64And);
                        func.instruction(&Instruction::I64Store(wasm_encoder::MemArg {
                            align: 3,
                            offset: 0,
                            memory_index: 0,
                        }));
                        func.instruction(&Instruction::LocalGet(future));
                        func.instruction(&Instruction::LocalGet(future));
                        func.instruction(&Instruction::I32WrapI64);
                        func.instruction(&Instruction::I32Const(-24));
                        func.instruction(&Instruction::I32Add);
                        func.instruction(&Instruction::I32Load(wasm_encoder::MemArg {
                            align: 2,
                            offset: 0,
                            memory_index: 0,
                        }));
                        func.instruction(&Instruction::CallIndirect { ty: 2, table: 0 });
                        let out = locals[op.out.as_ref().unwrap()];
                        func.instruction(&Instruction::LocalSet(out));
                        func.instruction(&Instruction::LocalGet(out));
                        func.instruction(&Instruction::I64Const(box_pending()));
                        func.instruction(&Instruction::I64Eq);
                        func.instruction(&Instruction::If(BlockType::Empty));
                        func.instruction(&Instruction::I64Const(box_pending()));
                        func.instruction(&Instruction::Return);
                        func.instruction(&Instruction::End);
                        if let Some(slot) = slot_bits {
                            func.instruction(&Instruction::LocalGet(self_param));
                            func.instruction(&Instruction::LocalGet(slot));
                            func.instruction(&Instruction::I64Const(INT_MASK as i64));
                            func.instruction(&Instruction::I64And);
                            func.instruction(&Instruction::LocalGet(out));
                            func.instruction(&Instruction::Call(import_ids["closure_store"]));
                            func.instruction(&Instruction::Drop);
                        }
                        func.instruction(&Instruction::LocalGet(self_param));
                        func.instruction(&Instruction::I32WrapI64);
                        func.instruction(&Instruction::I32Const(-16));
                        func.instruction(&Instruction::I32Add);
                        func.instruction(&Instruction::I64Const(next_state_id));
                        func.instruction(&Instruction::I64Store(wasm_encoder::MemArg {
                            align: 3,
                            offset: 0,
                            memory_index: 0,
                        }));
                        func.instruction(&Instruction::Br(depth));
                    }
                    "state_yield" => {
                        let args = op.args.as_ref().unwrap();
                        let pair = locals[&args[0]];
                        func.instruction(&Instruction::I64Const((idx + 1) as i64));
                        func.instruction(&Instruction::LocalSet(state_local));
                        func.instruction(&Instruction::LocalGet(self_param));
                        func.instruction(&Instruction::I32WrapI64);
                        func.instruction(&Instruction::I32Const(-16));
                        func.instruction(&Instruction::I32Add);
                        func.instruction(&Instruction::I64Const(op.value.unwrap()));
                        func.instruction(&Instruction::I64Store(wasm_encoder::MemArg {
                            align: 3,
                            offset: 0,
                            memory_index: 0,
                        }));
                        func.instruction(&Instruction::LocalGet(pair));
                        func.instruction(&Instruction::Return);
                    }
                    "chan_send_yield" => {
                        let args = op.args.as_ref().unwrap();
                        let chan = locals[&args[0]];
                        let val = locals[&args[1]];
                        let pending_state = locals[&args[2]];
                        let next_state_id = op.value.unwrap();
                        func.instruction(&Instruction::I64Const((idx + 1) as i64));
                        func.instruction(&Instruction::LocalSet(state_local));
                        func.instruction(&Instruction::LocalGet(self_param));
                        func.instruction(&Instruction::I32WrapI64);
                        func.instruction(&Instruction::I32Const(-16));
                        func.instruction(&Instruction::I32Add);
                        func.instruction(&Instruction::LocalGet(pending_state));
                        func.instruction(&Instruction::I64Const(INT_MASK as i64));
                        func.instruction(&Instruction::I64And);
                        func.instruction(&Instruction::I64Store(wasm_encoder::MemArg {
                            align: 3,
                            offset: 0,
                            memory_index: 0,
                        }));
                        func.instruction(&Instruction::LocalGet(chan));
                        func.instruction(&Instruction::LocalGet(val));
                        func.instruction(&Instruction::Call(import_ids["chan_send"]));
                        let out = locals[op.out.as_ref().unwrap()];
                        func.instruction(&Instruction::LocalSet(out));
                        func.instruction(&Instruction::LocalGet(out));
                        func.instruction(&Instruction::I64Const(box_pending()));
                        func.instruction(&Instruction::I64Eq);
                        func.instruction(&Instruction::If(BlockType::Empty));
                        func.instruction(&Instruction::I64Const(box_pending()));
                        func.instruction(&Instruction::Return);
                        func.instruction(&Instruction::End);
                        func.instruction(&Instruction::LocalGet(self_param));
                        func.instruction(&Instruction::I32WrapI64);
                        func.instruction(&Instruction::I32Const(-16));
                        func.instruction(&Instruction::I32Add);
                        func.instruction(&Instruction::I64Const(next_state_id));
                        func.instruction(&Instruction::I64Store(wasm_encoder::MemArg {
                            align: 3,
                            offset: 0,
                            memory_index: 0,
                        }));
                        func.instruction(&Instruction::Br(depth));
                    }
                    "chan_recv_yield" => {
                        let args = op.args.as_ref().unwrap();
                        let chan = locals[&args[0]];
                        let pending_state = locals[&args[1]];
                        let next_state_id = op.value.unwrap();
                        func.instruction(&Instruction::I64Const((idx + 1) as i64));
                        func.instruction(&Instruction::LocalSet(state_local));
                        func.instruction(&Instruction::LocalGet(self_param));
                        func.instruction(&Instruction::I32WrapI64);
                        func.instruction(&Instruction::I32Const(-16));
                        func.instruction(&Instruction::I32Add);
                        func.instruction(&Instruction::LocalGet(pending_state));
                        func.instruction(&Instruction::I64Const(INT_MASK as i64));
                        func.instruction(&Instruction::I64And);
                        func.instruction(&Instruction::I64Store(wasm_encoder::MemArg {
                            align: 3,
                            offset: 0,
                            memory_index: 0,
                        }));
                        func.instruction(&Instruction::LocalGet(chan));
                        func.instruction(&Instruction::Call(import_ids["chan_recv"]));
                        let out = locals[op.out.as_ref().unwrap()];
                        func.instruction(&Instruction::LocalSet(out));
                        func.instruction(&Instruction::LocalGet(out));
                        func.instruction(&Instruction::I64Const(box_pending()));
                        func.instruction(&Instruction::I64Eq);
                        func.instruction(&Instruction::If(BlockType::Empty));
                        func.instruction(&Instruction::I64Const(box_pending()));
                        func.instruction(&Instruction::Return);
                        func.instruction(&Instruction::End);
                        func.instruction(&Instruction::LocalGet(self_param));
                        func.instruction(&Instruction::I32WrapI64);
                        func.instruction(&Instruction::I32Const(-16));
                        func.instruction(&Instruction::I32Add);
                        func.instruction(&Instruction::I64Const(next_state_id));
                        func.instruction(&Instruction::I64Store(wasm_encoder::MemArg {
                            align: 3,
                            offset: 0,
                            memory_index: 0,
                        }));
                        func.instruction(&Instruction::Br(depth));
                    }
                    "if" => {
                        let args = op.args.as_ref().unwrap();
                        let cond = locals[&args[0]];
                        let else_idx = else_for_if.get(&idx).copied();
                        let end_idx = end_for_if.get(&idx).copied().expect("if without end_if");
                        let false_target = if let Some(else_pos) = else_idx {
                            else_pos + 1
                        } else {
                            end_idx + 1
                        };
                        func.instruction(&Instruction::LocalGet(cond));
                        func.instruction(&Instruction::Call(import_ids["is_truthy"]));
                        func.instruction(&Instruction::I64Const(0));
                        func.instruction(&Instruction::I64Ne);
                        func.instruction(&Instruction::If(BlockType::Empty));
                        func.instruction(&Instruction::I64Const((idx + 1) as i64));
                        func.instruction(&Instruction::LocalSet(state_local));
                        func.instruction(&Instruction::Br(depth + 1));
                        func.instruction(&Instruction::Else);
                        func.instruction(&Instruction::I64Const(false_target as i64));
                        func.instruction(&Instruction::LocalSet(state_local));
                        func.instruction(&Instruction::Br(depth + 1));
                        func.instruction(&Instruction::End);
                    }
                    "else" => {
                        let end_idx = end_for_else
                            .get(&idx)
                            .copied()
                            .expect("else without end_if");
                        func.instruction(&Instruction::I64Const((end_idx + 1) as i64));
                        func.instruction(&Instruction::LocalSet(state_local));
                        func.instruction(&Instruction::Br(depth));
                    }
                    "end_if" => {
                        func.instruction(&Instruction::I64Const((idx + 1) as i64));
                        func.instruction(&Instruction::LocalSet(state_local));
                        func.instruction(&Instruction::Br(depth));
                    }
                    "loop_start" => {
                        func.instruction(&Instruction::I64Const((idx + 1) as i64));
                        func.instruction(&Instruction::LocalSet(state_local));
                        func.instruction(&Instruction::Br(depth));
                    }
                    "loop_index_start" => {
                        let args = op.args.as_ref().unwrap();
                        let start = locals[&args[0]];
                        let out = locals[op.out.as_ref().unwrap()];
                        func.instruction(&Instruction::LocalGet(start));
                        func.instruction(&Instruction::LocalSet(out));
                        func.instruction(&Instruction::I64Const((idx + 1) as i64));
                        func.instruction(&Instruction::LocalSet(state_local));
                        func.instruction(&Instruction::Br(depth));
                    }
                    "loop_break_if_true" => {
                        let args = op.args.as_ref().unwrap();
                        let cond = locals[&args[0]];
                        let end_idx = loop_break_target
                            .get(&idx)
                            .copied()
                            .expect("loop break without loop");
                        func.instruction(&Instruction::LocalGet(cond));
                        func.instruction(&Instruction::Call(import_ids["is_truthy"]));
                        func.instruction(&Instruction::I64Const(0));
                        func.instruction(&Instruction::I64Ne);
                        func.instruction(&Instruction::If(BlockType::Empty));
                        func.instruction(&Instruction::I64Const((end_idx + 1) as i64));
                        func.instruction(&Instruction::LocalSet(state_local));
                        func.instruction(&Instruction::Br(depth + 1));
                        func.instruction(&Instruction::Else);
                        func.instruction(&Instruction::I64Const((idx + 1) as i64));
                        func.instruction(&Instruction::LocalSet(state_local));
                        func.instruction(&Instruction::Br(depth + 1));
                        func.instruction(&Instruction::End);
                    }
                    "loop_break_if_false" => {
                        let args = op.args.as_ref().unwrap();
                        let cond = locals[&args[0]];
                        let end_idx = loop_break_target
                            .get(&idx)
                            .copied()
                            .expect("loop break without loop");
                        func.instruction(&Instruction::LocalGet(cond));
                        func.instruction(&Instruction::Call(import_ids["is_truthy"]));
                        func.instruction(&Instruction::I64Const(0));
                        func.instruction(&Instruction::I64Eq);
                        func.instruction(&Instruction::If(BlockType::Empty));
                        func.instruction(&Instruction::I64Const((end_idx + 1) as i64));
                        func.instruction(&Instruction::LocalSet(state_local));
                        func.instruction(&Instruction::Br(depth + 1));
                        func.instruction(&Instruction::Else);
                        func.instruction(&Instruction::I64Const((idx + 1) as i64));
                        func.instruction(&Instruction::LocalSet(state_local));
                        func.instruction(&Instruction::Br(depth + 1));
                        func.instruction(&Instruction::End);
                    }
                    "loop_continue" => {
                        let start_idx = loop_continue_target
                            .get(&idx)
                            .copied()
                            .expect("loop continue without loop");
                        func.instruction(&Instruction::I64Const((start_idx + 1) as i64));
                        func.instruction(&Instruction::LocalSet(state_local));
                        func.instruction(&Instruction::Br(depth));
                    }
                    "loop_end" => {
                        func.instruction(&Instruction::I64Const((idx + 1) as i64));
                        func.instruction(&Instruction::LocalSet(state_local));
                        func.instruction(&Instruction::Br(depth));
                    }
                    "try_start" | "try_end" | "label" | "state_label" => {
                        func.instruction(&Instruction::I64Const((idx + 1) as i64));
                        func.instruction(&Instruction::LocalSet(state_local));
                        func.instruction(&Instruction::Br(depth));
                    }
                    "check_exception" => {
                        let target_label = op.value.expect("check_exception missing label");
                        let target_idx = label_to_index
                            .get(&target_label)
                            .copied()
                            .expect("unknown check_exception label");
                        func.instruction(&Instruction::Call(import_ids["exception_pending"]));
                        func.instruction(&Instruction::I64Const(0));
                        func.instruction(&Instruction::I64Ne);
                        func.instruction(&Instruction::If(BlockType::Empty));
                        func.instruction(&Instruction::I64Const(target_idx as i64));
                        func.instruction(&Instruction::LocalSet(state_local));
                        func.instruction(&Instruction::Br(depth + 1));
                        func.instruction(&Instruction::Else);
                        func.instruction(&Instruction::I64Const((idx + 1) as i64));
                        func.instruction(&Instruction::LocalSet(state_local));
                        func.instruction(&Instruction::Br(depth + 1));
                        func.instruction(&Instruction::End);
                    }
                    "ret" => {
                        func.instruction(&Instruction::LocalGet(locals[op.var.as_ref().unwrap()]));
                        func.instruction(&Instruction::Return);
                    }
                    "ret_void" => {
                        if type_idx == 0 || type_idx == 2 {
                            func.instruction(&Instruction::I64Const(0));
                        }
                        func.instruction(&Instruction::Return);
                    }
                    _ => {
                        emit_ops(
                            &mut func,
                            std::slice::from_ref(op),
                            &mut scratch_control,
                            &mut scratch_try,
                        );
                        func.instruction(&Instruction::I64Const((idx + 1) as i64));
                        func.instruction(&Instruction::LocalSet(state_local));
                        func.instruction(&Instruction::Br(depth));
                    }
                }
                if idx + 1 < op_count {
                    func.instruction(&Instruction::End);
                }
            }

            func.instruction(&Instruction::End);
            func.instruction(&Instruction::I64Const(box_none()));
            func.instruction(&Instruction::Return);
            func.instruction(&Instruction::End);
        } else {
            emit_ops(&mut func, &func_ir.ops, &mut control_stack, &mut try_stack);
            func.instruction(&Instruction::End);
        }
        self.codes.function(&func);
    }
}
