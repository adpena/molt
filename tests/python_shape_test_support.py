"""Shared shape assertions keep independent source provenance orthogonal."""

from molt.compiler_analysis.static_truth import (
    StaticExpressionResult,
    UNKNOWN_EXPRESSION_RESULT,
)


def has_unknown_shape(result: StaticExpressionResult) -> bool:
    # Provenance is orthogonal to shape; preserve all prior shape/lifetime checks.
    from dataclasses import replace

    pending = [result]
    while pending:
        current = pending.pop()
        if (
            replace(
                current,
                identities=UNKNOWN_EXPRESSION_RESULT.identities,
                source_origins=frozenset(),
                exposed_source_origins=frozenset(),
                deferred=frozenset(),
                exposed_deferred=frozenset(),
                exposes_module_globals=False,
                element_result=None,
            )
            != UNKNOWN_EXPRESSION_RESULT
        ):
            return False
        if current.element_result is not None:
            pending.append(current.element_result)
    return True
