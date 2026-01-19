use crate::{FunctionIR, OpIR, SimpleIR, TrampolineSpec};
use std::borrow::Cow;
use std::collections::{HashMap, HashSet};
use wasm_encoder::{
    BlockType, CodeSection, ConstExpr, CustomSection, DataSection, DataSymbolDefinition,
    ElementMode, ElementSection, ElementSegment, Elements, Encode, EntityType, ExportKind,
    ExportSection, Function, FunctionSection, ImportSection, Instruction, LinkingSection,
    MemorySection, MemoryType, Module, RawSection, RefType, SymbolTable, TableSection, TableType,
    TypeSection, ValType,
};
use wasmparser::{DataKind, ElementItems, ExternalKind, Operator, Parser, Payload, TypeRef};

const QNAN: u64 = 0x7ff8_0000_0000_0000;
const TAG_INT: u64 = 0x0001_0000_0000_0000;
const TAG_BOOL: u64 = 0x0002_0000_0000_0000;
const TAG_NONE: u64 = 0x0003_0000_0000_0000;
const TAG_PTR: u64 = 0x0004_0000_0000_0000;
const TAG_PENDING: u64 = 0x0005_0000_0000_0000;
const TAG_MASK: u64 = 0x0007_0000_0000_0000;
const POINTER_MASK: u64 = 0x0000_FFFF_FFFF_FFFF;
const QNAN_TAG_MASK_I64: i64 = (QNAN | TAG_MASK) as i64;
const QNAN_TAG_PTR_I64: i64 = (QNAN | TAG_PTR) as i64;
const INT_MASK: u64 = (1 << 47) - 1;
const HEADER_SIZE_BYTES: i32 = 40;
const HEADER_STATE_OFFSET: i32 = -(HEADER_SIZE_BYTES - 16);
const GEN_CONTROL_SIZE: i32 = 48;
const TASK_KIND_FUTURE: i64 = 0;
const TASK_KIND_GENERATOR: i64 = 1;
const RELOC_TABLE_BASE_DEFAULT: u32 = 4096;

#[derive(Clone, Copy)]
struct DataSegmentInfo {
    size: u32,
}

#[derive(Clone, Copy)]
struct DataRelocSite {
    func_index: u32,
    offset_in_func: u32,
    segment_index: u32,
}

#[derive(Clone, Copy)]
struct DataSegmentRef {
    offset: u32,
    index: u32,
}

