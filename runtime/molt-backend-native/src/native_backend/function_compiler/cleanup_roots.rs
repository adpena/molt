use super::*;

/// Executable native ownership, carried through Cranelift SSA rather than
/// recorded in source-emission order. Each alias root has one boxed owner
/// token: None before acquisition/after release, the owned value while live.
/// A branch writes only its own SSA state; joins and loop backedges receive
/// the actual predecessor state through Cranelift's variable construction.
#[cfg(feature = "native-backend")]
pub(in crate::native_backend::function_compiler) struct NativeCleanupRoots {
    roots: BTreeMap<String, Variable>,
    aliases: BTreeMap<String, String>,
    borrowed_params: BTreeSet<String>,
    explicit_credits: BTreeMap<String, Variable>,
    empty_token: Option<Value>,
    release_generation: std::rc::Rc<std::cell::Cell<u64>>,
}

#[cfg(feature = "native-backend")]
impl NativeCleanupRoots {
    pub(super) fn new(
        builder: &mut FunctionBuilder,
        function: &FunctionIR,
        aliases: &BTreeMap<String, String>,
        representation: &ScalarRepresentationPlan,
        authority: NativeRcAuthority,
    ) -> Self {
        let mut roots = BTreeMap::new();
        let mut borrowed_params = BTreeSet::new();
        let mut explicit_credits = BTreeMap::new();
        let boxed = |name: &str| {
            name != "none"
                && !representation.is_raw_int_carrier_name(name)
                && !representation.is_float_unboxed(name)
                && !representation.is_bool_unboxed(name)
        };
        if authority.native_value_tracking_enabled() {
            borrowed_params.extend(function.params.iter().filter(|name| boxed(name)).cloned());
            // Actual defining operations, not the var registry, determine which
            // roots can acquire owners. Helper _ptr/_len carriers and never-
            // assigned borrowed parameters must not generate cleanup phis.
            for op in &function.ops {
                crate::tir::simple_def_use::visit_simple_ir_defined_names(op, |name| {
                    let stored_binding =
                        simple_ir_binding(op).is_some_and(|binding| binding.destination == name);
                    if boxed(name)
                        && (stored_binding
                            || preanalyze_alias_source(op).is_none_or(|source| {
                                native_alias_mints_owner(aliases, source, name)
                            }))
                        && op.kind != "delete_var"
                    {
                        let root = alias_root_name(aliases, name);
                        roots
                            .entry(root.to_string())
                            .or_insert_with(|| builder.declare_var(types::I64));
                    }
                });
                // No-result retains are explicit credits, not another automatic
                // owner. Their matching explicit release must not erase the
                // original root's independent cleanup obligation.
                if matches!(op.kind.as_str(), "inc_ref" | "borrow")
                    && op.out.as_deref().is_none_or(|name| name == "none")
                    && let Some(source) = op.args.as_ref().and_then(|args| args.first())
                {
                    let root = alias_root_name(aliases, source);
                    explicit_credits
                        .entry(root.to_string())
                        .or_insert_with(|| builder.declare_var(types::I64));
                }
            }
        }
        Self {
            roots,
            aliases: aliases.clone(),
            borrowed_params,
            explicit_credits,
            empty_token: None,
            release_generation: Default::default(),
        }
    }

    pub(super) fn initialize(&mut self, builder: &mut FunctionBuilder) {
        if !self.roots.is_empty() {
            // Construction declares variables before the entry block exists.
            // Materialize one dominating sentinel only after entry parameters
            // are complete, so empty sibling owners share the same SSA value.
            assert!(
                self.empty_token.is_none(),
                "cleanup roots initialized twice"
            );
            let none = builder.ins().iconst(types::I64, box_none());
            self.empty_token = Some(none);
            for &token in self.roots.values() {
                builder.def_var(token, none);
            }
        }
        if !self.explicit_credits.is_empty() {
            let zero = builder.ins().iconst(types::I64, 0);
            for &credits in self.explicit_credits.values() {
                builder.def_var(credits, zero);
            }
        }
    }

    pub(super) fn retain_explicit(&self, builder: &mut FunctionBuilder, name: &str) {
        if let Some(&credits) = self
            .explicit_credits
            .get(alias_root_name(&self.aliases, name))
        {
            let count = builder.use_var(credits);
            let next = builder.ins().iadd_imm(count, 1);
            builder.def_var(credits, next);
        }
    }

    pub(super) fn consume_explicit(&self, builder: &mut FunctionBuilder, name: &str) {
        let root = alias_root_name(&self.aliases, name);
        if let Some(&credits) = self.explicit_credits.get(root) {
            let count = builder.use_var(credits);
            let zero = builder.ins().iconst(types::I64, 0);
            let extra =
                builder
                    .ins()
                    .icmp_imm(cranelift_codegen::ir::condcodes::IntCC::NotEqual, count, 0);
            let decremented = builder.ins().iadd_imm(count, -1);
            let remaining = builder.ins().select(extra, decremented, zero);
            builder.def_var(credits, remaining);
            if let Some(token) = self.token(name) {
                let owner = builder.use_var(token);
                let none = self.empty_token.expect("a tracked root has an empty token");
                let remaining_owner = builder.ins().select(extra, owner, none);
                builder.def_var(token, remaining_owner);
            }
        } else {
            self.transfer(builder, name);
        }
    }

    fn token(&self, name: &str) -> Option<Variable> {
        self.roots
            .get(alias_root_name(&self.aliases, name))
            .copied()
    }

    pub(super) fn contains(&self, name: &str) -> bool {
        self.token(name).is_some()
    }

    /// Emission custody for observations invalidated by arbitrary finalizers.
    /// Shared consumers check this at use time, including within one IR op.
    pub(super) fn release_generation(&self) -> std::rc::Rc<std::cell::Cell<u64>> {
        self.release_generation.clone()
    }

