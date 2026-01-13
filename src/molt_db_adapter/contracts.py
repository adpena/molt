from __future__ import annotations

from dataclasses import dataclass
from typing import Any, Mapping, Sequence

from molt_accel.contracts import build_list_items_payload
from molt_accel.errors import MoltInvalidInput


_ALLOWED_FORMATS = {"arrow_ipc", "json", "msgpack"}


@dataclass(frozen=True)
class DbParam:
    value: Any
    type: str | None = None


@dataclass(frozen=True)
class DbParamsSpec:
    mode: str
    values: list[Any]


@dataclass(frozen=True)
class DbQueryPayload:
    db_alias: str | None
    sql: str
    params: DbParamsSpec
    max_rows: int | None
    result_format: str
    allow_write: bool
    tag: str | None


def _normalize_param_type(param_type: Any) -> str:
    if param_type is None:
        raise MoltInvalidInput("Null params must include a type")
    if not isinstance(param_type, str) or not param_type.strip():
        raise MoltInvalidInput("Param type must be a non-empty string")
    return param_type


def _normalize_param(value: Any) -> Any:
    param_type: str | None = None
    if isinstance(value, DbParam):
        param_type = value.type
        value = value.value
    if value is None:
        param_type = _normalize_param_type(param_type)
        return {"value": None, "type": param_type}
    if isinstance(value, bytearray):
        value = bytes(value)
    if isinstance(value, (bool, int, float, str, bytes)):
        if param_type is None:
            return value
        return {"value": value, "type": _normalize_param_type(param_type)}
    raise MoltInvalidInput(f"Unsupported param type: {type(value).__name__}")


def _normalize_params(
    params: Sequence[Any] | Mapping[str, Any] | None,
) -> DbParamsSpec:
    if params is None:
        return DbParamsSpec(mode="positional", values=[])
    if isinstance(params, Mapping):
        items = []
        keys: list[str] = []
        for key in params.keys():
            if not isinstance(key, str):
                raise MoltInvalidInput("Named params must use string keys")
            keys.append(key)
        keys.sort()
        for key in keys:
            normalized = _normalize_param(params[key])
            if isinstance(normalized, dict):
                items.append({"name": key, **normalized})
            else:
                items.append({"name": key, "value": normalized})
        return DbParamsSpec(mode="named", values=items)
    if isinstance(params, (str, bytes, bytearray)):
        raise MoltInvalidInput("Params must be a sequence or mapping")
    if isinstance(params, Sequence):
        values = []
        for value in params:
            normalized = _normalize_param(value)
            values.append(normalized)
        return DbParamsSpec(
            mode="positional",
            values=values,
        )
    raise MoltInvalidInput("Params must be a sequence or mapping")


def build_db_query_payload(
    *,
    db_alias: str | None = "default",
    sql: str,
    params: Sequence[Any] | Mapping[str, Any] | None = None,
    max_rows: int | None = 1000,
    result_format: str = "json",
    allow_write: bool = False,
    tag: str | None = None,
) -> dict[str, Any]:
    if not sql or not sql.strip():
        raise MoltInvalidInput("SQL must be a non-empty string")
    if result_format not in _ALLOWED_FORMATS:
        raise MoltInvalidInput(f"Unsupported result_format '{result_format}'")
    if max_rows is not None and max_rows <= 0:
        raise MoltInvalidInput("max_rows must be positive or None")
    if db_alias is not None and not db_alias.strip():
        raise MoltInvalidInput("db_alias must be a non-empty string")
    normalized_params = _normalize_params(params)
    payload = DbQueryPayload(
        db_alias=db_alias,
        sql=sql,
        params=normalized_params,
        max_rows=max_rows,
        result_format=result_format,
        allow_write=allow_write,
        tag=tag,
    )
    return {
        "db_alias": payload.db_alias,
        "sql": payload.sql,
        "params": {"mode": payload.params.mode, "values": payload.params.values},
        "max_rows": payload.max_rows,
        "result_format": payload.result_format,
        "allow_write": payload.allow_write,
        "tag": payload.tag,
    }


__all__ = [
    "DbParam",
    "DbParamsSpec",
    "DbQueryPayload",
    "build_db_query_payload",
    "build_list_items_payload",
]
