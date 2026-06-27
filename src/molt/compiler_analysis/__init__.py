from __future__ import annotations

from molt.compiler_analysis.backend_ir import (
    backend_ir_allocation_categories,
    backend_ir_binary_image_analysis_payload,
    backend_ir_canonical_kind,
    backend_ir_op_source_site,
)
from molt.compiler_analysis.hashing import stable_payload_hash
from molt.compiler_analysis.static_truth import (
    is_type_checking_test,
    static_if_live_branch,
    static_test_truthiness,
)
from molt.compiler_analysis.tir_fact_graph import summarize_tir_fact_graph
from molt.compiler_analysis.validation import (
    check_compiler_analysis_against_closure,
    summarize_compiler_binary_image_analysis,
    validate_binary_image_closure_diagnostics,
)

__all__ = [
    "backend_ir_allocation_categories",
    "backend_ir_binary_image_analysis_payload",
    "backend_ir_canonical_kind",
    "backend_ir_op_source_site",
    "check_compiler_analysis_against_closure",
    "is_type_checking_test",
    "stable_payload_hash",
    "static_if_live_branch",
    "static_test_truthiness",
    "summarize_compiler_binary_image_analysis",
    "summarize_tir_fact_graph",
    "validate_binary_image_closure_diagnostics",
]