struct CompileFuncContext<'a> {
    func_map: &'a HashMap<String, u32>,
    func_indices: &'a HashMap<String, u32>,
    trampoline_map: &'a HashMap<String, u32>,
    table_base: u32,
    import_ids: &'a HashMap<String, u32>,
    reloc_enabled: bool,
}

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
    func_count: u32,
    import_ids: HashMap<String, u32>,
    data_offset: u32,
    data_segments: Vec<DataSegmentInfo>,
    data_relocs: Vec<DataRelocSite>,
    molt_main_index: Option<u32>,
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
            func_count: 0,
            import_ids: HashMap::new(),
            data_offset: 8,
            data_segments: Vec::new(),
            data_relocs: Vec::new(),
            molt_main_index: None,
        }
    }

    fn add_data_segment(&mut self, reloc_enabled: bool, bytes: &[u8]) -> DataSegmentRef {
        let offset = self.data_offset;
        let index = self.data_segments.len() as u32;
        let const_expr = if reloc_enabled {
            const_expr_i32_const_padded(offset as i32)
        } else {
            ConstExpr::i32_const(offset as i32)
        };
        self.data.active(0, &const_expr, bytes.iter().copied());
        self.data_offset = (self.data_offset + bytes.len() as u32 + 7) & !7;
        let info = DataSegmentInfo {
            size: bytes.len() as u32,
        };
        self.data_segments.push(info);
        DataSegmentRef { offset, index }
    }

    fn emit_data_ptr(
        &mut self,
        reloc_enabled: bool,
        func_index: u32,
        func: &mut Function,
        data: DataSegmentRef,
    ) {
        let imm_offset = func.byte_len() as u32 + 1;
        self.data_relocs.push(DataRelocSite {
            func_index,
            offset_in_func: imm_offset,
            segment_index: data.index,
        });
        emit_i32_const(func, reloc_enabled, data.offset as i32);
        func.instruction(&Instruction::I64ExtendI32U);
    }

    pub fn compile(mut self, ir: SimpleIR) -> Vec<u8> {
        let mut ir = ir;
        for func_ir in &mut ir.functions {
            crate::elide_dead_struct_allocs(func_ir);
        }
        let mut func_trampoline_spec: HashMap<String, (usize, bool)> = HashMap::new();
        let mut generator_funcs: HashSet<String> = HashSet::new();
        let mut generator_closure_sizes: HashMap<String, i64> = HashMap::new();
        for func_ir in &ir.functions {
            let mut func_obj_names: HashMap<String, String> = HashMap::new();
            let mut const_values: HashMap<String, i64> = HashMap::new();
            let mut const_bools: HashMap<String, bool> = HashMap::new();
            for op in &func_ir.ops {
                match op.kind.as_str() {
                    "const" => {
                        let Some(out) = op.out.as_ref() else {
                            continue;
                        };
                        let val = op.value.unwrap_or(0);
                        const_values.insert(out.clone(), val);
                    }
                    "const_bool" => {
                        let Some(out) = op.out.as_ref() else {
                            continue;
                        };
                        let val = op.value.unwrap_or(0) != 0;
                        const_bools.insert(out.clone(), val);
                    }
                    "func_new" | "func_new_closure" => {
                        let Some(name) = op.s_value.as_ref() else {
                            continue;
                        };
                        let arity = op.value.unwrap_or(0) as usize;
                        let has_closure = op.kind == "func_new_closure";
                        if let Some(out) = op.out.as_ref() {
                            func_obj_names.insert(out.clone(), name.clone());
                        }
                        if let Some((prev_arity, prev_closure)) = func_trampoline_spec.get(name) {
                            if *prev_arity != arity || *prev_closure != has_closure {
                                panic!("func_new arity mismatch for {name}");
                            }
                        } else {
                            func_trampoline_spec.insert(name.clone(), (arity, has_closure));
                        }
                    }
                    _ => {}
                }
            }
            for op in &func_ir.ops {
                if op.kind != "set_attr_generic_obj" {
                    continue;
                }
                let Some(attr) = op.s_value.as_deref() else {
                    continue;
                };
                if attr != "__molt_is_generator__" && attr != "__molt_closure_size__" {
                    continue;
                }
                let args = op.args.as_ref().expect("set_attr_generic_obj args missing");
                let Some(func_name) = func_obj_names.get(&args[0]) else {
                    continue;
                };
                match attr {
                    "__molt_is_generator__" => {
                        let val_name = &args[1];
                        let is_gen = const_bools
                            .get(val_name)
                            .copied()
                            .or_else(|| const_values.get(val_name).map(|val| *val != 0))
                            .unwrap_or(false);
                        if is_gen {
                            generator_funcs.insert(func_name.clone());
                        }
                    }
                    "__molt_closure_size__" => {
                        let val_name = &args[1];
                        if let Some(size) = const_values.get(val_name) {
                            generator_closure_sizes.insert(func_name.clone(), *size);
                        }
                    }
                    _ => {}
                }
            }
        }
        // Type 0: () -> i64 (User functions)
        self.types
            .function(std::iter::empty::<ValType>(), std::iter::once(ValType::I64));
        // Type 1: (i64) -> () (print_obj)
        self.types
            .function(std::iter::once(ValType::I64), std::iter::empty::<ValType>());
        // Type 2: (i64) -> i64 (alloc, sleep, block_on, is_truthy, is_bound_method)
        self.types
            .function(std::iter::once(ValType::I64), std::iter::once(ValType::I64));
        // Type 3: (i64, i64) -> i64 (add/sub/mul/lt/list_append/list_pop/alloc_class)
        self.types.function(
            std::iter::repeat_n(ValType::I64, 2),
            std::iter::once(ValType::I64),
        );
        // Type 4: (i64, i64, i64) -> i32 (parse_scalar)
        self.types.function(
            std::iter::repeat_n(ValType::I64, 3),
            std::iter::once(ValType::I32),
        );
        // Type 5: (i64, i64, i64) -> i64 (stream_send, ws_send, slice, slice_new, dict_get, task_new)
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
        // Type 10: (i64, i64, i64, i64, i64, i64, i64) -> i64 (guarded_field_set/init)
        self.types.function(
            std::iter::repeat_n(ValType::I64, 7),
            std::iter::once(ValType::I64),
        );
        // Type 11: (i64, i64, i64, i64) -> i32 (db_query/db_exec)
        self.types.function(
            std::iter::repeat_n(ValType::I64, 4),
            std::iter::once(ValType::I32),
        );
        // Type 12: (i64, i64, i64, i64, i64) -> i64 (print_builtin)
        self.types.function(
            std::iter::repeat_n(ValType::I64, 5),
            std::iter::once(ValType::I64),
        );
        // Type 13: (i64) -> i32 (chan_new, handle_resolve)
        self.types
            .function(std::iter::once(ValType::I64), std::iter::once(ValType::I32));
        // Type 14: (i32) -> i64 (chan_recv)
        self.types
            .function(std::iter::once(ValType::I32), std::iter::once(ValType::I64));
        // Type 15: (i32) -> () (chan_drop)
        self.types
            .function(std::iter::once(ValType::I32), std::iter::empty::<ValType>());
        // Type 16: (i32, i64) -> i64 (chan_send, object_field_get_ptr, closure_load, object_set_class)
        self.types
            .function([ValType::I32, ValType::I64], std::iter::once(ValType::I64));
        // Type 17: (i32, i64, i64) -> i64 (guard_layout_ptr, closure_store, object_field_set/init)
        self.types.function(
            [ValType::I32, ValType::I64, ValType::I64],
            std::iter::once(ValType::I64),
        );
        // Type 18: (i64, i32, i64) -> i64 (stream_send, ws_send, get_attr_object)
        self.types.function(
            [ValType::I64, ValType::I32, ValType::I64],
            std::iter::once(ValType::I64),
        );
        // Type 19: (i32, i64, i32) -> i32 (parse_scalar, ws_connect)
        self.types.function(
            [ValType::I32, ValType::I64, ValType::I32],
            std::iter::once(ValType::I32),
        );
        // Type 20: (i64, i32, i32) -> i32 (ws_pair)
        self.types.function(
            [ValType::I64, ValType::I32, ValType::I32],
            std::iter::once(ValType::I32),
        );
        // Type 21: (i32, i64, i64, i64, i32, i64) -> i64 (guarded_field_get_ptr)
        self.types.function(
            [
                ValType::I32,
                ValType::I64,
                ValType::I64,
                ValType::I64,
                ValType::I32,
                ValType::I64,
            ],
            std::iter::once(ValType::I64),
        );
        // Type 22: (i32, i64, i64, i64, i64, i32, i64) -> i64 (guarded_field_set/init)
        self.types.function(
            [
                ValType::I32,
                ValType::I64,
                ValType::I64,
                ValType::I64,
                ValType::I64,
                ValType::I32,
                ValType::I64,
            ],
            std::iter::once(ValType::I64),
        );
        // Type 23: (i32, i32, i64) -> i64 (get/del_attr_generic/ptr)
        self.types.function(
            [ValType::I32, ValType::I32, ValType::I64],
            std::iter::once(ValType::I64),
        );
        // Type 24: (i32, i32, i64, i64) -> i64 (set_attr_generic/ptr)
        self.types.function(
            [ValType::I32, ValType::I32, ValType::I64, ValType::I64],
            std::iter::once(ValType::I64),
        );
        // Type 25: (i64, i32, i64, i64) -> i64 (set_attr_object)
        self.types.function(
            [ValType::I64, ValType::I32, ValType::I64, ValType::I64],
            std::iter::once(ValType::I64),
        );
        // Type 26: (i32, i64, i32, i64) -> i32 (db_query/db_exec)
        self.types.function(
            [ValType::I32, ValType::I64, ValType::I32, ValType::I64],
            std::iter::once(ValType::I32),
        );
        // Type 27: (i32, i32) -> i64 (sleep_register)
        self.types
            .function([ValType::I32, ValType::I32], std::iter::once(ValType::I64));
        // Type 28: (i64, i64, i64, i64, i64, i64, i64, i64) -> i64 (open_builtin)
        self.types.function(
            std::iter::repeat_n(ValType::I64, 8),
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
        add_import("runtime_init", 0, &mut self.import_ids);
        add_import("runtime_shutdown", 0, &mut self.import_ids);
        add_import("print_obj", 1, &mut self.import_ids);
        add_import("print_newline", 8, &mut self.import_ids);
        add_import("alloc", 2, &mut self.import_ids);
        add_import("alloc_class", 3, &mut self.import_ids);
        add_import("alloc_class_trusted", 3, &mut self.import_ids);
        add_import("alloc_class_static", 3, &mut self.import_ids);
        add_import("async_sleep", 2, &mut self.import_ids);
        add_import("anext_default_poll", 2, &mut self.import_ids);
        add_import("future_poll", 2, &mut self.import_ids);
        add_import("sleep_register", 27, &mut self.import_ids);
        add_import("block_on", 2, &mut self.import_ids);
        add_import("cancel_token_new", 2, &mut self.import_ids);
        add_import("cancel_token_clone", 2, &mut self.import_ids);
        add_import("cancel_token_drop", 2, &mut self.import_ids);
        add_import("cancel_token_cancel", 2, &mut self.import_ids);
        add_import("cancel_token_is_cancelled", 2, &mut self.import_ids);
        add_import("cancel_token_set_current", 2, &mut self.import_ids);
        add_import("cancel_token_get_current", 0, &mut self.import_ids);
        add_import("cancelled", 0, &mut self.import_ids);
        add_import("cancel_current", 0, &mut self.import_ids);
        add_import("chan_new", 13, &mut self.import_ids);
        add_import("chan_send", 16, &mut self.import_ids);
        add_import("chan_recv", 14, &mut self.import_ids);
        add_import("chan_drop", 15, &mut self.import_ids);
        add_import("add", 3, &mut self.import_ids);
        add_import("inplace_add", 3, &mut self.import_ids);
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
        add_import("inplace_sub", 3, &mut self.import_ids);
        add_import("inplace_mul", 3, &mut self.import_ids);
        add_import("bit_or", 3, &mut self.import_ids);
        add_import("bit_and", 3, &mut self.import_ids);
        add_import("bit_xor", 3, &mut self.import_ids);
        add_import("invert", 2, &mut self.import_ids);
        add_import("inplace_bit_or", 3, &mut self.import_ids);
        add_import("inplace_bit_and", 3, &mut self.import_ids);
        add_import("inplace_bit_xor", 3, &mut self.import_ids);
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
        add_import("string_eq", 3, &mut self.import_ids);
        add_import("is", 3, &mut self.import_ids);
        add_import("closure_load", 16, &mut self.import_ids);
        add_import("closure_store", 17, &mut self.import_ids);
        add_import("not", 2, &mut self.import_ids);
        add_import("contains", 3, &mut self.import_ids);
        add_import("guard_type", 3, &mut self.import_ids);
        add_import("guard_layout_ptr", 17, &mut self.import_ids);
        add_import("guarded_field_get_ptr", 21, &mut self.import_ids);
        add_import("guarded_field_set_ptr", 22, &mut self.import_ids);
        add_import("guarded_field_init_ptr", 22, &mut self.import_ids);
        add_import("handle_resolve", 13, &mut self.import_ids);
        add_import("inc_ref_obj", 1, &mut self.import_ids);
        add_import("get_attr_generic", 23, &mut self.import_ids);
        add_import("get_attr_ptr", 23, &mut self.import_ids);
        add_import("get_attr_object", 18, &mut self.import_ids);
        add_import("get_attr_special", 18, &mut self.import_ids);
        add_import("set_attr_generic", 24, &mut self.import_ids);
        add_import("set_attr_ptr", 24, &mut self.import_ids);
        add_import("set_attr_object", 25, &mut self.import_ids);
        add_import("del_attr_generic", 23, &mut self.import_ids);
        add_import("del_attr_ptr", 23, &mut self.import_ids);
        add_import("del_attr_object", 18, &mut self.import_ids);
        add_import("object_field_get", 3, &mut self.import_ids);
        add_import("object_field_get_ptr", 16, &mut self.import_ids);
        add_import("object_field_set", 5, &mut self.import_ids);
        add_import("object_field_set_ptr", 17, &mut self.import_ids);
        add_import("object_field_init", 5, &mut self.import_ids);
        add_import("object_field_init_ptr", 17, &mut self.import_ids);
        add_import("module_new", 2, &mut self.import_ids);
        add_import("module_cache_get", 2, &mut self.import_ids);
        add_import("module_cache_set", 3, &mut self.import_ids);
        add_import("module_get_attr", 3, &mut self.import_ids);
        add_import("module_get_global", 3, &mut self.import_ids);
        add_import("module_get_name", 3, &mut self.import_ids);
        add_import("module_set_attr", 5, &mut self.import_ids);
        add_import("get_attr_name", 3, &mut self.import_ids);
        add_import("get_attr_name_default", 5, &mut self.import_ids);
        add_import("has_attr_name", 3, &mut self.import_ids);
        add_import("set_attr_name", 5, &mut self.import_ids);
        add_import("del_attr_name", 3, &mut self.import_ids);
        add_import("is_truthy", 2, &mut self.import_ids);
        add_import("is_bound_method", 2, &mut self.import_ids);
        add_import("is_function_obj", 2, &mut self.import_ids);
        add_import("function_default_kind", 2, &mut self.import_ids);
        add_import("function_closure_bits", 2, &mut self.import_ids);
        add_import("function_is_generator", 2, &mut self.import_ids);
        add_import("function_is_coroutine", 2, &mut self.import_ids);
        add_import("call_arity_error", 3, &mut self.import_ids);
        add_import("missing", 0, &mut self.import_ids);
        add_import("not_implemented", 0, &mut self.import_ids);
        add_import("json_parse_scalar", 19, &mut self.import_ids);
        add_import("msgpack_parse_scalar", 19, &mut self.import_ids);
        add_import("cbor_parse_scalar", 19, &mut self.import_ids);
        add_import("json_parse_scalar_obj", 2, &mut self.import_ids);
        add_import("msgpack_parse_scalar_obj", 2, &mut self.import_ids);
        add_import("cbor_parse_scalar_obj", 2, &mut self.import_ids);
        add_import("string_from_bytes", 19, &mut self.import_ids);
        add_import("bytes_from_bytes", 19, &mut self.import_ids);
        add_import("bigint_from_str", 16, &mut self.import_ids);
        add_import("str_from_obj", 2, &mut self.import_ids);
        add_import("repr_from_obj", 2, &mut self.import_ids);
        add_import("repr_builtin", 2, &mut self.import_ids);
        add_import("format_builtin", 3, &mut self.import_ids);
        add_import("ascii_from_obj", 2, &mut self.import_ids);
        add_import("bin_builtin", 2, &mut self.import_ids);
        add_import("oct_builtin", 2, &mut self.import_ids);
        add_import("hex_builtin", 2, &mut self.import_ids);
        add_import("callable_builtin", 2, &mut self.import_ids);
        add_import("int_from_obj", 5, &mut self.import_ids);
        add_import("float_from_obj", 2, &mut self.import_ids);
        add_import("memoryview_new", 2, &mut self.import_ids);
        add_import("memoryview_tobytes", 2, &mut self.import_ids);
        add_import("memoryview_cast", 7, &mut self.import_ids);
        add_import("intarray_from_seq", 2, &mut self.import_ids);
        add_import("len", 2, &mut self.import_ids);
        add_import("id", 2, &mut self.import_ids);
        add_import("ord", 2, &mut self.import_ids);
        add_import("chr", 2, &mut self.import_ids);
        add_import("abs_builtin", 2, &mut self.import_ids);
        add_import("divmod_builtin", 3, &mut self.import_ids);
        add_import("open_builtin", 28, &mut self.import_ids);
        add_import("getargv", 0, &mut self.import_ids);
        add_import("getrecursionlimit", 0, &mut self.import_ids);
        add_import("setrecursionlimit", 2, &mut self.import_ids);
        add_import("recursion_guard_enter", 0, &mut self.import_ids);
        add_import("recursion_guard_exit", 8, &mut self.import_ids);
        add_import("trace_enter_slot", 2, &mut self.import_ids);
        add_import("trace_set_line", 2, &mut self.import_ids);
        add_import("trace_exit", 0, &mut self.import_ids);
        add_import("code_slots_init", 2, &mut self.import_ids);
        add_import("code_slot_set", 3, &mut self.import_ids);
        add_import("code_new", 7, &mut self.import_ids);
        add_import("round_builtin", 3, &mut self.import_ids);
        add_import("enumerate_builtin", 3, &mut self.import_ids);
        add_import("iter_sentinel", 3, &mut self.import_ids);
        add_import("next_builtin", 3, &mut self.import_ids);
        add_import("any_builtin", 2, &mut self.import_ids);
        add_import("all_builtin", 2, &mut self.import_ids);
        add_import("sum_builtin", 3, &mut self.import_ids);
        add_import("min_builtin", 5, &mut self.import_ids);
        add_import("max_builtin", 5, &mut self.import_ids);
        add_import("sorted_builtin", 5, &mut self.import_ids);
        add_import("map_builtin", 3, &mut self.import_ids);
        add_import("filter_builtin", 3, &mut self.import_ids);
        add_import("zip_builtin", 2, &mut self.import_ids);
        add_import("reversed_builtin", 2, &mut self.import_ids);
        add_import("getattr_builtin", 5, &mut self.import_ids);
        add_import("anext_builtin", 3, &mut self.import_ids);
        add_import("print_builtin", 12, &mut self.import_ids);
        add_import("super_builtin", 3, &mut self.import_ids);
        add_import("callargs_new", 3, &mut self.import_ids);
        add_import("callargs_push_pos", 3, &mut self.import_ids);
        add_import("callargs_push_kw", 5, &mut self.import_ids);
        add_import("callargs_expand_star", 3, &mut self.import_ids);
        add_import("callargs_expand_kwstar", 3, &mut self.import_ids);
        add_import("call_bind", 3, &mut self.import_ids);
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
        add_import("list_clear", 2, &mut self.import_ids);
        add_import("list_copy", 2, &mut self.import_ids);
        add_import("list_reverse", 2, &mut self.import_ids);
        add_import("list_sort", 5, &mut self.import_ids);
        add_import("list_count", 3, &mut self.import_ids);
        add_import("list_index", 3, &mut self.import_ids);
        add_import("list_index_range", 7, &mut self.import_ids);
        add_import("heapq_heapify", 2, &mut self.import_ids);
        add_import("heapq_heappush", 3, &mut self.import_ids);
        add_import("heapq_heappop", 2, &mut self.import_ids);
        add_import("heapq_heapreplace", 3, &mut self.import_ids);
        add_import("heapq_heappushpop", 3, &mut self.import_ids);
        add_import("tuple_from_list", 2, &mut self.import_ids);
        add_import("dict_new", 2, &mut self.import_ids);
        add_import("dict_from_obj", 2, &mut self.import_ids);
        add_import("dict_set", 5, &mut self.import_ids);
        add_import("dict_get", 5, &mut self.import_ids);
        add_import("dict_pop", 7, &mut self.import_ids);
        add_import("dict_setdefault", 5, &mut self.import_ids);
        add_import("dict_update", 3, &mut self.import_ids);
        add_import("dict_clear", 2, &mut self.import_ids);
        add_import("dict_copy", 2, &mut self.import_ids);
        add_import("dict_popitem", 2, &mut self.import_ids);
        add_import("dict_update_kwstar", 3, &mut self.import_ids);
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
        add_import("enumerate", 5, &mut self.import_ids);
        add_import("aiter", 2, &mut self.import_ids);
        add_import("iter_next", 2, &mut self.import_ids);
        add_import("anext", 2, &mut self.import_ids);
        add_import("task_new", 5, &mut self.import_ids);
        add_import("generator_send", 3, &mut self.import_ids);
        add_import("generator_throw", 3, &mut self.import_ids);
        add_import("generator_close", 2, &mut self.import_ids);
        add_import("is_generator", 2, &mut self.import_ids);
        add_import("is_callable", 2, &mut self.import_ids);
        add_import("index", 3, &mut self.import_ids);
        add_import("store_index", 5, &mut self.import_ids);
        add_import("del_index", 3, &mut self.import_ids);
        add_import("bytes_find", 3, &mut self.import_ids);
        add_import("bytearray_find", 3, &mut self.import_ids);
        add_import("string_find", 3, &mut self.import_ids);
        add_import("bytes_find_slice", 9, &mut self.import_ids);
        add_import("bytearray_find_slice", 9, &mut self.import_ids);
        add_import("string_find_slice", 9, &mut self.import_ids);
        add_import("string_format", 3, &mut self.import_ids);
        add_import("string_startswith", 3, &mut self.import_ids);
        add_import("bytes_startswith", 3, &mut self.import_ids);
        add_import("bytearray_startswith", 3, &mut self.import_ids);
        add_import("string_startswith_slice", 9, &mut self.import_ids);
        add_import("bytes_startswith_slice", 9, &mut self.import_ids);
        add_import("bytearray_startswith_slice", 9, &mut self.import_ids);
        add_import("string_endswith", 3, &mut self.import_ids);
        add_import("bytes_endswith", 3, &mut self.import_ids);
        add_import("bytearray_endswith", 3, &mut self.import_ids);
        add_import("string_endswith_slice", 9, &mut self.import_ids);
        add_import("bytes_endswith_slice", 9, &mut self.import_ids);
        add_import("bytearray_endswith_slice", 9, &mut self.import_ids);
        add_import("string_count", 3, &mut self.import_ids);
        add_import("bytes_count", 3, &mut self.import_ids);
        add_import("bytearray_count", 3, &mut self.import_ids);
        add_import("string_count_slice", 9, &mut self.import_ids);
        add_import("bytes_count_slice", 9, &mut self.import_ids);
        add_import("bytearray_count_slice", 9, &mut self.import_ids);
        add_import("env_get", 3, &mut self.import_ids);
        add_import("getpid", 0, &mut self.import_ids);
        add_import("path_exists", 2, &mut self.import_ids);
        add_import("path_unlink", 2, &mut self.import_ids);
        add_import("string_join", 3, &mut self.import_ids);
        add_import("string_split", 3, &mut self.import_ids);
        add_import("string_split_max", 5, &mut self.import_ids);
        add_import("string_lower", 2, &mut self.import_ids);
        add_import("string_upper", 2, &mut self.import_ids);
        add_import("string_capitalize", 2, &mut self.import_ids);
        add_import("string_strip", 3, &mut self.import_ids);
        add_import("bytes_split", 3, &mut self.import_ids);
        add_import("bytes_split_max", 5, &mut self.import_ids);
        add_import("bytearray_split", 3, &mut self.import_ids);
        add_import("bytearray_split_max", 5, &mut self.import_ids);
        add_import("string_replace", 7, &mut self.import_ids);
        add_import("bytes_replace", 7, &mut self.import_ids);
        add_import("bytearray_replace", 7, &mut self.import_ids);
        add_import("bytes_from_obj", 2, &mut self.import_ids);
        add_import("bytearray_from_obj", 2, &mut self.import_ids);
        add_import("bytes_from_str", 5, &mut self.import_ids);
        add_import("bytearray_from_str", 5, &mut self.import_ids);
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
        add_import("class_layout_version", 2, &mut self.import_ids);
        add_import("class_set_layout_version", 3, &mut self.import_ids);
        add_import("isinstance", 3, &mut self.import_ids);
        add_import("issubclass", 3, &mut self.import_ids);
        add_import("object_new", 0, &mut self.import_ids);
        add_import("func_new", 5, &mut self.import_ids);
        add_import("func_new_closure", 7, &mut self.import_ids);
        add_import("bound_method_new", 3, &mut self.import_ids);
        add_import("classmethod_new", 2, &mut self.import_ids);
        add_import("staticmethod_new", 2, &mut self.import_ids);
        add_import("property_new", 5, &mut self.import_ids);
        add_import("object_set_class", 16, &mut self.import_ids);
        add_import("stream_new", 2, &mut self.import_ids);
        add_import("stream_send", 18, &mut self.import_ids);
        add_import("stream_recv", 2, &mut self.import_ids);
        add_import("stream_close", 1, &mut self.import_ids);
        add_import("stream_drop", 1, &mut self.import_ids);
        add_import("ws_connect", 19, &mut self.import_ids);
        add_import("ws_pair", 20, &mut self.import_ids);
        add_import("ws_send", 18, &mut self.import_ids);
        add_import("ws_recv", 2, &mut self.import_ids);
        add_import("ws_close", 1, &mut self.import_ids);
        add_import("ws_drop", 1, &mut self.import_ids);
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
        add_import("exception_active", 0, &mut self.import_ids);
        add_import("exception_new", 3, &mut self.import_ids);
        add_import("exception_new_from_class", 3, &mut self.import_ids);
        add_import("exception_clear", 0, &mut self.import_ids);
        add_import("exception_pending", 0, &mut self.import_ids);
        add_import("exception_kind", 2, &mut self.import_ids);
        add_import("exception_class", 2, &mut self.import_ids);
        add_import("exception_message", 2, &mut self.import_ids);
        add_import("exception_set_cause", 3, &mut self.import_ids);
        add_import("exception_set_value", 3, &mut self.import_ids);
        add_import("exception_context_set", 2, &mut self.import_ids);
        add_import("raise", 2, &mut self.import_ids);
        add_import("bridge_unavailable", 2, &mut self.import_ids);
        add_import("db_query", 26, &mut self.import_ids);
        add_import("db_exec", 26, &mut self.import_ids);
        add_import("file_open", 3, &mut self.import_ids);
        add_import("file_read", 3, &mut self.import_ids);
        add_import("file_write", 3, &mut self.import_ids);
        add_import("file_close", 2, &mut self.import_ids);
        // TODO(wasm-parity, owner:wasm, milestone:SL1): extend wasm host imports
        // to cover full open() + file method parity (readline(s), seek/tell,
        // flush, truncate, attrs) beyond basic read/write/close.

        self.func_count = import_idx;
        let reloc_enabled = should_emit_relocs();

        let mut max_func_arity = 0usize;
        let mut max_call_arity = 0usize;
        let mut builtin_trampoline_specs: HashMap<String, usize> = HashMap::new();
        for func_ir in &ir.functions {
            let is_poll = func_ir.name.ends_with("_poll");
            if !is_poll {
                max_func_arity = max_func_arity.max(func_ir.params.len());
            }
            for op in &func_ir.ops {
                if !is_poll && op.kind == "call_func" {
                    if let Some(args) = &op.args {
                        if !args.is_empty() {
                            max_call_arity = max_call_arity.max(args.len() - 1);
                        }
                    }
                }
                if op.kind == "builtin_func" {
                    if let Some(name) = op.s_value.as_ref() {
                        let arity = op.value.unwrap_or(0) as usize;
                        if let Some(prev) = builtin_trampoline_specs.get(name) {
                            if *prev != arity {
                                panic!(
                                    "builtin trampoline arity mismatch for {name}: {prev} vs {arity}"
                                );
                            }
                        } else {
                            builtin_trampoline_specs.insert(name.clone(), arity);
                        }
                    }
                }
            }
        }

        let mut user_type_map: HashMap<usize, u32> = HashMap::new();
        user_type_map.insert(0, 0);
        user_type_map.insert(1, 2);
        user_type_map.insert(2, 3);
        user_type_map.insert(3, 5);
        user_type_map.insert(4, 7);
        user_type_map.insert(6, 9);
        user_type_map.insert(7, 10);
        // Types 0-28 are defined above; start new signatures after them.
        let mut next_type_idx = 29u32;
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

        let max_call_indirect = 13usize;
        let max_needed_arity = max_func_arity
            .max(max_call_arity.saturating_add(3))
            .max(max_call_indirect + 1);
        for arity in 0..=max_needed_arity {
            if let std::collections::hash_map::Entry::Vacant(entry) = user_type_map.entry(arity) {
                self.types.function(
                    std::iter::repeat_n(ValType::I64, arity),
                    std::iter::once(ValType::I64),
                );
                entry.insert(next_type_idx);
                next_type_idx += 1;
            }
        }

        for arity in 0..=max_call_indirect {
            let sig_idx = *user_type_map.get(&(arity + 1)).unwrap_or_else(|| {
                panic!("missing call_indirect signature for arity {}", arity + 1)
            });
            let callee_idx = *user_type_map
                .get(&arity)
                .unwrap_or_else(|| panic!("missing call_indirect callee type for arity {}", arity));
            self.funcs.function(sig_idx);
            let export_name = format!("molt_call_indirect{arity}");
            self.exports
                .export(&export_name, ExportKind::Func, self.func_count);
            let mut call_indirect = Function::new_with_locals_types(Vec::new());
            for idx in 0..arity {
                call_indirect.instruction(&Instruction::LocalGet((idx + 1) as u32));
            }
            call_indirect.instruction(&Instruction::LocalGet(0));
            call_indirect.instruction(&Instruction::I32WrapI64);
            emit_call_indirect(&mut call_indirect, reloc_enabled, callee_idx, 0);
            call_indirect.instruction(&Instruction::End);
            self.codes.function(&call_indirect);
            self.func_count += 1;
        }

        let sentinel_func_idx = self.func_count;
        self.funcs.function(2);
        let mut sentinel = Function::new_with_locals_types(Vec::new());
        sentinel.instruction(&Instruction::I64Const(0));
        sentinel.instruction(&Instruction::End);
        self.codes.function(&sentinel);
        self.func_count += 1;

        // Memory & Table (imported for shared-instance linking)
        let memory_ty = MemoryType {
            minimum: 18,
            maximum: None,
            memory64: false,
            shared: false,
        };
        self.imports
            .import("env", "memory", EntityType::Memory(memory_ty));
        self.exports.export("molt_memory", ExportKind::Memory, 0);

        let builtin_table_funcs: [(&str, &str, usize); 54] = [
            ("molt_missing", "missing", 0),
            ("molt_repr_builtin", "repr_builtin", 1),
            ("molt_format_builtin", "format_builtin", 2),
            ("molt_callable_builtin", "callable_builtin", 1),
            ("molt_round_builtin", "round_builtin", 2),
            ("molt_enumerate_builtin", "enumerate_builtin", 2),
            ("molt_iter_sentinel", "iter_sentinel", 2),
            ("molt_next_builtin", "next_builtin", 2),
            ("molt_any_builtin", "any_builtin", 1),
            ("molt_all_builtin", "all_builtin", 1),
            ("molt_sum_builtin", "sum_builtin", 2),
            ("molt_min_builtin", "min_builtin", 3),
            ("molt_max_builtin", "max_builtin", 3),
            ("molt_sorted_builtin", "sorted_builtin", 3),
            ("molt_map_builtin", "map_builtin", 2),
            ("molt_filter_builtin", "filter_builtin", 2),
            ("molt_zip_builtin", "zip_builtin", 1),
            ("molt_reversed_builtin", "reversed_builtin", 1),
            ("molt_getattr_builtin", "getattr_builtin", 3),
            ("molt_anext_builtin", "anext_builtin", 2),
            ("molt_print_builtin", "print_builtin", 5),
            ("molt_super_builtin", "super_builtin", 2),
            ("molt_set_attr_name", "set_attr_name", 3),
            ("molt_del_attr_name", "del_attr_name", 2),
            ("molt_has_attr_name", "has_attr_name", 2),
            ("molt_isinstance", "isinstance", 2),
            ("molt_issubclass", "issubclass", 2),
            ("molt_len", "len", 1),
            ("molt_id", "id", 1),
            ("molt_ord", "ord", 1),
            ("molt_chr", "chr", 1),
            ("molt_ascii_from_obj", "ascii_from_obj", 1),
            ("molt_bin_builtin", "bin_builtin", 1),
            ("molt_oct_builtin", "oct_builtin", 1),
            ("molt_hex_builtin", "hex_builtin", 1),
            ("molt_abs_builtin", "abs_builtin", 1),
            ("molt_divmod_builtin", "divmod_builtin", 2),
            ("molt_open_builtin", "open_builtin", 8),
            ("molt_getargv", "getargv", 0),
            ("molt_env_get", "env_get", 2),
            ("molt_getpid", "getpid", 0),
            ("molt_path_exists", "path_exists", 1),
            ("molt_path_unlink", "path_unlink", 1),
            ("molt_getrecursionlimit", "getrecursionlimit", 0),
            ("molt_setrecursionlimit", "setrecursionlimit", 1),
            ("molt_exception_last", "exception_last", 0),
            ("molt_exception_active", "exception_active", 0),
            ("molt_iter_checked", "iter", 1),
            ("molt_aiter", "aiter", 1),
            ("molt_heapq_heapify", "heapq_heapify", 1),
            ("molt_heapq_heappush", "heapq_heappush", 2),
            ("molt_heapq_heappop", "heapq_heappop", 1),
            ("molt_heapq_heapreplace", "heapq_heapreplace", 2),
            ("molt_heapq_heappushpop", "heapq_heappushpop", 2),
        ];
        let mut builtin_trampoline_funcs: Vec<(String, usize)> = Vec::new();
        for (runtime_name, _, _) in builtin_table_funcs.iter() {
            if let Some(arity) = builtin_trampoline_specs.get(*runtime_name) {
                builtin_trampoline_funcs.push(((*runtime_name).to_string(), *arity));
            }
        }
        let mut builtin_wrapper_funcs: Vec<(String, &str, usize)> = Vec::new();
        let wrap_all_builtins = reloc_enabled;
        for (runtime_name, import_name, arity) in builtin_table_funcs.iter() {
            if wrap_all_builtins || builtin_trampoline_specs.contains_key(*runtime_name) {
                builtin_wrapper_funcs.push(((*runtime_name).to_string(), *import_name, *arity));
            }
        }
        if builtin_trampoline_specs.len() != builtin_trampoline_funcs.len() {
            for name in builtin_trampoline_specs.keys() {
                if !builtin_table_funcs
                    .iter()
                    .any(|(entry, _, _)| entry == name)
                {
                    panic!("builtin {name} missing from wasm table");
                }
            }
        }
        let table_base: u32 = if reloc_enabled {
            table_base_for_reloc()
        } else {
            256
        };
        let table_len = (3
            + builtin_table_funcs.len()
            + builtin_trampoline_funcs.len()
            + ir.functions.len() * 2) as u32;
        let table_min = table_base + table_len;
        let table_ty = TableType {
            element_type: RefType::FUNCREF,
            minimum: table_min,
            maximum: None,
        };
        self.imports.import(
            "env",
            "__indirect_function_table",
            EntityType::Table(table_ty),
        );
        self.exports.export("molt_table", ExportKind::Table, 0);

        let mut builtin_wrapper_indices = HashMap::new();
        for (runtime_name, import_name, arity) in &builtin_wrapper_funcs {
            let type_idx = *user_type_map
                .get(arity)
                .unwrap_or_else(|| panic!("missing builtin wrapper signature for arity {arity}"));
            let import_idx = *self
                .import_ids
                .get(*import_name)
                .unwrap_or_else(|| panic!("missing builtin import for {import_name}"));
            self.funcs.function(type_idx);
            let func_index = self.func_count;
            self.func_count += 1;
            let mut func = Function::new_with_locals_types(Vec::new());
            for idx in 0..*arity {
                func.instruction(&Instruction::LocalGet(idx as u32));
            }
            emit_call(&mut func, reloc_enabled, import_idx);
            func.instruction(&Instruction::End);
            self.codes.function(&func);
            builtin_wrapper_indices.insert(runtime_name.clone(), func_index);
        }

        let mut table_import_wrappers = HashMap::new();
        if reloc_enabled {
            for (import_name, arity) in [("async_sleep", 1usize), ("anext_default_poll", 1usize)] {
                let type_idx = *user_type_map
                    .get(&arity)
                    .unwrap_or_else(|| panic!("missing wrapper signature for arity {arity}"));
                let import_idx = *self
                    .import_ids
                    .get(import_name)
                    .unwrap_or_else(|| panic!("missing import for {import_name}"));
                self.funcs.function(type_idx);
                let func_index = self.func_count;
                self.func_count += 1;
                let mut func = Function::new_with_locals_types(Vec::new());
                for idx in 0..arity {
                    func.instruction(&Instruction::LocalGet(idx as u32));
                }
                emit_call(&mut func, reloc_enabled, import_idx);
                func.instruction(&Instruction::End);
                self.codes.function(&func);
                table_import_wrappers.insert(import_name.to_string(), func_index);
            }
        }

        // Function indices for table
        let async_sleep_idx = *table_import_wrappers
            .get("async_sleep")
            .unwrap_or(&self.import_ids["async_sleep"]);
        let anext_default_poll_idx = *table_import_wrappers
            .get("anext_default_poll")
            .unwrap_or(&self.import_ids["anext_default_poll"]);
        let mut table_indices = vec![sentinel_func_idx, async_sleep_idx, anext_default_poll_idx];
        let mut func_to_table_idx = HashMap::new();
        let mut func_to_index = HashMap::new();
        func_to_index.insert(
            "molt_runtime_init".to_string(),
            self.import_ids["runtime_init"],
        );
        func_to_index.insert(
            "molt_runtime_shutdown".to_string(),
            self.import_ids["runtime_shutdown"],
        );
        func_to_table_idx.insert("molt_async_sleep".to_string(), 1);
        func_to_table_idx.insert("molt_anext_default_poll".to_string(), 2);

        for (offset, (runtime_name, import_name, _)) in builtin_table_funcs.iter().enumerate() {
            let idx = (offset + 3) as u32;
            let runtime_key = (*runtime_name).to_string();
            func_to_table_idx.insert(runtime_key.clone(), idx);
            if let Some(wrapper_idx) = builtin_wrapper_indices.get(&runtime_key) {
                func_to_index.insert(runtime_key, *wrapper_idx);
                table_indices.push(*wrapper_idx);
            } else {
                func_to_index.insert(runtime_key, self.import_ids[*import_name]);
                table_indices.push(self.import_ids[*import_name]);
            }
        }

        let user_func_start = self.func_count;
        let user_func_count = ir.functions.len() as u32;
        let builtin_trampoline_count = builtin_trampoline_funcs.len() as u32;
        let builtin_trampoline_start = user_func_start + user_func_count;
        let user_trampoline_start = builtin_trampoline_start + builtin_trampoline_count;
        for (i, func_ir) in ir.functions.iter().enumerate() {
            let idx = (i + 3 + builtin_table_funcs.len()) as u32;
            func_to_table_idx.insert(func_ir.name.clone(), idx);
            func_to_index.insert(func_ir.name.clone(), user_func_start + i as u32);
            table_indices.push(user_func_start + i as u32);
        }
        let mut func_to_trampoline_idx = HashMap::new();
        for (i, (name, _)) in builtin_trampoline_funcs.iter().enumerate() {
            let idx = (i + 3 + builtin_table_funcs.len() + ir.functions.len()) as u32;
            func_to_trampoline_idx.insert(name.clone(), idx);
            table_indices.push(builtin_trampoline_start + i as u32);
        }
        for (i, func_ir) in ir.functions.iter().enumerate() {
            let idx = (i
                + 3
                + builtin_table_funcs.len()
                + ir.functions.len()
                + builtin_trampoline_funcs.len()) as u32;
            func_to_trampoline_idx.insert(func_ir.name.clone(), idx);
            table_indices.push(user_trampoline_start + i as u32);
        }

        let import_ids = self.import_ids.clone();
        let compile_ctx = CompileFuncContext {
            func_map: &func_to_table_idx,
            func_indices: &func_to_index,
            trampoline_map: &func_to_trampoline_idx,
            import_ids: &import_ids,
            reloc_enabled,
            table_base,
        };
        for func_ir in &ir.functions {
            let type_idx = if func_ir.name.ends_with("_poll") {
                2
            } else {
                *user_type_map.get(&func_ir.params.len()).unwrap_or(&0)
            };
            self.compile_func(func_ir, type_idx, &compile_ctx);
        }

        if self.func_count != builtin_trampoline_start {
            panic!(
                "wasm builtin trampoline index mismatch: expected {builtin_trampoline_start}, got {}",
                self.func_count
            );
        }
        for (name, arity) in &builtin_trampoline_funcs {
            let target_idx = *func_to_index
                .get(name)
                .unwrap_or_else(|| panic!("builtin trampoline target missing for {name}"));
            let table_slot = *func_to_table_idx
                .get(name)
                .unwrap_or_else(|| panic!("builtin trampoline table slot missing for {name}"));
            let table_idx = table_base + table_slot;
            self.compile_trampoline(
                reloc_enabled,
                target_idx,
                table_idx,
                TrampolineSpec {
                    arity: *arity,
                    has_closure: false,
                    is_generator: false,
                    closure_size: 0,
                },
            );
        }
        if self.func_count != user_trampoline_start {
            panic!(
                "wasm user trampoline index mismatch: expected {user_trampoline_start}, got {}",
                self.func_count
            );
        }
        for func_ir in &ir.functions {
            let default_has_closure = func_ir
                .params
                .first()
                .is_some_and(|name| name == "__molt_closure__");
            let mut default_arity = func_ir.params.len();
            if default_has_closure && default_arity > 0 {
                default_arity = default_arity.saturating_sub(1);
            }
            let (arity, has_closure) = match func_trampoline_spec.get(&func_ir.name).copied() {
                Some(spec) => spec,
                None => (default_arity, default_has_closure),
            };
            let target_idx = *func_to_index
                .get(&func_ir.name)
                .expect("trampoline target missing");
            let table_slot = *func_to_table_idx
                .get(&func_ir.name)
                .expect("trampoline table slot missing");
            let table_idx = table_base + table_slot;
            let is_generator = generator_funcs.contains(&func_ir.name);
            let closure_size = if is_generator {
                *generator_closure_sizes
                    .get(&func_ir.name)
                    .unwrap_or_else(|| {
                        panic!("generator closure size missing for {}", func_ir.name)
                    })
            } else {
                0
            };
            self.compile_trampoline(
                reloc_enabled,
                target_idx,
                table_idx,
                TrampolineSpec {
                    arity,
                    has_closure,
                    is_generator,
                    closure_size,
                },
            );
        }

        let mut element_section = None;
        let mut element_payload = None;
        if reloc_enabled {
            let table_init_index =
                self.compile_table_init(reloc_enabled, table_base, &table_indices);
            self.exports
                .export("molt_table_init", ExportKind::Func, table_init_index);
            let main_index = self
                .molt_main_index
                .unwrap_or_else(|| panic!("molt_main missing for table init wrapper"));
            let wrapper_index =
                self.compile_molt_main_wrapper(reloc_enabled, main_index, table_init_index);
            self.exports
                .export("molt_main", ExportKind::Func, wrapper_index);

            let mut ref_exported = HashSet::new();
            for func_index in &table_indices {
                if ref_exported.insert(*func_index) {
                    let name = format!("__molt_table_ref_{func_index}");
                    self.exports.export(&name, ExportKind::Func, *func_index);
                }
            }

            let mut payload = Vec::new();
            1u32.encode(&mut payload);
            payload.push(0x01);
            payload.push(0x00);
            (table_indices.len() as u32).encode(&mut payload);
            for func_index in &table_indices {
                encode_u32_leb128_padded(*func_index, &mut payload);
            }
            element_payload = Some(payload);
        } else {
            let mut section = ElementSection::new();
            let offset = ConstExpr::i32_const(table_base as i32);
            section.segment(ElementSegment {
                mode: ElementMode::Active {
                    table: None,
                    offset: &offset,
                },
                elements: Elements::Functions(&table_indices),
            });
            element_section = Some(section);
        }

        self.module.section(&self.types);
        self.module.section(&self.imports);
        self.module.section(&self.funcs);
        self.module.section(&self.tables);
        self.module.section(&self.memories);
        self.module.section(&self.exports);
        if let Some(element_section) = element_section.as_ref() {
            self.module.section(element_section);
        }
        if let Some(payload) = element_payload.as_ref() {
            let raw_section = RawSection {
                id: 9,
                data: payload,
            };
            self.module.section(&raw_section);
        }
        self.module.section(&self.codes);
        self.module.section(&self.data);
        let mut bytes = self.module.finish();
        if reloc_enabled {
            bytes = add_reloc_sections(bytes, &self.data_segments, &self.data_relocs);
        }
        bytes
    }

    fn compile_trampoline(
        &mut self,
        reloc_enabled: bool,
        target_func_index: u32,
        table_idx: u32,
        spec: TrampolineSpec,
    ) {
        let TrampolineSpec {
            arity,
            has_closure,
            is_generator,
            closure_size,
        } = spec;
        self.funcs.function(5);
        self.func_count += 1;
        let mut local_types = Vec::new();
        if is_generator {
            local_types.push(ValType::I64);
            local_types.push(ValType::I32);
            local_types.push(ValType::I64);
        }
        let mut func = Function::new_with_locals_types(local_types);
        if is_generator {
            if closure_size < 0 {
                panic!("generator closure size must be non-negative");
            }
            let payload_slots = arity + usize::from(has_closure);
            let needed = GEN_CONTROL_SIZE as i64 + (payload_slots as i64) * 8;
            if closure_size < needed {
                panic!("generator closure size too small for trampoline");
            }
            let gen_local = 3;
            let base_local = 4;
            let val_local = 5;
            emit_table_index_i64(&mut func, reloc_enabled, table_idx);
            func.instruction(&Instruction::I64Const(closure_size));
            func.instruction(&Instruction::I64Const(TASK_KIND_GENERATOR));
            emit_call(&mut func, reloc_enabled, self.import_ids["task_new"]);
            func.instruction(&Instruction::LocalSet(gen_local));
            if payload_slots > 0 {
                func.instruction(&Instruction::LocalGet(gen_local));
                emit_call(&mut func, reloc_enabled, self.import_ids["handle_resolve"]);
                func.instruction(&Instruction::LocalSet(base_local));
                let mut offset = GEN_CONTROL_SIZE;
                if has_closure {
                    func.instruction(&Instruction::LocalGet(base_local));
                    func.instruction(&Instruction::I32Const(offset));
                    func.instruction(&Instruction::I32Add);
                    func.instruction(&Instruction::LocalGet(0));
                    func.instruction(&Instruction::I64Store(wasm_encoder::MemArg {
                        align: 3,
                        offset: 0,
                        memory_index: 0,
                    }));
                    func.instruction(&Instruction::LocalGet(0));
                    emit_call(&mut func, reloc_enabled, self.import_ids["inc_ref_obj"]);
                    offset += 8;
                }
                for idx in 0..arity {
                    let arg_offset = offset + (idx as i32) * 8;
                    func.instruction(&Instruction::LocalGet(1));
                    func.instruction(&Instruction::I32WrapI64);
                    func.instruction(&Instruction::I64Load(wasm_encoder::MemArg {
                        align: 3,
                        offset: (idx * std::mem::size_of::<u64>()) as u64,
                        memory_index: 0,
                    }));
                    func.instruction(&Instruction::LocalSet(val_local));
                    func.instruction(&Instruction::LocalGet(base_local));
                    func.instruction(&Instruction::I32Const(arg_offset));
                    func.instruction(&Instruction::I32Add);
                    func.instruction(&Instruction::LocalGet(val_local));
                    func.instruction(&Instruction::I64Store(wasm_encoder::MemArg {
                        align: 3,
                        offset: 0,
                        memory_index: 0,
                    }));
                    func.instruction(&Instruction::LocalGet(val_local));
                    emit_call(&mut func, reloc_enabled, self.import_ids["inc_ref_obj"]);
                }
            }
            func.instruction(&Instruction::LocalGet(gen_local));
            func.instruction(&Instruction::End);
            self.codes.function(&func);
            return;
        }
        if has_closure {
            func.instruction(&Instruction::LocalGet(0));
        }
        for idx in 0..arity {
            func.instruction(&Instruction::LocalGet(1));
            func.instruction(&Instruction::I32WrapI64);
            func.instruction(&Instruction::I64Load(wasm_encoder::MemArg {
                align: 3,
                offset: (idx * std::mem::size_of::<u64>()) as u64,
                memory_index: 0,
            }));
        }
        emit_call(&mut func, reloc_enabled, target_func_index);
        func.instruction(&Instruction::End);
        self.codes.function(&func);
    }

    fn compile_table_init(
        &mut self,
        reloc_enabled: bool,
        table_base: u32,
        table_indices: &[u32],
    ) -> u32 {
        let func_index = self.func_count;
        self.funcs.function(8);
        self.func_count += 1;
        let mut func = Function::new_with_locals_types(Vec::new());
        for (slot, target_index) in table_indices.iter().enumerate() {
            let table_index = table_base + slot as u32;
            emit_i32_const(&mut func, reloc_enabled, table_index as i32);
            emit_ref_func(&mut func, reloc_enabled, *target_index);
            func.instruction(&Instruction::TableSet(0));
        }
        func.instruction(&Instruction::End);
        self.codes.function(&func);
        func_index
    }

    fn compile_molt_main_wrapper(
        &mut self,
        reloc_enabled: bool,
        main_index: u32,
        table_init_index: u32,
    ) -> u32 {
        let func_index = self.func_count;
        self.funcs.function(0);
        self.func_count += 1;
        let mut func = Function::new_with_locals_types(Vec::new());
        emit_call(&mut func, reloc_enabled, table_init_index);
        emit_call(&mut func, reloc_enabled, main_index);
        func.instruction(&Instruction::End);
        self.codes.function(&func);
        func_index
    }

    fn compile_func(&mut self, func_ir: &FunctionIR, type_idx: u32, ctx: &CompileFuncContext<'_>) {
        let func_index = self.func_count;
        let reloc_enabled = ctx.reloc_enabled;
        self.funcs.function(type_idx);
        if reloc_enabled && func_ir.name == "molt_main" {
            self.molt_main_index = Some(func_index);
        } else {
            self.exports
                .export(&func_ir.name, ExportKind::Func, self.func_count);
        }
        self.func_count += 1;
        let func_map = ctx.func_map;
        let func_indices = ctx.func_indices;
        let trampoline_map = ctx.trampoline_map;
        let table_base = ctx.table_base;
        let import_ids = ctx.import_ids;
        let mut locals = HashMap::new();
        let mut local_count = 0;
        let mut local_types = Vec::new();

        for (idx, name) in func_ir.params.iter().enumerate() {
            locals.insert(name.clone(), idx as u32);
            local_count += 1;
        }

        if func_ir.name.ends_with("_poll") {
            let self_param_idx = locals.get("self").copied().unwrap_or(0);
            locals.insert("self_param".to_string(), self_param_idx);
            let self_idx = locals.get("self").copied();
            if self_idx.is_none() || self_idx == Some(self_param_idx) {
                locals.insert("self".to_string(), local_count);
                local_types.push(ValType::I64);
                local_count += 1;
            }
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
                if op.kind == "const_str" || op.kind == "const_bytes" || op.kind == "const_bigint" {
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

        let needs_field_fast = func_ir.ops.iter().any(|op| {
            op.kind == "store"
                || op.kind == "store_init"
                || op.kind == "load"
                || op.kind == "guarded_load"
                || op.kind == "guarded_field_get"
                || op.kind == "guarded_field_set"
                || op.kind == "guarded_field_init"
        });
        if needs_field_fast {
            // TODO(optimization, owner:backend, milestone:WASM1, priority:P2): use i32
            // locals for wasm pointer temporaries to reduce wrap/extend churn.
            for name in ["__wasm_tmp0", "__wasm_tmp1"] {
                if let std::collections::hash_map::Entry::Vacant(entry) =
                    locals.entry(name.to_string())
                {
                    entry.insert(local_count);
                    local_types.push(ValType::I64);
                    local_count += 1;
                }
            }
        }

        for name in ["__molt_tmp0", "__molt_tmp1", "__molt_tmp2", "__molt_tmp3"] {
            if let std::collections::hash_map::Entry::Vacant(entry) = locals.entry(name.to_string())
            {
                entry.insert(local_count);
                local_types.push(ValType::I64);
                local_count += 1;
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
        let jumpful = !stateful
            && func_ir
                .ops
                .iter()
                .any(|op| matches!(op.kind.as_str(), "jump" | "label"));
        if stateful && !locals.contains_key("self_param") {
            let self_param_idx = locals
                .get("self")
                .copied()
                .or_else(|| {
                    func_ir
                        .params
                        .first()
                        .and_then(|name| locals.get(name))
                        .copied()
                })
                .unwrap_or_else(|| {
                    panic!(
                        "stateful wasm function {} missing self parameter",
                        func_ir.name
                    )
                });
            locals.insert("self_param".to_string(), self_param_idx);
            locals.entry("self".to_string()).or_insert(self_param_idx);
        }
        let self_ptr_local = if stateful {
            let idx = local_count;
            local_types.push(ValType::I64);
            local_count += 1;
            Some(idx)
        } else {
            None
        };
        let state_local = if stateful || jumpful {
            let idx = local_count;
            local_types.push(ValType::I64);
            local_count += 1;
            Some(idx)
        } else {
            None
        };
        let _ = local_count;
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
        let mut label_stack: Vec<i64> = Vec::new();
        let mut label_depths: HashMap<i64, usize> = HashMap::new();

        let mut emit_ops = |func: &mut Function,
                            ops: &[OpIR],
                            control_stack: &mut Vec<ControlKind>,
                            try_stack: &mut Vec<usize>,
                            label_stack: &mut Vec<i64>,
                            label_depths: &mut HashMap<i64, usize>| {
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
                    "const_not_implemented" => {
                        emit_call(func, reloc_enabled, import_ids["not_implemented"]);
                        let local_idx = locals[op.out.as_ref().unwrap()];
                        func.instruction(&Instruction::LocalSet(local_idx));
                    }
                    "const_str" => {
                        let s = op.s_value.as_ref().unwrap();
                        let out_name = op.out.as_ref().unwrap();
                        let bytes = s.as_bytes();
                        let data = self.add_data_segment(reloc_enabled, bytes);

                        let ptr_local = locals[&format!("{out_name}_ptr")];
                        let len_local = locals[&format!("{out_name}_len")];
                        self.emit_data_ptr(reloc_enabled, func_index, func, data);
                        func.instruction(&Instruction::LocalSet(ptr_local));
                        func.instruction(&Instruction::I64Const(bytes.len() as i64));
                        func.instruction(&Instruction::LocalSet(len_local));

                        func.instruction(&Instruction::I64Const(8));
                        emit_call(func, reloc_enabled, import_ids["alloc"]);
                        let out_local = locals[out_name];
                        func.instruction(&Instruction::LocalSet(out_local));

                        func.instruction(&Instruction::LocalGet(ptr_local));
                        func.instruction(&Instruction::I32WrapI64);
                        func.instruction(&Instruction::LocalGet(len_local));
                        func.instruction(&Instruction::LocalGet(out_local));
                        emit_call(func, reloc_enabled, import_ids["handle_resolve"]);
                        emit_call(func, reloc_enabled, import_ids["string_from_bytes"]);
                        func.instruction(&Instruction::Drop);

                        func.instruction(&Instruction::LocalGet(out_local));
                        emit_call(func, reloc_enabled, import_ids["handle_resolve"]);
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
                        let data = self.add_data_segment(reloc_enabled, bytes);

                        let ptr_local = locals[&format!("{out_name}_ptr")];
                        let len_local = locals[&format!("{out_name}_len")];
                        self.emit_data_ptr(reloc_enabled, func_index, func, data);
                        func.instruction(&Instruction::LocalSet(ptr_local));
                        func.instruction(&Instruction::I64Const(bytes.len() as i64));
                        func.instruction(&Instruction::LocalSet(len_local));

                        func.instruction(&Instruction::LocalGet(ptr_local));
                        func.instruction(&Instruction::I32WrapI64);
                        func.instruction(&Instruction::LocalGet(len_local));
                        emit_call(func, reloc_enabled, import_ids["bigint_from_str"]);
                        let out_local = locals[out_name];
                        func.instruction(&Instruction::LocalSet(out_local));
                    }
                    "const_bytes" => {
                        let bytes = op.bytes.as_ref().expect("Bytes not found");
                        let out_name = op.out.as_ref().unwrap();
                        let data = self.add_data_segment(reloc_enabled, bytes);

                        let ptr_local = locals[&format!("{out_name}_ptr")];
                        let len_local = locals[&format!("{out_name}_len")];
                        self.emit_data_ptr(reloc_enabled, func_index, func, data);
                        func.instruction(&Instruction::LocalSet(ptr_local));
                        func.instruction(&Instruction::I64Const(bytes.len() as i64));
                        func.instruction(&Instruction::LocalSet(len_local));

                        func.instruction(&Instruction::I64Const(8));
                        emit_call(func, reloc_enabled, import_ids["alloc"]);
                        let out_local = locals[out_name];
                        func.instruction(&Instruction::LocalSet(out_local));

                        func.instruction(&Instruction::LocalGet(ptr_local));
                        func.instruction(&Instruction::I32WrapI64);
                        func.instruction(&Instruction::LocalGet(len_local));
                        func.instruction(&Instruction::LocalGet(out_local));
                        emit_call(func, reloc_enabled, import_ids["handle_resolve"]);
                        emit_call(func, reloc_enabled, import_ids["bytes_from_bytes"]);
                        func.instruction(&Instruction::Drop);

                        func.instruction(&Instruction::LocalGet(out_local));
                        emit_call(func, reloc_enabled, import_ids["handle_resolve"]);
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
                        emit_call(func, reloc_enabled, import_ids["add"]);
                        let res = locals[op.out.as_ref().unwrap()];
                        func.instruction(&Instruction::LocalSet(res));
                    }
                    "inplace_add" => {
                        let args = op.args.as_ref().unwrap();
                        let lhs = locals[&args[0]];
                        let rhs = locals[&args[1]];
                        func.instruction(&Instruction::LocalGet(lhs));
                        func.instruction(&Instruction::LocalGet(rhs));
                        emit_call(func, reloc_enabled, import_ids["inplace_add"]);
                        let res = locals[op.out.as_ref().unwrap()];
                        func.instruction(&Instruction::LocalSet(res));
                    }
                    "vec_sum_int" => {
                        let args = op.args.as_ref().unwrap();
                        let seq = locals[&args[0]];
                        let acc = locals[&args[1]];
                        func.instruction(&Instruction::LocalGet(seq));
                        func.instruction(&Instruction::LocalGet(acc));
                        emit_call(func, reloc_enabled, import_ids["vec_sum_int"]);
                        let res = locals[op.out.as_ref().unwrap()];
                        func.instruction(&Instruction::LocalSet(res));
                    }
                    "vec_sum_int_trusted" => {
                        let args = op.args.as_ref().unwrap();
                        let seq = locals[&args[0]];
                        let acc = locals[&args[1]];
                        func.instruction(&Instruction::LocalGet(seq));
                        func.instruction(&Instruction::LocalGet(acc));
                        emit_call(func, reloc_enabled, import_ids["vec_sum_int_trusted"]);
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
                        emit_call(func, reloc_enabled, import_ids["vec_sum_int_range"]);
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
                        emit_call(func, reloc_enabled, import_ids["vec_sum_int_range_trusted"]);
                        let res = locals[op.out.as_ref().unwrap()];
                        func.instruction(&Instruction::LocalSet(res));
                    }
                    "vec_prod_int" => {
                        let args = op.args.as_ref().unwrap();
                        let seq = locals[&args[0]];
                        let acc = locals[&args[1]];
                        func.instruction(&Instruction::LocalGet(seq));
                        func.instruction(&Instruction::LocalGet(acc));
                        emit_call(func, reloc_enabled, import_ids["vec_prod_int"]);
                        let res = locals[op.out.as_ref().unwrap()];
                        func.instruction(&Instruction::LocalSet(res));
                    }
                    "vec_prod_int_trusted" => {
                        let args = op.args.as_ref().unwrap();
                        let seq = locals[&args[0]];
                        let acc = locals[&args[1]];
                        func.instruction(&Instruction::LocalGet(seq));
                        func.instruction(&Instruction::LocalGet(acc));
                        emit_call(func, reloc_enabled, import_ids["vec_prod_int_trusted"]);
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
                        emit_call(func, reloc_enabled, import_ids["vec_prod_int_range"]);
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
                        emit_call(
                            func,
                            reloc_enabled,
                            import_ids["vec_prod_int_range_trusted"],
                        );
                        let res = locals[op.out.as_ref().unwrap()];
                        func.instruction(&Instruction::LocalSet(res));
                    }
                    "vec_min_int" => {
                        let args = op.args.as_ref().unwrap();
                        let seq = locals[&args[0]];
                        let acc = locals[&args[1]];
                        func.instruction(&Instruction::LocalGet(seq));
                        func.instruction(&Instruction::LocalGet(acc));
                        emit_call(func, reloc_enabled, import_ids["vec_min_int"]);
                        let res = locals[op.out.as_ref().unwrap()];
                        func.instruction(&Instruction::LocalSet(res));
                    }
                    "vec_min_int_trusted" => {
                        let args = op.args.as_ref().unwrap();
                        let seq = locals[&args[0]];
                        let acc = locals[&args[1]];
                        func.instruction(&Instruction::LocalGet(seq));
                        func.instruction(&Instruction::LocalGet(acc));
                        emit_call(func, reloc_enabled, import_ids["vec_min_int_trusted"]);
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
                        emit_call(func, reloc_enabled, import_ids["vec_min_int_range"]);
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
                        emit_call(func, reloc_enabled, import_ids["vec_min_int_range_trusted"]);
                        let res = locals[op.out.as_ref().unwrap()];
                        func.instruction(&Instruction::LocalSet(res));
                    }
                    "vec_max_int" => {
                        let args = op.args.as_ref().unwrap();
                        let seq = locals[&args[0]];
                        let acc = locals[&args[1]];
                        func.instruction(&Instruction::LocalGet(seq));
                        func.instruction(&Instruction::LocalGet(acc));
                        emit_call(func, reloc_enabled, import_ids["vec_max_int"]);
                        let res = locals[op.out.as_ref().unwrap()];
                        func.instruction(&Instruction::LocalSet(res));
                    }
                    "vec_max_int_trusted" => {
                        let args = op.args.as_ref().unwrap();
                        let seq = locals[&args[0]];
                        let acc = locals[&args[1]];
                        func.instruction(&Instruction::LocalGet(seq));
                        func.instruction(&Instruction::LocalGet(acc));
                        emit_call(func, reloc_enabled, import_ids["vec_max_int_trusted"]);
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
                        emit_call(func, reloc_enabled, import_ids["vec_max_int_range"]);
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
                        emit_call(func, reloc_enabled, import_ids["vec_max_int_range_trusted"]);
                        let res = locals[op.out.as_ref().unwrap()];
                        func.instruction(&Instruction::LocalSet(res));
                    }
                    "sub" => {
                        let args = op.args.as_ref().unwrap();
                        let lhs = locals[&args[0]];
                        let rhs = locals[&args[1]];
                        func.instruction(&Instruction::LocalGet(lhs));
                        func.instruction(&Instruction::LocalGet(rhs));
                        emit_call(func, reloc_enabled, import_ids["sub"]);
                        let res = locals[op.out.as_ref().unwrap()];
                        func.instruction(&Instruction::LocalSet(res));
                    }
                    "mul" => {
                        let args = op.args.as_ref().unwrap();
                        let lhs = locals[&args[0]];
                        let rhs = locals[&args[1]];
                        func.instruction(&Instruction::LocalGet(lhs));
                        func.instruction(&Instruction::LocalGet(rhs));
                        emit_call(func, reloc_enabled, import_ids["mul"]);
                        let res = locals[op.out.as_ref().unwrap()];
                        func.instruction(&Instruction::LocalSet(res));
                    }
                    "inplace_sub" => {
                        let args = op.args.as_ref().unwrap();
                        let lhs = locals[&args[0]];
                        let rhs = locals[&args[1]];
                        func.instruction(&Instruction::LocalGet(lhs));
                        func.instruction(&Instruction::LocalGet(rhs));
                        emit_call(func, reloc_enabled, import_ids["inplace_sub"]);
                        let res = locals[op.out.as_ref().unwrap()];
                        func.instruction(&Instruction::LocalSet(res));
                    }
                    "inplace_mul" => {
                        let args = op.args.as_ref().unwrap();
                        let lhs = locals[&args[0]];
                        let rhs = locals[&args[1]];
                        func.instruction(&Instruction::LocalGet(lhs));
                        func.instruction(&Instruction::LocalGet(rhs));
                        emit_call(func, reloc_enabled, import_ids["inplace_mul"]);
                        let res = locals[op.out.as_ref().unwrap()];
                        func.instruction(&Instruction::LocalSet(res));
                    }
                    "bit_or" => {
                        let args = op.args.as_ref().unwrap();
                        let lhs = locals[&args[0]];
                        let rhs = locals[&args[1]];
                        func.instruction(&Instruction::LocalGet(lhs));
                        func.instruction(&Instruction::LocalGet(rhs));
                        emit_call(func, reloc_enabled, import_ids["bit_or"]);
                        let res = locals[op.out.as_ref().unwrap()];
                        func.instruction(&Instruction::LocalSet(res));
                    }
                    "bit_and" => {
                        let args = op.args.as_ref().unwrap();
                        let lhs = locals[&args[0]];
                        let rhs = locals[&args[1]];
                        func.instruction(&Instruction::LocalGet(lhs));
                        func.instruction(&Instruction::LocalGet(rhs));
                        emit_call(func, reloc_enabled, import_ids["bit_and"]);
                        let res = locals[op.out.as_ref().unwrap()];
                        func.instruction(&Instruction::LocalSet(res));
                    }
                    "bit_xor" => {
                        let args = op.args.as_ref().unwrap();
                        let lhs = locals[&args[0]];
                        let rhs = locals[&args[1]];
                        func.instruction(&Instruction::LocalGet(lhs));
                        func.instruction(&Instruction::LocalGet(rhs));
                        emit_call(func, reloc_enabled, import_ids["bit_xor"]);
                        let res = locals[op.out.as_ref().unwrap()];
                        func.instruction(&Instruction::LocalSet(res));
                    }
                    "invert" => {
                        let args = op.args.as_ref().unwrap();
                        let val = locals[&args[0]];
                        func.instruction(&Instruction::LocalGet(val));
                        emit_call(func, reloc_enabled, import_ids["invert"]);
                        let res = locals[op.out.as_ref().unwrap()];
                        func.instruction(&Instruction::LocalSet(res));
                    }
                    "inplace_bit_or" => {
                        let args = op.args.as_ref().unwrap();
                        let lhs = locals[&args[0]];
                        let rhs = locals[&args[1]];
                        func.instruction(&Instruction::LocalGet(lhs));
                        func.instruction(&Instruction::LocalGet(rhs));
                        emit_call(func, reloc_enabled, import_ids["inplace_bit_or"]);
                        let res = locals[op.out.as_ref().unwrap()];
                        func.instruction(&Instruction::LocalSet(res));
                    }
                    "inplace_bit_and" => {
                        let args = op.args.as_ref().unwrap();
                        let lhs = locals[&args[0]];
                        let rhs = locals[&args[1]];
                        func.instruction(&Instruction::LocalGet(lhs));
                        func.instruction(&Instruction::LocalGet(rhs));
                        emit_call(func, reloc_enabled, import_ids["inplace_bit_and"]);
                        let res = locals[op.out.as_ref().unwrap()];
                        func.instruction(&Instruction::LocalSet(res));
                    }
                    "inplace_bit_xor" => {
                        let args = op.args.as_ref().unwrap();
                        let lhs = locals[&args[0]];
                        let rhs = locals[&args[1]];
                        func.instruction(&Instruction::LocalGet(lhs));
                        func.instruction(&Instruction::LocalGet(rhs));
                        emit_call(func, reloc_enabled, import_ids["inplace_bit_xor"]);
                        let res = locals[op.out.as_ref().unwrap()];
                        func.instruction(&Instruction::LocalSet(res));
                    }
                    "lshift" => {
                        let args = op.args.as_ref().unwrap();
                        let lhs = locals[&args[0]];
                        let rhs = locals[&args[1]];
                        func.instruction(&Instruction::LocalGet(lhs));
                        func.instruction(&Instruction::LocalGet(rhs));
                        emit_call(func, reloc_enabled, import_ids["lshift"]);
                        let res = locals[op.out.as_ref().unwrap()];
                        func.instruction(&Instruction::LocalSet(res));
                    }
                    "rshift" => {
                        let args = op.args.as_ref().unwrap();
                        let lhs = locals[&args[0]];
                        let rhs = locals[&args[1]];
                        func.instruction(&Instruction::LocalGet(lhs));
                        func.instruction(&Instruction::LocalGet(rhs));
                        emit_call(func, reloc_enabled, import_ids["rshift"]);
                        let res = locals[op.out.as_ref().unwrap()];
                        func.instruction(&Instruction::LocalSet(res));
                    }
                    "matmul" => {
                        let args = op.args.as_ref().unwrap();
                        let lhs = locals[&args[0]];
                        let rhs = locals[&args[1]];
                        func.instruction(&Instruction::LocalGet(lhs));
                        func.instruction(&Instruction::LocalGet(rhs));
                        emit_call(func, reloc_enabled, import_ids["matmul"]);
                        let res = locals[op.out.as_ref().unwrap()];
                        func.instruction(&Instruction::LocalSet(res));
                    }
                    "div" => {
                        let args = op.args.as_ref().unwrap();
                        let lhs = locals[&args[0]];
                        let rhs = locals[&args[1]];
                        func.instruction(&Instruction::LocalGet(lhs));
                        func.instruction(&Instruction::LocalGet(rhs));
                        emit_call(func, reloc_enabled, import_ids["div"]);
                        let res = locals[op.out.as_ref().unwrap()];
                        func.instruction(&Instruction::LocalSet(res));
                    }
                    "floordiv" => {
                        let args = op.args.as_ref().unwrap();
                        let lhs = locals[&args[0]];
                        let rhs = locals[&args[1]];
                        func.instruction(&Instruction::LocalGet(lhs));
                        func.instruction(&Instruction::LocalGet(rhs));
                        emit_call(func, reloc_enabled, import_ids["floordiv"]);
                        let res = locals[op.out.as_ref().unwrap()];
                        func.instruction(&Instruction::LocalSet(res));
                    }
                    "mod" => {
                        let args = op.args.as_ref().unwrap();
                        let lhs = locals[&args[0]];
                        let rhs = locals[&args[1]];
                        func.instruction(&Instruction::LocalGet(lhs));
                        func.instruction(&Instruction::LocalGet(rhs));
                        emit_call(func, reloc_enabled, import_ids["mod"]);
                        let res = locals[op.out.as_ref().unwrap()];
                        func.instruction(&Instruction::LocalSet(res));
                    }
                    "pow" => {
                        let args = op.args.as_ref().unwrap();
                        let lhs = locals[&args[0]];
                        let rhs = locals[&args[1]];
                        func.instruction(&Instruction::LocalGet(lhs));
                        func.instruction(&Instruction::LocalGet(rhs));
                        emit_call(func, reloc_enabled, import_ids["pow"]);
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
                        emit_call(func, reloc_enabled, import_ids["pow_mod"]);
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
                        emit_call(func, reloc_enabled, import_ids["round"]);
                        let res = locals[op.out.as_ref().unwrap()];
                        func.instruction(&Instruction::LocalSet(res));
                    }
                    "trunc" => {
                        let args = op.args.as_ref().unwrap();
                        let val = locals[&args[0]];
                        func.instruction(&Instruction::LocalGet(val));
                        emit_call(func, reloc_enabled, import_ids["trunc"]);
                        let res = locals[op.out.as_ref().unwrap()];
                        func.instruction(&Instruction::LocalSet(res));
                    }
                    "lt" => {
                        let args = op.args.as_ref().unwrap();
                        let lhs = locals[&args[0]];
                        let rhs = locals[&args[1]];
                        func.instruction(&Instruction::LocalGet(lhs));
                        func.instruction(&Instruction::LocalGet(rhs));
                        emit_call(func, reloc_enabled, import_ids["lt"]);
                        let res = locals[op.out.as_ref().unwrap()];
                        func.instruction(&Instruction::LocalSet(res));
                    }
                    "le" => {
                        let args = op.args.as_ref().unwrap();
                        let lhs = locals[&args[0]];
                        let rhs = locals[&args[1]];
                        func.instruction(&Instruction::LocalGet(lhs));
                        func.instruction(&Instruction::LocalGet(rhs));
                        emit_call(func, reloc_enabled, import_ids["le"]);
                        let res = locals[op.out.as_ref().unwrap()];
                        func.instruction(&Instruction::LocalSet(res));
                    }
                    "gt" => {
                        let args = op.args.as_ref().unwrap();
                        let lhs = locals[&args[0]];
                        let rhs = locals[&args[1]];
                        func.instruction(&Instruction::LocalGet(lhs));
                        func.instruction(&Instruction::LocalGet(rhs));
                        emit_call(func, reloc_enabled, import_ids["gt"]);
                        let res = locals[op.out.as_ref().unwrap()];
                        func.instruction(&Instruction::LocalSet(res));
                    }
                    "ge" => {
                        let args = op.args.as_ref().unwrap();
                        let lhs = locals[&args[0]];
                        let rhs = locals[&args[1]];
                        func.instruction(&Instruction::LocalGet(lhs));
                        func.instruction(&Instruction::LocalGet(rhs));
                        emit_call(func, reloc_enabled, import_ids["ge"]);
                        let res = locals[op.out.as_ref().unwrap()];
                        func.instruction(&Instruction::LocalSet(res));
                    }
                    "eq" => {
                        let args = op.args.as_ref().unwrap();
                        let lhs = locals[&args[0]];
                        let rhs = locals[&args[1]];
                        func.instruction(&Instruction::LocalGet(lhs));
                        func.instruction(&Instruction::LocalGet(rhs));
                        emit_call(func, reloc_enabled, import_ids["eq"]);
                        let res = locals[op.out.as_ref().unwrap()];
                        func.instruction(&Instruction::LocalSet(res));
                    }
                    "string_eq" => {
                        let args = op.args.as_ref().unwrap();
                        let lhs = locals[&args[0]];
                        let rhs = locals[&args[1]];
                        func.instruction(&Instruction::LocalGet(lhs));
                        func.instruction(&Instruction::LocalGet(rhs));
                        emit_call(func, reloc_enabled, import_ids["string_eq"]);
                        let res = locals[op.out.as_ref().unwrap()];
                        func.instruction(&Instruction::LocalSet(res));
                    }
                    "is" => {
                        let args = op.args.as_ref().unwrap();
                        let lhs = locals[&args[0]];
                        let rhs = locals[&args[1]];
                        func.instruction(&Instruction::LocalGet(lhs));
                        func.instruction(&Instruction::LocalGet(rhs));
                        emit_call(func, reloc_enabled, import_ids["is"]);
                        let res = locals[op.out.as_ref().unwrap()];
                        func.instruction(&Instruction::LocalSet(res));
                    }
                    "not" => {
                        let args = op.args.as_ref().unwrap();
                        let val = locals[&args[0]];
                        func.instruction(&Instruction::LocalGet(val));
                        emit_call(func, reloc_enabled, import_ids["not"]);
                        let res = locals[op.out.as_ref().unwrap()];
                        func.instruction(&Instruction::LocalSet(res));
                    }
                    "and" => {
                        let args = op.args.as_ref().unwrap();
                        let lhs = locals[&args[0]];
                        let rhs = locals[&args[1]];
                        func.instruction(&Instruction::LocalGet(lhs));
                        emit_call(func, reloc_enabled, import_ids["is_truthy"]);
                        func.instruction(&Instruction::LocalGet(rhs));
                        emit_call(func, reloc_enabled, import_ids["is_truthy"]);
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
                        emit_call(func, reloc_enabled, import_ids["is_truthy"]);
                        func.instruction(&Instruction::LocalGet(rhs));
                        emit_call(func, reloc_enabled, import_ids["is_truthy"]);
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
                        emit_call(func, reloc_enabled, import_ids["contains"]);
                        let res = locals[op.out.as_ref().unwrap()];
                        func.instruction(&Instruction::LocalSet(res));
                    }
                    "guard_type" => {
                        let args = op.args.as_ref().unwrap();
                        let val = locals[&args[0]];
                        let expected = locals[&args[1]];
                        func.instruction(&Instruction::LocalGet(val));
                        func.instruction(&Instruction::LocalGet(expected));
                        emit_call(func, reloc_enabled, import_ids["guard_type"]);
                        if let Some(out) = op.out.as_ref() {
                            let res = locals[out];
                            func.instruction(&Instruction::LocalSet(res));
                        } else {
                            func.instruction(&Instruction::Drop);
                        }
                    }
                    "guard_layout" => {
                        let args = op.args.as_ref().unwrap();
                        let obj = locals[&args[0]];
                        let class_bits = locals[&args[1]];
                        let expected = locals[&args[2]];
                        func.instruction(&Instruction::LocalGet(obj));
                        emit_call(func, reloc_enabled, import_ids["handle_resolve"]);
                        func.instruction(&Instruction::LocalGet(class_bits));
                        func.instruction(&Instruction::LocalGet(expected));
                        emit_call(func, reloc_enabled, import_ids["guard_layout_ptr"]);
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
                            emit_call(func, reloc_enabled, import_ids["print_obj"]);
                        }
                    }
                    "print_newline" => {
                        emit_call(func, reloc_enabled, import_ids["print_newline"]);
                    }
                    "alloc" => {
                        func.instruction(&Instruction::I64Const(op.value.unwrap()));
                        emit_call(func, reloc_enabled, import_ids["alloc"]);
                        func.instruction(&Instruction::LocalSet(locals[op.out.as_ref().unwrap()]));
                    }
                    "alloc_class" => {
                        let args = op.args.as_ref().unwrap();
                        let class_bits = locals[&args[0]];
                        func.instruction(&Instruction::I64Const(op.value.unwrap()));
                        func.instruction(&Instruction::LocalGet(class_bits));
                        emit_call(func, reloc_enabled, import_ids["alloc_class"]);
                        func.instruction(&Instruction::LocalSet(locals[op.out.as_ref().unwrap()]));
                    }
                    "alloc_class_trusted" => {
                        let args = op.args.as_ref().unwrap();
                        let class_bits = locals[&args[0]];
                        func.instruction(&Instruction::I64Const(op.value.unwrap()));
                        func.instruction(&Instruction::LocalGet(class_bits));
                        emit_call(func, reloc_enabled, import_ids["alloc_class_trusted"]);
                        func.instruction(&Instruction::LocalSet(locals[op.out.as_ref().unwrap()]));
                    }
                    "alloc_class_static" => {
                        let args = op.args.as_ref().unwrap();
                        let class_bits = locals[&args[0]];
                        func.instruction(&Instruction::I64Const(op.value.unwrap()));
                        func.instruction(&Instruction::LocalGet(class_bits));
                        emit_call(func, reloc_enabled, import_ids["alloc_class_static"]);
                        func.instruction(&Instruction::LocalSet(locals[op.out.as_ref().unwrap()]));
                    }
                    "json_parse" => {
                        let args = op.args.as_ref().unwrap();
                        let arg_name = &args[0];
                        if let Some(len) = locals.get(&format!("{arg_name}_len")).copied() {
                            let ptr = locals
                                .get(&format!("{arg_name}_ptr"))
                                .copied()
                                .unwrap_or(locals[arg_name]);
                            let tmp_rc = locals["__molt_tmp0"];

                            func.instruction(&Instruction::I64Const(8));
                            emit_call(func, reloc_enabled, import_ids["alloc"]);
                            let out_ptr = locals[op.out.as_ref().unwrap()];
                            func.instruction(&Instruction::LocalSet(out_ptr));

                            func.instruction(&Instruction::LocalGet(ptr));
                            func.instruction(&Instruction::I32WrapI64);
                            func.instruction(&Instruction::LocalGet(len));
                            func.instruction(&Instruction::LocalGet(out_ptr));
                            emit_call(func, reloc_enabled, import_ids["handle_resolve"]);
                            emit_call(func, reloc_enabled, import_ids["json_parse_scalar"]);
                            func.instruction(&Instruction::I64ExtendI32U);
                            func.instruction(&Instruction::LocalSet(tmp_rc));

                            func.instruction(&Instruction::LocalGet(tmp_rc));
                            func.instruction(&Instruction::I64Eqz);
                            func.instruction(&Instruction::If(BlockType::Empty));
                            func.instruction(&Instruction::LocalGet(out_ptr));
                            emit_call(func, reloc_enabled, import_ids["handle_resolve"]);
                            func.instruction(&Instruction::I64Load(wasm_encoder::MemArg {
                                align: 3,
                                offset: 0,
                                memory_index: 0,
                            }));
                            func.instruction(&Instruction::LocalSet(out_ptr));
                            func.instruction(&Instruction::Else);
                            func.instruction(&Instruction::LocalGet(locals[arg_name]));
                            emit_call(func, reloc_enabled, import_ids["json_parse_scalar_obj"]);
                            func.instruction(&Instruction::LocalSet(out_ptr));
                            func.instruction(&Instruction::End);
                        } else {
                            let out_ptr = locals[op.out.as_ref().unwrap()];
                            func.instruction(&Instruction::LocalGet(locals[arg_name]));
                            emit_call(func, reloc_enabled, import_ids["json_parse_scalar_obj"]);
                            func.instruction(&Instruction::LocalSet(out_ptr));
                        }
                    }
                    "msgpack_parse" => {
                        let args = op.args.as_ref().unwrap();
                        let arg_name = &args[0];
                        if let Some(len) = locals.get(&format!("{arg_name}_len")).copied() {
                            let ptr = locals
                                .get(&format!("{arg_name}_ptr"))
                                .copied()
                                .unwrap_or(locals[arg_name]);
                            let tmp_rc = locals["__molt_tmp0"];

                            func.instruction(&Instruction::I64Const(8));
                            emit_call(func, reloc_enabled, import_ids["alloc"]);
                            let out_ptr = locals[op.out.as_ref().unwrap()];
                            func.instruction(&Instruction::LocalSet(out_ptr));

                            func.instruction(&Instruction::LocalGet(ptr));
                            func.instruction(&Instruction::I32WrapI64);
                            func.instruction(&Instruction::LocalGet(len));
                            func.instruction(&Instruction::LocalGet(out_ptr));
                            emit_call(func, reloc_enabled, import_ids["handle_resolve"]);
                            emit_call(func, reloc_enabled, import_ids["msgpack_parse_scalar"]);
                            func.instruction(&Instruction::I64ExtendI32U);
                            func.instruction(&Instruction::LocalSet(tmp_rc));

                            func.instruction(&Instruction::LocalGet(tmp_rc));
                            func.instruction(&Instruction::I64Eqz);
                            func.instruction(&Instruction::If(BlockType::Empty));
                            func.instruction(&Instruction::LocalGet(out_ptr));
                            emit_call(func, reloc_enabled, import_ids["handle_resolve"]);
                            func.instruction(&Instruction::I64Load(wasm_encoder::MemArg {
                                align: 3,
                                offset: 0,
                                memory_index: 0,
                            }));
                            func.instruction(&Instruction::LocalSet(out_ptr));
                            func.instruction(&Instruction::Else);
                            func.instruction(&Instruction::LocalGet(locals[arg_name]));
                            emit_call(func, reloc_enabled, import_ids["msgpack_parse_scalar_obj"]);
                            func.instruction(&Instruction::LocalSet(out_ptr));
                            func.instruction(&Instruction::End);
                        } else {
                            let out_ptr = locals[op.out.as_ref().unwrap()];
                            func.instruction(&Instruction::LocalGet(locals[arg_name]));
                            emit_call(func, reloc_enabled, import_ids["msgpack_parse_scalar_obj"]);
                            func.instruction(&Instruction::LocalSet(out_ptr));
                        }
                    }
                    "cbor_parse" => {
                        let args = op.args.as_ref().unwrap();
                        let arg_name = &args[0];
                        if let Some(len) = locals.get(&format!("{arg_name}_len")).copied() {
                            let ptr = locals
                                .get(&format!("{arg_name}_ptr"))
                                .copied()
                                .unwrap_or(locals[arg_name]);
                            let tmp_rc = locals["__molt_tmp0"];

                            func.instruction(&Instruction::I64Const(8));
                            emit_call(func, reloc_enabled, import_ids["alloc"]);
                            let out_ptr = locals[op.out.as_ref().unwrap()];
                            func.instruction(&Instruction::LocalSet(out_ptr));

                            func.instruction(&Instruction::LocalGet(ptr));
                            func.instruction(&Instruction::I32WrapI64);
                            func.instruction(&Instruction::LocalGet(len));
                            func.instruction(&Instruction::LocalGet(out_ptr));
                            emit_call(func, reloc_enabled, import_ids["handle_resolve"]);
                            emit_call(func, reloc_enabled, import_ids["cbor_parse_scalar"]);
                            func.instruction(&Instruction::I64ExtendI32U);
                            func.instruction(&Instruction::LocalSet(tmp_rc));

                            func.instruction(&Instruction::LocalGet(tmp_rc));
                            func.instruction(&Instruction::I64Eqz);
                            func.instruction(&Instruction::If(BlockType::Empty));
                            func.instruction(&Instruction::LocalGet(out_ptr));
                            emit_call(func, reloc_enabled, import_ids["handle_resolve"]);
                            func.instruction(&Instruction::I64Load(wasm_encoder::MemArg {
                                align: 3,
                                offset: 0,
                                memory_index: 0,
                            }));
                            func.instruction(&Instruction::LocalSet(out_ptr));
                            func.instruction(&Instruction::Else);
                            func.instruction(&Instruction::LocalGet(locals[arg_name]));
                            emit_call(func, reloc_enabled, import_ids["cbor_parse_scalar_obj"]);
                            func.instruction(&Instruction::LocalSet(out_ptr));
                            func.instruction(&Instruction::End);
                        } else {
                            let out_ptr = locals[op.out.as_ref().unwrap()];
                            func.instruction(&Instruction::LocalGet(locals[arg_name]));
                            emit_call(func, reloc_enabled, import_ids["cbor_parse_scalar_obj"]);
                            func.instruction(&Instruction::LocalSet(out_ptr));
                        }
                    }
                    "len" => {
                        let args = op.args.as_ref().unwrap();
                        let arg = locals[&args[0]];
                        func.instruction(&Instruction::LocalGet(arg));
                        emit_call(func, reloc_enabled, import_ids["len"]);
                        let res = locals[op.out.as_ref().unwrap()];
                        func.instruction(&Instruction::LocalSet(res));
                    }
                    "id" => {
                        let args = op.args.as_ref().unwrap();
                        let arg = locals[&args[0]];
                        func.instruction(&Instruction::LocalGet(arg));
                        emit_call(func, reloc_enabled, import_ids["id"]);
                        let res = locals[op.out.as_ref().unwrap()];
                        func.instruction(&Instruction::LocalSet(res));
                    }
                    "ord" => {
                        let args = op.args.as_ref().unwrap();
                        let arg = locals[&args[0]];
                        func.instruction(&Instruction::LocalGet(arg));
                        emit_call(func, reloc_enabled, import_ids["ord"]);
                        let res = locals[op.out.as_ref().unwrap()];
                        func.instruction(&Instruction::LocalSet(res));
                    }
                    "chr" => {
                        let args = op.args.as_ref().unwrap();
                        let arg = locals[&args[0]];
                        func.instruction(&Instruction::LocalGet(arg));
                        emit_call(func, reloc_enabled, import_ids["chr"]);
                        let res = locals[op.out.as_ref().unwrap()];
                        func.instruction(&Instruction::LocalSet(res));
                    }
                    "callargs_new" => {
                        let out = locals[op.out.as_ref().unwrap()];
                        func.instruction(&Instruction::I64Const(0));
                        func.instruction(&Instruction::I64Const(0));
                        emit_call(func, reloc_enabled, import_ids["callargs_new"]);
                        func.instruction(&Instruction::LocalSet(out));
                    }
                    "list_new" => {
                        let args = op.args.as_ref().unwrap();
                        let out = locals[op.out.as_ref().unwrap()];
                        func.instruction(&Instruction::I64Const(args.len() as i64));
                        emit_call(func, reloc_enabled, import_ids["list_builder_new"]);
                        func.instruction(&Instruction::LocalSet(out));
                        for name in args {
                            let val = locals[name];
                            func.instruction(&Instruction::LocalGet(out));
                            func.instruction(&Instruction::LocalGet(val));
                            emit_call(func, reloc_enabled, import_ids["list_builder_append"]);
                        }
                        func.instruction(&Instruction::LocalGet(out));
                        emit_call(func, reloc_enabled, import_ids["list_builder_finish"]);
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
                        emit_call(func, reloc_enabled, import_ids["range_new"]);
                        func.instruction(&Instruction::LocalSet(out));
                    }
                    "tuple_new" => {
                        let args = op.args.as_ref().unwrap();
                        let out = locals[op.out.as_ref().unwrap()];
                        func.instruction(&Instruction::I64Const(args.len() as i64));
                        emit_call(func, reloc_enabled, import_ids["list_builder_new"]);
                        func.instruction(&Instruction::LocalSet(out));
                        for name in args {
                            let val = locals[name];
                            func.instruction(&Instruction::LocalGet(out));
                            func.instruction(&Instruction::LocalGet(val));
                            emit_call(func, reloc_enabled, import_ids["list_builder_append"]);
                        }
                        func.instruction(&Instruction::LocalGet(out));
                        emit_call(func, reloc_enabled, import_ids["tuple_builder_finish"]);
                        func.instruction(&Instruction::LocalSet(out));
                    }
                    "callargs_push_pos" => {
                        let args = op.args.as_ref().unwrap();
                        let builder_ptr = locals[&args[0]];
                        let val = locals[&args[1]];
                        func.instruction(&Instruction::LocalGet(builder_ptr));
                        func.instruction(&Instruction::LocalGet(val));
                        emit_call(func, reloc_enabled, import_ids["callargs_push_pos"]);
                        let res = locals[op.out.as_ref().unwrap()];
                        func.instruction(&Instruction::LocalSet(res));
                    }
                    "callargs_push_kw" => {
                        let args = op.args.as_ref().unwrap();
                        let builder_ptr = locals[&args[0]];
                        let name = locals[&args[1]];
                        let val = locals[&args[2]];
                        func.instruction(&Instruction::LocalGet(builder_ptr));
                        func.instruction(&Instruction::LocalGet(name));
                        func.instruction(&Instruction::LocalGet(val));
                        emit_call(func, reloc_enabled, import_ids["callargs_push_kw"]);
                        let res = locals[op.out.as_ref().unwrap()];
                        func.instruction(&Instruction::LocalSet(res));
                    }
                    "callargs_expand_star" => {
                        let args = op.args.as_ref().unwrap();
                        let builder_ptr = locals[&args[0]];
                        let iterable = locals[&args[1]];
                        func.instruction(&Instruction::LocalGet(builder_ptr));
                        func.instruction(&Instruction::LocalGet(iterable));
                        emit_call(func, reloc_enabled, import_ids["callargs_expand_star"]);
                        let res = locals[op.out.as_ref().unwrap()];
                        func.instruction(&Instruction::LocalSet(res));
                    }
                    "callargs_expand_kwstar" => {
                        let args = op.args.as_ref().unwrap();
                        let builder_ptr = locals[&args[0]];
                        let mapping = locals[&args[1]];
                        func.instruction(&Instruction::LocalGet(builder_ptr));
                        func.instruction(&Instruction::LocalGet(mapping));
                        emit_call(func, reloc_enabled, import_ids["callargs_expand_kwstar"]);
                        let res = locals[op.out.as_ref().unwrap()];
                        func.instruction(&Instruction::LocalSet(res));
                    }
                    "list_append" => {
                        let args = op.args.as_ref().unwrap();
                        let list = locals[&args[0]];
                        let val = locals[&args[1]];
                        func.instruction(&Instruction::LocalGet(list));
                        func.instruction(&Instruction::LocalGet(val));
                        emit_call(func, reloc_enabled, import_ids["list_append"]);
                        let res = locals[op.out.as_ref().unwrap()];
                        func.instruction(&Instruction::LocalSet(res));
                    }
                    "list_pop" => {
                        let args = op.args.as_ref().unwrap();
                        let list = locals[&args[0]];
                        let idx = locals[&args[1]];
                        func.instruction(&Instruction::LocalGet(list));
                        func.instruction(&Instruction::LocalGet(idx));
                        emit_call(func, reloc_enabled, import_ids["list_pop"]);
                        let res = locals[op.out.as_ref().unwrap()];
                        func.instruction(&Instruction::LocalSet(res));
                    }
                    "list_extend" => {
                        let args = op.args.as_ref().unwrap();
                        let list = locals[&args[0]];
                        let other = locals[&args[1]];
                        func.instruction(&Instruction::LocalGet(list));
                        func.instruction(&Instruction::LocalGet(other));
                        emit_call(func, reloc_enabled, import_ids["list_extend"]);
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
                        emit_call(func, reloc_enabled, import_ids["list_insert"]);
                        let res = locals[op.out.as_ref().unwrap()];
                        func.instruction(&Instruction::LocalSet(res));
                    }
                    "list_remove" => {
                        let args = op.args.as_ref().unwrap();
                        let list = locals[&args[0]];
                        let val = locals[&args[1]];
                        func.instruction(&Instruction::LocalGet(list));
                        func.instruction(&Instruction::LocalGet(val));
                        emit_call(func, reloc_enabled, import_ids["list_remove"]);
                        let res = locals[op.out.as_ref().unwrap()];
                        func.instruction(&Instruction::LocalSet(res));
                    }
                    "list_clear" => {
                        let args = op.args.as_ref().unwrap();
                        let list = locals[&args[0]];
                        func.instruction(&Instruction::LocalGet(list));
                        emit_call(func, reloc_enabled, import_ids["list_clear"]);
                        let res = locals[op.out.as_ref().unwrap()];
                        func.instruction(&Instruction::LocalSet(res));
                    }
                    "list_copy" => {
                        let args = op.args.as_ref().unwrap();
                        let list = locals[&args[0]];
                        func.instruction(&Instruction::LocalGet(list));
                        emit_call(func, reloc_enabled, import_ids["list_copy"]);
                        let res = locals[op.out.as_ref().unwrap()];
                        func.instruction(&Instruction::LocalSet(res));
                    }
                    "list_reverse" => {
                        let args = op.args.as_ref().unwrap();
                        let list = locals[&args[0]];
                        func.instruction(&Instruction::LocalGet(list));
                        emit_call(func, reloc_enabled, import_ids["list_reverse"]);
                        let res = locals[op.out.as_ref().unwrap()];
                        func.instruction(&Instruction::LocalSet(res));
                    }
                    "list_count" => {
                        let args = op.args.as_ref().unwrap();
                        let list = locals[&args[0]];
                        let val = locals[&args[1]];
                        func.instruction(&Instruction::LocalGet(list));
                        func.instruction(&Instruction::LocalGet(val));
                        emit_call(func, reloc_enabled, import_ids["list_count"]);
                        let res = locals[op.out.as_ref().unwrap()];
                        func.instruction(&Instruction::LocalSet(res));
                    }
                    "list_index" => {
                        let args = op.args.as_ref().unwrap();
                        let list = locals[&args[0]];
                        let val = locals[&args[1]];
                        func.instruction(&Instruction::LocalGet(list));
                        func.instruction(&Instruction::LocalGet(val));
                        emit_call(func, reloc_enabled, import_ids["list_index"]);
                        let res = locals[op.out.as_ref().unwrap()];
                        func.instruction(&Instruction::LocalSet(res));
                    }
                    "list_index_range" => {
                        let args = op.args.as_ref().unwrap();
                        let list = locals[&args[0]];
                        let val = locals[&args[1]];
                        let start = locals[&args[2]];
                        let stop = locals[&args[3]];
                        func.instruction(&Instruction::LocalGet(list));
                        func.instruction(&Instruction::LocalGet(val));
                        func.instruction(&Instruction::LocalGet(start));
                        func.instruction(&Instruction::LocalGet(stop));
                        emit_call(func, reloc_enabled, import_ids["list_index_range"]);
                        let res = locals[op.out.as_ref().unwrap()];
                        func.instruction(&Instruction::LocalSet(res));
                    }
                    "tuple_from_list" => {
                        let args = op.args.as_ref().unwrap();
                        let list = locals[&args[0]];
                        func.instruction(&Instruction::LocalGet(list));
                        emit_call(func, reloc_enabled, import_ids["tuple_from_list"]);
                        let res = locals[op.out.as_ref().unwrap()];
                        func.instruction(&Instruction::LocalSet(res));
                    }
                    "dict_new" => {
                        let args = op.args.as_ref().unwrap();
                        let out = locals[op.out.as_ref().unwrap()];
                        func.instruction(&Instruction::I64Const((args.len() / 2) as i64));
                        emit_call(func, reloc_enabled, import_ids["dict_new"]);
                        func.instruction(&Instruction::LocalSet(out));
                        for pair in args.chunks(2) {
                            let key = locals[&pair[0]];
                            let val = locals[&pair[1]];
                            func.instruction(&Instruction::LocalGet(out));
                            func.instruction(&Instruction::LocalGet(key));
                            func.instruction(&Instruction::LocalGet(val));
                            emit_call(func, reloc_enabled, import_ids["dict_set"]);
                            func.instruction(&Instruction::LocalSet(out));
                        }
                    }
                    "dict_from_obj" => {
                        let args = op.args.as_ref().unwrap();
                        let obj = locals[&args[0]];
                        func.instruction(&Instruction::LocalGet(obj));
                        emit_call(func, reloc_enabled, import_ids["dict_from_obj"]);
                        let out = locals[op.out.as_ref().unwrap()];
                        func.instruction(&Instruction::LocalSet(out));
                    }
                    "set_new" => {
                        let args = op.args.as_ref().unwrap();
                        let out = locals[op.out.as_ref().unwrap()];
                        func.instruction(&Instruction::I64Const(args.len() as i64));
                        emit_call(func, reloc_enabled, import_ids["set_new"]);
                        func.instruction(&Instruction::LocalSet(out));
                        for name in args {
                            let val = locals[name];
                            func.instruction(&Instruction::LocalGet(out));
                            func.instruction(&Instruction::LocalGet(val));
                            emit_call(func, reloc_enabled, import_ids["set_add"]);
                            func.instruction(&Instruction::Drop);
                        }
                    }
                    "frozenset_new" => {
                        let args = op.args.as_ref().unwrap();
                        let out = locals[op.out.as_ref().unwrap()];
                        func.instruction(&Instruction::I64Const(args.len() as i64));
                        emit_call(func, reloc_enabled, import_ids["frozenset_new"]);
                        func.instruction(&Instruction::LocalSet(out));
                        for name in args {
                            let val = locals[name];
                            func.instruction(&Instruction::LocalGet(out));
                            func.instruction(&Instruction::LocalGet(val));
                            emit_call(func, reloc_enabled, import_ids["frozenset_add"]);
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
                        emit_call(func, reloc_enabled, import_ids["dict_get"]);
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
                        emit_call(func, reloc_enabled, import_ids["dict_pop"]);
                        let res = locals[op.out.as_ref().unwrap()];
                        func.instruction(&Instruction::LocalSet(res));
                    }
                    "dict_setdefault" => {
                        let args = op.args.as_ref().unwrap();
                        let dict = locals[&args[0]];
                        let key = locals[&args[1]];
                        let default = locals[&args[2]];
                        func.instruction(&Instruction::LocalGet(dict));
                        func.instruction(&Instruction::LocalGet(key));
                        func.instruction(&Instruction::LocalGet(default));
                        emit_call(func, reloc_enabled, import_ids["dict_setdefault"]);
                        let res = locals[op.out.as_ref().unwrap()];
                        func.instruction(&Instruction::LocalSet(res));
                    }
                    "dict_update" => {
                        let args = op.args.as_ref().unwrap();
                        let dict = locals[&args[0]];
                        let other = locals[&args[1]];
                        func.instruction(&Instruction::LocalGet(dict));
                        func.instruction(&Instruction::LocalGet(other));
                        emit_call(func, reloc_enabled, import_ids["dict_update"]);
                        let res = locals[op.out.as_ref().unwrap()];
                        func.instruction(&Instruction::LocalSet(res));
                    }
                    "dict_clear" => {
                        let args = op.args.as_ref().unwrap();
                        let dict = locals[&args[0]];
                        func.instruction(&Instruction::LocalGet(dict));
                        emit_call(func, reloc_enabled, import_ids["dict_clear"]);
                        let res = locals[op.out.as_ref().unwrap()];
                        func.instruction(&Instruction::LocalSet(res));
                    }
                    "dict_copy" => {
                        let args = op.args.as_ref().unwrap();
                        let dict = locals[&args[0]];
                        func.instruction(&Instruction::LocalGet(dict));
                        emit_call(func, reloc_enabled, import_ids["dict_copy"]);
                        let res = locals[op.out.as_ref().unwrap()];
                        func.instruction(&Instruction::LocalSet(res));
                    }
                    "dict_popitem" => {
                        let args = op.args.as_ref().unwrap();
                        let dict = locals[&args[0]];
                        func.instruction(&Instruction::LocalGet(dict));
                        emit_call(func, reloc_enabled, import_ids["dict_popitem"]);
                        let res = locals[op.out.as_ref().unwrap()];
                        func.instruction(&Instruction::LocalSet(res));
                    }
                    "dict_update_kwstar" => {
                        let args = op.args.as_ref().unwrap();
                        let dict = locals[&args[0]];
                        let other = locals[&args[1]];
                        func.instruction(&Instruction::LocalGet(dict));
                        func.instruction(&Instruction::LocalGet(other));
                        emit_call(func, reloc_enabled, import_ids["dict_update_kwstar"]);
                        let res = locals[op.out.as_ref().unwrap()];
                        func.instruction(&Instruction::LocalSet(res));
                    }
                    "set_add" => {
                        let args = op.args.as_ref().unwrap();
                        let set_bits = locals[&args[0]];
                        let key = locals[&args[1]];
                        func.instruction(&Instruction::LocalGet(set_bits));
                        func.instruction(&Instruction::LocalGet(key));
                        emit_call(func, reloc_enabled, import_ids["set_add"]);
                        let res = locals[op.out.as_ref().unwrap()];
                        func.instruction(&Instruction::LocalSet(res));
                    }
                    "frozenset_add" => {
                        let args = op.args.as_ref().unwrap();
                        let set_bits = locals[&args[0]];
                        let key = locals[&args[1]];
                        func.instruction(&Instruction::LocalGet(set_bits));
                        func.instruction(&Instruction::LocalGet(key));
                        emit_call(func, reloc_enabled, import_ids["frozenset_add"]);
                        let res = locals[op.out.as_ref().unwrap()];
                        func.instruction(&Instruction::LocalSet(res));
                    }
                    "set_discard" => {
                        let args = op.args.as_ref().unwrap();
                        let set_bits = locals[&args[0]];
                        let key = locals[&args[1]];
                        func.instruction(&Instruction::LocalGet(set_bits));
                        func.instruction(&Instruction::LocalGet(key));
                        emit_call(func, reloc_enabled, import_ids["set_discard"]);
                        let res = locals[op.out.as_ref().unwrap()];
                        func.instruction(&Instruction::LocalSet(res));
                    }
                    "set_remove" => {
                        let args = op.args.as_ref().unwrap();
                        let set_bits = locals[&args[0]];
                        let key = locals[&args[1]];
                        func.instruction(&Instruction::LocalGet(set_bits));
                        func.instruction(&Instruction::LocalGet(key));
                        emit_call(func, reloc_enabled, import_ids["set_remove"]);
                        let res = locals[op.out.as_ref().unwrap()];
                        func.instruction(&Instruction::LocalSet(res));
                    }
                    "set_pop" => {
                        let args = op.args.as_ref().unwrap();
                        let set_bits = locals[&args[0]];
                        func.instruction(&Instruction::LocalGet(set_bits));
                        emit_call(func, reloc_enabled, import_ids["set_pop"]);
                        let res = locals[op.out.as_ref().unwrap()];
                        func.instruction(&Instruction::LocalSet(res));
                    }
                    "set_update" => {
                        let args = op.args.as_ref().unwrap();
                        let set_bits = locals[&args[0]];
                        let other = locals[&args[1]];
                        func.instruction(&Instruction::LocalGet(set_bits));
                        func.instruction(&Instruction::LocalGet(other));
                        emit_call(func, reloc_enabled, import_ids["set_update"]);
                        let res = locals[op.out.as_ref().unwrap()];
                        func.instruction(&Instruction::LocalSet(res));
                    }
                    "set_intersection_update" => {
                        let args = op.args.as_ref().unwrap();
                        let set_bits = locals[&args[0]];
                        let other = locals[&args[1]];
                        func.instruction(&Instruction::LocalGet(set_bits));
                        func.instruction(&Instruction::LocalGet(other));
                        emit_call(func, reloc_enabled, import_ids["set_intersection_update"]);
                        let res = locals[op.out.as_ref().unwrap()];
                        func.instruction(&Instruction::LocalSet(res));
                    }
                    "set_difference_update" => {
                        let args = op.args.as_ref().unwrap();
                        let set_bits = locals[&args[0]];
                        let other = locals[&args[1]];
                        func.instruction(&Instruction::LocalGet(set_bits));
                        func.instruction(&Instruction::LocalGet(other));
                        emit_call(func, reloc_enabled, import_ids["set_difference_update"]);
                        let res = locals[op.out.as_ref().unwrap()];
                        func.instruction(&Instruction::LocalSet(res));
                    }
                    "set_symdiff_update" => {
                        let args = op.args.as_ref().unwrap();
                        let set_bits = locals[&args[0]];
                        let other = locals[&args[1]];
                        func.instruction(&Instruction::LocalGet(set_bits));
                        func.instruction(&Instruction::LocalGet(other));
                        emit_call(func, reloc_enabled, import_ids["set_symdiff_update"]);
                        let res = locals[op.out.as_ref().unwrap()];
                        func.instruction(&Instruction::LocalSet(res));
                    }
                    "dict_keys" => {
                        let args = op.args.as_ref().unwrap();
                        let dict = locals[&args[0]];
                        func.instruction(&Instruction::LocalGet(dict));
                        emit_call(func, reloc_enabled, import_ids["dict_keys"]);
                        let res = locals[op.out.as_ref().unwrap()];
                        func.instruction(&Instruction::LocalSet(res));
                    }
                    "dict_values" => {
                        let args = op.args.as_ref().unwrap();
                        let dict = locals[&args[0]];
                        func.instruction(&Instruction::LocalGet(dict));
                        emit_call(func, reloc_enabled, import_ids["dict_values"]);
                        let res = locals[op.out.as_ref().unwrap()];
                        func.instruction(&Instruction::LocalSet(res));
                    }
                    "dict_items" => {
                        let args = op.args.as_ref().unwrap();
                        let dict = locals[&args[0]];
                        func.instruction(&Instruction::LocalGet(dict));
                        emit_call(func, reloc_enabled, import_ids["dict_items"]);
                        let res = locals[op.out.as_ref().unwrap()];
                        func.instruction(&Instruction::LocalSet(res));
                    }
                    "tuple_count" => {
                        let args = op.args.as_ref().unwrap();
                        let tuple = locals[&args[0]];
                        let val = locals[&args[1]];
                        func.instruction(&Instruction::LocalGet(tuple));
                        func.instruction(&Instruction::LocalGet(val));
                        emit_call(func, reloc_enabled, import_ids["tuple_count"]);
                        let res = locals[op.out.as_ref().unwrap()];
                        func.instruction(&Instruction::LocalSet(res));
                    }
                    "tuple_index" => {
                        let args = op.args.as_ref().unwrap();
                        let tuple = locals[&args[0]];
                        let val = locals[&args[1]];
                        func.instruction(&Instruction::LocalGet(tuple));
                        func.instruction(&Instruction::LocalGet(val));
                        emit_call(func, reloc_enabled, import_ids["tuple_index"]);
                        let res = locals[op.out.as_ref().unwrap()];
                        func.instruction(&Instruction::LocalSet(res));
                    }
                    "iter" => {
                        let args = op.args.as_ref().unwrap();
                        let obj = locals[&args[0]];
                        func.instruction(&Instruction::LocalGet(obj));
                        emit_call(func, reloc_enabled, import_ids["iter"]);
                        let res = locals[op.out.as_ref().unwrap()];
                        func.instruction(&Instruction::LocalSet(res));
                    }
                    "enumerate" => {
                        let args = op.args.as_ref().unwrap();
                        let iterable = locals[&args[0]];
                        let start = locals[&args[1]];
                        let has_start = locals[&args[2]];
                        func.instruction(&Instruction::LocalGet(iterable));
                        func.instruction(&Instruction::LocalGet(start));
                        func.instruction(&Instruction::LocalGet(has_start));
                        emit_call(func, reloc_enabled, import_ids["enumerate"]);
                        let res = locals[op.out.as_ref().unwrap()];
                        func.instruction(&Instruction::LocalSet(res));
                    }
                    "aiter" => {
                        let args = op.args.as_ref().unwrap();
                        let obj = locals[&args[0]];
                        func.instruction(&Instruction::LocalGet(obj));
                        emit_call(func, reloc_enabled, import_ids["aiter"]);
                        let res = locals[op.out.as_ref().unwrap()];
                        func.instruction(&Instruction::LocalSet(res));
                    }
                    "iter_next" => {
                        let args = op.args.as_ref().unwrap();
                        let iter = locals[&args[0]];
                        func.instruction(&Instruction::LocalGet(iter));
                        emit_call(func, reloc_enabled, import_ids["iter_next"]);
                        let res = locals[op.out.as_ref().unwrap()];
                        func.instruction(&Instruction::LocalSet(res));
                    }
                    "anext" => {
                        let args = op.args.as_ref().unwrap();
                        let iter = locals[&args[0]];
                        func.instruction(&Instruction::LocalGet(iter));
                        emit_call(func, reloc_enabled, import_ids["anext"]);
                        let res = locals[op.out.as_ref().unwrap()];
                        func.instruction(&Instruction::LocalSet(res));
                    }
                    "gen_send" => {
                        let args = op.args.as_ref().unwrap();
                        let gen = locals[&args[0]];
                        let val = locals[&args[1]];
                        func.instruction(&Instruction::LocalGet(gen));
                        func.instruction(&Instruction::LocalGet(val));
                        emit_call(func, reloc_enabled, import_ids["generator_send"]);
                        let res = locals[op.out.as_ref().unwrap()];
                        func.instruction(&Instruction::LocalSet(res));
                    }
                    "gen_throw" => {
                        let args = op.args.as_ref().unwrap();
                        let gen = locals[&args[0]];
                        let val = locals[&args[1]];
                        func.instruction(&Instruction::LocalGet(gen));
                        func.instruction(&Instruction::LocalGet(val));
                        emit_call(func, reloc_enabled, import_ids["generator_throw"]);
                        let res = locals[op.out.as_ref().unwrap()];
                        func.instruction(&Instruction::LocalSet(res));
                    }
                    "gen_close" => {
                        let args = op.args.as_ref().unwrap();
                        let gen = locals[&args[0]];
                        func.instruction(&Instruction::LocalGet(gen));
                        emit_call(func, reloc_enabled, import_ids["generator_close"]);
                        let res = locals[op.out.as_ref().unwrap()];
                        func.instruction(&Instruction::LocalSet(res));
                    }
                    "is_generator" => {
                        let args = op.args.as_ref().unwrap();
                        let obj = locals[&args[0]];
                        func.instruction(&Instruction::LocalGet(obj));
                        emit_call(func, reloc_enabled, import_ids["is_generator"]);
                        let res = locals[op.out.as_ref().unwrap()];
                        func.instruction(&Instruction::LocalSet(res));
                    }
                    "is_bound_method" => {
                        let args = op.args.as_ref().unwrap();
                        let obj = locals[&args[0]];
                        func.instruction(&Instruction::LocalGet(obj));
                        emit_call(func, reloc_enabled, import_ids["is_bound_method"]);
                        let res = locals[op.out.as_ref().unwrap()];
                        func.instruction(&Instruction::LocalSet(res));
                    }
                    "is_callable" => {
                        let args = op.args.as_ref().unwrap();
                        let obj = locals[&args[0]];
                        func.instruction(&Instruction::LocalGet(obj));
                        emit_call(func, reloc_enabled, import_ids["is_callable"]);
                        let res = locals[op.out.as_ref().unwrap()];
                        func.instruction(&Instruction::LocalSet(res));
                    }
                    "index" => {
                        let args = op.args.as_ref().unwrap();
                        let obj = locals[&args[0]];
                        let idx = locals[&args[1]];
                        func.instruction(&Instruction::LocalGet(obj));
                        func.instruction(&Instruction::LocalGet(idx));
                        emit_call(func, reloc_enabled, import_ids["index"]);
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
                        emit_call(func, reloc_enabled, import_ids["store_index"]);
                        if let Some(out) = op.out.as_ref() {
                            let res = locals[out];
                            func.instruction(&Instruction::LocalSet(res));
                        } else {
                            func.instruction(&Instruction::Drop);
                        }
                    }
                    "del_index" => {
                        let args = op.args.as_ref().unwrap();
                        let obj = locals[&args[0]];
                        let idx = locals[&args[1]];
                        func.instruction(&Instruction::LocalGet(obj));
                        func.instruction(&Instruction::LocalGet(idx));
                        emit_call(func, reloc_enabled, import_ids["del_index"]);
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
                        emit_call(func, reloc_enabled, import_ids["slice"]);
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
                        emit_call(func, reloc_enabled, import_ids["slice_new"]);
                        let res = locals[op.out.as_ref().unwrap()];
                        func.instruction(&Instruction::LocalSet(res));
                    }
                    "bytes_find" => {
                        let args = op.args.as_ref().unwrap();
                        let hay = locals[&args[0]];
                        let needle = locals[&args[1]];
                        func.instruction(&Instruction::LocalGet(hay));
                        func.instruction(&Instruction::LocalGet(needle));
                        emit_call(func, reloc_enabled, import_ids["bytes_find"]);
                        let res = locals[op.out.as_ref().unwrap()];
                        func.instruction(&Instruction::LocalSet(res));
                    }
                    "bytes_find_slice" => {
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
                        emit_call(func, reloc_enabled, import_ids["bytes_find_slice"]);
                        let res = locals[op.out.as_ref().unwrap()];
                        func.instruction(&Instruction::LocalSet(res));
                    }
                    "bytearray_find" => {
                        let args = op.args.as_ref().unwrap();
                        let hay = locals[&args[0]];
                        let needle = locals[&args[1]];
                        func.instruction(&Instruction::LocalGet(hay));
                        func.instruction(&Instruction::LocalGet(needle));
                        emit_call(func, reloc_enabled, import_ids["bytearray_find"]);
                        let res = locals[op.out.as_ref().unwrap()];
                        func.instruction(&Instruction::LocalSet(res));
                    }
                    "bytearray_find_slice" => {
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
                        emit_call(func, reloc_enabled, import_ids["bytearray_find_slice"]);
                        let res = locals[op.out.as_ref().unwrap()];
                        func.instruction(&Instruction::LocalSet(res));
                    }
                    "string_find" => {
                        let args = op.args.as_ref().unwrap();
                        let hay = locals[&args[0]];
                        let needle = locals[&args[1]];
                        func.instruction(&Instruction::LocalGet(hay));
                        func.instruction(&Instruction::LocalGet(needle));
                        emit_call(func, reloc_enabled, import_ids["string_find"]);
                        let res = locals[op.out.as_ref().unwrap()];
                        func.instruction(&Instruction::LocalSet(res));
                    }
                    "string_find_slice" => {
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
                        emit_call(func, reloc_enabled, import_ids["string_find_slice"]);
                        let res = locals[op.out.as_ref().unwrap()];
                        func.instruction(&Instruction::LocalSet(res));
                    }
                    "string_format" => {
                        let args = op.args.as_ref().unwrap();
                        let val = locals[&args[0]];
                        let spec = locals[&args[1]];
                        func.instruction(&Instruction::LocalGet(val));
                        func.instruction(&Instruction::LocalGet(spec));
                        emit_call(func, reloc_enabled, import_ids["string_format"]);
                        let res = locals[op.out.as_ref().unwrap()];
                        func.instruction(&Instruction::LocalSet(res));
                    }
                    "string_startswith" => {
                        let args = op.args.as_ref().unwrap();
                        let hay = locals[&args[0]];
                        let needle = locals[&args[1]];
                        func.instruction(&Instruction::LocalGet(hay));
                        func.instruction(&Instruction::LocalGet(needle));
                        emit_call(func, reloc_enabled, import_ids["string_startswith"]);
                        let res = locals[op.out.as_ref().unwrap()];
                        func.instruction(&Instruction::LocalSet(res));
                    }
                    "string_startswith_slice" => {
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
                        emit_call(func, reloc_enabled, import_ids["string_startswith_slice"]);
                        let res = locals[op.out.as_ref().unwrap()];
                        func.instruction(&Instruction::LocalSet(res));
                    }
                    "bytes_startswith" => {
                        let args = op.args.as_ref().unwrap();
                        let hay = locals[&args[0]];
                        let needle = locals[&args[1]];
                        func.instruction(&Instruction::LocalGet(hay));
                        func.instruction(&Instruction::LocalGet(needle));
                        emit_call(func, reloc_enabled, import_ids["bytes_startswith"]);
                        let res = locals[op.out.as_ref().unwrap()];
                        func.instruction(&Instruction::LocalSet(res));
                    }
                    "bytes_startswith_slice" => {
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
                        emit_call(func, reloc_enabled, import_ids["bytes_startswith_slice"]);
                        let res = locals[op.out.as_ref().unwrap()];
                        func.instruction(&Instruction::LocalSet(res));
                    }
                    "bytearray_startswith" => {
                        let args = op.args.as_ref().unwrap();
                        let hay = locals[&args[0]];
                        let needle = locals[&args[1]];
                        func.instruction(&Instruction::LocalGet(hay));
                        func.instruction(&Instruction::LocalGet(needle));
                        emit_call(func, reloc_enabled, import_ids["bytearray_startswith"]);
                        let res = locals[op.out.as_ref().unwrap()];
                        func.instruction(&Instruction::LocalSet(res));
                    }
                    "bytearray_startswith_slice" => {
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
                        emit_call(
                            func,
                            reloc_enabled,
                            import_ids["bytearray_startswith_slice"],
                        );
                        let res = locals[op.out.as_ref().unwrap()];
                        func.instruction(&Instruction::LocalSet(res));
                    }
                    "string_endswith" => {
                        let args = op.args.as_ref().unwrap();
                        let hay = locals[&args[0]];
                        let needle = locals[&args[1]];
                        func.instruction(&Instruction::LocalGet(hay));
                        func.instruction(&Instruction::LocalGet(needle));
                        emit_call(func, reloc_enabled, import_ids["string_endswith"]);
                        let res = locals[op.out.as_ref().unwrap()];
                        func.instruction(&Instruction::LocalSet(res));
                    }
                    "string_endswith_slice" => {
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
                        emit_call(func, reloc_enabled, import_ids["string_endswith_slice"]);
                        let res = locals[op.out.as_ref().unwrap()];
                        func.instruction(&Instruction::LocalSet(res));
                    }
                    "bytes_endswith" => {
                        let args = op.args.as_ref().unwrap();
                        let hay = locals[&args[0]];
                        let needle = locals[&args[1]];
                        func.instruction(&Instruction::LocalGet(hay));
                        func.instruction(&Instruction::LocalGet(needle));
                        emit_call(func, reloc_enabled, import_ids["bytes_endswith"]);
                        let res = locals[op.out.as_ref().unwrap()];
                        func.instruction(&Instruction::LocalSet(res));
                    }
                    "bytes_endswith_slice" => {
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
                        emit_call(func, reloc_enabled, import_ids["bytes_endswith_slice"]);
                        let res = locals[op.out.as_ref().unwrap()];
                        func.instruction(&Instruction::LocalSet(res));
                    }
                    "bytearray_endswith" => {
                        let args = op.args.as_ref().unwrap();
                        let hay = locals[&args[0]];
                        let needle = locals[&args[1]];
                        func.instruction(&Instruction::LocalGet(hay));
                        func.instruction(&Instruction::LocalGet(needle));
                        emit_call(func, reloc_enabled, import_ids["bytearray_endswith"]);
                        let res = locals[op.out.as_ref().unwrap()];
                        func.instruction(&Instruction::LocalSet(res));
                    }
                    "bytearray_endswith_slice" => {
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
                        emit_call(func, reloc_enabled, import_ids["bytearray_endswith_slice"]);
                        let res = locals[op.out.as_ref().unwrap()];
                        func.instruction(&Instruction::LocalSet(res));
                    }
                    "string_count" => {
                        let args = op.args.as_ref().unwrap();
                        let hay = locals[&args[0]];
                        let needle = locals[&args[1]];
                        func.instruction(&Instruction::LocalGet(hay));
                        func.instruction(&Instruction::LocalGet(needle));
                        emit_call(func, reloc_enabled, import_ids["string_count"]);
                        let res = locals[op.out.as_ref().unwrap()];
                        func.instruction(&Instruction::LocalSet(res));
                    }
                    "bytes_count" => {
                        let args = op.args.as_ref().unwrap();
                        let hay = locals[&args[0]];
                        let needle = locals[&args[1]];
                        func.instruction(&Instruction::LocalGet(hay));
                        func.instruction(&Instruction::LocalGet(needle));
                        emit_call(func, reloc_enabled, import_ids["bytes_count"]);
                        let res = locals[op.out.as_ref().unwrap()];
                        func.instruction(&Instruction::LocalSet(res));
                    }
                    "bytearray_count" => {
                        let args = op.args.as_ref().unwrap();
                        let hay = locals[&args[0]];
                        let needle = locals[&args[1]];
                        func.instruction(&Instruction::LocalGet(hay));
                        func.instruction(&Instruction::LocalGet(needle));
                        emit_call(func, reloc_enabled, import_ids["bytearray_count"]);
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
                        emit_call(func, reloc_enabled, import_ids["string_count_slice"]);
                        let res = locals[op.out.as_ref().unwrap()];
                        func.instruction(&Instruction::LocalSet(res));
                    }
                    "bytes_count_slice" => {
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
                        emit_call(func, reloc_enabled, import_ids["bytes_count_slice"]);
                        let res = locals[op.out.as_ref().unwrap()];
                        func.instruction(&Instruction::LocalSet(res));
                    }
                    "bytearray_count_slice" => {
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
                        emit_call(func, reloc_enabled, import_ids["bytearray_count_slice"]);
                        let res = locals[op.out.as_ref().unwrap()];
                        func.instruction(&Instruction::LocalSet(res));
                    }
                    "env_get" => {
                        let args = op.args.as_ref().unwrap();
                        let key = locals[&args[0]];
                        let default = locals[&args[1]];
                        func.instruction(&Instruction::LocalGet(key));
                        func.instruction(&Instruction::LocalGet(default));
                        emit_call(func, reloc_enabled, import_ids["env_get"]);
                        let res = locals[op.out.as_ref().unwrap()];
                        func.instruction(&Instruction::LocalSet(res));
                    }
                    "string_join" => {
                        let args = op.args.as_ref().unwrap();
                        let sep = locals[&args[0]];
                        let items = locals[&args[1]];
                        func.instruction(&Instruction::LocalGet(sep));
                        func.instruction(&Instruction::LocalGet(items));
                        emit_call(func, reloc_enabled, import_ids["string_join"]);
                        let res = locals[op.out.as_ref().unwrap()];
                        func.instruction(&Instruction::LocalSet(res));
                    }
                    "string_split" => {
                        let args = op.args.as_ref().unwrap();
                        let hay = locals[&args[0]];
                        let needle = locals[&args[1]];
                        func.instruction(&Instruction::LocalGet(hay));
                        func.instruction(&Instruction::LocalGet(needle));
                        emit_call(func, reloc_enabled, import_ids["string_split"]);
                        let res = locals[op.out.as_ref().unwrap()];
                        func.instruction(&Instruction::LocalSet(res));
                    }
                    "string_split_max" => {
                        let args = op.args.as_ref().unwrap();
                        let hay = locals[&args[0]];
                        let needle = locals[&args[1]];
                        let maxsplit = locals[&args[2]];
                        func.instruction(&Instruction::LocalGet(hay));
                        func.instruction(&Instruction::LocalGet(needle));
                        func.instruction(&Instruction::LocalGet(maxsplit));
                        emit_call(func, reloc_enabled, import_ids["string_split_max"]);
                        let res = locals[op.out.as_ref().unwrap()];
                        func.instruction(&Instruction::LocalSet(res));
                    }
                    "string_lower" => {
                        let args = op.args.as_ref().unwrap();
                        let hay = locals[&args[0]];
                        func.instruction(&Instruction::LocalGet(hay));
                        emit_call(func, reloc_enabled, import_ids["string_lower"]);
                        let res = locals[op.out.as_ref().unwrap()];
                        func.instruction(&Instruction::LocalSet(res));
                    }
                    "string_upper" => {
                        let args = op.args.as_ref().unwrap();
                        let hay = locals[&args[0]];
                        func.instruction(&Instruction::LocalGet(hay));
                        emit_call(func, reloc_enabled, import_ids["string_upper"]);
                        let res = locals[op.out.as_ref().unwrap()];
                        func.instruction(&Instruction::LocalSet(res));
                    }
                    "string_capitalize" => {
                        let args = op.args.as_ref().unwrap();
                        let hay = locals[&args[0]];
                        func.instruction(&Instruction::LocalGet(hay));
                        emit_call(func, reloc_enabled, import_ids["string_capitalize"]);
                        let res = locals[op.out.as_ref().unwrap()];
                        func.instruction(&Instruction::LocalSet(res));
                    }
                    "string_strip" => {
                        let args = op.args.as_ref().unwrap();
                        let hay = locals[&args[0]];
                        let chars = locals[&args[1]];
                        func.instruction(&Instruction::LocalGet(hay));
                        func.instruction(&Instruction::LocalGet(chars));
                        emit_call(func, reloc_enabled, import_ids["string_strip"]);
                        let res = locals[op.out.as_ref().unwrap()];
                        func.instruction(&Instruction::LocalSet(res));
                    }
                    "bytes_split" => {
                        let args = op.args.as_ref().unwrap();
                        let hay = locals[&args[0]];
                        let needle = locals[&args[1]];
                        func.instruction(&Instruction::LocalGet(hay));
                        func.instruction(&Instruction::LocalGet(needle));
                        emit_call(func, reloc_enabled, import_ids["bytes_split"]);
                        let res = locals[op.out.as_ref().unwrap()];
                        func.instruction(&Instruction::LocalSet(res));
                    }
                    "bytes_split_max" => {
                        let args = op.args.as_ref().unwrap();
                        let hay = locals[&args[0]];
                        let needle = locals[&args[1]];
                        let maxsplit = locals[&args[2]];
                        func.instruction(&Instruction::LocalGet(hay));
                        func.instruction(&Instruction::LocalGet(needle));
                        func.instruction(&Instruction::LocalGet(maxsplit));
                        emit_call(func, reloc_enabled, import_ids["bytes_split_max"]);
                        let res = locals[op.out.as_ref().unwrap()];
                        func.instruction(&Instruction::LocalSet(res));
                    }
                    "bytearray_split" => {
                        let args = op.args.as_ref().unwrap();
                        let hay = locals[&args[0]];
                        let needle = locals[&args[1]];
                        func.instruction(&Instruction::LocalGet(hay));
                        func.instruction(&Instruction::LocalGet(needle));
                        emit_call(func, reloc_enabled, import_ids["bytearray_split"]);
                        let res = locals[op.out.as_ref().unwrap()];
                        func.instruction(&Instruction::LocalSet(res));
                    }
                    "bytearray_split_max" => {
                        let args = op.args.as_ref().unwrap();
                        let hay = locals[&args[0]];
                        let needle = locals[&args[1]];
                        let maxsplit = locals[&args[2]];
                        func.instruction(&Instruction::LocalGet(hay));
                        func.instruction(&Instruction::LocalGet(needle));
                        func.instruction(&Instruction::LocalGet(maxsplit));
                        emit_call(func, reloc_enabled, import_ids["bytearray_split_max"]);
                        let res = locals[op.out.as_ref().unwrap()];
                        func.instruction(&Instruction::LocalSet(res));
                    }
                    "bytes_replace" => {
                        let args = op.args.as_ref().unwrap();
                        let hay = locals[&args[0]];
                        let needle = locals[&args[1]];
                        let replacement = locals[&args[2]];
                        let count = locals[&args[3]];
                        func.instruction(&Instruction::LocalGet(hay));
                        func.instruction(&Instruction::LocalGet(needle));
                        func.instruction(&Instruction::LocalGet(replacement));
                        func.instruction(&Instruction::LocalGet(count));
                        emit_call(func, reloc_enabled, import_ids["bytes_replace"]);
                        let res = locals[op.out.as_ref().unwrap()];
                        func.instruction(&Instruction::LocalSet(res));
                    }
                    "string_replace" => {
                        let args = op.args.as_ref().unwrap();
                        let hay = locals[&args[0]];
                        let needle = locals[&args[1]];
                        let replacement = locals[&args[2]];
                        let count = locals[&args[3]];
                        func.instruction(&Instruction::LocalGet(hay));
                        func.instruction(&Instruction::LocalGet(needle));
                        func.instruction(&Instruction::LocalGet(replacement));
                        func.instruction(&Instruction::LocalGet(count));
                        emit_call(func, reloc_enabled, import_ids["string_replace"]);
                        let res = locals[op.out.as_ref().unwrap()];
                        func.instruction(&Instruction::LocalSet(res));
                    }
                    "bytearray_replace" => {
                        let args = op.args.as_ref().unwrap();
                        let hay = locals[&args[0]];
                        let needle = locals[&args[1]];
                        let replacement = locals[&args[2]];
                        let count = locals[&args[3]];
                        func.instruction(&Instruction::LocalGet(hay));
                        func.instruction(&Instruction::LocalGet(needle));
                        func.instruction(&Instruction::LocalGet(replacement));
                        func.instruction(&Instruction::LocalGet(count));
                        emit_call(func, reloc_enabled, import_ids["bytearray_replace"]);
                        let res = locals[op.out.as_ref().unwrap()];
                        func.instruction(&Instruction::LocalSet(res));
                    }
                    "bytes_from_obj" => {
                        let args = op.args.as_ref().unwrap();
                        let src = locals[&args[0]];
                        func.instruction(&Instruction::LocalGet(src));
                        emit_call(func, reloc_enabled, import_ids["bytes_from_obj"]);
                        let res = locals[op.out.as_ref().unwrap()];
                        func.instruction(&Instruction::LocalSet(res));
                    }
                    "bytes_from_str" => {
                        let args = op.args.as_ref().unwrap();
                        let src = locals[&args[0]];
                        let encoding = locals[&args[1]];
                        let errors = locals[&args[2]];
                        func.instruction(&Instruction::LocalGet(src));
                        func.instruction(&Instruction::LocalGet(encoding));
                        func.instruction(&Instruction::LocalGet(errors));
                        emit_call(func, reloc_enabled, import_ids["bytes_from_str"]);
                        let res = locals[op.out.as_ref().unwrap()];
                        func.instruction(&Instruction::LocalSet(res));
                    }
                    "bytearray_from_obj" => {
                        let args = op.args.as_ref().unwrap();
                        let src = locals[&args[0]];
                        func.instruction(&Instruction::LocalGet(src));
                        emit_call(func, reloc_enabled, import_ids["bytearray_from_obj"]);
                        let res = locals[op.out.as_ref().unwrap()];
                        func.instruction(&Instruction::LocalSet(res));
                    }
                    "bytearray_from_str" => {
                        let args = op.args.as_ref().unwrap();
                        let src = locals[&args[0]];
                        let encoding = locals[&args[1]];
                        let errors = locals[&args[2]];
                        func.instruction(&Instruction::LocalGet(src));
                        func.instruction(&Instruction::LocalGet(encoding));
                        func.instruction(&Instruction::LocalGet(errors));
                        emit_call(func, reloc_enabled, import_ids["bytearray_from_str"]);
                        let res = locals[op.out.as_ref().unwrap()];
                        func.instruction(&Instruction::LocalSet(res));
                    }
                    "float_from_obj" => {
                        let args = op.args.as_ref().unwrap();
                        let src = locals[&args[0]];
                        func.instruction(&Instruction::LocalGet(src));
                        emit_call(func, reloc_enabled, import_ids["float_from_obj"]);
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
                        emit_call(func, reloc_enabled, import_ids["int_from_obj"]);
                        let res = locals[op.out.as_ref().unwrap()];
                        func.instruction(&Instruction::LocalSet(res));
                    }
                    "intarray_from_seq" => {
                        let args = op.args.as_ref().unwrap();
                        let src = locals[&args[0]];
                        func.instruction(&Instruction::LocalGet(src));
                        emit_call(func, reloc_enabled, import_ids["intarray_from_seq"]);
                        let res = locals[op.out.as_ref().unwrap()];
                        func.instruction(&Instruction::LocalSet(res));
                    }
                    "memoryview_new" => {
                        let args = op.args.as_ref().unwrap();
                        let src = locals[&args[0]];
                        func.instruction(&Instruction::LocalGet(src));
                        emit_call(func, reloc_enabled, import_ids["memoryview_new"]);
                        let res = locals[op.out.as_ref().unwrap()];
                        func.instruction(&Instruction::LocalSet(res));
                    }
                    "memoryview_tobytes" => {
                        let args = op.args.as_ref().unwrap();
                        let src = locals[&args[0]];
                        func.instruction(&Instruction::LocalGet(src));
                        emit_call(func, reloc_enabled, import_ids["memoryview_tobytes"]);
                        let res = locals[op.out.as_ref().unwrap()];
                        func.instruction(&Instruction::LocalSet(res));
                    }
                    "memoryview_cast" => {
                        let args = op.args.as_ref().unwrap();
                        let view = locals[&args[0]];
                        let format = locals[&args[1]];
                        let shape = locals[&args[2]];
                        let has_shape = locals[&args[3]];
                        func.instruction(&Instruction::LocalGet(view));
                        func.instruction(&Instruction::LocalGet(format));
                        func.instruction(&Instruction::LocalGet(shape));
                        func.instruction(&Instruction::LocalGet(has_shape));
                        emit_call(func, reloc_enabled, import_ids["memoryview_cast"]);
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
                        emit_call(func, reloc_enabled, import_ids["buffer2d_new"]);
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
                        emit_call(func, reloc_enabled, import_ids["buffer2d_get"]);
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
                        emit_call(func, reloc_enabled, import_ids["buffer2d_set"]);
                        let res = locals[op.out.as_ref().unwrap()];
                        func.instruction(&Instruction::LocalSet(res));
                    }
                    "buffer2d_matmul" => {
                        let args = op.args.as_ref().unwrap();
                        let lhs = locals[&args[0]];
                        let rhs = locals[&args[1]];
                        func.instruction(&Instruction::LocalGet(lhs));
                        func.instruction(&Instruction::LocalGet(rhs));
                        emit_call(func, reloc_enabled, import_ids["buffer2d_matmul"]);
                        let res = locals[op.out.as_ref().unwrap()];
                        func.instruction(&Instruction::LocalSet(res));
                    }
                    "str_from_obj" => {
                        let args = op.args.as_ref().unwrap();
                        let src = locals[&args[0]];
                        func.instruction(&Instruction::LocalGet(src));
                        emit_call(func, reloc_enabled, import_ids["str_from_obj"]);
                        let res = locals[op.out.as_ref().unwrap()];
                        func.instruction(&Instruction::LocalSet(res));
                    }
                    "repr_from_obj" => {
                        let args = op.args.as_ref().unwrap();
                        let src = locals[&args[0]];
                        func.instruction(&Instruction::LocalGet(src));
                        emit_call(func, reloc_enabled, import_ids["repr_from_obj"]);
                        let res = locals[op.out.as_ref().unwrap()];
                        func.instruction(&Instruction::LocalSet(res));
                    }
                    "ascii_from_obj" => {
                        let args = op.args.as_ref().unwrap();
                        let src = locals[&args[0]];
                        func.instruction(&Instruction::LocalGet(src));
                        emit_call(func, reloc_enabled, import_ids["ascii_from_obj"]);
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
                        emit_call(func, reloc_enabled, import_ids["dataclass_new"]);
                        let res = locals[op.out.as_ref().unwrap()];
                        func.instruction(&Instruction::LocalSet(res));
                    }
                    "dataclass_get" => {
                        let args = op.args.as_ref().unwrap();
                        let obj = locals[&args[0]];
                        let idx = locals[&args[1]];
                        func.instruction(&Instruction::LocalGet(obj));
                        func.instruction(&Instruction::LocalGet(idx));
                        emit_call(func, reloc_enabled, import_ids["dataclass_get"]);
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
                        emit_call(func, reloc_enabled, import_ids["dataclass_set"]);
                        let res = locals[op.out.as_ref().unwrap()];
                        func.instruction(&Instruction::LocalSet(res));
                    }
                    "dataclass_set_class" => {
                        let args = op.args.as_ref().unwrap();
                        let obj = locals[&args[0]];
                        let class_obj = locals[&args[1]];
                        func.instruction(&Instruction::LocalGet(obj));
                        func.instruction(&Instruction::LocalGet(class_obj));
                        emit_call(func, reloc_enabled, import_ids["dataclass_set_class"]);
                        let res = locals[op.out.as_ref().unwrap()];
                        func.instruction(&Instruction::LocalSet(res));
                    }
                    "class_new" => {
                        let args = op.args.as_ref().unwrap();
                        let name = locals[&args[0]];
                        func.instruction(&Instruction::LocalGet(name));
                        emit_call(func, reloc_enabled, import_ids["class_new"]);
                        let res = locals[op.out.as_ref().unwrap()];
                        func.instruction(&Instruction::LocalSet(res));
                    }
                    "class_set_base" => {
                        let args = op.args.as_ref().unwrap();
                        let class_bits = locals[&args[0]];
                        let base_bits = locals[&args[1]];
                        func.instruction(&Instruction::LocalGet(class_bits));
                        func.instruction(&Instruction::LocalGet(base_bits));
                        emit_call(func, reloc_enabled, import_ids["class_set_base"]);
                        let res = locals[op.out.as_ref().unwrap()];
                        func.instruction(&Instruction::LocalSet(res));
                    }
                    "class_apply_set_name" => {
                        let args = op.args.as_ref().unwrap();
                        let class_bits = locals[&args[0]];
                        func.instruction(&Instruction::LocalGet(class_bits));
                        emit_call(func, reloc_enabled, import_ids["class_apply_set_name"]);
                        let res = locals[op.out.as_ref().unwrap()];
                        func.instruction(&Instruction::LocalSet(res));
                    }
                    "super_new" => {
                        let args = op.args.as_ref().unwrap();
                        let type_bits = locals[&args[0]];
                        let obj_bits = locals[&args[1]];
                        func.instruction(&Instruction::LocalGet(type_bits));
                        func.instruction(&Instruction::LocalGet(obj_bits));
                        emit_call(func, reloc_enabled, import_ids["super_new"]);
                        let res = locals[op.out.as_ref().unwrap()];
                        func.instruction(&Instruction::LocalSet(res));
                    }
                    "builtin_type" => {
                        let args = op.args.as_ref().unwrap();
                        let tag = locals[&args[0]];
                        func.instruction(&Instruction::LocalGet(tag));
                        emit_call(func, reloc_enabled, import_ids["builtin_type"]);
                        let res = locals[op.out.as_ref().unwrap()];
                        func.instruction(&Instruction::LocalSet(res));
                    }
                    "type_of" => {
                        let args = op.args.as_ref().unwrap();
                        let obj = locals[&args[0]];
                        func.instruction(&Instruction::LocalGet(obj));
                        emit_call(func, reloc_enabled, import_ids["type_of"]);
                        let res = locals[op.out.as_ref().unwrap()];
                        func.instruction(&Instruction::LocalSet(res));
                    }
                    "class_layout_version" => {
                        let args = op.args.as_ref().unwrap();
                        let class_bits = locals[&args[0]];
                        func.instruction(&Instruction::LocalGet(class_bits));
                        emit_call(func, reloc_enabled, import_ids["class_layout_version"]);
                        let res = locals[op.out.as_ref().unwrap()];
                        func.instruction(&Instruction::LocalSet(res));
                    }
                    "class_set_layout_version" => {
                        let args = op.args.as_ref().unwrap();
                        let class_bits = locals[&args[0]];
                        let version_bits = locals[&args[1]];
                        func.instruction(&Instruction::LocalGet(class_bits));
                        func.instruction(&Instruction::LocalGet(version_bits));
                        emit_call(func, reloc_enabled, import_ids["class_set_layout_version"]);
                        if let Some(out) = op.out.as_ref() {
                            if out != "none" {
                                let res = locals[out];
                                func.instruction(&Instruction::LocalSet(res));
                            }
                        }
                    }
                    "isinstance" => {
                        let args = op.args.as_ref().unwrap();
                        let obj = locals[&args[0]];
                        let cls = locals[&args[1]];
                        func.instruction(&Instruction::LocalGet(obj));
                        func.instruction(&Instruction::LocalGet(cls));
                        emit_call(func, reloc_enabled, import_ids["isinstance"]);
                        let res = locals[op.out.as_ref().unwrap()];
                        func.instruction(&Instruction::LocalSet(res));
                    }
                    "issubclass" => {
                        let args = op.args.as_ref().unwrap();
                        let sub = locals[&args[0]];
                        let cls = locals[&args[1]];
                        func.instruction(&Instruction::LocalGet(sub));
                        func.instruction(&Instruction::LocalGet(cls));
                        emit_call(func, reloc_enabled, import_ids["issubclass"]);
                        let res = locals[op.out.as_ref().unwrap()];
                        func.instruction(&Instruction::LocalSet(res));
                    }
                    "object_new" => {
                        emit_call(func, reloc_enabled, import_ids["object_new"]);
                        let res = locals[op.out.as_ref().unwrap()];
                        func.instruction(&Instruction::LocalSet(res));
                    }
                    "classmethod_new" => {
                        let args = op.args.as_ref().unwrap();
                        let func_bits = locals[&args[0]];
                        func.instruction(&Instruction::LocalGet(func_bits));
                        emit_call(func, reloc_enabled, import_ids["classmethod_new"]);
                        let res = locals[op.out.as_ref().unwrap()];
                        func.instruction(&Instruction::LocalSet(res));
                    }
                    "staticmethod_new" => {
                        let args = op.args.as_ref().unwrap();
                        let func_bits = locals[&args[0]];
                        func.instruction(&Instruction::LocalGet(func_bits));
                        emit_call(func, reloc_enabled, import_ids["staticmethod_new"]);
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
                        emit_call(func, reloc_enabled, import_ids["property_new"]);
                        let res = locals[op.out.as_ref().unwrap()];
                        func.instruction(&Instruction::LocalSet(res));
                    }
                    "object_set_class" => {
                        let args = op.args.as_ref().unwrap();
                        let obj = locals[&args[0]];
                        let class_obj = locals[&args[1]];
                        func.instruction(&Instruction::LocalGet(obj));
                        emit_call(func, reloc_enabled, import_ids["handle_resolve"]);
                        func.instruction(&Instruction::LocalGet(class_obj));
                        emit_call(func, reloc_enabled, import_ids["object_set_class"]);
                        let res = locals[op.out.as_ref().unwrap()];
                        func.instruction(&Instruction::LocalSet(res));
                    }
                    "get_attr_generic_ptr" => {
                        let args = op.args.as_ref().unwrap();
                        let obj = locals[&args[0]];
                        let attr = op.s_value.as_ref().unwrap();
                        let bytes = attr.as_bytes();
                        let data = self.add_data_segment(reloc_enabled, bytes);
                        func.instruction(&Instruction::LocalGet(obj));
                        emit_call(func, reloc_enabled, import_ids["handle_resolve"]);
                        self.emit_data_ptr(reloc_enabled, func_index, func, data);
                        func.instruction(&Instruction::I32WrapI64);
                        func.instruction(&Instruction::I64Const(bytes.len() as i64));
                        emit_call(func, reloc_enabled, import_ids["get_attr_ptr"]);
                        let res = locals[op.out.as_ref().unwrap()];
                        func.instruction(&Instruction::LocalSet(res));
                    }
                    "get_attr_generic_obj" => {
                        let args = op.args.as_ref().unwrap();
                        let obj = locals[&args[0]];
                        let attr = op.s_value.as_ref().unwrap();
                        let bytes = attr.as_bytes();
                        let data = self.add_data_segment(reloc_enabled, bytes);
                        func.instruction(&Instruction::LocalGet(obj));
                        self.emit_data_ptr(reloc_enabled, func_index, func, data);
                        func.instruction(&Instruction::I32WrapI64);
                        func.instruction(&Instruction::I64Const(bytes.len() as i64));
                        emit_call(func, reloc_enabled, import_ids["get_attr_object"]);
                        let res = locals[op.out.as_ref().unwrap()];
                        func.instruction(&Instruction::LocalSet(res));
                    }
                    "get_attr_special_obj" => {
                        let args = op.args.as_ref().unwrap();
                        let obj = locals[&args[0]];
                        let attr = op.s_value.as_ref().unwrap();
                        let bytes = attr.as_bytes();
                        let data = self.add_data_segment(reloc_enabled, bytes);
                        func.instruction(&Instruction::LocalGet(obj));
                        self.emit_data_ptr(reloc_enabled, func_index, func, data);
                        func.instruction(&Instruction::I32WrapI64);
                        func.instruction(&Instruction::I64Const(bytes.len() as i64));
                        emit_call(func, reloc_enabled, import_ids["get_attr_special"]);
                        let res = locals[op.out.as_ref().unwrap()];
                        func.instruction(&Instruction::LocalSet(res));
                    }
                    "set_attr_generic_ptr" => {
                        let args = op.args.as_ref().unwrap();
                        let obj = locals[&args[0]];
                        let val = locals[&args[1]];
                        let attr = op.s_value.as_ref().unwrap();
                        let bytes = attr.as_bytes();
                        let data = self.add_data_segment(reloc_enabled, bytes);
                        func.instruction(&Instruction::LocalGet(obj));
                        emit_call(func, reloc_enabled, import_ids["handle_resolve"]);
                        self.emit_data_ptr(reloc_enabled, func_index, func, data);
                        func.instruction(&Instruction::I32WrapI64);
                        func.instruction(&Instruction::I64Const(bytes.len() as i64));
                        func.instruction(&Instruction::LocalGet(val));
                        emit_call(func, reloc_enabled, import_ids["set_attr_ptr"]);
                        let res = locals[op.out.as_ref().unwrap()];
                        func.instruction(&Instruction::LocalSet(res));
                    }
                    "set_attr_generic_obj" => {
                        let args = op.args.as_ref().unwrap();
                        let obj = locals[&args[0]];
                        let val = locals[&args[1]];
                        let attr = op.s_value.as_ref().unwrap();
                        let bytes = attr.as_bytes();
                        let data = self.add_data_segment(reloc_enabled, bytes);
                        func.instruction(&Instruction::LocalGet(obj));
                        self.emit_data_ptr(reloc_enabled, func_index, func, data);
                        func.instruction(&Instruction::I32WrapI64);
                        func.instruction(&Instruction::I64Const(bytes.len() as i64));
                        func.instruction(&Instruction::LocalGet(val));
                        emit_call(func, reloc_enabled, import_ids["set_attr_object"]);
                        let res = locals[op.out.as_ref().unwrap()];
                        func.instruction(&Instruction::LocalSet(res));
                    }
                    "del_attr_generic_ptr" => {
                        let args = op.args.as_ref().unwrap();
                        let obj = locals[&args[0]];
                        let attr = op.s_value.as_ref().unwrap();
                        let bytes = attr.as_bytes();
                        let data = self.add_data_segment(reloc_enabled, bytes);
                        func.instruction(&Instruction::LocalGet(obj));
                        emit_call(func, reloc_enabled, import_ids["handle_resolve"]);
                        self.emit_data_ptr(reloc_enabled, func_index, func, data);
                        func.instruction(&Instruction::I32WrapI64);
                        func.instruction(&Instruction::I64Const(bytes.len() as i64));
                        emit_call(func, reloc_enabled, import_ids["del_attr_ptr"]);
                        let res = locals[op.out.as_ref().unwrap()];
                        func.instruction(&Instruction::LocalSet(res));
                    }
                    "del_attr_generic_obj" => {
                        let args = op.args.as_ref().unwrap();
                        let obj = locals[&args[0]];
                        let attr = op.s_value.as_ref().unwrap();
                        let bytes = attr.as_bytes();
                        let data = self.add_data_segment(reloc_enabled, bytes);
                        func.instruction(&Instruction::LocalGet(obj));
                        self.emit_data_ptr(reloc_enabled, func_index, func, data);
                        func.instruction(&Instruction::I32WrapI64);
                        func.instruction(&Instruction::I64Const(bytes.len() as i64));
                        emit_call(func, reloc_enabled, import_ids["del_attr_object"]);
                        let res = locals[op.out.as_ref().unwrap()];
                        func.instruction(&Instruction::LocalSet(res));
                    }
                    "get_attr_name" => {
                        let args = op.args.as_ref().unwrap();
                        let obj = locals[&args[0]];
                        let name = locals[&args[1]];
                        func.instruction(&Instruction::LocalGet(obj));
                        func.instruction(&Instruction::LocalGet(name));
                        emit_call(func, reloc_enabled, import_ids["get_attr_name"]);
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
                        emit_call(func, reloc_enabled, import_ids["get_attr_name_default"]);
                        let res = locals[op.out.as_ref().unwrap()];
                        func.instruction(&Instruction::LocalSet(res));
                    }
                    "has_attr_name" => {
                        let args = op.args.as_ref().unwrap();
                        let obj = locals[&args[0]];
                        let name = locals[&args[1]];
                        func.instruction(&Instruction::LocalGet(obj));
                        func.instruction(&Instruction::LocalGet(name));
                        emit_call(func, reloc_enabled, import_ids["has_attr_name"]);
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
                        emit_call(func, reloc_enabled, import_ids["set_attr_name"]);
                        let res = locals[op.out.as_ref().unwrap()];
                        func.instruction(&Instruction::LocalSet(res));
                    }
                    "del_attr_name" => {
                        let args = op.args.as_ref().unwrap();
                        let obj = locals[&args[0]];
                        let name = locals[&args[1]];
                        func.instruction(&Instruction::LocalGet(obj));
                        func.instruction(&Instruction::LocalGet(name));
                        emit_call(func, reloc_enabled, import_ids["del_attr_name"]);
                        let res = locals[op.out.as_ref().unwrap()];
                        func.instruction(&Instruction::LocalSet(res));
                    }
                    "store" => {
                        let args = op.args.as_ref().unwrap();
                        func.instruction(&Instruction::LocalGet(locals[&args[0]]));
                        let obj = locals[&args[0]];
                        let val = locals[&args[1]];
                        let offset = op.value.unwrap();
                        let tmp_addr = locals["__wasm_tmp0"];
                        let tmp_old = locals["__wasm_tmp1"];

                        func.instruction(&Instruction::I64Const(QNAN_TAG_MASK_I64));
                        func.instruction(&Instruction::I64And);
                        func.instruction(&Instruction::I64Const(QNAN_TAG_PTR_I64));
                        func.instruction(&Instruction::I64Eq);
                        func.instruction(&Instruction::If(BlockType::Empty));

                        func.instruction(&Instruction::LocalGet(obj));
                        func.instruction(&Instruction::I64Const(POINTER_MASK as i64));
                        func.instruction(&Instruction::I64And);
                        func.instruction(&Instruction::I64Const(offset));
                        func.instruction(&Instruction::I64Add);
                        func.instruction(&Instruction::LocalSet(tmp_addr));

                        func.instruction(&Instruction::LocalGet(tmp_addr));
                        func.instruction(&Instruction::I32WrapI64);
                        func.instruction(&Instruction::I64Load(wasm_encoder::MemArg {
                            align: 3,
                            offset: 0,
                            memory_index: 0,
                        }));
                        func.instruction(&Instruction::LocalSet(tmp_old));

                        func.instruction(&Instruction::LocalGet(tmp_old));
                        func.instruction(&Instruction::I64Const(QNAN_TAG_MASK_I64));
                        func.instruction(&Instruction::I64And);
                        func.instruction(&Instruction::I64Const(QNAN_TAG_PTR_I64));
                        func.instruction(&Instruction::I64Eq);

                        func.instruction(&Instruction::LocalGet(val));
                        func.instruction(&Instruction::I64Const(QNAN_TAG_MASK_I64));
                        func.instruction(&Instruction::I64And);
                        func.instruction(&Instruction::I64Const(QNAN_TAG_PTR_I64));
                        func.instruction(&Instruction::I64Eq);
                        func.instruction(&Instruction::I32Or);
                        func.instruction(&Instruction::If(BlockType::Empty));

                        func.instruction(&Instruction::LocalGet(obj));
                        func.instruction(&Instruction::I64Const(offset));
                        func.instruction(&Instruction::LocalGet(val));
                        emit_call(func, reloc_enabled, import_ids["object_field_set"]);
                        if let Some(out) = op.out.as_ref() {
                            if out != "none" {
                                func.instruction(&Instruction::LocalSet(locals[out]));
                            } else {
                                func.instruction(&Instruction::Drop);
                            }
                        } else {
                            func.instruction(&Instruction::Drop);
                        }

                        func.instruction(&Instruction::Else);
                        func.instruction(&Instruction::LocalGet(tmp_addr));
                        func.instruction(&Instruction::I32WrapI64);
                        func.instruction(&Instruction::LocalGet(val));
                        func.instruction(&Instruction::I64Store(wasm_encoder::MemArg {
                            align: 3,
                            offset: 0,
                            memory_index: 0,
                        }));
                        if let Some(out) = op.out.as_ref() {
                            if out != "none" {
                                func.instruction(&Instruction::I64Const(box_none()));
                                func.instruction(&Instruction::LocalSet(locals[out]));
                            }
                        }
                        func.instruction(&Instruction::End);

                        func.instruction(&Instruction::Else);
                        func.instruction(&Instruction::LocalGet(obj));
                        func.instruction(&Instruction::I64Const(offset));
                        func.instruction(&Instruction::LocalGet(val));
                        emit_call(func, reloc_enabled, import_ids["object_field_set"]);
                        if let Some(out) = op.out.as_ref() {
                            if out != "none" {
                                func.instruction(&Instruction::LocalSet(locals[out]));
                            } else {
                                func.instruction(&Instruction::Drop);
                            }
                        } else {
                            func.instruction(&Instruction::Drop);
                        }
                        func.instruction(&Instruction::End);
                    }
                    "store_init" => {
                        let args = op.args.as_ref().unwrap();
                        func.instruction(&Instruction::LocalGet(locals[&args[0]]));
                        let obj = locals[&args[0]];
                        let val = locals[&args[1]];
                        let offset = op.value.unwrap();
                        let tmp_addr = locals["__wasm_tmp0"];

                        func.instruction(&Instruction::I64Const(QNAN_TAG_MASK_I64));
                        func.instruction(&Instruction::I64And);
                        func.instruction(&Instruction::I64Const(QNAN_TAG_PTR_I64));
                        func.instruction(&Instruction::I64Eq);
                        func.instruction(&Instruction::If(BlockType::Empty));

                        func.instruction(&Instruction::LocalGet(obj));
                        func.instruction(&Instruction::I64Const(POINTER_MASK as i64));
                        func.instruction(&Instruction::I64And);
                        func.instruction(&Instruction::I64Const(offset));
                        func.instruction(&Instruction::I64Add);
                        func.instruction(&Instruction::LocalSet(tmp_addr));

                        func.instruction(&Instruction::LocalGet(val));
                        func.instruction(&Instruction::I64Const(QNAN_TAG_MASK_I64));
                        func.instruction(&Instruction::I64And);
                        func.instruction(&Instruction::I64Const(QNAN_TAG_PTR_I64));
                        func.instruction(&Instruction::I64Eq);
                        func.instruction(&Instruction::If(BlockType::Empty));

                        func.instruction(&Instruction::LocalGet(obj));
                        func.instruction(&Instruction::I64Const(offset));
                        func.instruction(&Instruction::LocalGet(val));
                        emit_call(func, reloc_enabled, import_ids["object_field_init"]);
                        if let Some(out) = op.out.as_ref() {
                            if out != "none" {
                                func.instruction(&Instruction::LocalSet(locals[out]));
                            } else {
                                func.instruction(&Instruction::Drop);
                            }
                        } else {
                            func.instruction(&Instruction::Drop);
                        }

                        func.instruction(&Instruction::Else);
                        func.instruction(&Instruction::LocalGet(tmp_addr));
                        func.instruction(&Instruction::I32WrapI64);
                        func.instruction(&Instruction::LocalGet(val));
                        func.instruction(&Instruction::I64Store(wasm_encoder::MemArg {
                            align: 3,
                            offset: 0,
                            memory_index: 0,
                        }));
                        if let Some(out) = op.out.as_ref() {
                            if out != "none" {
                                func.instruction(&Instruction::I64Const(box_none()));
                                func.instruction(&Instruction::LocalSet(locals[out]));
                            }
                        }
                        func.instruction(&Instruction::End);

                        func.instruction(&Instruction::Else);
                        func.instruction(&Instruction::LocalGet(obj));
                        func.instruction(&Instruction::I64Const(offset));
                        func.instruction(&Instruction::LocalGet(val));
                        emit_call(func, reloc_enabled, import_ids["object_field_init"]);
                        if let Some(out) = op.out.as_ref() {
                            if out != "none" {
                                func.instruction(&Instruction::LocalSet(locals[out]));
                            } else {
                                func.instruction(&Instruction::Drop);
                            }
                        } else {
                            func.instruction(&Instruction::Drop);
                        }
                        func.instruction(&Instruction::End);
                    }
                    "load" => {
                        let args = op.args.as_ref().unwrap();
                        let obj = locals[&args[0]];
                        let offset = op.value.unwrap();
                        let tmp_addr = locals["__wasm_tmp0"];
                        let tmp_val = locals["__wasm_tmp1"];
                        let out = locals[op.out.as_ref().unwrap()];

                        func.instruction(&Instruction::LocalGet(obj));
                        func.instruction(&Instruction::I64Const(QNAN_TAG_MASK_I64));
                        func.instruction(&Instruction::I64And);
                        func.instruction(&Instruction::I64Const(QNAN_TAG_PTR_I64));
                        func.instruction(&Instruction::I64Eq);
                        func.instruction(&Instruction::If(BlockType::Empty));

                        func.instruction(&Instruction::LocalGet(obj));
                        func.instruction(&Instruction::I64Const(POINTER_MASK as i64));
                        func.instruction(&Instruction::I64And);
                        func.instruction(&Instruction::I64Const(offset));
                        func.instruction(&Instruction::I64Add);
                        func.instruction(&Instruction::LocalSet(tmp_addr));

                        func.instruction(&Instruction::LocalGet(tmp_addr));
                        func.instruction(&Instruction::I32WrapI64);
                        func.instruction(&Instruction::I64Load(wasm_encoder::MemArg {
                            align: 3,
                            offset: 0,
                            memory_index: 0,
                        }));
                        func.instruction(&Instruction::LocalSet(tmp_val));

                        func.instruction(&Instruction::LocalGet(tmp_val));
                        func.instruction(&Instruction::I64Const(QNAN_TAG_MASK_I64));
                        func.instruction(&Instruction::I64And);
                        func.instruction(&Instruction::I64Const(QNAN_TAG_PTR_I64));
                        func.instruction(&Instruction::I64Eq);
                        func.instruction(&Instruction::If(BlockType::Empty));

                        func.instruction(&Instruction::LocalGet(tmp_val));
                        emit_call(func, reloc_enabled, import_ids["inc_ref_obj"]);
                        func.instruction(&Instruction::LocalGet(tmp_val));
                        func.instruction(&Instruction::LocalSet(out));

                        func.instruction(&Instruction::Else);
                        func.instruction(&Instruction::LocalGet(tmp_val));
                        func.instruction(&Instruction::LocalSet(out));
                        func.instruction(&Instruction::End);

                        func.instruction(&Instruction::Else);
                        func.instruction(&Instruction::LocalGet(obj));
                        func.instruction(&Instruction::I64Const(offset));
                        emit_call(func, reloc_enabled, import_ids["object_field_get"]);
                        func.instruction(&Instruction::LocalSet(out));
                        func.instruction(&Instruction::End);
                    }
                    "closure_load" => {
                        let args = op.args.as_ref().unwrap();
                        let obj = locals[&args[0]];
                        let tmp_ptr = locals["__molt_tmp0"];
                        func.instruction(&Instruction::LocalGet(obj));
                        emit_call(func, reloc_enabled, import_ids["handle_resolve"]);
                        func.instruction(&Instruction::I64ExtendI32U);
                        func.instruction(&Instruction::LocalSet(tmp_ptr));
                        func.instruction(&Instruction::LocalGet(tmp_ptr));
                        func.instruction(&Instruction::I32WrapI64);
                        func.instruction(&Instruction::I64Const(op.value.unwrap()));
                        emit_call(func, reloc_enabled, import_ids["closure_load"]);
                        func.instruction(&Instruction::LocalSet(locals[op.out.as_ref().unwrap()]));
                    }
                    "closure_store" => {
                        let args = op.args.as_ref().unwrap();
                        let obj = locals[&args[0]];
                        let tmp_ptr = locals["__molt_tmp0"];
                        func.instruction(&Instruction::LocalGet(obj));
                        emit_call(func, reloc_enabled, import_ids["handle_resolve"]);
                        func.instruction(&Instruction::I64ExtendI32U);
                        func.instruction(&Instruction::LocalSet(tmp_ptr));
                        func.instruction(&Instruction::LocalGet(tmp_ptr));
                        func.instruction(&Instruction::I32WrapI64);
                        func.instruction(&Instruction::I64Const(op.value.unwrap()));
                        func.instruction(&Instruction::LocalGet(locals[&args[1]]));
                        emit_call(func, reloc_enabled, import_ids["closure_store"]);
                        if let Some(out) = op.out.as_ref() {
                            func.instruction(&Instruction::LocalSet(locals[out]));
                        } else {
                            func.instruction(&Instruction::Drop);
                        }
                    }
                    "guarded_load" => {
                        let args = op.args.as_ref().unwrap();
                        let obj = locals[&args[0]];
                        let offset = op.value.unwrap();
                        let tmp_addr = locals["__wasm_tmp0"];
                        let tmp_val = locals["__wasm_tmp1"];
                        let out = locals[op.out.as_ref().unwrap()];

                        func.instruction(&Instruction::LocalGet(obj));
                        func.instruction(&Instruction::I64Const(QNAN_TAG_MASK_I64));
                        func.instruction(&Instruction::I64And);
                        func.instruction(&Instruction::I64Const(QNAN_TAG_PTR_I64));
                        func.instruction(&Instruction::I64Eq);
                        func.instruction(&Instruction::If(BlockType::Empty));

                        func.instruction(&Instruction::LocalGet(obj));
                        func.instruction(&Instruction::I64Const(POINTER_MASK as i64));
                        func.instruction(&Instruction::I64And);
                        func.instruction(&Instruction::I64Const(offset));
                        func.instruction(&Instruction::I64Add);
                        func.instruction(&Instruction::LocalSet(tmp_addr));

                        func.instruction(&Instruction::LocalGet(tmp_addr));
                        func.instruction(&Instruction::I32WrapI64);
                        func.instruction(&Instruction::I64Load(wasm_encoder::MemArg {
                            align: 3,
                            offset: 0,
                            memory_index: 0,
                        }));
                        func.instruction(&Instruction::LocalSet(tmp_val));

                        func.instruction(&Instruction::LocalGet(tmp_val));
                        func.instruction(&Instruction::I64Const(QNAN_TAG_MASK_I64));
                        func.instruction(&Instruction::I64And);
                        func.instruction(&Instruction::I64Const(QNAN_TAG_PTR_I64));
                        func.instruction(&Instruction::I64Eq);
                        func.instruction(&Instruction::If(BlockType::Empty));

                        func.instruction(&Instruction::LocalGet(tmp_val));
                        emit_call(func, reloc_enabled, import_ids["inc_ref_obj"]);
                        func.instruction(&Instruction::LocalGet(tmp_val));
                        func.instruction(&Instruction::LocalSet(out));

                        func.instruction(&Instruction::Else);
                        func.instruction(&Instruction::LocalGet(tmp_val));
                        func.instruction(&Instruction::LocalSet(out));
                        func.instruction(&Instruction::End);

                        func.instruction(&Instruction::Else);
                        func.instruction(&Instruction::LocalGet(obj));
                        func.instruction(&Instruction::I64Const(offset));
                        emit_call(func, reloc_enabled, import_ids["object_field_get"]);
                        func.instruction(&Instruction::LocalSet(out));
                        func.instruction(&Instruction::End);
                    }
                    "guarded_field_get" => {
                        let args = op.args.as_ref().unwrap();
                        let obj = locals[&args[0]];
                        let class_bits = locals[&args[1]];
                        let expected = locals[&args[2]];
                        let tmp_ptr = locals["__wasm_tmp0"];
                        let tmp_val = locals["__wasm_tmp1"];
                        let guard_val = locals["__molt_tmp0"];
                        let attr = op.s_value.as_ref().unwrap();
                        let bytes = attr.as_bytes();
                        let data = self.add_data_segment(reloc_enabled, bytes);
                        func.instruction(&Instruction::LocalGet(obj));
                        emit_call(func, reloc_enabled, import_ids["handle_resolve"]);
                        func.instruction(&Instruction::I64ExtendI32U);
                        func.instruction(&Instruction::LocalSet(tmp_ptr));

                        func.instruction(&Instruction::LocalGet(tmp_ptr));
                        func.instruction(&Instruction::I32WrapI64);
                        func.instruction(&Instruction::LocalGet(class_bits));
                        func.instruction(&Instruction::LocalGet(expected));
                        emit_call(func, reloc_enabled, import_ids["guard_layout_ptr"]);
                        func.instruction(&Instruction::LocalSet(guard_val));

                        func.instruction(&Instruction::LocalGet(guard_val));
                        func.instruction(&Instruction::I64Const(box_bool(1)));
                        func.instruction(&Instruction::I64Eq);
                        func.instruction(&Instruction::If(BlockType::Empty));

                        func.instruction(&Instruction::LocalGet(tmp_ptr));
                        func.instruction(&Instruction::I64Const(op.value.unwrap()));
                        func.instruction(&Instruction::I64Add);
                        func.instruction(&Instruction::I32WrapI64);
                        func.instruction(&Instruction::I64Load(wasm_encoder::MemArg {
                            align: 3,
                            offset: 0,
                            memory_index: 0,
                        }));
                        func.instruction(&Instruction::LocalSet(tmp_val));

                        func.instruction(&Instruction::LocalGet(tmp_val));
                        func.instruction(&Instruction::I64Const(QNAN_TAG_MASK_I64));
                        func.instruction(&Instruction::I64And);
                        func.instruction(&Instruction::I64Const(QNAN_TAG_PTR_I64));
                        func.instruction(&Instruction::I64Eq);
                        func.instruction(&Instruction::If(BlockType::Empty));

                        func.instruction(&Instruction::LocalGet(tmp_val));
                        emit_call(func, reloc_enabled, import_ids["inc_ref_obj"]);
                        func.instruction(&Instruction::LocalGet(tmp_val));
                        func.instruction(&Instruction::LocalSet(locals[op.out.as_ref().unwrap()]));

                        func.instruction(&Instruction::Else);
                        func.instruction(&Instruction::LocalGet(tmp_val));
                        func.instruction(&Instruction::LocalSet(locals[op.out.as_ref().unwrap()]));
                        func.instruction(&Instruction::End);

                        func.instruction(&Instruction::Else);
                        func.instruction(&Instruction::LocalGet(tmp_ptr));
                        func.instruction(&Instruction::I32WrapI64);
                        func.instruction(&Instruction::LocalGet(class_bits));
                        func.instruction(&Instruction::LocalGet(expected));
                        func.instruction(&Instruction::I64Const(op.value.unwrap()));
                        self.emit_data_ptr(reloc_enabled, func_index, func, data);
                        func.instruction(&Instruction::I32WrapI64);
                        func.instruction(&Instruction::I64Const(bytes.len() as i64));
                        emit_call(func, reloc_enabled, import_ids["guarded_field_get_ptr"]);
                        func.instruction(&Instruction::LocalSet(locals[op.out.as_ref().unwrap()]));
                        func.instruction(&Instruction::End);
                    }
                    "guarded_field_set" => {
                        let args = op.args.as_ref().unwrap();
                        let obj = locals[&args[0]];
                        let class_bits = locals[&args[1]];
                        let expected = locals[&args[2]];
                        let val = locals[&args[3]];
                        let tmp_ptr = locals["__wasm_tmp0"];
                        let tmp_old = locals["__wasm_tmp1"];
                        let guard_val = locals["__molt_tmp0"];
                        let tmp_addr = locals["__molt_tmp1"];
                        let attr = op.s_value.as_ref().unwrap();
                        let bytes = attr.as_bytes();
                        let data = self.add_data_segment(reloc_enabled, bytes);
                        func.instruction(&Instruction::LocalGet(obj));
                        emit_call(func, reloc_enabled, import_ids["handle_resolve"]);
                        func.instruction(&Instruction::I64ExtendI32U);
                        func.instruction(&Instruction::LocalSet(tmp_ptr));

                        func.instruction(&Instruction::LocalGet(tmp_ptr));
                        func.instruction(&Instruction::I32WrapI64);
                        func.instruction(&Instruction::LocalGet(class_bits));
                        func.instruction(&Instruction::LocalGet(expected));
                        emit_call(func, reloc_enabled, import_ids["guard_layout_ptr"]);
                        func.instruction(&Instruction::LocalSet(guard_val));

                        func.instruction(&Instruction::LocalGet(guard_val));
                        func.instruction(&Instruction::I64Const(box_bool(1)));
                        func.instruction(&Instruction::I64Eq);
                        func.instruction(&Instruction::If(BlockType::Empty));

                        func.instruction(&Instruction::LocalGet(tmp_ptr));
                        func.instruction(&Instruction::I64Const(op.value.unwrap()));
                        func.instruction(&Instruction::I64Add);
                        func.instruction(&Instruction::LocalSet(tmp_addr));

                        func.instruction(&Instruction::LocalGet(tmp_addr));
                        func.instruction(&Instruction::I32WrapI64);
                        func.instruction(&Instruction::I64Load(wasm_encoder::MemArg {
                            align: 3,
                            offset: 0,
                            memory_index: 0,
                        }));
                        func.instruction(&Instruction::LocalSet(tmp_old));

                        func.instruction(&Instruction::LocalGet(tmp_old));
                        func.instruction(&Instruction::I64Const(QNAN_TAG_MASK_I64));
                        func.instruction(&Instruction::I64And);
                        func.instruction(&Instruction::I64Const(QNAN_TAG_PTR_I64));
                        func.instruction(&Instruction::I64Eq);

                        func.instruction(&Instruction::LocalGet(val));
                        func.instruction(&Instruction::I64Const(QNAN_TAG_MASK_I64));
                        func.instruction(&Instruction::I64And);
                        func.instruction(&Instruction::I64Const(QNAN_TAG_PTR_I64));
                        func.instruction(&Instruction::I64Eq);
                        func.instruction(&Instruction::I32Or);
                        func.instruction(&Instruction::If(BlockType::Empty));

                        func.instruction(&Instruction::LocalGet(tmp_ptr));
                        func.instruction(&Instruction::I32WrapI64);
                        func.instruction(&Instruction::I64Const(op.value.unwrap()));
                        func.instruction(&Instruction::LocalGet(val));
                        emit_call(func, reloc_enabled, import_ids["object_field_set_ptr"]);
                        if let Some(out) = op.out.as_ref() {
                            if out != "none" {
                                func.instruction(&Instruction::LocalSet(locals[out]));
                            } else {
                                func.instruction(&Instruction::Drop);
                            }
                        } else {
                            func.instruction(&Instruction::Drop);
                        }

                        func.instruction(&Instruction::Else);
                        func.instruction(&Instruction::LocalGet(tmp_addr));
                        func.instruction(&Instruction::I32WrapI64);
                        func.instruction(&Instruction::LocalGet(val));
                        func.instruction(&Instruction::I64Store(wasm_encoder::MemArg {
                            align: 3,
                            offset: 0,
                            memory_index: 0,
                        }));
                        if let Some(out) = op.out.as_ref() {
                            if out != "none" {
                                func.instruction(&Instruction::I64Const(box_none()));
                                func.instruction(&Instruction::LocalSet(locals[out]));
                            }
                        }
                        func.instruction(&Instruction::End);

                        func.instruction(&Instruction::Else);
                        func.instruction(&Instruction::LocalGet(tmp_ptr));
                        func.instruction(&Instruction::I32WrapI64);
                        func.instruction(&Instruction::LocalGet(class_bits));
                        func.instruction(&Instruction::LocalGet(expected));
                        func.instruction(&Instruction::I64Const(op.value.unwrap()));
                        func.instruction(&Instruction::LocalGet(val));
                        self.emit_data_ptr(reloc_enabled, func_index, func, data);
                        func.instruction(&Instruction::I32WrapI64);
                        func.instruction(&Instruction::I64Const(bytes.len() as i64));
                        emit_call(func, reloc_enabled, import_ids["guarded_field_set_ptr"]);
                        if let Some(out) = op.out.as_ref() {
                            if out != "none" {
                                func.instruction(&Instruction::LocalSet(locals[out]));
                            } else {
                                func.instruction(&Instruction::Drop);
                            }
                        } else {
                            func.instruction(&Instruction::Drop);
                        }
                        func.instruction(&Instruction::End);
                    }
                    "guarded_field_init" => {
                        let args = op.args.as_ref().unwrap();
                        let obj = locals[&args[0]];
                        let class_bits = locals[&args[1]];
                        let expected = locals[&args[2]];
                        let val = locals[&args[3]];
                        let tmp_ptr = locals["__wasm_tmp0"];
                        let guard_val = locals["__molt_tmp0"];
                        let attr = op.s_value.as_ref().unwrap();
                        let bytes = attr.as_bytes();
                        let data = self.add_data_segment(reloc_enabled, bytes);
                        func.instruction(&Instruction::LocalGet(obj));
                        emit_call(func, reloc_enabled, import_ids["handle_resolve"]);
                        func.instruction(&Instruction::I64ExtendI32U);
                        func.instruction(&Instruction::LocalSet(tmp_ptr));

                        func.instruction(&Instruction::LocalGet(tmp_ptr));
                        func.instruction(&Instruction::I32WrapI64);
                        func.instruction(&Instruction::LocalGet(class_bits));
                        func.instruction(&Instruction::LocalGet(expected));
                        emit_call(func, reloc_enabled, import_ids["guard_layout_ptr"]);
                        func.instruction(&Instruction::LocalSet(guard_val));

                        func.instruction(&Instruction::LocalGet(guard_val));
                        func.instruction(&Instruction::I64Const(box_bool(1)));
                        func.instruction(&Instruction::I64Eq);
                        func.instruction(&Instruction::If(BlockType::Empty));

                        func.instruction(&Instruction::LocalGet(val));
                        func.instruction(&Instruction::I64Const(QNAN_TAG_MASK_I64));
                        func.instruction(&Instruction::I64And);
                        func.instruction(&Instruction::I64Const(QNAN_TAG_PTR_I64));
                        func.instruction(&Instruction::I64Eq);
                        func.instruction(&Instruction::If(BlockType::Empty));

                        func.instruction(&Instruction::LocalGet(tmp_ptr));
                        func.instruction(&Instruction::I32WrapI64);
                        func.instruction(&Instruction::I64Const(op.value.unwrap()));
                        func.instruction(&Instruction::LocalGet(val));
                        emit_call(func, reloc_enabled, import_ids["object_field_init_ptr"]);
                        if let Some(out) = op.out.as_ref() {
                            if out != "none" {
                                func.instruction(&Instruction::LocalSet(locals[out]));
                            } else {
                                func.instruction(&Instruction::Drop);
                            }
                        } else {
                            func.instruction(&Instruction::Drop);
                        }

                        func.instruction(&Instruction::Else);
                        func.instruction(&Instruction::LocalGet(tmp_ptr));
                        func.instruction(&Instruction::I64Const(op.value.unwrap()));
                        func.instruction(&Instruction::I64Add);
                        func.instruction(&Instruction::I32WrapI64);
                        func.instruction(&Instruction::LocalGet(val));
                        func.instruction(&Instruction::I64Store(wasm_encoder::MemArg {
                            align: 3,
                            offset: 0,
                            memory_index: 0,
                        }));
                        if let Some(out) = op.out.as_ref() {
                            if out != "none" {
                                func.instruction(&Instruction::I64Const(box_none()));
                                func.instruction(&Instruction::LocalSet(locals[out]));
                            }
                        }
                        func.instruction(&Instruction::End);

                        func.instruction(&Instruction::Else);
                        func.instruction(&Instruction::LocalGet(tmp_ptr));
                        func.instruction(&Instruction::I32WrapI64);
                        func.instruction(&Instruction::LocalGet(class_bits));
                        func.instruction(&Instruction::LocalGet(expected));
                        func.instruction(&Instruction::I64Const(op.value.unwrap()));
                        func.instruction(&Instruction::LocalGet(val));
                        self.emit_data_ptr(reloc_enabled, func_index, func, data);
                        func.instruction(&Instruction::I32WrapI64);
                        func.instruction(&Instruction::I64Const(bytes.len() as i64));
                        emit_call(func, reloc_enabled, import_ids["guarded_field_init_ptr"]);
                        if let Some(out) = op.out.as_ref() {
                            if out != "none" {
                                func.instruction(&Instruction::LocalSet(locals[out]));
                            } else {
                                func.instruction(&Instruction::Drop);
                            }
                        } else {
                            func.instruction(&Instruction::Drop);
                        }
                        func.instruction(&Instruction::End);
                    }
                    "state_switch" => {}
                    "state_transition" => {
                        let args = op.args.as_ref().unwrap();
                        let future = locals[&args[0]];
                        let slot_bits = args.get(1).map(|name| locals[name]);
                        let out = locals[op.out.as_ref().unwrap()];
                        let self_ptr = locals["__molt_tmp0"];
                        func.instruction(&Instruction::LocalGet(0));
                        emit_call(func, reloc_enabled, import_ids["handle_resolve"]);
                        func.instruction(&Instruction::I64ExtendI32U);
                        func.instruction(&Instruction::LocalSet(self_ptr));
                        func.instruction(&Instruction::LocalGet(self_ptr));
                        func.instruction(&Instruction::I32WrapI64);
                        func.instruction(&Instruction::I32Const(HEADER_STATE_OFFSET));
                        func.instruction(&Instruction::I32Add);
                        func.instruction(&Instruction::I64Const(op.value.unwrap()));
                        func.instruction(&Instruction::I64Store(wasm_encoder::MemArg {
                            align: 3,
                            offset: 0,
                            memory_index: 0,
                        }));
                        func.instruction(&Instruction::LocalGet(future));
                        emit_call(func, reloc_enabled, import_ids["future_poll"]);
                        func.instruction(&Instruction::LocalSet(out));
                        if let Some(slot) = slot_bits {
                            func.instruction(&Instruction::LocalGet(self_ptr));
                            func.instruction(&Instruction::I32WrapI64);
                            func.instruction(&Instruction::LocalGet(slot));
                            func.instruction(&Instruction::I64Const(INT_MASK as i64));
                            func.instruction(&Instruction::I64And);
                            func.instruction(&Instruction::LocalGet(out));
                            emit_call(func, reloc_enabled, import_ids["closure_store"]);
                            func.instruction(&Instruction::Drop);
                        }
                        func.instruction(&Instruction::LocalGet(out));
                        func.instruction(&Instruction::I64Const(box_pending()));
                        func.instruction(&Instruction::I64Eq);
                        func.instruction(&Instruction::If(BlockType::Empty));
                        func.instruction(&Instruction::LocalGet(self_ptr));
                        func.instruction(&Instruction::I32WrapI64);
                        func.instruction(&Instruction::LocalGet(future));
                        emit_call(func, reloc_enabled, import_ids["handle_resolve"]);
                        emit_call(func, reloc_enabled, import_ids["sleep_register"]);
                        func.instruction(&Instruction::Drop);
                        func.instruction(&Instruction::I64Const(box_pending()));
                        func.instruction(&Instruction::Return);
                        func.instruction(&Instruction::End);
                    }
                    "call_async" => {
                        let payload_len = op.args.as_ref().map(|args| args.len()).unwrap_or(0);
                        let table_slot = func_map[op.s_value.as_ref().unwrap()];
                        let table_idx = table_base + table_slot;
                        emit_table_index_i64(func, reloc_enabled, table_idx);
                        func.instruction(&Instruction::I64Const((payload_len * 8) as i64));
                        func.instruction(&Instruction::I64Const(TASK_KIND_FUTURE));
                        emit_call(func, reloc_enabled, import_ids["task_new"]);
                        let res = locals[op.out.as_ref().unwrap()];
                        func.instruction(&Instruction::LocalSet(res));
                        if let Some(args) = op.args.as_ref() {
                            for (idx, arg) in args.iter().enumerate() {
                                let arg_val = locals[arg];
                                func.instruction(&Instruction::LocalGet(res));
                                emit_call(func, reloc_enabled, import_ids["handle_resolve"]);
                                func.instruction(&Instruction::I32Const((idx * 8) as i32));
                                func.instruction(&Instruction::I32Add);
                                func.instruction(&Instruction::LocalGet(arg_val));
                                func.instruction(&Instruction::I64Store(wasm_encoder::MemArg {
                                    align: 3,
                                    offset: 0,
                                    memory_index: 0,
                                }));
                                func.instruction(&Instruction::LocalGet(arg_val));
                                emit_call(func, reloc_enabled, import_ids["inc_ref_obj"]);
                            }
                        }
                    }
                    "call" => {
                        let target_name = op.s_value.as_ref().unwrap();
                        let args_names = op.args.as_ref().unwrap();
                        let out = locals[op.out.as_ref().unwrap()];
                        let func_idx = *func_indices
                            .get(target_name)
                            .expect("call target not found");
                        emit_call(func, reloc_enabled, import_ids["recursion_guard_enter"]);
                        func.instruction(&Instruction::I64Const(0));
                        func.instruction(&Instruction::I64Ne);
                        func.instruction(&Instruction::If(BlockType::Empty));
                        let code_id = op.value.unwrap_or(0);
                        func.instruction(&Instruction::I64Const(code_id));
                        emit_call(func, reloc_enabled, import_ids["trace_enter_slot"]);
                        func.instruction(&Instruction::Drop);
                        for arg_name in args_names {
                            let arg = locals[arg_name];
                            func.instruction(&Instruction::LocalGet(arg));
                        }
                        emit_call(func, reloc_enabled, func_idx);
                        func.instruction(&Instruction::LocalSet(out));
                        emit_call(func, reloc_enabled, import_ids["trace_exit"]);
                        func.instruction(&Instruction::Drop);
                        emit_call(func, reloc_enabled, import_ids["recursion_guard_exit"]);
                        func.instruction(&Instruction::Else);
                        func.instruction(&Instruction::I64Const(box_none()));
                        func.instruction(&Instruction::LocalSet(out));
                        func.instruction(&Instruction::End);
                    }
                    "func_new" => {
                        let func_name = op.s_value.as_ref().unwrap();
                        let arity = op.value.unwrap_or(0);
                        let table_slot = func_map[func_name];
                        let table_idx = table_base + table_slot;
                        let tramp_slot = trampoline_map[func_name];
                        let tramp_idx = table_base + tramp_slot;
                        emit_table_index_i64(func, reloc_enabled, table_idx);
                        emit_table_index_i64(func, reloc_enabled, tramp_idx);
                        func.instruction(&Instruction::I64Const(arity));
                        emit_call(func, reloc_enabled, import_ids["func_new"]);
                        let res = locals[op.out.as_ref().unwrap()];
                        func.instruction(&Instruction::LocalSet(res));
                    }
                    "func_new_closure" => {
                        let func_name = op.s_value.as_ref().unwrap();
                        let arity = op.value.unwrap_or(0);
                        let closure_name = op
                            .args
                            .as_ref()
                            .and_then(|args| args.first())
                            .expect("func_new_closure expects closure arg");
                        let closure_bits = locals[closure_name];
                        let table_slot = func_map[func_name];
                        let table_idx = table_base + table_slot;
                        let tramp_slot = trampoline_map[func_name];
                        let tramp_idx = table_base + tramp_slot;
                        emit_table_index_i64(func, reloc_enabled, table_idx);
                        emit_table_index_i64(func, reloc_enabled, tramp_idx);
                        func.instruction(&Instruction::I64Const(arity));
                        func.instruction(&Instruction::LocalGet(closure_bits));
                        emit_call(func, reloc_enabled, import_ids["func_new_closure"]);
                        let res = locals[op.out.as_ref().unwrap()];
                        func.instruction(&Instruction::LocalSet(res));
                    }
                    "code_new" => {
                        let args = op.args.as_ref().unwrap();
                        let filename_bits = locals[&args[0]];
                        let name_bits = locals[&args[1]];
                        let firstlineno_bits = locals[&args[2]];
                        let linetable_bits = locals[&args[3]];
                        func.instruction(&Instruction::LocalGet(filename_bits));
                        func.instruction(&Instruction::LocalGet(name_bits));
                        func.instruction(&Instruction::LocalGet(firstlineno_bits));
                        func.instruction(&Instruction::LocalGet(linetable_bits));
                        emit_call(func, reloc_enabled, import_ids["code_new"]);
                        let res = locals[op.out.as_ref().unwrap()];
                        func.instruction(&Instruction::LocalSet(res));
                    }
                    "code_slot_set" => {
                        let args = op.args.as_ref().unwrap();
                        let code_bits = locals[&args[0]];
                        let code_id = op.value.unwrap_or(0);
                        func.instruction(&Instruction::I64Const(code_id));
                        func.instruction(&Instruction::LocalGet(code_bits));
                        emit_call(func, reloc_enabled, import_ids["code_slot_set"]);
                        func.instruction(&Instruction::Drop);
                    }
                    "code_slots_init" => {
                        let count = op.value.unwrap_or(0);
                        func.instruction(&Instruction::I64Const(count));
                        emit_call(func, reloc_enabled, import_ids["code_slots_init"]);
                        func.instruction(&Instruction::Drop);
                    }
                    "line" => {
                        let line = op.value.unwrap_or(0);
                        func.instruction(&Instruction::I64Const(line));
                        emit_call(func, reloc_enabled, import_ids["trace_set_line"]);
                        func.instruction(&Instruction::Drop);
                    }
                    "builtin_func" => {
                        let func_name = op.s_value.as_ref().unwrap();
                        let arity = op.value.unwrap_or(0);
                        let table_slot = func_map[func_name];
                        let table_idx = table_base + table_slot;
                        let tramp_slot = trampoline_map[func_name];
                        let tramp_idx = table_base + tramp_slot;
                        emit_table_index_i64(func, reloc_enabled, table_idx);
                        emit_table_index_i64(func, reloc_enabled, tramp_idx);
                        func.instruction(&Instruction::I64Const(arity));
                        emit_call(func, reloc_enabled, import_ids["func_new"]);
                        let res = locals[op.out.as_ref().unwrap()];
                        func.instruction(&Instruction::LocalSet(res));
                    }
                    "missing" => {
                        let out = locals[op.out.as_ref().unwrap()];
                        emit_call(func, reloc_enabled, import_ids["missing"]);
                        func.instruction(&Instruction::LocalSet(out));
                    }
                    "function_closure_bits" => {
                        let args = op.args.as_ref().unwrap();
                        let func_bits = locals[&args[0]];
                        let out = locals[op.out.as_ref().unwrap()];
                        func.instruction(&Instruction::LocalGet(func_bits));
                        emit_call(func, reloc_enabled, import_ids["function_closure_bits"]);
                        func.instruction(&Instruction::LocalSet(out));
                        func.instruction(&Instruction::LocalGet(out));
                        emit_call(func, reloc_enabled, import_ids["inc_ref_obj"]);
                    }
                    "bound_method_new" => {
                        let args = op.args.as_ref().unwrap();
                        let func_bits = locals[&args[0]];
                        let self_bits = locals[&args[1]];
                        func.instruction(&Instruction::LocalGet(func_bits));
                        func.instruction(&Instruction::LocalGet(self_bits));
                        emit_call(func, reloc_enabled, import_ids["bound_method_new"]);
                        let res = locals[op.out.as_ref().unwrap()];
                        func.instruction(&Instruction::LocalSet(res));
                    }
                    "call_func" => {
                        let args_names = op.args.as_ref().unwrap();
                        let func_bits = locals[&args_names[0]];
                        let out = locals[op.out.as_ref().unwrap()];
                        let callargs_tmp = locals["__molt_tmp0"];
                        let arity = args_names.len().saturating_sub(1);
                        func.instruction(&Instruction::I64Const(arity as i64));
                        func.instruction(&Instruction::I64Const(0));
                        emit_call(func, reloc_enabled, import_ids["callargs_new"]);
                        func.instruction(&Instruction::LocalSet(callargs_tmp));
                        for arg_name in &args_names[1..] {
                            let arg = locals[arg_name];
                            func.instruction(&Instruction::LocalGet(callargs_tmp));
                            func.instruction(&Instruction::LocalGet(arg));
                            emit_call(func, reloc_enabled, import_ids["callargs_push_pos"]);
                            func.instruction(&Instruction::Drop);
                        }
                        func.instruction(&Instruction::LocalGet(func_bits));
                        func.instruction(&Instruction::LocalGet(callargs_tmp));
                        emit_call(func, reloc_enabled, import_ids["call_bind"]);
                        func.instruction(&Instruction::LocalSet(out));
                    }
                    "call_bind" => {
                        let args_names = op.args.as_ref().unwrap();
                        let func_bits = locals[&args_names[0]];
                        let builder_ptr = locals[&args_names[1]];
                        let out = locals[op.out.as_ref().unwrap()];
                        func.instruction(&Instruction::LocalGet(func_bits));
                        func.instruction(&Instruction::LocalGet(builder_ptr));
                        emit_call(func, reloc_enabled, import_ids["call_bind"]);
                        func.instruction(&Instruction::LocalSet(out));
                    }
                    "call_method" => {
                        let args_names = op.args.as_ref().unwrap();
                        let method_bits = locals[&args_names[0]];
                        let out = locals[op.out.as_ref().unwrap()];
                        let callargs_tmp = locals["__molt_tmp0"];
                        let arity = args_names.len().saturating_sub(1);
                        func.instruction(&Instruction::I64Const(arity as i64));
                        func.instruction(&Instruction::I64Const(0));
                        emit_call(func, reloc_enabled, import_ids["callargs_new"]);
                        func.instruction(&Instruction::LocalSet(callargs_tmp));
                        for arg_name in &args_names[1..] {
                            let arg = locals[arg_name];
                            func.instruction(&Instruction::LocalGet(callargs_tmp));
                            func.instruction(&Instruction::LocalGet(arg));
                            emit_call(func, reloc_enabled, import_ids["callargs_push_pos"]);
                            func.instruction(&Instruction::Drop);
                        }
                        func.instruction(&Instruction::LocalGet(method_bits));
                        func.instruction(&Instruction::LocalGet(callargs_tmp));
                        emit_call(func, reloc_enabled, import_ids["call_bind"]);
                        func.instruction(&Instruction::LocalSet(out));
                    }
                    "chan_new" => {
                        let args = op.args.as_ref().unwrap();
                        let cap = locals[&args[0]];
                        func.instruction(&Instruction::LocalGet(cap));
                        emit_call(func, reloc_enabled, import_ids["chan_new"]);
                        func.instruction(&Instruction::I64ExtendI32U);
                        func.instruction(&Instruction::LocalSet(locals[op.out.as_ref().unwrap()]));
                    }
                    "chan_drop" => {
                        let args = op.args.as_ref().unwrap();
                        let chan = locals[&args[0]];
                        func.instruction(&Instruction::LocalGet(chan));
                        func.instruction(&Instruction::I32WrapI64);
                        emit_call(func, reloc_enabled, import_ids["chan_drop"]);
                    }
                    "module_new" => {
                        let args = op.args.as_ref().unwrap();
                        let name = locals[&args[0]];
                        func.instruction(&Instruction::LocalGet(name));
                        emit_call(func, reloc_enabled, import_ids["module_new"]);
                        let res = locals[op.out.as_ref().unwrap()];
                        func.instruction(&Instruction::LocalSet(res));
                    }
                    "module_cache_get" => {
                        let args = op.args.as_ref().unwrap();
                        let name = locals[&args[0]];
                        func.instruction(&Instruction::LocalGet(name));
                        emit_call(func, reloc_enabled, import_ids["module_cache_get"]);
                        let res = locals[op.out.as_ref().unwrap()];
                        func.instruction(&Instruction::LocalSet(res));
                    }
                    "module_cache_set" => {
                        let args = op.args.as_ref().unwrap();
                        let name = locals[&args[0]];
                        let module = locals[&args[1]];
                        func.instruction(&Instruction::LocalGet(name));
                        func.instruction(&Instruction::LocalGet(module));
                        emit_call(func, reloc_enabled, import_ids["module_cache_set"]);
                        let res = locals[op.out.as_ref().unwrap()];
                        func.instruction(&Instruction::LocalSet(res));
                    }
                    "module_get_attr" => {
                        let args = op.args.as_ref().unwrap();
                        let module = locals[&args[0]];
                        let name = locals[&args[1]];
                        func.instruction(&Instruction::LocalGet(module));
                        func.instruction(&Instruction::LocalGet(name));
                        emit_call(func, reloc_enabled, import_ids["module_get_attr"]);
                        let res = locals[op.out.as_ref().unwrap()];
                        func.instruction(&Instruction::LocalSet(res));
                    }
                    "module_get_global" => {
                        let args = op.args.as_ref().unwrap();
                        let module = locals[&args[0]];
                        let name = locals[&args[1]];
                        func.instruction(&Instruction::LocalGet(module));
                        func.instruction(&Instruction::LocalGet(name));
                        emit_call(func, reloc_enabled, import_ids["module_get_global"]);
                        let res = locals[op.out.as_ref().unwrap()];
                        func.instruction(&Instruction::LocalSet(res));
                    }
                    "module_get_name" => {
                        let args = op.args.as_ref().unwrap();
                        let module = locals[&args[0]];
                        let name = locals[&args[1]];
                        func.instruction(&Instruction::LocalGet(module));
                        func.instruction(&Instruction::LocalGet(name));
                        emit_call(func, reloc_enabled, import_ids["module_get_name"]);
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
                        emit_call(func, reloc_enabled, import_ids["module_set_attr"]);
                        let res = locals[op.out.as_ref().unwrap()];
                        func.instruction(&Instruction::LocalSet(res));
                    }
                    "alloc_task" => {
                        let total = op.value.unwrap_or(0);
                        let task_kind = op.task_kind.as_deref().unwrap_or("future");
                        let (kind_bits, payload_base) = match task_kind {
                            "generator" => (TASK_KIND_GENERATOR, GEN_CONTROL_SIZE),
                            "future" => (TASK_KIND_FUTURE, 0),
                            _ => panic!("unknown task kind: {task_kind}"),
                        };
                        let table_slot = func_map[op.s_value.as_ref().unwrap()];
                        let table_idx = table_base + table_slot;
                        emit_table_index_i64(func, reloc_enabled, table_idx);
                        func.instruction(&Instruction::I64Const(total));
                        func.instruction(&Instruction::I64Const(kind_bits));
                        emit_call(func, reloc_enabled, import_ids["task_new"]);
                        let res = locals[op.out.as_ref().unwrap()];
                        func.instruction(&Instruction::LocalSet(res));
                        if let Some(args) = op.args.as_ref() {
                            for (i, name) in args.iter().enumerate() {
                                let arg_local = locals[name];
                                func.instruction(&Instruction::LocalGet(res));
                                emit_call(func, reloc_enabled, import_ids["handle_resolve"]);
                                func.instruction(&Instruction::I32Const(
                                    payload_base + (i as i32) * 8,
                                ));
                                func.instruction(&Instruction::I32Add);
                                func.instruction(&Instruction::LocalGet(arg_local));
                                func.instruction(&Instruction::I64Store(wasm_encoder::MemArg {
                                    align: 3,
                                    offset: 0,
                                    memory_index: 0,
                                }));
                                func.instruction(&Instruction::LocalGet(arg_local));
                                emit_call(func, reloc_enabled, import_ids["inc_ref_obj"]);
                            }
                        }
                    }
                    "state_yield" => {
                        let args = op.args.as_ref().unwrap();
                        func.instruction(&Instruction::LocalGet(0));
                        func.instruction(&Instruction::I32WrapI64);
                        func.instruction(&Instruction::I32Const(HEADER_STATE_OFFSET));
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
                        emit_call(func, reloc_enabled, import_ids["context_null"]);
                        func.instruction(&Instruction::LocalSet(locals[op.out.as_ref().unwrap()]));
                    }
                    "context_enter" => {
                        let args = op.args.as_ref().unwrap();
                        let ctx = locals[&args[0]];
                        func.instruction(&Instruction::LocalGet(ctx));
                        emit_call(func, reloc_enabled, import_ids["context_enter"]);
                        func.instruction(&Instruction::LocalSet(locals[op.out.as_ref().unwrap()]));
                    }
                    "context_exit" => {
                        let args = op.args.as_ref().unwrap();
                        let ctx = locals[&args[0]];
                        let exc = locals[&args[1]];
                        func.instruction(&Instruction::LocalGet(ctx));
                        func.instruction(&Instruction::LocalGet(exc));
                        emit_call(func, reloc_enabled, import_ids["context_exit"]);
                        func.instruction(&Instruction::LocalSet(locals[op.out.as_ref().unwrap()]));
                    }
                    "context_unwind" => {
                        let args = op.args.as_ref().unwrap();
                        let exc = locals[&args[0]];
                        func.instruction(&Instruction::LocalGet(exc));
                        emit_call(func, reloc_enabled, import_ids["context_unwind"]);
                        func.instruction(&Instruction::LocalSet(locals[op.out.as_ref().unwrap()]));
                    }
                    "context_depth" => {
                        emit_call(func, reloc_enabled, import_ids["context_depth"]);
                        func.instruction(&Instruction::LocalSet(locals[op.out.as_ref().unwrap()]));
                    }
                    "context_unwind_to" => {
                        let args = op.args.as_ref().unwrap();
                        let depth = locals[&args[0]];
                        let exc = locals[&args[1]];
                        func.instruction(&Instruction::LocalGet(depth));
                        func.instruction(&Instruction::LocalGet(exc));
                        emit_call(func, reloc_enabled, import_ids["context_unwind_to"]);
                        func.instruction(&Instruction::LocalSet(locals[op.out.as_ref().unwrap()]));
                    }
                    "context_closing" => {
                        let args = op.args.as_ref().unwrap();
                        let payload = locals[&args[0]];
                        func.instruction(&Instruction::LocalGet(payload));
                        emit_call(func, reloc_enabled, import_ids["context_closing"]);
                        func.instruction(&Instruction::LocalSet(locals[op.out.as_ref().unwrap()]));
                    }
                    "exception_push" => {
                        emit_call(func, reloc_enabled, import_ids["exception_push"]);
                        func.instruction(&Instruction::LocalSet(locals[op.out.as_ref().unwrap()]));
                    }
                    "exception_pop" => {
                        emit_call(func, reloc_enabled, import_ids["exception_pop"]);
                        func.instruction(&Instruction::LocalSet(locals[op.out.as_ref().unwrap()]));
                    }
                    "exception_last" => {
                        emit_call(func, reloc_enabled, import_ids["exception_last"]);
                        func.instruction(&Instruction::LocalSet(locals[op.out.as_ref().unwrap()]));
                    }
                    "exception_new" => {
                        let args = op.args.as_ref().unwrap();
                        let kind = locals[&args[0]];
                        let args_bits = locals[&args[1]];
                        func.instruction(&Instruction::LocalGet(kind));
                        func.instruction(&Instruction::LocalGet(args_bits));
                        emit_call(func, reloc_enabled, import_ids["exception_new"]);
                        func.instruction(&Instruction::LocalSet(locals[op.out.as_ref().unwrap()]));
                    }
                    "exception_new_from_class" => {
                        let args = op.args.as_ref().unwrap();
                        let class_bits = locals[&args[0]];
                        let args_bits = locals[&args[1]];
                        func.instruction(&Instruction::LocalGet(class_bits));
                        func.instruction(&Instruction::LocalGet(args_bits));
                        emit_call(func, reloc_enabled, import_ids["exception_new_from_class"]);
                        func.instruction(&Instruction::LocalSet(locals[op.out.as_ref().unwrap()]));
                    }
                    "exception_clear" => {
                        emit_call(func, reloc_enabled, import_ids["exception_clear"]);
                        func.instruction(&Instruction::LocalSet(locals[op.out.as_ref().unwrap()]));
                    }
                    "exception_kind" => {
                        let args = op.args.as_ref().unwrap();
                        let exc = locals[&args[0]];
                        func.instruction(&Instruction::LocalGet(exc));
                        emit_call(func, reloc_enabled, import_ids["exception_kind"]);
                        func.instruction(&Instruction::LocalSet(locals[op.out.as_ref().unwrap()]));
                    }
                    "exception_class" => {
                        let args = op.args.as_ref().unwrap();
                        let kind = locals[&args[0]];
                        func.instruction(&Instruction::LocalGet(kind));
                        emit_call(func, reloc_enabled, import_ids["exception_class"]);
                        func.instruction(&Instruction::LocalSet(locals[op.out.as_ref().unwrap()]));
                    }
                    "exception_message" => {
                        let args = op.args.as_ref().unwrap();
                        let exc = locals[&args[0]];
                        func.instruction(&Instruction::LocalGet(exc));
                        emit_call(func, reloc_enabled, import_ids["exception_message"]);
                        func.instruction(&Instruction::LocalSet(locals[op.out.as_ref().unwrap()]));
                    }
                    "exception_set_cause" => {
                        let args = op.args.as_ref().unwrap();
                        let exc = locals[&args[0]];
                        let cause = locals[&args[1]];
                        func.instruction(&Instruction::LocalGet(exc));
                        func.instruction(&Instruction::LocalGet(cause));
                        emit_call(func, reloc_enabled, import_ids["exception_set_cause"]);
                        func.instruction(&Instruction::LocalSet(locals[op.out.as_ref().unwrap()]));
                    }
                    "exception_set_value" => {
                        let args = op.args.as_ref().unwrap();
                        let exc = locals[&args[0]];
                        let value = locals[&args[1]];
                        func.instruction(&Instruction::LocalGet(exc));
                        func.instruction(&Instruction::LocalGet(value));
                        emit_call(func, reloc_enabled, import_ids["exception_set_value"]);
                        func.instruction(&Instruction::LocalSet(locals[op.out.as_ref().unwrap()]));
                    }
                    "exception_context_set" => {
                        let args = op.args.as_ref().unwrap();
                        let exc = locals[&args[0]];
                        func.instruction(&Instruction::LocalGet(exc));
                        emit_call(func, reloc_enabled, import_ids["exception_context_set"]);
                        func.instruction(&Instruction::LocalSet(locals[op.out.as_ref().unwrap()]));
                    }
                    "raise" => {
                        let args = op.args.as_ref().unwrap();
                        let exc = locals[&args[0]];
                        func.instruction(&Instruction::LocalGet(exc));
                        emit_call(func, reloc_enabled, import_ids["raise"]);
                        func.instruction(&Instruction::LocalSet(locals[op.out.as_ref().unwrap()]));
                    }
                    "bridge_unavailable" => {
                        let args = op.args.as_ref().unwrap();
                        let msg = locals[&args[0]];
                        func.instruction(&Instruction::LocalGet(msg));
                        emit_call(func, reloc_enabled, import_ids["bridge_unavailable"]);
                        func.instruction(&Instruction::LocalSet(locals[op.out.as_ref().unwrap()]));
                    }
                    "file_open" => {
                        let args = op.args.as_ref().unwrap();
                        let path = locals[&args[0]];
                        let mode = locals[&args[1]];
                        func.instruction(&Instruction::LocalGet(path));
                        func.instruction(&Instruction::LocalGet(mode));
                        emit_call(func, reloc_enabled, import_ids["file_open"]);
                        func.instruction(&Instruction::LocalSet(locals[op.out.as_ref().unwrap()]));
                    }
                    "file_read" => {
                        let args = op.args.as_ref().unwrap();
                        let handle = locals[&args[0]];
                        let size = locals[&args[1]];
                        func.instruction(&Instruction::LocalGet(handle));
                        func.instruction(&Instruction::LocalGet(size));
                        emit_call(func, reloc_enabled, import_ids["file_read"]);
                        func.instruction(&Instruction::LocalSet(locals[op.out.as_ref().unwrap()]));
                    }
                    "file_write" => {
                        let args = op.args.as_ref().unwrap();
                        let handle = locals[&args[0]];
                        let data = locals[&args[1]];
                        func.instruction(&Instruction::LocalGet(handle));
                        func.instruction(&Instruction::LocalGet(data));
                        emit_call(func, reloc_enabled, import_ids["file_write"]);
                        func.instruction(&Instruction::LocalSet(locals[op.out.as_ref().unwrap()]));
                    }
                    "file_close" => {
                        let args = op.args.as_ref().unwrap();
                        let handle = locals[&args[0]];
                        func.instruction(&Instruction::LocalGet(handle));
                        emit_call(func, reloc_enabled, import_ids["file_close"]);
                        func.instruction(&Instruction::LocalSet(locals[op.out.as_ref().unwrap()]));
                    }
                    "cancel_token_new" => {
                        let args = op.args.as_ref().unwrap();
                        let parent = locals[&args[0]];
                        func.instruction(&Instruction::LocalGet(parent));
                        emit_call(func, reloc_enabled, import_ids["cancel_token_new"]);
                        func.instruction(&Instruction::LocalSet(locals[op.out.as_ref().unwrap()]));
                    }
                    "cancel_token_clone" => {
                        let args = op.args.as_ref().unwrap();
                        let token = locals[&args[0]];
                        func.instruction(&Instruction::LocalGet(token));
                        emit_call(func, reloc_enabled, import_ids["cancel_token_clone"]);
                        func.instruction(&Instruction::LocalSet(locals[op.out.as_ref().unwrap()]));
                    }
                    "cancel_token_drop" => {
                        let args = op.args.as_ref().unwrap();
                        let token = locals[&args[0]];
                        func.instruction(&Instruction::LocalGet(token));
                        emit_call(func, reloc_enabled, import_ids["cancel_token_drop"]);
                        func.instruction(&Instruction::LocalSet(locals[op.out.as_ref().unwrap()]));
                    }
                    "cancel_token_cancel" => {
                        let args = op.args.as_ref().unwrap();
                        let token = locals[&args[0]];
                        func.instruction(&Instruction::LocalGet(token));
                        emit_call(func, reloc_enabled, import_ids["cancel_token_cancel"]);
                        func.instruction(&Instruction::LocalSet(locals[op.out.as_ref().unwrap()]));
                    }
                    "cancel_token_is_cancelled" => {
                        let args = op.args.as_ref().unwrap();
                        let token = locals[&args[0]];
                        func.instruction(&Instruction::LocalGet(token));
                        emit_call(func, reloc_enabled, import_ids["cancel_token_is_cancelled"]);
                        func.instruction(&Instruction::LocalSet(locals[op.out.as_ref().unwrap()]));
                    }
                    "cancel_token_set_current" => {
                        let args = op.args.as_ref().unwrap();
                        let token = locals[&args[0]];
                        func.instruction(&Instruction::LocalGet(token));
                        emit_call(func, reloc_enabled, import_ids["cancel_token_set_current"]);
                        func.instruction(&Instruction::LocalSet(locals[op.out.as_ref().unwrap()]));
                    }
                    "cancel_token_get_current" => {
                        emit_call(func, reloc_enabled, import_ids["cancel_token_get_current"]);
                        func.instruction(&Instruction::LocalSet(locals[op.out.as_ref().unwrap()]));
                    }
                    "cancelled" => {
                        emit_call(func, reloc_enabled, import_ids["cancelled"]);
                        func.instruction(&Instruction::LocalSet(locals[op.out.as_ref().unwrap()]));
                    }
                    "cancel_current" => {
                        emit_call(func, reloc_enabled, import_ids["cancel_current"]);
                        func.instruction(&Instruction::LocalSet(locals[op.out.as_ref().unwrap()]));
                    }
                    "block_on" => {
                        let args = op.args.as_ref().unwrap();
                        func.instruction(&Instruction::LocalGet(locals[&args[0]]));
                        emit_call(func, reloc_enabled, import_ids["block_on"]);
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
                    "jump" => {
                        let target = op.value.expect("jump missing label");
                        let depth = label_depths
                            .get(&target)
                            .map(|idx| control_stack.len().saturating_sub(1 + idx))
                            .unwrap_or_else(|| {
                                panic!("jump target {} missing label block", target)
                            });
                        func.instruction(&Instruction::Br(depth as u32));
                    }
                    "if" => {
                        let args = op.args.as_ref().unwrap();
                        let cond = locals[&args[0]];
                        func.instruction(&Instruction::LocalGet(cond));
                        emit_call(func, reloc_enabled, import_ids["is_truthy"]);
                        func.instruction(&Instruction::I64Const(0));
                        func.instruction(&Instruction::I64Ne);
                        func.instruction(&Instruction::If(BlockType::Empty));
                        control_stack.push(ControlKind::If);
                    }
                    "label" => {
                        if let Some(label_id) = op.value {
                            if let Some(top) = label_stack.last().copied() {
                                if top == label_id {
                                    label_stack.pop();
                                    label_depths.remove(&label_id);
                                    func.instruction(&Instruction::End);
                                    control_stack.pop();
                                }
                            }
                        }
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
                        emit_call(func, reloc_enabled, import_ids["is_truthy"]);
                        func.instruction(&Instruction::I64Const(0));
                        func.instruction(&Instruction::I64Ne);
                        func.instruction(&Instruction::BrIf(1));
                    }
                    "loop_break_if_false" => {
                        let args = op.args.as_ref().unwrap();
                        let cond = locals[&args[0]];
                        func.instruction(&Instruction::LocalGet(cond));
                        emit_call(func, reloc_enabled, import_ids["is_truthy"]);
                        func.instruction(&Instruction::I64Const(0));
                        func.instruction(&Instruction::I64Eq);
                        func.instruction(&Instruction::BrIf(1));
                    }
                    "loop_break" => {
                        func.instruction(&Instruction::Br(1));
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
                            emit_call(func, reloc_enabled, import_ids["exception_pending"]);
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
            let func = &mut func;
            let state_local = state_local.expect("state local missing for stateful wasm");
            let self_ptr_local = self_ptr_local.expect("self ptr local missing for stateful wasm");
            let self_param = *locals
                .get("self_param")
                .expect("self_param missing for stateful wasm");
            let self_local = *locals
                .get("self")
                .expect("self local missing for stateful wasm");
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
                    "loop_break_if_true" | "loop_break_if_false" | "loop_break" => {
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
                match op.kind.as_str() {
                    "state_yield" => {
                        if let Some(state_id) = op.value {
                            state_map.insert(state_id, idx + 1);
                        }
                    }
                    "state_label" => {
                        if let Some(state_id) = op.value {
                            state_map.insert(state_id, idx);
                        }
                    }
                    _ => {}
                }
            }

            let dispatch_depths: Vec<u32> = (0..op_count)
                .map(|idx| (op_count - 1 - idx) as u32)
                .collect();

            func.instruction(&Instruction::LocalGet(self_param));
            func.instruction(&Instruction::LocalSet(self_ptr_local));

            func.instruction(&Instruction::LocalGet(self_param));
            func.instruction(&Instruction::I64Const(POINTER_MASK as i64));
            func.instruction(&Instruction::I64And);
            func.instruction(&Instruction::I64Const(QNAN_TAG_PTR_I64));
            func.instruction(&Instruction::I64Or);
            func.instruction(&Instruction::LocalSet(self_local));

            func.instruction(&Instruction::LocalGet(self_ptr_local));
            func.instruction(&Instruction::I32WrapI64);
            func.instruction(&Instruction::I32Const(HEADER_STATE_OFFSET));
            func.instruction(&Instruction::I32Add);
            func.instruction(&Instruction::I64Load(wasm_encoder::MemArg {
                align: 3,
                offset: 0,
                memory_index: 0,
            }));
            func.instruction(&Instruction::LocalSet(state_local));
            func.instruction(&Instruction::LocalGet(state_local));
            func.instruction(&Instruction::I64Const(0));
            func.instruction(&Instruction::I64LtS);
            func.instruction(&Instruction::If(BlockType::Empty));
            func.instruction(&Instruction::LocalGet(state_local));
            func.instruction(&Instruction::I64Const(-1));
            func.instruction(&Instruction::I64Xor);
            func.instruction(&Instruction::LocalSet(state_local));
            func.instruction(&Instruction::Else);
            for (state_id, target_idx) in &state_map {
                func.instruction(&Instruction::LocalGet(state_local));
                func.instruction(&Instruction::I64Const(*state_id));
                func.instruction(&Instruction::I64Eq);
                func.instruction(&Instruction::If(BlockType::Empty));
                func.instruction(&Instruction::I64Const(*target_idx as i64));
                func.instruction(&Instruction::LocalSet(state_local));
                func.instruction(&Instruction::End);
            }
            func.instruction(&Instruction::End);

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
            let mut const_ints: HashMap<String, i64> = HashMap::new();

            for (idx, op) in func_ir.ops.iter().enumerate() {
                let depth = dispatch_depths[idx];
                if op.kind == "const" {
                    if let (Some(out), Some(value)) = (op.out.as_ref(), op.value) {
                        const_ints.insert(out.clone(), value);
                    }
                }
                match op.kind.as_str() {
                    "state_switch" => {
                        func.instruction(&Instruction::I64Const((idx + 1) as i64));
                        func.instruction(&Instruction::LocalSet(state_local));
                        func.instruction(&Instruction::Br(depth));
                    }
                    "aiter" => {
                        let args = op.args.as_ref().unwrap();
                        let iter = locals[&args[0]];
                        func.instruction(&Instruction::LocalGet(iter));
                        emit_call(func, reloc_enabled, import_ids["aiter"]);
                        func.instruction(&Instruction::LocalSet(locals[op.out.as_ref().unwrap()]));
                    }
                    "state_transition" => {
                        let args = op.args.as_ref().unwrap();
                        let future = locals[&args[0]];
                        let (slot_bits, pending_state) = if args.len() == 2 {
                            (None, locals[&args[1]])
                        } else {
                            (Some(locals[&args[1]]), locals[&args[2]])
                        };
                        let pending_state_name = if args.len() == 2 { &args[1] } else { &args[2] };
                        let pending_target_idx = const_ints
                            .get(pending_state_name)
                            .and_then(|state_id| state_map.get(state_id).copied())
                            .map(|idx| !(idx as i64));
                        let next_state_id = op.value.unwrap();
                        let out = locals[op.out.as_ref().unwrap()];
                        func.instruction(&Instruction::I64Const((idx + 1) as i64));
                        func.instruction(&Instruction::LocalSet(state_local));
                        func.instruction(&Instruction::LocalGet(self_ptr_local));
                        func.instruction(&Instruction::I32WrapI64);
                        func.instruction(&Instruction::I32Const(HEADER_STATE_OFFSET));
                        func.instruction(&Instruction::I32Add);
                        if let Some(pending_encoded) = pending_target_idx {
                            func.instruction(&Instruction::I64Const(pending_encoded));
                        } else {
                            func.instruction(&Instruction::LocalGet(pending_state));
                            func.instruction(&Instruction::I64Const(INT_MASK as i64));
                            func.instruction(&Instruction::I64And);
                        }
                        func.instruction(&Instruction::I64Store(wasm_encoder::MemArg {
                            align: 3,
                            offset: 0,
                            memory_index: 0,
                        }));
                        func.instruction(&Instruction::LocalGet(future));
                        emit_call(func, reloc_enabled, import_ids["future_poll"]);
                        func.instruction(&Instruction::LocalSet(out));
                        func.instruction(&Instruction::LocalGet(out));
                        func.instruction(&Instruction::I64Const(box_pending()));
                        func.instruction(&Instruction::I64Eq);
                        func.instruction(&Instruction::If(BlockType::Empty));
                        func.instruction(&Instruction::LocalGet(self_ptr_local));
                        func.instruction(&Instruction::I32WrapI64);
                        func.instruction(&Instruction::LocalGet(future));
                        emit_call(func, reloc_enabled, import_ids["handle_resolve"]);
                        emit_call(func, reloc_enabled, import_ids["sleep_register"]);
                        func.instruction(&Instruction::Drop);
                        func.instruction(&Instruction::I64Const(box_pending()));
                        func.instruction(&Instruction::Return);
                        func.instruction(&Instruction::End);
                        if let Some(slot) = slot_bits {
                            func.instruction(&Instruction::LocalGet(self_ptr_local));
                            func.instruction(&Instruction::I32WrapI64);
                            func.instruction(&Instruction::LocalGet(slot));
                            func.instruction(&Instruction::I64Const(INT_MASK as i64));
                            func.instruction(&Instruction::I64And);
                            func.instruction(&Instruction::LocalGet(out));
                            emit_call(func, reloc_enabled, import_ids["closure_store"]);
                            func.instruction(&Instruction::Drop);
                        }
                        func.instruction(&Instruction::LocalGet(self_ptr_local));
                        func.instruction(&Instruction::I32WrapI64);
                        func.instruction(&Instruction::I32Const(HEADER_STATE_OFFSET));
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
                        let resume_state_id = op.value.unwrap();
                        let resume_encoded = state_map
                            .get(&resume_state_id)
                            .copied()
                            .map(|idx| !(idx as i64));
                        func.instruction(&Instruction::I64Const((idx + 1) as i64));
                        func.instruction(&Instruction::LocalSet(state_local));
                        func.instruction(&Instruction::LocalGet(self_ptr_local));
                        func.instruction(&Instruction::I32WrapI64);
                        func.instruction(&Instruction::I32Const(HEADER_STATE_OFFSET));
                        func.instruction(&Instruction::I32Add);
                        if let Some(encoded) = resume_encoded {
                            func.instruction(&Instruction::I64Const(encoded));
                        } else {
                            func.instruction(&Instruction::I64Const(resume_state_id));
                        }
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
                        let pending_state_name = &args[2];
                        let pending_target_idx = const_ints
                            .get(pending_state_name)
                            .and_then(|state_id| state_map.get(state_id).copied())
                            .map(|idx| !(idx as i64));
                        let next_state_id = op.value.unwrap();
                        func.instruction(&Instruction::I64Const((idx + 1) as i64));
                        func.instruction(&Instruction::LocalSet(state_local));
                        func.instruction(&Instruction::LocalGet(self_ptr_local));
                        func.instruction(&Instruction::I32WrapI64);
                        func.instruction(&Instruction::I32Const(HEADER_STATE_OFFSET));
                        func.instruction(&Instruction::I32Add);
                        if let Some(pending_encoded) = pending_target_idx {
                            func.instruction(&Instruction::I64Const(pending_encoded));
                        } else {
                            func.instruction(&Instruction::LocalGet(pending_state));
                            func.instruction(&Instruction::I64Const(INT_MASK as i64));
                            func.instruction(&Instruction::I64And);
                        }
                        func.instruction(&Instruction::I64Store(wasm_encoder::MemArg {
                            align: 3,
                            offset: 0,
                            memory_index: 0,
                        }));
                        func.instruction(&Instruction::LocalGet(chan));
                        func.instruction(&Instruction::I32WrapI64);
                        func.instruction(&Instruction::LocalGet(val));
                        emit_call(func, reloc_enabled, import_ids["chan_send"]);
                        let out = locals[op.out.as_ref().unwrap()];
                        func.instruction(&Instruction::LocalSet(out));
                        func.instruction(&Instruction::LocalGet(out));
                        func.instruction(&Instruction::I64Const(box_pending()));
                        func.instruction(&Instruction::I64Eq);
                        func.instruction(&Instruction::If(BlockType::Empty));
                        func.instruction(&Instruction::I64Const(box_pending()));
                        func.instruction(&Instruction::Return);
                        func.instruction(&Instruction::End);
                        func.instruction(&Instruction::LocalGet(self_ptr_local));
                        func.instruction(&Instruction::I32WrapI64);
                        func.instruction(&Instruction::I32Const(HEADER_STATE_OFFSET));
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
                        let pending_state_name = &args[1];
                        let pending_target_idx = const_ints
                            .get(pending_state_name)
                            .and_then(|state_id| state_map.get(state_id).copied())
                            .map(|idx| !(idx as i64));
                        let next_state_id = op.value.unwrap();
                        func.instruction(&Instruction::I64Const((idx + 1) as i64));
                        func.instruction(&Instruction::LocalSet(state_local));
                        func.instruction(&Instruction::LocalGet(self_ptr_local));
                        func.instruction(&Instruction::I32WrapI64);
                        func.instruction(&Instruction::I32Const(HEADER_STATE_OFFSET));
                        func.instruction(&Instruction::I32Add);
                        if let Some(pending_encoded) = pending_target_idx {
                            func.instruction(&Instruction::I64Const(pending_encoded));
                        } else {
                            func.instruction(&Instruction::LocalGet(pending_state));
                            func.instruction(&Instruction::I64Const(INT_MASK as i64));
                            func.instruction(&Instruction::I64And);
                        }
                        func.instruction(&Instruction::I64Store(wasm_encoder::MemArg {
                            align: 3,
                            offset: 0,
                            memory_index: 0,
                        }));
                        func.instruction(&Instruction::LocalGet(chan));
                        func.instruction(&Instruction::I32WrapI64);
                        emit_call(func, reloc_enabled, import_ids["chan_recv"]);
                        let out = locals[op.out.as_ref().unwrap()];
                        func.instruction(&Instruction::LocalSet(out));
                        func.instruction(&Instruction::LocalGet(out));
                        func.instruction(&Instruction::I64Const(box_pending()));
                        func.instruction(&Instruction::I64Eq);
                        func.instruction(&Instruction::If(BlockType::Empty));
                        func.instruction(&Instruction::I64Const(box_pending()));
                        func.instruction(&Instruction::Return);
                        func.instruction(&Instruction::End);
                        func.instruction(&Instruction::LocalGet(self_ptr_local));
                        func.instruction(&Instruction::I32WrapI64);
                        func.instruction(&Instruction::I32Const(HEADER_STATE_OFFSET));
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
                        emit_call(func, reloc_enabled, import_ids["is_truthy"]);
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
                        emit_call(func, reloc_enabled, import_ids["is_truthy"]);
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
                        emit_call(func, reloc_enabled, import_ids["is_truthy"]);
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
                    "loop_break" => {
                        let end_idx = loop_break_target
                            .get(&idx)
                            .copied()
                            .expect("loop break without loop");
                        func.instruction(&Instruction::I64Const((end_idx + 1) as i64));
                        func.instruction(&Instruction::LocalSet(state_local));
                        func.instruction(&Instruction::Br(depth + 1));
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
                    "jump" => {
                        let target_label = op.value.expect("jump missing label");
                        let target_idx = label_to_index
                            .get(&target_label)
                            .copied()
                            .expect("unknown jump label");
                        func.instruction(&Instruction::I64Const(target_idx as i64));
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
                        emit_call(func, reloc_enabled, import_ids["exception_pending"]);
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
                            func,
                            std::slice::from_ref(op),
                            &mut scratch_control,
                            &mut scratch_try,
                            &mut label_stack,
                            &mut label_depths,
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
        } else if jumpful {
            let func = &mut func;
            let state_local = state_local.expect("state local missing for jumpful wasm");
            let op_count = func_ir.ops.len();
            let mut label_to_index: HashMap<i64, usize> = HashMap::new();
            for (idx, op) in func_ir.ops.iter().enumerate() {
                if op.kind == "label" {
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
                    "loop_break_if_true" | "loop_break_if_false" | "loop_break" => {
                        if let Some(start_idx) = loop_scan.last().copied() {
                            if let Some(end_idx) = loop_end_for_start.get(&start_idx).copied() {
                                loop_break_target.insert(idx, end_idx);
                            }
                        }
                    }
                    _ => {}
                }
            }

            let dispatch_depths: Vec<u32> = (0..op_count)
                .map(|idx| (op_count - 1 - idx) as u32)
                .collect();
            let mut scratch_control: Vec<ControlKind> = Vec::new();
            let mut scratch_try: Vec<usize> = Vec::new();
            let mut label_stack: Vec<i64> = Vec::new();
            let mut label_depths: HashMap<i64, usize> = HashMap::new();

            func.instruction(&Instruction::I64Const(0));
            func.instruction(&Instruction::LocalSet(state_local));

            func.instruction(&Instruction::Loop(BlockType::Empty));
            for _ in (0..op_count).rev() {
                func.instruction(&Instruction::Block(BlockType::Empty));
            }

            func.instruction(&Instruction::LocalGet(state_local));
            func.instruction(&Instruction::I32WrapI64);
            let targets: Vec<u32> = (0..op_count).map(|idx| idx as u32).collect();
            func.instruction(&Instruction::BrTable(targets.into(), op_count as u32));
            func.instruction(&Instruction::End);

            for (idx, op) in func_ir.ops.iter().enumerate() {
                let depth = dispatch_depths[idx];
                match op.kind.as_str() {
                    "state_switch" | "state_transition" | "state_yield" | "chan_send_yield"
                    | "chan_recv_yield" => {
                        panic!(
                            "jumpful wasm path hit stateful op {} in {}",
                            op.kind, func_ir.name
                        );
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
                        emit_call(func, reloc_enabled, import_ids["is_truthy"]);
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
                        emit_call(func, reloc_enabled, import_ids["is_truthy"]);
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
                        emit_call(func, reloc_enabled, import_ids["is_truthy"]);
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
                    "loop_break" => {
                        let end_idx = loop_break_target
                            .get(&idx)
                            .copied()
                            .expect("loop break without loop");
                        func.instruction(&Instruction::I64Const((end_idx + 1) as i64));
                        func.instruction(&Instruction::LocalSet(state_local));
                        func.instruction(&Instruction::Br(depth));
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
                    "jump" => {
                        let target_label = op.value.expect("jump missing label");
                        let target_idx = label_to_index
                            .get(&target_label)
                            .copied()
                            .expect("unknown jump label");
                        func.instruction(&Instruction::I64Const(target_idx as i64));
                        func.instruction(&Instruction::LocalSet(state_local));
                        func.instruction(&Instruction::Br(depth));
                    }
                    "try_start" | "try_end" | "label" => {
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
                        emit_call(func, reloc_enabled, import_ids["exception_pending"]);
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
                            func,
                            std::slice::from_ref(op),
                            &mut scratch_control,
                            &mut scratch_try,
                            &mut label_stack,
                            &mut label_depths,
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
            let func = &mut func;
            let jump_labels: HashSet<i64> = func_ir
                .ops
                .iter()
                .filter_map(|op| {
                    if op.kind.as_str() == "jump" {
                        op.value
                    } else {
                        None
                    }
                })
                .collect();
            let label_ids: Vec<i64> = func_ir
                .ops
                .iter()
                .filter_map(|op| {
                    if op.kind.as_str() == "label" {
                        op.value.filter(|val| jump_labels.contains(val))
                    } else {
                        None
                    }
                })
                .collect();
            if !label_ids.is_empty() {
                for label_id in label_ids.iter().rev() {
                    func.instruction(&Instruction::Block(BlockType::Empty));
                    control_stack.push(ControlKind::Block);
                    label_depths.insert(*label_id, control_stack.len() - 1);
                    label_stack.push(*label_id);
                }
            }
            emit_ops(
                func,
                &func_ir.ops,
                &mut control_stack,
                &mut try_stack,
                &mut label_stack,
                &mut label_depths,
            );
            while !label_stack.is_empty() {
                label_stack.pop();
                func.instruction(&Instruction::End);
                control_stack.pop();
            }
            func.instruction(&Instruction::End);
        }
        self.codes.function(&func);
    }
}

fn should_emit_relocs() -> bool {
    matches!(std::env::var("MOLT_WASM_LINK").as_deref(), Ok("1"))
}

fn table_base_for_reloc() -> u32 {
    // Allow the driver to pin the table base to the runtime table size.
    match std::env::var("MOLT_WASM_TABLE_BASE") {
        Ok(value) => value.parse::<u32>().unwrap_or(RELOC_TABLE_BASE_DEFAULT),
        Err(_) => RELOC_TABLE_BASE_DEFAULT,
    }
}

fn encode_u32_leb128_padded(mut value: u32, out: &mut Vec<u8>) {
    for i in 0..5 {
        let mut byte = (value & 0x7f) as u8;
        value >>= 7;
        if i < 4 {
            byte |= 0x80;
        }
        out.push(byte);
    }
}

fn encode_i32_sleb128_padded(mut value: i32, out: &mut Vec<u8>) {
    for i in 0..5 {
        let mut byte = (value & 0x7f) as u8;
        value >>= 7;
        if i < 4 {
            byte |= 0x80;
        }
        out.push(byte);
    }
}

fn emit_call(func: &mut Function, reloc_enabled: bool, func_index: u32) {
    if reloc_enabled {
        let mut bytes = Vec::with_capacity(6);
        bytes.push(0x10);
        encode_u32_leb128_padded(func_index, &mut bytes);
        func.raw(bytes);
    } else {
        func.instruction(&Instruction::Call(func_index));
    }
}

fn emit_call_indirect(func: &mut Function, reloc_enabled: bool, ty: u32, table: u32) {
    if reloc_enabled {
        let mut bytes = Vec::with_capacity(11);
        bytes.push(0x11);
        encode_u32_leb128_padded(ty, &mut bytes);
        encode_u32_leb128_padded(table, &mut bytes);
        func.raw(bytes);
    } else {
        func.instruction(&Instruction::CallIndirect { ty, table });
    }
}

fn emit_i32_const(func: &mut Function, reloc_enabled: bool, value: i32) {
    if reloc_enabled {
        let mut bytes = Vec::with_capacity(6);
        bytes.push(0x41);
        encode_i32_sleb128_padded(value, &mut bytes);
        func.raw(bytes);
    } else {
        func.instruction(&Instruction::I32Const(value));
    }
}

fn emit_ref_func(func: &mut Function, reloc_enabled: bool, func_index: u32) {
    if reloc_enabled {
        let mut bytes = Vec::with_capacity(6);
        bytes.push(0xD2);
        encode_u32_leb128_padded(func_index, &mut bytes);
        func.raw(bytes);
    } else {
        func.instruction(&Instruction::RefFunc(func_index));
    }
}

fn emit_table_index_i32(func: &mut Function, reloc_enabled: bool, table_index: u32) {
    emit_i32_const(func, reloc_enabled, table_index as i32);
}

fn emit_table_index_i64(func: &mut Function, reloc_enabled: bool, table_index: u32) {
    emit_table_index_i32(func, reloc_enabled, table_index);
    func.instruction(&Instruction::I64ExtendI32U);
}

fn const_expr_i32_const_padded(value: i32) -> ConstExpr {
    let mut bytes = Vec::with_capacity(6);
    bytes.push(0x41);
    encode_i32_sleb128_padded(value, &mut bytes);
    ConstExpr::raw(bytes)
}

#[derive(Clone, Copy)]
enum PendingReloc {
    Function { offset: u32, func_index: u32 },
    Type { offset: u32, type_index: u32 },
    DataAddr { offset: u32, segment_index: u32 },
}

#[derive(Clone, Copy)]
struct RelocEntry {
    ty: u8,
    offset: u32,
    index: u32,
    addend: i32,
}

fn encode_reloc_section(
    name: &'static str,
    section_index: u32,
    entries: &[RelocEntry],
) -> CustomSection<'static> {
    let mut data = Vec::new();
    section_index.encode(&mut data);
    (entries.len() as u32).encode(&mut data);
    for entry in entries {
        data.push(entry.ty);
        entry.offset.encode(&mut data);
        entry.index.encode(&mut data);
        if matches!(entry.ty, 4 | 5) {
            entry.addend.encode(&mut data);
        }
    }
    CustomSection {
        name: name.into(),
        data: Cow::Owned(data),
    }
}

fn append_custom_section(bytes: &mut Vec<u8>, section: &impl Encode) {
    bytes.push(0);
    section.encode(bytes);
}

fn add_reloc_sections(
    mut bytes: Vec<u8>,
    data_segments: &[DataSegmentInfo],
    data_relocs: &[DataRelocSite],
) -> Vec<u8> {
    let mut func_imports: Vec<String> = Vec::new();
    let mut func_exports: HashMap<u32, String> = HashMap::new();
    let mut func_import_count = 0u32;
    let mut defined_func_count = 0u32;
    let mut table_import_count = 0u32;
    let mut table_defined_count = 0u32;
    let mut code_section_start = None;
    let mut code_section_index = None;
    let mut data_section_index = None;
    let mut element_section_index = None;
    let mut func_body_starts: Vec<usize> = Vec::new();
    let mut pending_code: Vec<PendingReloc> = Vec::new();
    let mut pending_data: Vec<PendingReloc> = Vec::new();
    let mut pending_elem: Vec<PendingReloc> = Vec::new();
    let mut section_index = 0u32;

    let mut parse_failed = false;
    for payload in Parser::new(0).parse_all(&bytes) {
        let payload = match payload {
            Ok(payload) => payload,
            Err(_) => {
                parse_failed = true;
                break;
            }
        };
        match payload {
            Payload::TypeSection(_) => {
                section_index += 1;
            }
            Payload::ImportSection(reader) => {
                section_index += 1;
                for import in reader.into_iter().flatten() {
                    match import.ty {
                        TypeRef::Func(_) => {
                            func_imports.push(import.name.to_string());
                            func_import_count += 1;
                        }
                        TypeRef::Table(_) => {
                            table_import_count += 1;
                        }
                        _ => {}
                    }
                }
            }
            Payload::FunctionSection(reader) => {
                defined_func_count = reader.count();
                section_index += 1;
            }
            Payload::TableSection(reader) => {
                table_defined_count = reader.count();
                section_index += 1;
            }
            Payload::MemorySection(_) => {
                section_index += 1;
            }
            Payload::GlobalSection(_) => {
                section_index += 1;
            }
            Payload::ExportSection(reader) => {
                for export in reader.into_iter().flatten() {
                    if export.kind == ExternalKind::Func {
                        func_exports.insert(export.index, export.name.to_string());
                    }
                }
                section_index += 1;
            }
            Payload::StartSection { .. } => {
                section_index += 1;
            }
            Payload::ElementSection(reader) => {
                let element_section_start = reader.range().start;
                element_section_index = Some(section_index);
                section_index += 1;
                for element in reader.into_iter().flatten() {
                    if let ElementItems::Functions(funcs) = element.items {
                        for func in funcs.into_iter_with_offsets().flatten() {
                            let (pos, func_index) = func;
                            let offset = (pos.saturating_sub(element_section_start)) as u32;
                            pending_elem.push(PendingReloc::Function { offset, func_index });
                        }
                    }
                }
            }
            Payload::CodeSectionStart { range, .. } => {
                code_section_start = Some(range.start);
                code_section_index = Some(section_index);
                section_index += 1;
            }
            Payload::CodeSectionEntry(body) => {
                func_body_starts.push(body.range().start);
                if let Ok(mut ops) = body.get_operators_reader() {
                    while let Ok((op, op_offset)) = ops.read_with_offset() {
                        let start = match code_section_start {
                            Some(start) => start,
                            None => break,
                        };
                        match op {
                            Operator::Call { function_index } => {
                                let offset = (op_offset + 1).saturating_sub(start) as u32;
                                pending_code.push(PendingReloc::Function {
                                    offset,
                                    func_index: function_index,
                                });
                            }
                            Operator::CallIndirect { type_index, .. } => {
                                let type_offset = (op_offset + 1).saturating_sub(start) as u32;
                                pending_code.push(PendingReloc::Type {
                                    offset: type_offset,
                                    type_index,
                                });
                            }
                            Operator::RefFunc { function_index } => {
                                let offset = (op_offset + 1).saturating_sub(start) as u32;
                                pending_code.push(PendingReloc::Function {
                                    offset,
                                    func_index: function_index,
                                });
                            }
                            _ => {}
                        }
                    }
                }
            }
            Payload::DataSection(reader) => {
                let data_section_start = reader.range().start;
                data_section_index = Some(section_index);
                section_index += 1;
                for (segment_index, data) in reader.into_iter().enumerate() {
                    if let Ok(data) = data {
                        if let DataKind::Active { offset_expr, .. } = data.kind {
                            let mut ops = offset_expr.get_operators_reader();
                            if let Ok((Operator::I32Const { .. }, op_offset)) =
                                ops.read_with_offset()
                            {
                                let offset =
                                    (op_offset + 1).saturating_sub(data_section_start) as u32;
                                pending_data.push(PendingReloc::DataAddr {
                                    offset,
                                    segment_index: segment_index as u32,
                                });
                            }
                        }
                    }
                }
            }
            Payload::DataCountSection { .. } => {
                section_index += 1;
            }
            _ => {}
        }
    }
    if parse_failed {
        return bytes;
    }

    let code_section_start = match code_section_start {
        Some(start) => start,
        None => return bytes,
    };
    let code_section_index = match code_section_index {
        Some(index) => index,
        None => return bytes,
    };
    let data_section_index = data_section_index;

    for site in data_relocs {
        let def_index = site.func_index.saturating_sub(func_import_count) as usize;
        if let Some(body_start) = func_body_starts.get(def_index) {
            let offset = (body_start.saturating_sub(code_section_start) as u32)
                .saturating_add(site.offset_in_func);
            pending_code.push(PendingReloc::DataAddr {
                offset,
                segment_index: site.segment_index,
            });
        }
    }

    let total_funcs = func_import_count + defined_func_count;
    let mut func_symbol_map = vec![0u32; total_funcs as usize];
    let mut data_symbol_map = vec![0u32; data_segments.len()];
    let mut symbol_index = 0u32;

    let mut sym_tab = SymbolTable::new();
    let mut import_names: Vec<String> = Vec::new();
    for (idx, name) in func_imports.iter().enumerate() {
        let flags = SymbolTable::WASM_SYM_UNDEFINED | SymbolTable::WASM_SYM_EXPLICIT_NAME;
        let symbol_name = format!("molt_{name}");
        import_names.push(symbol_name);
        let name_ref = import_names.last().unwrap();
        sym_tab.function(flags, idx as u32, Some(name_ref));
        func_symbol_map[idx] = symbol_index;
        symbol_index += 1;
    }
    let mut func_names: Vec<String> = Vec::new();
    for def_idx in 0..defined_func_count {
        let func_index = func_import_count + def_idx;
        let export_name = func_exports.get(&func_index).cloned();
        let name = export_name
            .clone()
            .unwrap_or_else(|| format!("__molt_fn_{func_index}"));
        func_names.push(name);
        let name_ref = func_names.last().unwrap();
        let flags = if export_name.is_some() {
            SymbolTable::WASM_SYM_EXPORTED | SymbolTable::WASM_SYM_NO_STRIP
        } else {
            0
        };
        sym_tab.function(flags, func_index, Some(name_ref));
        func_symbol_map[func_index as usize] = symbol_index;
        symbol_index += 1;
    }

    for table_idx in 0..table_import_count {
        let flags = SymbolTable::WASM_SYM_UNDEFINED | SymbolTable::WASM_SYM_NO_STRIP;
        sym_tab.table(flags, table_idx, None);
        symbol_index += 1;
    }
    let mut table_names: Vec<String> = Vec::new();
    for table_idx in 0..table_defined_count {
        let index = table_import_count + table_idx;
        let name = format!("__molt_table_{index}");
        table_names.push(name);
        let name_ref = table_names.last().unwrap();
        sym_tab.table(0, index, Some(name_ref));
        symbol_index += 1;
    }

    let mut data_names: Vec<String> = Vec::new();
    for (idx, info) in data_segments.iter().enumerate() {
        let name = format!("__molt_data_{idx}");
        data_names.push(name);
        let name_ref = data_names.last().unwrap();
        sym_tab.data(
            0,
            name_ref,
            Some(DataSymbolDefinition {
                index: idx as u32,
                offset: 0,
                size: info.size,
            }),
        );
        data_symbol_map[idx] = symbol_index;
        symbol_index += 1;
    }

    let mut code_entries: Vec<RelocEntry> = Vec::new();
    let mut data_entries: Vec<RelocEntry> = Vec::new();
    let mut elem_entries: Vec<RelocEntry> = Vec::new();
    for reloc in pending_code {
        match reloc {
            PendingReloc::Function { offset, func_index } => {
                if let Some(index) = func_symbol_map.get(func_index as usize) {
                    code_entries.push(RelocEntry {
                        ty: 0,
                        offset,
                        index: *index,
                        addend: 0,
                    });
                }
            }
            PendingReloc::Type { offset, type_index } => {
                code_entries.push(RelocEntry {
                    ty: 6,
                    offset,
                    index: type_index,
                    addend: 0,
                });
            }
            PendingReloc::DataAddr {
                offset,
                segment_index,
            } => {
                if let Some(index) = data_symbol_map.get(segment_index as usize) {
                    code_entries.push(RelocEntry {
                        ty: 4,
                        offset,
                        index: *index,
                        addend: 0,
                    });
                }
            }
        }
    }

    for reloc in pending_data {
        if let PendingReloc::DataAddr {
            offset,
            segment_index,
        } = reloc
        {
            if let Some(index) = data_symbol_map.get(segment_index as usize) {
                data_entries.push(RelocEntry {
                    ty: 4,
                    offset,
                    index: *index,
                    addend: 0,
                });
            }
        }
    }

    for reloc in pending_elem {
        if let PendingReloc::Function { offset, func_index } = reloc {
            if let Some(index) = func_symbol_map.get(func_index as usize) {
                elem_entries.push(RelocEntry {
                    ty: 0,
                    offset,
                    index: *index,
                    addend: 0,
                });
            }
        }
    }

    code_entries.sort_by_key(|entry| entry.offset);
    data_entries.sort_by_key(|entry| entry.offset);
    elem_entries.sort_by_key(|entry| entry.offset);

    let mut linking = LinkingSection::new();
    linking.symbol_table(&sym_tab);
    append_custom_section(&mut bytes, &linking);
    if !code_entries.is_empty() {
        let reloc_code = encode_reloc_section("reloc.CODE", code_section_index, &code_entries);
        append_custom_section(&mut bytes, &reloc_code);
    }
    if !data_entries.is_empty() {
        if let Some(index) = data_section_index {
            let reloc_data = encode_reloc_section("reloc.DATA", index, &data_entries);
            append_custom_section(&mut bytes, &reloc_data);
        }
    }
    if !elem_entries.is_empty() {
        if let Some(index) = element_section_index {
            let reloc_elem = encode_reloc_section("reloc.ELEM", index, &elem_entries);
            append_custom_section(&mut bytes, &reloc_elem);
        }
    }

    bytes
}
