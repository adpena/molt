use super::WasmFunctionFrame;
use crate::FunctionIR;
use crate::wasm::WasmBackend;
use crate::wasm::const_materialization::WasmConstOpPolicy;
use crate::wasm::constant_ops::{emit_const_anchor_materialization, emit_release_const_anchors};
use crate::wasm_abi_generated::WasmRuntimeImport;
use crate::wasm_binary::emit_call;
use crate::wasm_data::DataSegmentRef;
use crate::wasm_import_tracking::TrackedImportIds;
use std::borrow::Cow;
use std::fmt::Write as _;
use wasm_encoder::{BlockType, Catch, Function, Instruction, RefType, ValType};

impl WasmFunctionFrame {
    pub(in crate::wasm) fn emit_debug_local_map(&self, func_ir: &FunctionIR, func_index: u32) {
        let Some(filter) = std::env::var("MOLT_DEBUG_WASM_LOCALS_FUNC").ok() else {
            return;
        };
        if filter != "1" && !func_ir.name.contains(&filter) {
            return;
        }
        let mut dump = format!("WASM_DEBUG_FUNC {} index={func_index}\n", func_ir.name);
        for (idx, op) in func_ir.ops.iter().enumerate() {
            let mut mentioned: Vec<String> = Vec::new();
            if let Some(args) = &op.args {
                mentioned.extend(args.iter().cloned());
            }
            if let Some(var) = &op.var {
                mentioned.push(var.clone());
            }
            if let Some(out) = &op.out {
                mentioned.push(out.clone());
            }
            mentioned.sort();
            mentioned.dedup();
            let mapped: Vec<String> = mentioned
                .into_iter()
                .filter_map(|name| self.locals.get(&name).map(|slot| format!("{name}->{slot}")))
                .collect();
            let result_slot = op.out.as_deref().map(|name| self.locals.result_slot(name));
            let _ = writeln!(
                dump,
                "WASM_DEBUG_OP {} kind={} var={:?} out={:?} args={:?} locals={:?} out_slot={:?}",
                idx, op.kind, op.var, op.out, op.args, mapped, result_slot
            );
        }
        eprint!("{dump}");
        let sanitized: String = func_ir
            .name
            .chars()
            .map(|ch| match ch {
                'a'..='z' | 'A'..='Z' | '0'..='9' | '_' | '-' => ch,
                _ => '_',
            })
            .collect();
        let _ = crate::debug_artifacts::write_debug_artifact(
            format!("wasm/locals/{sanitized}.log"),
            &dump,
        );
    }

    pub(in crate::wasm) fn emit_const_anchor_initializers(
        &self,
        backend: &mut WasmBackend,
        func: &mut Function,
        func_index: u32,
        reloc_enabled: bool,
        import_ids: &TrackedImportIds,
        const_str_scratch_segment: DataSegmentRef,
    ) {
        for anchor in &self.const_anchors {
            self.const_cache.emit_none(func);
            func.instruction(&Instruction::LocalSet(anchor.local));
        }

        for anchor in &self.const_anchors {
            emit_const_anchor_materialization(
                backend,
                func,
                &anchor.op,
                &self.locals,
                func_index,
                reloc_enabled,
                import_ids,
                const_str_scratch_segment,
                anchor.local,
            );
            let policy = WasmConstOpPolicy::for_op(&anchor.op)
                .unwrap_or_else(|| panic!("missing WASM const policy for {}", anchor.op.kind));
            if policy.materialization_can_fail() {
                emit_call(
                    func,
                    reloc_enabled,
                    import_ids[WasmRuntimeImport::ExceptionPending],
                );
                func.instruction(&Instruction::I64Const(0));
                func.instruction(&Instruction::I64Ne);
                func.instruction(&Instruction::If(wasm_encoder::BlockType::Empty));
                self.const_cache.emit_none(func);
                self.emit_return(func, 1);
                func.instruction(&Instruction::End);
            }
            if let Some((source, aliases)) = anchor.scratch_aliases.split_first() {
                for alias in aliases {
                    func.instruction(&Instruction::LocalGet(source.ptr_local()));
                    func.instruction(&Instruction::LocalSet(alias.ptr_local()));
                    func.instruction(&Instruction::LocalGet(source.len_local()));
                    func.instruction(&Instruction::LocalSet(alias.len_local()));
                }
            }
        }

        if self.control_mode.needs_dispatch() {
            for (local_idx, bits) in self.const_seed_locals.iter().copied() {
                func.instruction(&Instruction::I64Const(bits));
                func.instruction(&Instruction::LocalSet(local_idx));
            }
        }
        if let Some(homes) = self.frame_homes.filter(|homes| homes.at_entry) {
            // A split chunk runs inside its stub's frame and borrows its homes;
            // none lent leaves at once with the exception pending.
            self.emit_frame_homes_lend(func, homes, import_ids, reloc_enabled);
            func.instruction(&Instruction::LocalGet(homes.local));
            func.instruction(&Instruction::I32Eqz);
            func.instruction(&Instruction::If(wasm_encoder::BlockType::Empty));
            self.const_cache.emit_none(func);
            self.emit_return(func, 1);
            func.instruction(&Instruction::End);
        }
    }

