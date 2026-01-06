from __future__ import annotations

import types

from molt_accel.contracts import build_list_items_payload


def test_build_list_items_payload() -> None:
    request = types.SimpleNamespace(
        GET={"user_id": "7", "limit": "25", "status": "open"}
    )
    payload = build_list_items_payload(request)
    assert payload["user_id"] == 7
    assert payload["limit"] == 25
    assert payload["status"] == "open"
