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
        self.emit_const_bytes_via_runtime(
            op,
            const_bytes_from_attrs(op),
            "__const_str_",
            "str_out",
            "sfb",
            "str_bits",
            TirType::Str,
        );
    }

    pub(super) fn emit_const_bigint(&mut self, op: &TirOp) {
        let result_id = op.results[0];
        let i64_ty = self.backend.context.i64_type();

        let digits: Vec<u8> = match op.attrs.get("s_value") {
            Some(AttrValue::Str(s)) => s.as_bytes().to_vec(),
            other => panic!("ConstBigInt missing s_value attribute: {:?}", other),
        };

        let byte_array_ty = self
            .backend
            .context
            .i8_type()
            .array_type(digits.len() as u32);
        let global = self.backend.module.add_global(
            byte_array_ty,
            None,
            &format!("__const_bigint_{}", self.const_str_counter),
        );
        self.const_str_counter += 1;
        global.set_linkage(inkwell::module::Linkage::Private);
        global.set_initializer(&self.backend.context.const_string(&digits, false));
        global.set_constant(true);
        global.set_unnamed_addr(true);

        let ptr_ty = self
            .backend
            .context
            .ptr_type(inkwell::AddressSpace::default());
        let bigint_from_str_ty = i64_ty.fn_type(&[ptr_ty.into(), i64_ty.into()], false);
        let bfs_fn = if let Some(f) = self.backend.module.get_function("molt_bigint_from_str") {
            require_llvm_function_type("molt_bigint_from_str", f, bigint_from_str_ty)
        } else {
            declare_conservative_runtime_function(
                self.backend.context,
                &self.backend.module,
                "molt_bigint_from_str",
                bigint_from_str_ty,
            )
        };

        let ptr_val = global.as_pointer_value();
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
        self.emit_const_bytes_via_runtime(
            op,
            const_bytes_from_attrs(op),
            "__const_bytes_",
            "bytes_out",
            "bfb",
            "bytes_bits",
            TirType::DynBox,
        );
    }

    fn emit_const_bytes_via_runtime(
        &mut self,
        op: &TirOp,
        bytes: Vec<u8>,
        global_prefix: &str,
        out_name: &str,
        call_name: &str,
        load_name: &str,
        result_ty: TirType,
    ) {
        let result_id = op.results[0];
        let i64_ty = self.backend.context.i64_type();

        let byte_array_ty = self
            .backend
            .context
            .i8_type()
            .array_type(bytes.len() as u32);
        let global = self.backend.module.add_global(
            byte_array_ty,
            None,
            &format!("{}{}", global_prefix, self.const_str_counter),
        );
        self.const_str_counter += 1;
        global.set_linkage(inkwell::module::Linkage::Private);
        global.set_initializer(&self.backend.context.const_string(&bytes, false));
        global.set_constant(true);
        global.set_unnamed_addr(true);

        let sfb_fn = self.ensure_string_from_bytes_fn();
        let out_alloca = self.backend.builder.build_alloca(i64_ty, out_name).unwrap();

        let ptr_val = global.as_pointer_value();
        let len_val = i64_ty.const_int(bytes.len() as u64, false);
        self.backend
            .builder
            .build_call(
                sfb_fn,
                &[ptr_val.into(), len_val.into(), out_alloca.into()],
                call_name,
            )
            .unwrap();

        let result = self
            .backend
            .builder
            .build_load(i64_ty, out_alloca, load_name)
            .unwrap();
        self.values.insert(result_id, result);
        self.value_types.insert(result_id, result_ty);
    }

    fn ensure_string_from_bytes_fn(&self) -> FunctionValue<'ctx> {
        if let Some(f) = self.backend.module.get_function("molt_string_from_bytes") {
            return f;
        }

        let ptr_ty = self
            .backend
            .context
            .ptr_type(inkwell::AddressSpace::default());
        let i32_ty = self.backend.context.i32_type();
        let i64_ty = self.backend.context.i64_type();
        let fn_ty = i32_ty.fn_type(&[ptr_ty.into(), i64_ty.into(), ptr_ty.into()], false);
        self.backend.module.add_function(
            "molt_string_from_bytes",
            fn_ty,
            Some(inkwell::module::Linkage::External),
        )
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