    pub(in crate::wasm) fn emit_entry_initializers(&self, func: &mut Function) {
        self.const_cache.emit_init(func);
    }

    pub(in crate::wasm) fn emit_unwind_scope_start(&self, func: &mut Function) {
        // Keep the original exception reference, including its tag and payload.
        // This scope also catches exceptions propagated through callees.
        func.instruction(&Instruction::Block(BlockType::Result(ValType::Ref(
            RefType::EXNREF,
        ))));
        func.instruction(&Instruction::TryTable(
            BlockType::Empty,
            Cow::Borrowed(&[Catch::AllRef { label: 0 }]),
        ));
    }

    pub(in crate::wasm) fn emit_return_scope_start(
        &mut self,
        func: &mut Function,
        python_eh_boundary: bool,
    ) {
        self.python_eh_boundary = python_eh_boundary;
        func.instruction(&Instruction::Block(BlockType::Result(ValType::I64)));
        if python_eh_boundary {
            func.instruction(&Instruction::Block(BlockType::Result(ValType::I64)));
            func.instruction(&Instruction::TryTable(
                BlockType::Empty,
                Cow::Borrowed(&[Catch::One {
                    tag: crate::wasm_abi::TAG_EXCEPTION_INDEX,
                    label: 0,
                }]),
            ));
        }
    }

    /// Carry the boxed result out to the single activation epilogue. `depth`
    /// counts structured labels inside the return scope, including synthetic
    /// dispatch and call-guard labels, never source-level Python frame depth.
    pub(in crate::wasm) fn emit_return(&self, func: &mut Function, depth: u32) {
        func.instruction(&Instruction::Br(
            depth + if self.python_eh_boundary { 2 } else { 0 },
        ));
    }

    pub(in crate::wasm) fn emit_owned_frame_entry(
        &self,
        func: &mut Function,
        slot: i64,
        import_ids: &TrackedImportIds,
        reloc_enabled: bool,
    ) {
        let attempted = self
            .owned_frame_attempt
            .expect("trace_enter_slot requires local execution-context ownership");
        func.instruction(&Instruction::I64Const(slot));
        emit_call(
            func,
            reloc_enabled,
            import_ids[WasmRuntimeImport::TraceEnterSlot],
        );
        func.instruction(&Instruction::Drop);
        // The runtime records even an unsuccessful entry. Retire that marker
        // on exit without accidentally popping the caller's Python frame.
        func.instruction(&Instruction::I32Const(1));
        func.instruction(&Instruction::LocalSet(attempted));
        if let Some(homes) = self.frame_homes.filter(|homes| !homes.at_entry) {
            // The entry took the frame's binding homes. A failed entry lends 0
            // with its exception pending, and the entry's adjacent exception
            // check leaves before any home op runs.
            self.emit_frame_homes_lend(func, homes, import_ids, reloc_enabled);
        }
    }