    pub(super) fn shares_owner(&self, source: &str, destination: &str) -> bool {
        alias_root_name(&self.aliases, source) == alias_root_name(&self.aliases, destination)
    }

    /// Publish replacement ownership before running the displaced finalizer.
    /// This is the same rule for first acquisition, rebinding and loop re-entry;
    /// no loop-only old-value cleanup lane is needed.
    pub(super) fn acquire(
        &self,
        builder: &mut FunctionBuilder,
        callee: FuncRef,
        name: &str,
        value: Value,
    ) {
        if let Some(token) = self.token(name) {
            let previous = builder.use_var(token);
            builder.def_var(token, value);
            if let Some(&credits) = self
                .explicit_credits
                .get(alias_root_name(&self.aliases, name))
            {
                // A rebind begins a new local-owner epoch. Prior no-result
                // retain credits remain explicit external obligations; they
                // must not suppress release of this replacement owner.
                let zero = builder.ins().iconst(types::I64, 0);
                builder.def_var(credits, zero);
            }
            self.release_value(builder, callee, previous);
        }
    }

    /// The return ABI requires one owner even when this path still holds the
    /// initially borrowed parameter. An empty token retains the returned value;
    /// an acquired token transfers its existing credit without an extra retain.
    pub(super) fn return_owned(
        &self,
        builder: &mut FunctionBuilder,
        callee: FuncRef,
        name: &str,
        value: Value,
    ) {
        if !self
            .borrowed_params
            .contains(alias_root_name(&self.aliases, name))
        {
            self.transfer(builder, name);
            return;
        }
        if let Some(token) = self.token(name) {
            let owned = builder.use_var(token);
            let none = self.empty_token.expect("a tracked root has an empty token");
            let borrowed = builder.ins().icmp_imm(
                cranelift_codegen::ir::condcodes::IntCC::Equal,
                owned,
                box_none(),
            );
            let retain = builder.ins().select(borrowed, value, none);
            emit_inc_ref_obj(builder, retain, callee);
            builder.def_var(token, none);
        } else if self
            .borrowed_params
            .contains(alias_root_name(&self.aliases, name))
        {
            emit_inc_ref_obj(builder, value, callee);
        }
    }

    /// Transfer ownership to the caller/runtime without releasing it here.
    pub(super) fn transfer(&self, builder: &mut FunctionBuilder, name: &str) {
        if let Some(token) = self.token(name) {
            let none = self.empty_token.expect("a tracked root has an empty token");
            builder.def_var(token, none);
        }
    }

    pub(super) fn release(&self, builder: &mut FunctionBuilder, callee: FuncRef, name: &str) {
        let Some(token) = self.token(name) else {
            return;
        };
        let value = builder.use_var(token);
        self.transfer(builder, name);
        self.release_value(builder, callee, value);
    }

    fn release_value(&self, builder: &mut FunctionBuilder, callee: FuncRef, value: Value) {
        // Avoid emitting calls for statically empty tokens. Mixed-path tokens
        // remain ordinary boxed SSA values; dec_ref_obj(None) is a no-op.
        let value = builder.func.dfg.resolve_aliases(value);
        let empty = match builder.func.dfg.value_def(value) {
            cranelift_codegen::ir::ValueDef::Result(inst, _) => matches!(
                builder.func.dfg.insts[inst],
                cranelift_codegen::ir::InstructionData::UnaryImm { opcode: cranelift_codegen::ir::Opcode::Iconst, imm }
                    if imm.bits() == box_none()
            ),
            _ => false,
        };
        if !empty {
            self.release_generation.set(
                self.release_generation
                    .get()
                    .checked_add(1)
                    .expect("native release emission generation overflow"),
            );
            builder.ins().call(callee, &[value]);
        }
    }

    pub(super) fn release_all(&self, builder: &mut FunctionBuilder, callee: FuncRef) {
        for name in self.roots.keys() {
            self.release(builder, callee, name);
        }
    }
}

#[cfg(feature = "native-backend")]
pub(in crate::native_backend::function_compiler) fn is_join_slot_name(name: &str) -> bool {
    name.starts_with("_bb") && name.contains("_arg")
}

#[cfg(feature = "native-backend")]
pub(in crate::native_backend::function_compiler) fn is_compiler_value_temp_name(
    name: &str,
) -> bool {
    name.strip_prefix("_v")
        .or_else(|| name.strip_prefix('v'))
        .is_some_and(|suffix| suffix.as_bytes().first().is_some_and(u8::is_ascii_digit))
}

#[cfg(feature = "native-backend")]
pub(in crate::native_backend::function_compiler) fn is_persistent_local_slot_name(
    name: &str,
) -> bool {
    is_join_slot_name(name) || !is_compiler_value_temp_name(name)
}

#[cfg(feature = "native-backend")]
pub(in crate::native_backend::function_compiler) fn alias_root_name<'a>(
    alias_roots: &'a BTreeMap<String, String>,
    name: &'a str,
) -> &'a str {
    alias_roots.get(name).map(String::as_str).unwrap_or(name)
}

#[cfg(feature = "native-backend")]
pub(in crate::native_backend::function_compiler) fn protect_cleanup_names(
    carry: &mut Vec<String>,
    cleanup: Vec<String>,
    protected: &BTreeSet<&str>,
) -> Vec<String> {
    if protected.is_empty() {
        return cleanup;
    }
    let mut preserved = Vec::new();
    let mut actual = Vec::new();
    for name in cleanup {
        if protected.contains(name.as_str()) {
            preserved.push(name);
        } else {
            actual.push(name);
        }
    }
    crate::extend_unique_tracked(carry, preserved);
    actual
}

