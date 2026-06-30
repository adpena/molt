"""Canonical native callable ABI tokens shared by package admission and lowering."""

from __future__ import annotations

from typing import Any, TypedDict

NATIVE_CALLABLE_ABI_OBJECT_CALL_V1 = "molt.object_call_v1"
NATIVE_CALLABLE_ABI_OBJECT_CALLARGS_V1 = "molt.object_callargs_v1"
NATIVE_CALLABLE_ABI_FORWARD_F32_V1 = "molt.forward_f32_v1"

NATIVE_CALLABLE_ABIS: tuple[str, ...] = (
    NATIVE_CALLABLE_ABI_OBJECT_CALL_V1,
    NATIVE_CALLABLE_ABI_OBJECT_CALLARGS_V1,
    NATIVE_CALLABLE_ABI_FORWARD_F32_V1,
)
KNOWN_NATIVE_CALLABLE_ABIS: frozenset[str] = frozenset(NATIVE_CALLABLE_ABIS)
NATIVE_CALLABLE_ABI_CHOICES = ", ".join(NATIVE_CALLABLE_ABIS)

class _NativeCallableBrowserSignature(TypedDict):
    params: list[str]
    result: str


_NATIVE_CALLABLE_BROWSER_SIGNATURES: dict[
    str, _NativeCallableBrowserSignature
] = {
    NATIVE_CALLABLE_ABI_OBJECT_CALL_V1: {
        "params": ["molt.value..."],
        "result": "molt.value",
    },
    NATIVE_CALLABLE_ABI_OBJECT_CALLARGS_V1: {
        "params": ["molt.callargs"],
        "result": "molt.value",
    },
    NATIVE_CALLABLE_ABI_FORWARD_F32_V1: {
        "params": ["bytes.float32"],
        "result": "bytes.float32",
    },
}

_NATIVE_CALLABLE_FIXED_ARITY: dict[str, int | None] = {
    NATIVE_CALLABLE_ABI_OBJECT_CALL_V1: None,
    NATIVE_CALLABLE_ABI_OBJECT_CALLARGS_V1: 1,
    NATIVE_CALLABLE_ABI_FORWARD_F32_V1: 1,
}


def normalize_native_callable_abi(value: Any) -> str | None:
    if not isinstance(value, str):
        return None
    abi = value.strip()
    if abi not in KNOWN_NATIVE_CALLABLE_ABIS:
        return None
    return abi


def native_callable_abi_choices() -> str:
    return NATIVE_CALLABLE_ABI_CHOICES


def native_callable_browser_signature(abi: str) -> dict[str, object]:
    signature = _NATIVE_CALLABLE_BROWSER_SIGNATURES[abi]
    return {
        "params": list(signature["params"]),
        "result": signature["result"],
    }


def native_callable_fixed_arity(abi: str) -> int | None:
    return _NATIVE_CALLABLE_FIXED_ARITY[abi]


def native_callable_uses_callargs(abi: str) -> bool:
    return abi == NATIVE_CALLABLE_ABI_OBJECT_CALLARGS_V1
