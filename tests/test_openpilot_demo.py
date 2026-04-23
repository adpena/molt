from __future__ import annotations

import pytest

from tests.helpers.tinygrad_stdlib_loader import tinygrad_stdlib_context


def test_plan_head_feeds_second_layer_from_activated_hidden_projection() -> None:
    with tinygrad_stdlib_context("openpilot_demo") as modules:
        Tensor = modules["tensor"].Tensor
        plan_head = modules["openpilot_demo"].PlanHead.__new__(
            modules["openpilot_demo"].PlanHead
        )
        plan_head.fc1 = (Tensor.ones(3, 2), Tensor.zeros(3))
        plan_head.fc2 = (Tensor.ones(4, 3), Tensor.zeros(4))

        out = plan_head(Tensor.ones(1, 2))

        assert out.shape == (1, 4)
        assert out.tolist()[0] == pytest.approx([6.0, 6.0, 6.0, 6.0])
