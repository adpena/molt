"""MidendOptimizationMixin: composed frontend IR midend optimizer.

The optimizer authorities live in cohesive sibling mixins: policy/state,
value canonicalization, SCCP/dataflow, CFG rewrites, and pipeline rounds.
This module is the single MRO assembly point consumed by SimpleTIRGenerator.
"""

from __future__ import annotations

from molt.frontend.lowering.midend_canonicalization import (
    MidendCanonicalizationMixin,
)
from molt.frontend.lowering.midend_cfg import MidendCFGMixin
from molt.frontend.lowering.midend_dataflow import MidendDataflowMixin
from molt.frontend.lowering.midend_pipeline import MidendPipelineMixin
from molt.frontend.lowering.midend_policy import MidendPolicyMixin


class MidendOptimizationMixin(
    MidendPipelineMixin,
    MidendPolicyMixin,
    MidendCanonicalizationMixin,
    MidendDataflowMixin,
    MidendCFGMixin,
):
    """Composed midend optimizer authority for SimpleTIRGenerator."""

    pass