    fn emit_frame_homes_lend(
        &self,
        func: &mut Function,
        homes: super::WasmFrameHomes,
        import_ids: &TrackedImportIds,
        reloc_enabled: bool,
    ) {
        func.instruction(&Instruction::I64Const(homes.slots));
        emit_call(func, reloc_enabled, import_ids[WasmRuntimeImport::FrameHomes]);
        func.instruction(&Instruction::I32WrapI64);
        func.instruction(&Instruction::LocalSet(homes.local));
    }

    fn frame_homes(&self) -> super::WasmFrameHomes {
        self.frame_homes
            .expect("frame home ops require the frame's lent homes")
    }

    fn frame_home_args(slot: i64) -> (wasm_encoder::MemArg, wasm_encoder::MemArg) {
        let offset = u64::try_from(slot * molt_codegen_abi::FRAME_HOME_BYTES)
            .expect("admitted frame home slot");
        let arg = |field: i32| wasm_encoder::MemArg {
            align: 3,
            offset: offset + field as u64,
            memory_index: 0,
        };
        (
            arg(molt_codegen_abi::FRAME_HOME_KIND_OFFSET),
            arg(molt_codegen_abi::FRAME_HOME_BITS_OFFSET),
        )
    }

    /// Replace one slot's pair with `(kind, bits)`, the bits pushed by
    /// `push_bits`, then release what the pair held when its kind owns a
    /// reference: after publication, as STORE_FAST releases, so the finalizer
    /// sees the new binding.
    fn emit_frame_home_replace(
        &self,
        func: &mut Function,
        slot: i64,
        kind: i64,
        push_bits: impl FnOnce(&mut Function),
        import_ids: &TrackedImportIds,
        reloc_enabled: bool,
    ) {
        let homes = self.frame_homes();
        let (kind_arg, bits_arg) = Self::frame_home_args(slot);
        func.instruction(&Instruction::LocalGet(homes.local));
        func.instruction(&Instruction::I64Load(bits_arg));
        func.instruction(&Instruction::LocalSet(homes.displaced));
        // The displaced kind's reference bit stays on the stack across the
        // two stores.
        func.instruction(&Instruction::LocalGet(homes.local));
        func.instruction(&Instruction::I64Load(kind_arg));
        func.instruction(&Instruction::I64Const(
            molt_codegen_abi::FRAME_HOME_HOLDS_REFERENCE,
        ));
        func.instruction(&Instruction::I64And);
        func.instruction(&Instruction::I32WrapI64);
        func.instruction(&Instruction::LocalGet(homes.local));
        func.instruction(&Instruction::I64Const(kind));
        func.instruction(&Instruction::I64Store(kind_arg));
        func.instruction(&Instruction::LocalGet(homes.local));
        push_bits(func);
        func.instruction(&Instruction::I64Store(bits_arg));
        func.instruction(&Instruction::If(wasm_encoder::BlockType::Empty));
        func.instruction(&Instruction::LocalGet(homes.displaced));
        emit_call(func, reloc_enabled, import_ids[WasmRuntimeImport::DecRefObj]);
        func.instruction(&Instruction::End);
    }

    /// Store a binding into its home (`frame_home_store`/`_cell`/
    /// `_private_cell`): the home takes the value's reference.
    pub(in crate::wasm) fn emit_frame_home_store(
        &self,
        func: &mut Function,
        slot: i64,
        kind: i64,
        value_local: u32,
        import_ids: &TrackedImportIds,
        reloc_enabled: bool,
    ) {
        self.emit_frame_home_replace(
            func,
            slot,
            kind,
            |func| {
                func.instruction(&Instruction::LocalGet(value_local));
            },
            import_ids,
            reloc_enabled,
        );
    }

    /// `del`: the slot becomes unbound, then what it held is released.
    pub(in crate::wasm) fn emit_frame_home_clear(
        &self,
        func: &mut Function,
        slot: i64,
        import_ids: &TrackedImportIds,
        reloc_enabled: bool,
    ) {
        self.emit_frame_home_replace(
            func,
            slot,
            molt_codegen_abi::FRAME_HOME_UNBOUND,
            |func| {
                func.instruction(&Instruction::I64Const(0));
            },
            import_ids,
            reloc_enabled,
        );
    }

