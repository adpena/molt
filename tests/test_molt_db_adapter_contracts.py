from __future__ import annotations

import pytest

from molt_accel.errors import MoltInvalidInput
from molt_db_adapter.contracts import (
    DbParam,
    build_db_exec_payload,
    build_db_query_payload,
)


def test_build_db_query_payload_positional() -> None:
    payload = build_db_query_payload(
        sql="select * from items where id = ?",
        params=[7, "open"],
    )
    assert payload["db_alias"] == "default"
    assert payload["params"]["mode"] == "positional"
    assert payload["params"]["values"] == [7, "open"]
    assert payload["max_rows"] == 1000
    assert payload["result_format"] == "json"
    assert payload["allow_write"] is False


def test_build_db_query_payload_named() -> None:
    payload = build_db_query_payload(
        sql="select * from items where status = :status and id = :id",
        params={"status": "open", "id": 4},
        result_format="msgpack",
        max_rows=50,
        tag="items_list",
    )
    assert payload["params"]["mode"] == "named"
    assert payload["params"]["values"] == [
        {"name": "id", "value": 4},
        {"name": "status", "value": "open"},
    ]
    assert payload["result_format"] == "msgpack"
    assert payload["max_rows"] == 50
    assert payload["tag"] == "items_list"


def test_build_db_query_payload_bytes_param() -> None:
    payload = build_db_query_payload(
        sql="select * from blobs where data = ?",
        params=[bytearray(b"blob")],
        result_format="arrow_ipc",
    )
    assert payload["params"]["values"] == [b"blob"]


def test_build_db_query_payload_null_requires_type() -> None:
    payload = build_db_query_payload(
        sql="select * from items where id is ?",
        params=[DbParam(None, "int8")],
    )
    assert payload["params"]["values"] == [{"value": None, "type": "int8"}]


def test_build_db_query_payload_invalid_param_type() -> None:
    with pytest.raises(MoltInvalidInput):
        build_db_query_payload(sql="select 1", params=[object()])


def test_build_db_query_payload_null_without_type() -> None:
    with pytest.raises(MoltInvalidInput):
        build_db_query_payload(sql="select 1", params=[None])


def test_build_db_query_payload_invalid_format() -> None:
    with pytest.raises(MoltInvalidInput):
        build_db_query_payload(sql="select 1", result_format="yaml")


def test_build_db_query_payload_invalid_db_alias() -> None:
    with pytest.raises(MoltInvalidInput):
        build_db_query_payload(sql="select 1", db_alias=" ")


def test_build_db_query_payload_invalid_params_shape() -> None:
    with pytest.raises(MoltInvalidInput):
        build_db_query_payload(sql="select 1", params="oops")


def test_build_db_exec_payload_defaults() -> None:
    payload = build_db_exec_payload(
        sql="update items set status = :status where id = :id",
        params={"status": "open", "id": 1},
    )
    assert payload["allow_write"] is True
    assert payload["max_rows"] is None
    assert payload["result_format"] == "json"


def test_build_db_exec_payload_invalid_format() -> None:
    with pytest.raises(MoltInvalidInput):
        build_db_exec_payload(
            sql="update items set status = 'x'", result_format="arrow_ipc"
        )
