from __future__ import annotations

import types

import pytest

from molt_accel.contracts import (
    build_compute_payload,
    build_list_items_payload,
    build_offload_table_payload,
)
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
    payload = build_list_items_payload(request)
    assert payload["limit"] == 50


def test_build_list_items_payload_missing_user() -> None:
    request = types.SimpleNamespace(GET={"limit": "10"})
    with pytest.raises(MoltInvalidInput):
        build_list_items_payload(request)


def test_build_compute_payload_query_params() -> None:
    request = types.SimpleNamespace(
        GET={"values": "1,2,3", "scale": "2", "offset": "1"}
    )
    payload = build_compute_payload(request)
    assert payload["values"] == [1.0, 2.0, 3.0]
    assert payload["scale"] == 2.0
    assert payload["offset"] == 1.0


def test_build_compute_payload_body_json() -> None:
    request = types.SimpleNamespace(body=b'{"values":[1,"2",3],"scale":3,"offset":-1}')
    payload = build_compute_payload(request)
    assert payload["values"] == [1.0, 2.0, 3.0]
    assert payload["scale"] == 3.0
    assert payload["offset"] == -1.0


def test_build_offload_table_payload_defaults() -> None:
    request = types.SimpleNamespace()
    payload = build_offload_table_payload(request)
    assert payload["rows"] == 10_000


def test_build_offload_table_payload_body_clamp() -> None:
    request = types.SimpleNamespace(body=b'{"rows": 60000}')
    payload = build_offload_table_payload(request)
    assert payload["rows"] == 50_000
