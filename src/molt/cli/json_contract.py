from __future__ import annotations

from pathlib import Path
from typing import Any, Mapping


def _coerce_json_path(value: Any) -> Path | None:
    if not isinstance(value, str) or not value:
        return None
    return Path(value)


def _extract_json_errors(payload: Mapping[str, Any] | None) -> list[str]:
    if payload is None:
        return []
    raw_errors = payload.get("errors")
    if not isinstance(raw_errors, list):
        return []
    errors: list[str] = []
    for item in raw_errors:
        if isinstance(item, str) and item:
            errors.append(item)
    return errors


def _extract_json_warnings(payload: Mapping[str, Any] | None) -> list[str]:
    if payload is None:
        return []
    raw_warnings = payload.get("warnings")
    if not isinstance(raw_warnings, list):
        return []
    warnings: list[str] = []
    for item in raw_warnings:
        if isinstance(item, str) and item:
            warnings.append(item)
    return warnings


def _wrapper_build_payload_data(payload: Mapping[str, Any] | None) -> Mapping[str, Any]:
    if payload is None:
        return {}
    raw_data = payload.get("data")
    if not isinstance(raw_data, dict):
        return {}
    return raw_data


def _extract_payload_text_list(value: Any) -> list[str]:
    if not isinstance(value, list):
        return []
    items: list[str] = []
    for item in value:
        if isinstance(item, str) and item:
            items.append(item)
    return items
