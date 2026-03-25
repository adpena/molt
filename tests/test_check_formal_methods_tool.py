from __future__ import annotations

import tools.check_formal_methods as check_formal_methods


def test_check_inventory_returns_result() -> None:
    """Verify check_inventory runs without error (smoke test)."""
    result = check_formal_methods.check_inventory()
    assert isinstance(result, check_formal_methods.CheckResult)
