use super::*;

impl<'ctx, 'func> FunctionLowering<'ctx, 'func> {
    pub(super) fn emit_const_int(&mut self, op: &TirOp) {
        let val = match op.attrs.get("value") {
            Some(AttrValue::Int(v)) => *v,
            other => panic!("ConstInt missing integer value attribute: {:?}", other),
        };
        let result_id = op.results[0];
        let llvm_val = self
            .backend
            .context
            .i64_type()
            .const_int(val as u64, val < 0)
            .into();
        self.values.insert(result_id, llvm_val);
        self.value_types.insert(result_id, TirType::I64);
    }

    pub(super) fn emit_const_float(&mut self, op: &TirOp) {
        let val = match op.attrs.get("f_value").or_else(|| op.attrs.get("value")) {
            Some(AttrValue::Float(v)) => *v,
            other => panic!("ConstFloat missing float value attribute: {:?}", other),
        };
        let result_id = op.results[0];
        let llvm_val = self.backend.context.f64_type().const_float(val).into();
        self.values.insert(result_id, llvm_val);
        self.value_types.insert(result_id, TirType::F64);
    }

    pub(super) fn emit_const_bool(&mut self, op: &TirOp) {
        let val = match op.attrs.get("value") {
            Some(AttrValue::Bool(v)) => *v,
            Some(AttrValue::Int(v)) => *v != 0,
            other => panic!("ConstBool missing bool value attribute: {:?}", other),
        };
        let result_id = op.results[0];
        let llvm_val = self
            .backend
            .context
            .bool_type()
            .const_int(val as u64, false)
            .into();
        self.values.insert(result_id, llvm_val);
        self.value_types.insert(result_id, TirType::Bool);
    }

    pub(super) fn emit_const_none(&mut self, op: &TirOp) {
        let result_id = op.results[0];
        let none_bits = nanbox::QNAN | nanbox::TAG_NONE;
        let llvm_val = self
            .backend
            .context
            .i64_type()
            .const_int(none_bits, false)
            .into();
        self.values.insert(result_id, llvm_val);
        self.value_types.insert(result_id, TirType::None);
    }

    pub(super) fn emit_const_str(&mut self, op: &TirOp) {
        let (result, _) = self.emit_byte_literal(&const_bytes_from_attrs(op), TirType::Str);
        self.values.insert(op.results[0], result);
        self.value_types.insert(op.results[0], TirType::Str);
    }

    pub(super) fn emit_const_bigint(&mut self, op: &TirOp) {
        let result_id = op.results[0];
        let i64_ty = self.backend.context.i64_type();

        let digits: Vec<u8> = match op.attrs.get("s_value") {
            Some(AttrValue::Str(s)) => s.as_bytes().to_vec(),
            other => panic!("ConstBigInt missing s_value attribute: {:?}", other),
        };

        let ptr_val = self.add_private_bytes_global(&digits, "__const_bigint_", "");

        let ptr_ty = self
            .backend
            .context
            .ptr_type(inkwell::AddressSpace::default());
        let bigint_from_str_ty = i64_ty.fn_type(&[ptr_ty.into(), i64_ty.into()], false);
        let bfs_fn = declare_fixed_runtime_function(
            self.backend.context,
            &self.backend.module,
            "molt_bigint_from_str",
        )
        .unwrap_or_else(|| panic!("molt_bigint_from_str must be a fixed LLVM runtime import"));
        let bfs_fn = require_llvm_function_type("molt_bigint_from_str", bfs_fn, bigint_from_str_ty);

        let len_val = i64_ty.const_int(digits.len() as u64, false);
        let call = self
            .backend
            .builder
            .build_call(bfs_fn, &[ptr_val.into(), len_val.into()], "bigint_bits")
            .unwrap();
        let result = call
            .try_as_basic_value()
            .basic()
            .expect("molt_bigint_from_str returns i64 bits");
        self.values.insert(result_id, result);
        self.value_types.insert(result_id, TirType::DynBox);
    }