    /// Push a borrowed view of one slot's plain binding: `PLAIN` read inline,
    /// any other kind through `molt_frame_home_load`, which boxes a raw
    /// integer into the home, reports an unbound slot as the missing sentinel
    /// and raises for a cell.
    pub(in crate::wasm) fn emit_frame_home_load(
        &self,
        func: &mut Function,
        slot: i64,
        import_ids: &TrackedImportIds,
        reloc_enabled: bool,
    ) {
        let homes = self.frame_homes();
        let (kind_arg, bits_arg) = Self::frame_home_args(slot);
        func.instruction(&Instruction::LocalGet(homes.local));
        func.instruction(&Instruction::I64Load(kind_arg));
        func.instruction(&Instruction::I64Const(molt_codegen_abi::FRAME_HOME_PLAIN));
        func.instruction(&Instruction::I64Eq);
        func.instruction(&Instruction::If(wasm_encoder::BlockType::Result(
            wasm_encoder::ValType::I64,
        )));
        func.instruction(&Instruction::LocalGet(homes.local));
        func.instruction(&Instruction::I64Load(bits_arg));
        func.instruction(&Instruction::Else);
        self.emit_frame_home_address(func, slot);
        emit_call(func, reloc_enabled, import_ids[WasmRuntimeImport::FrameHomeLoad]);
        func.instruction(&Instruction::End);
    }

    /// Push one slot's home address, an i64, for the runtime's home entries.
    pub(in crate::wasm) fn emit_frame_home_address(&self, func: &mut Function, slot: i64) {
        let homes = self.frame_homes();
        func.instruction(&Instruction::LocalGet(homes.local));
        func.instruction(&Instruction::I64ExtendI32U);
        func.instruction(&Instruction::I64Const(slot * molt_codegen_abi::FRAME_HOME_BYTES));
        func.instruction(&Instruction::I64Add);
    }

    /// Every activation exit, including backend-created failure and suspension
    /// edges, releases its own resources while retaining the caller's context.
    pub(in crate::wasm) fn emit_exit_cleanup(
        &self,
        func: &mut Function,
        import_ids: &TrackedImportIds,
        reloc_enabled: bool,
    ) {
        emit_release_const_anchors(func, self.const_anchor_locals(), import_ids, reloc_enabled);
        if let Some(attempted) = self.owned_frame_attempt {
            func.instruction(&Instruction::LocalGet(attempted));
            func.instruction(&Instruction::If(wasm_encoder::BlockType::Empty));
            // Consume custody before frame destruction can reenter Python.
            func.instruction(&Instruction::I32Const(0));
            func.instruction(&Instruction::LocalSet(attempted));
            emit_call(
                func,
                reloc_enabled,
                import_ids[WasmRuntimeImport::TraceExit],
            );
            func.instruction(&Instruction::Drop);
            func.instruction(&Instruction::End);
        }
    }

    pub(in crate::wasm) fn emit_epilogue(
        &self,
        func: &mut Function,
        import_ids: &TrackedImportIds,
        reloc_enabled: bool,
        wasm_eh_enabled: bool,
    ) {
        if self.python_eh_boundary {
            self.emit_return(func, 0); // ordinary fallthrough has its boxed result
            func.instruction(&Instruction::End); // Python-tag try_table
            func.instruction(&Instruction::Unreachable);
            func.instruction(&Instruction::End); // Python exception payload
            func.instruction(&Instruction::Drop);
            // Raise registered the exception in canonical runtime state. Do
            // not unwind through caller RC, invocation, recursion or GIL guards.
            self.const_cache.emit_none(func);
        }
        func.instruction(&Instruction::End); // boxed result from any body exit
        self.emit_exit_cleanup(func, import_ids, reloc_enabled);
        func.instruction(&Instruction::Return);
        if wasm_eh_enabled {
            func.instruction(&Instruction::End); // try_table
            func.instruction(&Instruction::Unreachable); // normal paths returned
            func.instruction(&Instruction::End); // exception-reference label
            self.emit_exit_cleanup(func, import_ids, reloc_enabled);
            func.instruction(&Instruction::ThrowRef);
        }
        func.instruction(&Instruction::End);
    }
}
