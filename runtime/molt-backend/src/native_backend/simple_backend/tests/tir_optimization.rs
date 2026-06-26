use super::*;

#[test]
fn tir_optimization_work_partition_respects_count_and_op_budgets() {
    let by_count: Vec<TirOptimizationWorkItem> = (0..(TIR_OPTIMIZATION_BATCH_FUNCTION_LIMIT + 1))
        .map(|index| TirOptimizationWorkItem {
            index,
            content_hash: format!("hash-{index}"),
            op_count: 1,
        })
        .collect();
    let count_batches = partition_tir_optimization_work_items(by_count);
    assert_eq!(count_batches.len(), 2);
    assert_eq!(
        count_batches[0].len(),
        TIR_OPTIMIZATION_BATCH_FUNCTION_LIMIT
    );
    assert_eq!(count_batches[1].len(), 1);

    let by_ops = vec![
        TirOptimizationWorkItem {
            index: 0,
            content_hash: "a".to_string(),
            op_count: TIR_OPTIMIZATION_BATCH_OP_BUDGET / 2,
        },
        TirOptimizationWorkItem {
            index: 1,
            content_hash: "b".to_string(),
            op_count: TIR_OPTIMIZATION_BATCH_OP_BUDGET / 2,
        },
        TirOptimizationWorkItem {
            index: 2,
            content_hash: "c".to_string(),
            op_count: 1,
        },
    ];
    let op_batches = partition_tir_optimization_work_items(by_ops);
    assert_eq!(op_batches.len(), 2);
    assert_eq!(
        op_batches[0]
            .iter()
            .map(|item| item.op_count)
            .sum::<usize>(),
        TIR_OPTIMIZATION_BATCH_OP_BUDGET
    );
    assert_eq!(op_batches[1][0].index, 2);
}

#[test]
fn tir_optimization_work_partition_accepts_inflight_limits() {
    let work: Vec<TirOptimizationWorkItem> = (0..5)
        .map(|index| TirOptimizationWorkItem {
            index,
            content_hash: format!("hash-{index}"),
            op_count: 3,
        })
        .collect();

    let waves = partition_tir_optimization_work_items_with_limits(work, 2, 6);

    assert_eq!(waves.len(), 3);
    assert_eq!(waves[0].len(), 2);
    assert_eq!(waves[1].len(), 2);
    assert_eq!(waves[2].len(), 1);
    assert!(
        waves
            .iter()
            .all(|wave| wave.iter().map(|item| item.op_count).sum::<usize>() <= 6)
    );
}

#[test]
fn tir_optimization_resource_plan_caps_inflight_work_by_memory_limit() {
    let memory_limit =
        TIR_OPTIMIZATION_BASELINE_MEMORY_BYTES + (2 * TIR_OPTIMIZATION_WORKER_MEMORY_BYTES);

    let plan = tir_optimization_resource_plan_from_limits(8, Some(memory_limit));

    assert_eq!(plan.threads, 2);
    assert_eq!(
        plan.wave_function_limit,
        2 * TIR_OPTIMIZATION_WAVE_FUNCTIONS_PER_THREAD
    );
    assert_eq!(
        plan.wave_op_budget,
        2 * TIR_OPTIMIZATION_WAVE_OPS_PER_THREAD
    );
}

#[test]
fn tir_optimization_resource_plan_serializes_under_twelve_gb_guard() {
    let memory_limit = 12 * 1024 * 1024 * 1024;

    let plan = tir_optimization_resource_plan_from_limits(8, Some(memory_limit));

    assert_eq!(plan.threads, 1);
    assert_eq!(plan.wave_function_limit, 1);
    assert_eq!(plan.wave_op_budget, TIR_OPTIMIZATION_WAVE_OPS_PER_THREAD);
}

#[test]
fn tir_optimization_resource_plan_keeps_cpu_parallelism_without_memory_limit() {
    let plan = tir_optimization_resource_plan_from_limits(3, None);

    assert_eq!(plan.threads, 3);
    assert_eq!(
        plan.wave_function_limit,
        3 * TIR_OPTIMIZATION_WAVE_FUNCTIONS_PER_THREAD
    );
    assert_eq!(
        plan.wave_op_budget,
        3 * TIR_OPTIMIZATION_WAVE_OPS_PER_THREAD
    );
}

#[test]
fn deferred_codegen_flush_predicate_bounds_function_and_op_retention() {
    assert!(!should_flush_deferred_codegen(
        0,
        DEFERRED_CODEGEN_FLUSH_OP_BUDGET
    ));
    assert!(!should_flush_deferred_codegen(
        DEFERRED_CODEGEN_FLUSH_FUNCTION_LIMIT - 1,
        DEFERRED_CODEGEN_FLUSH_OP_BUDGET - 1
    ));
    assert!(should_flush_deferred_codegen(
        DEFERRED_CODEGEN_FLUSH_FUNCTION_LIMIT,
        1
    ));
    assert!(should_flush_deferred_codegen(
        1,
        DEFERRED_CODEGEN_FLUSH_OP_BUDGET
    ));
}