    pub(super) fn emit_const_bytes(&mut self, op: &TirOp) {
        let (result, _) = self.emit_byte_literal(&const_bytes_from_attrs(op), TirType::Bytes);
        self.values.insert(op.results[0], result);
        self.value_types.insert(op.results[0], TirType::Bytes);
    }

    /// Every materialization returns one owned reference, even for equal payloads.
    /// String constants, attribute names, and bytes share the same outparam ABI;
    /// the semantic literal type selects the fixed runtime constructor.
    fn emit_byte_literal(
        &mut self,
        bytes: &[u8],
        literal_type: TirType,
    ) -> (BasicValueEnum<'ctx>, inkwell::values::IntValue<'ctx>) {
        let (runtime_name, global_prefix, out_name, call_name, load_name) = match literal_type {
            TirType::Str => (
                "molt_string_from_bytes",
                "__const_str_",
                "str_out",
                "sfb",
                "str_bits",
            ),
            TirType::Bytes => (
                "molt_bytes_from_bytes",
                "__const_bytes_",
                "bytes_out",
                "bfb",
                "bytes_bits",
            ),
            _ => panic!("byte literal requires Str or Bytes, got {literal_type:?}"),
        };
        let i64_ty = self.backend.context.i64_type();

        let ptr_val = self.add_private_bytes_global(bytes, global_prefix, "");

        let materializer = declare_fixed_runtime_function(
            self.backend.context,
            &self.backend.module,
            runtime_name,
        )
        .unwrap_or_else(|| panic!("{runtime_name} must be a fixed LLVM runtime import"));
        let ptr_ty = self
            .backend
            .context
            .ptr_type(inkwell::AddressSpace::default());
        let expected_abi = self
            .backend
            .context
            .i32_type()
            .fn_type(&[ptr_ty.into(), i64_ty.into(), ptr_ty.into()], false);
        let materializer = require_llvm_function_type(runtime_name, materializer, expected_abi);
        let out_alloca = self.backend.builder.build_alloca(i64_ty, out_name).unwrap();

        let len_val = i64_ty.const_int(bytes.len() as u64, false);
        let status = self
            .backend
            .builder
            .build_call(
                materializer,
                &[ptr_val.into(), len_val.into(), out_alloca.into()],
                call_name,
            )
            .unwrap()
            .try_as_basic_value()
            .unwrap_basic()
            .into_int_value();

        let value = self
            .backend
            .builder
            .build_load(i64_ty, out_alloca, load_name)
            .unwrap();
        (value, status)
    }

    pub(super) fn const_i64_operand(&self, operand_id: ValueId) -> i64 {
        for block in self.func.blocks.values() {
            for op in &block.ops {
                if op.results.first() == Some(&operand_id)
                    && op.opcode == OpCode::ConstInt
                    && let Some(AttrValue::Int(v)) = op.attrs.get("value")
                {
                    return *v;
                }
            }
        }
        panic!(
            "expected const int operand {:?} in {}",
            operand_id, self.func.name
        );
    }

