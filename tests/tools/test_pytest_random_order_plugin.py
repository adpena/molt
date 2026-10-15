from __future__ import annotations

from dataclasses import dataclass

from tools import pytest_random_order_plugin


@dataclass
class _Item:
    nodeid: str


def test_reorder_items_in_place_is_deterministic() -> None:
    module = pytest_random_order_plugin
    items_a = [_Item("a"), _Item("b"), _Item("c"), _Item("d")]
    items_b = [_Item("a"), _Item("b"), _Item("c"), _Item("d")]

    module.reorder_items_in_place(items_a, "17")
    module.reorder_items_in_place(items_b, "17")

    assert [item.nodeid for item in items_a] == [item.nodeid for item in items_b]
    assert [item.nodeid for item in items_a] != ["a", "b", "c", "d"]
