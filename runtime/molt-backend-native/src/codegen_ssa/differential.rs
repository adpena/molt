//! The walker's model tests on two machines: the interpreting builder, which
//! runs each instruction as it is built, and Cranelift machine code, built
//! through `NativeCodegen`, compiled by the JIT and called on the same inputs.
//!
//! A test states its walker code once, as a [`Kernel`] generic over the
//! builder. [`both`] runs it on each machine and requires the same results,
//! the same build-time decisions and the same runtime calls. A kernel whose
//! code branches and rejoins through block parameters runs on the machine
//! alone ([`machine`]): the interpreter builds every block as it goes and
//! cannot follow a parameter, so such a test checks the machine against its
//! oracle directly.

use molt_codegen_abi::inline_refcount::RefcountOperation;
use molt_ir::runtime_fn_generated::RuntimeFn;

use super::builder::{CodegenBuilder, MachineKind};
use super::scalar::tests::{Interpreter, Value};

/// Walker code under test, written once for every builder.
pub(crate) trait Kernel {
    /// What the walker decided while building (a carrier, a refusal); the
    /// machines must agree on it.
    type Fact: PartialEq + std::fmt::Debug;
    /// Build the code over `inputs`, one machine value per test input, and
    /// return the values the test reads, in the block the code ends in.
    fn build<B: CodegenBuilder>(
        &self,
        b: &mut B,
        inputs: &[B::Value],
    ) -> (Self::Fact, Vec<B::Value>);
}

/// One run of a kernel: what it decided, what it computed, and what it asked
/// the runtime for, in order.
#[derive(Debug)]
pub(crate) struct Run<F> {
    pub(crate) fact: F,
    pub(crate) results: Vec<Value>,
    pub(crate) runtime_calls: Vec<RuntimeFn>,
    /// Each reference-count transition that reached the runtime.
    pub(crate) refcounts: Vec<(RefcountOperation, u64)>,
    /// The words each `molt_call_inputs_release` read from its spill.
    pub(crate) released: Vec<Vec<u64>>,
}

/// A value's machine kind and bits: equal floats compare by their bits, so
/// a NaN equals itself. An address is a word on every machine.
fn bits(value: Value) -> (MachineKind, u64) {
    match value {
        Value::Bit(bit) => (MachineKind::Bit, u64::from(bit)),
        Value::Narrow(narrow) => (MachineKind::NarrowInt, u64::from(narrow)),
        Value::Word(word) | Value::Address(word) => (MachineKind::Word, word),
        Value::Float(float) => (MachineKind::Float, float.to_bits()),
    }
}

/// Whether the shared refcount generator forwards `word` to the runtime: a
/// non-null pointer-tagged word under the runtime admission. Every other
/// word (an inline int, None) is decided inline and never reaches it.
fn reaches_runtime(word: u64) -> bool {
    use molt_codegen_abi::{POINTER_MASK, QNAN, TAG_MASK, TAG_PTR};
    word & (QNAN | TAG_MASK) == QNAN | TAG_PTR && word & POINTER_MASK != 0
}

/// Run `kernel` on the interpreting builder. Its refcounts are the
/// transitions the runtime would see.
pub(crate) fn interpret<K: Kernel>(kernel: &K, inputs: &[Value]) -> Run<K::Fact> {
    let mut b = Interpreter::default();
    let (fact, results) = kernel.build(&mut b, inputs);
    Run {
        fact,
        results,
        runtime_calls: b.runtime_calls,
        refcounts: b
            .refcounts
            .into_iter()
            .filter(|&(_, word)| reaches_runtime(word))
            .collect(),
        released: b.runtime.released,
    }
}

/// Run `kernel` as Cranelift machine code. `fail_allocation` makes every
/// runtime constructor fail, so the failure paths execute.
#[cfg(feature = "native-backend")]
pub(crate) fn machine<K: Kernel>(
    kernel: &K,
    inputs: &[Value],
    fail_allocation: bool,
) -> Run<K::Fact> {
    crate::native_backend::codegen_builder::machine::run(kernel, inputs, fail_allocation)
}

/// Run `kernel` on both machines, require them to agree, and return the run.
pub(crate) fn both<K: Kernel>(kernel: &K, inputs: &[Value]) -> Run<K::Fact> {
    let interpreted = interpret(kernel, inputs);
    #[cfg(feature = "native-backend")]
    {
        let compiled = machine(kernel, inputs, false);
        assert_eq!(
            interpreted.fact, compiled.fact,
            "the machines decided differently on {inputs:?}"
        );
        let interpreted_bits: Vec<_> = interpreted.results.iter().copied().map(bits).collect();
        let compiled_bits: Vec<_> = compiled.results.iter().copied().map(bits).collect();
        assert_eq!(
            interpreted_bits, compiled_bits,
            "the machines computed differently on {inputs:?}: {:?} vs {:?}",
            interpreted.results, compiled.results
        );
        assert_eq!(
            interpreted.runtime_calls, compiled.runtime_calls,
            "the machines called the runtime differently on {inputs:?}"
        );
        assert_eq!(
            interpreted.refcounts, compiled.refcounts,
            "the machines counted references differently on {inputs:?}"
        );
        assert_eq!(
            interpreted.released, compiled.released,
            "the machines released different call inputs on {inputs:?}"
        );
    }
    interpreted
}

/// A [`Kernel`] from a body generic over the builder `B`. Captured fields are
/// owned (`name: Type = value`) and bound by reference in the body.
///
/// `kernel!(plan: ProducerCarrierPlan = plan.clone(); |b, inputs| -> Fact { .. })`
macro_rules! kernel {
    ($($field:ident : $ty:ty = $value:expr),* ; |$b:ident, $inputs:ident| -> $fact:ty $body:block) => {{
        struct K {
            $($field: $ty,)*
        }
        impl $crate::codegen_ssa::differential::Kernel for K {
            type Fact = $fact;
            #[allow(unused_variables)]
            fn build<B: $crate::codegen_ssa::CodegenBuilder>(
                &self,
                $b: &mut B,
                $inputs: &[B::Value],
            ) -> ($fact, Vec<B::Value>) {
                let K { $($field,)* } = self;
                $body
            }
        }
        K { $($field: $value,)* }
    }};
}
pub(crate) use kernel;
