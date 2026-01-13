from __future__ import annotations

import types

import pytest

from molt_accel.contracts import build_list_items_payload
from molt_accel.errors import MoltInvalidInput


def test_build_list_items_payload() -> None:
    request = types.SimpleNamespace(
        GET={"user_id": "7", "limit": "25", "status": "open"}
    )
    payload = build_list_items_payload(request)
    assert payload["user_id"] == 7
    assert payload["limit"] == 25
    assert payload["status"] == "open"


def test_build_list_items_payload_invalid_limit() -> None:
    request = types.SimpleNamespace(GET={"user_id": "7", "limit": "nope"})
    with pytest.raises(MoltInvalidInput):
        build_list_items_payload(request)


def test_build_list_items_payload_missing_user() -> None:
    request = types.SimpleNamespace(GET={"limit": "10"})
    with pytest.raises(MoltInvalidInput):
        build_list_items_payload(request)
