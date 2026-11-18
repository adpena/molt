//! Cranelift's spelling of task construction. The protocol is
//! `molt_codegen_abi::task_site::generate_task_construction`; this module
//! spells its runtime calls and runs it inline over the frame builder.

use super::*;
use molt_codegen_abi::task_site::{
    TaskConstructorLayout, TaskFrameContext, TaskPayload, TaskSiteBuilder,
    generate_task_construction,
};

/// The runtime entries a task construction calls.
#[derive(Clone, Copy)]
pub(crate) struct NativeTaskSites {
    /// `molt_task_new(poll, frame_bytes, kind, function)`.
    pub(crate) task_new: FuncRef,
    /// `molt_cancel_token_get_current()`.
    pub(crate) cancel_token_get_current: FuncRef,
    /// `molt_task_register_execution(task, token, inherited)`.
    pub(crate) task_register_execution: FuncRef,
    /// `molt_asyncgen_new(generator)`.
    pub(crate) asyncgen_new: FuncRef,
    /// `molt_int_from_i64(raw)`.
    pub(crate) int_from_i64: FuncRef,
}

impl TaskSiteBuilder for NativeFrameBuilder<'_, '_> {
    fn task_new(&mut self, poll: Value, frame_bytes: Value, kind: Value, function: Value) -> Value {
        let tasks = self.tasks.expect("a task construction sets its task sites");
        let call = self
            .words
            .builder
            .ins()
            .call(tasks.task_new, &[poll, frame_bytes, kind, function]);
        self.words.builder.inst_results(call)[0]
    }

    fn cancel_token_get_current(&mut self) -> Value {
        let tasks = self.tasks.expect("a task construction sets its task sites");
        let call = self
            .words
            .builder
            .ins()
            .call(tasks.cancel_token_get_current, &[]);
        self.words.builder.inst_results(call)[0]
    }

    fn task_register_execution(&mut self, task: Value, token: Value) {
        let tasks = self.tasks.expect("a task construction sets its task sites");
        let inherited = self.words.builder.ins().iconst(types::I64, box_none());
        self.words
            .builder
            .ins()
            .call(tasks.task_register_execution, &[task, token, inherited]);
    }

    fn asyncgen_new(&mut self, generator: Value) -> Value {
        let tasks = self.tasks.expect("a task construction sets its task sites");
        let call = self
            .words
            .builder
            .ins()
            .call(tasks.asyncgen_new, &[generator]);
        self.words.builder.inst_results(call)[0]
    }

    fn int_from_i64(&mut self, raw: Value) -> Value {
        let tasks = self.tasks.expect("a task construction sets its task sites");
        let call = self.words.builder.ins().call(tasks.int_from_i64, &[raw]);
        self.words.builder.inst_results(call)[0]
    }
}

/// Construct one `layout` task inline (`generate_task_construction`). The
/// generator's blocks join the caller's sealed set; the builder ends in the
/// construction's continuation.
#[allow(clippy::too_many_arguments)]
pub(crate) fn emit_task_construction(
    builder: &mut FunctionBuilder<'_>,
    sealed_blocks: &mut BTreeSet<Block>,
    sites: NativeFrameSites,
    tasks: NativeTaskSites,
    layout: TaskConstructorLayout,
    poll: Value,
    frame_bytes: Value,
    context: TaskFrameContext<Value>,
    payload: TaskPayload<'_, Value>,
) -> Value {
    let mut frames = NativeFrameBuilder {
        words: NativeWords { builder },
        sites,
        slow_blocks: Vec::new(),
        sealed: Vec::new(),
        tasks: Some(tasks),
    };
    let task = generate_task_construction(
        &mut frames,
        layout,
        crate::GENERATOR_CONTROL_BYTES,
        poll,
        frame_bytes,
        context,
        payload,
    );
    sealed_blocks.extend(frames.sealed);
    task
}
