//! A clear thread-local exception hint proves absence. A set hint may
//! belong to a stale task and must be confirmed against the current task.
use super::builder::{CodegenBuilder, MachineKind};
use molt_ir::runtime_fn_generated::RuntimeFn;

pub(crate) fn exception_pending_word<B: CodegenBuilder>(b: &mut B, thread: B::Word) -> B::Word {
    let hint = b.load_byte(
        thread,
        molt_codegen_abi::THREAD_RECORD_EXCEPTION_PENDING_OFFSET,
    );
    let clear = b.is_zero(hint);
    let confirm = b.create_block("exception_pending_confirm");
    let done = b.create_block("exception_pending_join");
    let answer = b.append_block_param(done, MachineKind::Word);
    let zero = b.constant(0);
    b.branch_if(clear, (done, &[b.word_value(zero)]), (confirm, &[]));
    b.switch_to_block(confirm);
    let callee = b.runtime_callee(RuntimeFn::ExceptionPending);
    let confirmed = b
        .call_cold(callee, &[])
        .expect("the exception reader returns a word");
    b.jump(done, &[confirmed]);
    b.switch_to_block(done);
    b.expect_word(answer)
}

#[cfg(all(test, feature = "native-backend"))]
mod tests {
    use super::*;
    use crate::codegen_ssa::differential::kernel;
    use crate::codegen_ssa::scalar::tests::Value;
    use crate::native_backend::codegen_builder::machine::run_with_pending_byte;

    #[test]
    fn pending_byte_confirms_the_current_exception_only_when_set() {
        let probe = kernel!(; |b, inputs| -> () {
            let thread = b.thread_record_word();
            let constructor = b.runtime_callee(RuntimeFn::IntFromI64);
            b.call(constructor, &[inputs[0]]);
            let answer = exception_pending_word(b, thread);
            ((), vec![b.word_value(answer)])
        });
        for (hint, fail, answer, confirm_calls) in
            [(0, false, 0, 0), (0xff, false, 0, 1), (1, true, 1, 1)]
        {
            let run = run_with_pending_byte(&probe, &[Value::Word(i64::MAX as u64)], fail, hint);
            assert_eq!(run.results, [Value::Word(answer)]);
            assert_eq!(run.runtime_calls[0], RuntimeFn::IntFromI64);
            assert_eq!(
                &run.runtime_calls[1..],
                vec![RuntimeFn::ExceptionPending; confirm_calls]
            );
        }
    }
    #[test]
    fn first_record_read_in_a_sibling_dominates_both_paths() {
        let probe = kernel!(; |b, inputs| -> () {
            let left = b.create_block("left");
            let right = b.create_block("right");
            let join = b.create_block("join");
            let answer = b.append_block_param(join, MachineKind::Word);
            let take_left = b.is_zero(inputs[0]);
            b.branch_if(take_left, (left, &[]), (right, &[]));
            for block in [left, right] {
                b.switch_to_block(block);
                let record = b.thread_record_word();
                let hint = b.load_byte(record, molt_codegen_abi::THREAD_RECORD_EXCEPTION_PENDING_OFFSET);
                let word = b.zero_extend(hint);
                b.jump(join, &[b.word_value(word)]);
            }
            b.switch_to_block(join);
            ((), vec![answer])
        });
        for input in [0, 1] {
            let run = run_with_pending_byte(&probe, &[Value::Word(input)], false, 0xac);
            assert_eq!(run.results, [Value::Word(0xac)]);
            assert!(run.runtime_calls.is_empty());
        }
    }
}
