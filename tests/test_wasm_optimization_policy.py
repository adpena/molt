from __future__ import annotations

import pytest

from molt.wasm_optimization import (
    WASM_OPT_FEATURE_FLAGS,
    WASM_OPT_LEVELS,
    wasm_link_policy,
    wasm_opt_pipeline,
)


@pytest.mark.parametrize("level", WASM_OPT_LEVELS)
def test_optimizer_policy_covers_every_binaryen_level(level: str) -> None:
    link = wasm_link_policy(level)

    assert link.level == level
    assert len(link.pipeline) == len(set(link.pipeline))
    assert set(WASM_OPT_FEATURE_FLAGS) <= set(link.pipeline)


def test_dev_link_policy_runs_one_nonconverging_o1_level() -> None:
    policy = wasm_link_policy("O1")

    assert policy.apply_level is True
    assert policy.pipeline[0] == "-O1"
    assert "--converge" not in policy.pipeline


def test_optimizer_pipeline_rejects_unknown_level() -> None:
    with pytest.raises(ValueError, match="unsupported wasm-opt level"):
        wasm_opt_pipeline("O99")
