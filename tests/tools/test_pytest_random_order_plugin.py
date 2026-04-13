from __future__ import annotations

import importlib.util
from dataclasses import dataclass
from pathlib import Path


REPO_ROOT = Path(__file__).resolve().parents[2]
PLUGIN_PY = REPO_ROOT / "tools" / "pytest_random_order_plugin.py"


def _load_plugin():
    spec = importlib.util.spec_from_file_location(
        "molt_pytest_random_order_plugin", PLUGIN_PY
    )
    assert spec is not None
    assert spec.loader is not None
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


@dataclass
class _Item:
    nodeid: str


def test_reorder_items_in_place_is_deterministic() -> None:
    module = _load_plugin()
    items_a = [_Item("a"), _Item("b"), _Item("c"), _Item("d")]
    items_b = [_Item("a"), _Item("b"), _Item("c"), _Item("d")]

    module.reorder_items_in_place(items_a, "17")
    module.reorder_items_in_place(items_b, "17")

    assert [item.nodeid for item in items_a] == [item.nodeid for item in items_b]
    assert [item.nodeid for item in items_a] != ["a", "b", "c", "d"]