    /// A synthesized name is one temporary owner, borrowed by the dependent
    /// runtime operation. On construction failure skip that operation entirely;
    /// merge a boxed None with the pending exception intact so the enclosing
    /// TIR operation's CheckException retains ownership of handler/cleanup flow.
    pub(super) fn with_owned_name(
        &mut self,
        name: &str,
        consume: impl FnOnce(&mut Self, inkwell::values::IntValue<'ctx>) -> BasicValueEnum<'ctx>,
    ) -> BasicValueEnum<'ctx> {
        let (name_bits, status) = self.emit_byte_literal(name.as_bytes(), TirType::Str);
        let name_bits = name_bits.into_int_value();
        let builder = &self.backend.builder;
        let source = builder.get_insert_block().expect("name construction block");
        let suffix = self.synthetic_block_counter;
        self.synthetic_block_counter += 1;
        let success = self
            .backend
            .context
            .append_basic_block(self.llvm_fn, &format!("owned_name_success{suffix}"));
        let merge = self
            .backend
            .context
            .append_basic_block(self.llvm_fn, &format!("owned_name_merge{suffix}"));
        self.all_llvm_blocks.extend([success, merge]);
        let ok = builder
            .build_int_compare(
                inkwell::IntPredicate::EQ,
                status,
                self.backend.context.i32_type().const_zero(),
                "owned_name_ok",
            )
            .unwrap();
        builder
            .build_conditional_branch(ok, success, merge)
            .unwrap();
        self.record_llvm_edge(source, success);
        self.record_llvm_edge(source, merge);
        self.backend.builder.position_at_end(success);
        let result = consume(self, name_bits);
        assert_eq!(
            result.get_type(),
            self.backend.context.i64_type().into(),
            "name consumer must return boxed i64 bits"
        );
        let drop_name = self.ensure_runtime_import(MOLT_DEC_REF_OBJ);
        self.backend
            .builder
            .build_call(drop_name, &[name_bits.into()], "owned_name_release")
            .unwrap();
        let consumed = self
            .backend
            .builder
            .get_insert_block()
            .expect("name consumer block");
        self.backend
            .builder
            .build_unconditional_branch(merge)
            .unwrap();
        self.record_llvm_edge(consumed, merge);
        self.backend.builder.position_at_end(merge);
        let i64_ty = self.backend.context.i64_type();
        let none = i64_ty.const_int(nanbox::QNAN | nanbox::TAG_NONE, false);
        let phi = self
            .backend
            .builder
            .build_phi(i64_ty, "owned_name_result")
            .unwrap();
        phi.add_incoming(&[(&none, source), (&result, consumed)]);
        phi.as_basic_value()
    }

    pub(super) fn raw_string_const_ptr_and_len(
        &mut self,
        s: &str,
    ) -> (
        inkwell::values::PointerValue<'ctx>,
        inkwell::values::IntValue<'ctx>,
    ) {
        let i64_ty = self.backend.context.i64_type();
        let name_bytes = s.as_bytes();
        let ptr = self.add_private_bytes_global(
            name_bytes,
            "__guard_attr_str_",
            &format!("_{}", sanitize_const_name(s)),
        );
        let len_bits = i64_ty.const_int(name_bytes.len() as u64, false);
        (ptr, len_bits)
    }

    pub(super) fn raw_string_const_ptr_len(
        &mut self,
        s: &str,
    ) -> (
        inkwell::values::IntValue<'ctx>,
        inkwell::values::IntValue<'ctx>,
    ) {
        let i64_ty = self.backend.context.i64_type();
        let (ptr, len_bits) = self.raw_string_const_ptr_and_len(s);
        let ptr_bits = self
            .backend
            .builder
            .build_ptr_to_int(ptr, i64_ty, "guard_attr_ptr")
            .unwrap();
        (ptr_bits, len_bits)
    }

    fn add_private_bytes_global(
        &mut self,
        bytes: &[u8],
        prefix: &str,
        suffix: &str,
    ) -> inkwell::values::PointerValue<'ctx> {
        let byte_array_ty = self
            .backend
            .context
            .i8_type()
            .array_type(bytes.len() as u32);
        let global_name = format!("{}{}{}", prefix, self.const_str_counter, suffix);
        self.const_str_counter += 1;
        let global = self
            .backend
            .module
            .add_global(byte_array_ty, None, &global_name);
        global.set_linkage(inkwell::module::Linkage::Private);
        global.set_initializer(&self.backend.context.const_string(bytes, false));
        global.set_constant(true);
        global.set_unnamed_addr(true);
        global.as_pointer_value()
    }
}

fn const_bytes_from_attrs(op: &TirOp) -> Vec<u8> {
    if let Some(AttrValue::Bytes(b)) = op.attrs.get("bytes") {
        b.clone()
    } else if let Some(AttrValue::Str(s)) = op.attrs.get("s_value") {
        s.as_bytes().to_vec()
    } else {
        Vec::new()
    }
}

fn sanitize_const_name(s: &str) -> String {
    s.replace(|c: char| !c.is_alphanumeric(), "_")
}
