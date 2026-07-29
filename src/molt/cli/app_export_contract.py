"""Canonical browser/WASM app-callable export contract.

The frontend owns Python binding semantics.  The linker must not rediscover
public functions from mangled WASM export spellings: those spellings lose
module-binding history (decorators, imports, redefinitions, and rebinds).  This
module projects the resolved entry-module bindings plus the exact frontend IR
symbols into one digest-bound contract consumed by linking and packaging.
"""

from __future__ import annotations

import hashlib
import json
from collections.abc import Mapping, Sequence
from pathlib import Path

from molt._wasm_abi_generated import WASM_OUTPUT_EXPORT_ALIAS_PREFIX


APP_EXPORT_CONTRACT_SCHEMA = 2
APP_EXPORT_CALL_ABI_SCHEMA = 1


def _canonical_call_abi() -> dict[str, object]:
    """Return the one browser/WASM app-call boundary ABI.

    Ordinary compiled functions may return a borrowed parameter.  A host call
    outlives the borrowed argument objects that the host releases immediately
    after the call, so the public adapter promotes the tagged result to one
    owned reference.  Immediate tagged values make the retain operation a
    no-op; heap results are released exactly once by the host after decoding.
    """

    return {
        "schema": APP_EXPORT_CALL_ABI_SCHEMA,
        "name": "molt.wasm.app-call",
        "parameters": {
            "representation": "tagged-i64",
            "ownership": "borrowed",
        },
        "result": {
            "representation": "tagged-i64",
            "ownership": "owned",
        },
        "adapter": {
            "strategy": "retain-result",
            "symbol_prefix": WASM_OUTPUT_EXPORT_ALIAS_PREFIX,
            "retain_import": {
                "module": "molt_runtime",
                "name": "molt_inc_ref_obj",
                "parameters": ["i64"],
                "results": [],
            },
        },
    }


def _validated_call_abi(raw_abi: object) -> dict[str, object]:
    expected = _canonical_call_abi()
    if raw_abi != expected:
        raise ValueError(
            "app export contract call_abi must match the canonical "
            f"{expected['name']} schema {APP_EXPORT_CALL_ABI_SCHEMA} boundary"
        )
    assert isinstance(raw_abi, Mapping)
    return dict(raw_abi)


def _frontend_resolved_bindings(
    ir: Mapping[str, object], *, entry_module: str
) -> list[dict[str, object]]:
    raw_functions = ir.get("functions")
    if not isinstance(raw_functions, Sequence) or isinstance(
        raw_functions, (str, bytes, bytearray)
    ):
        raise ValueError("backend IR has no functions list for app export custody")
    carriers: list[object] = []
    for index, raw_function in enumerate(raw_functions):
        if not isinstance(raw_function, Mapping):
            raise ValueError(f"backend IR function {index} is not an object")
        if "app_callable_bindings" in raw_function:
            carriers.append(raw_function["app_callable_bindings"])
    if len(carriers) != 1:
        raise ValueError(
            "backend IR must contain exactly one frontend-resolved app callable "
            f"binding table, found {len(carriers)}"
        )
    raw_bindings = carriers[0]
    if not isinstance(raw_bindings, list):
        raise ValueError("frontend app callable bindings must be a list")
    bindings = _validated_binding_rows(raw_bindings, entry_module=entry_module)
    names = [binding["name"] for binding in bindings]
    if names != sorted(names):
        raise ValueError("frontend app callable bindings must be sorted by name")
    return bindings


def _validated_binding_rows(
    raw_bindings: Sequence[object], *, entry_module: str
) -> list[dict[str, object]]:
    bindings: list[dict[str, object]] = []
    seen_names: set[str] = set()
    seen_export_symbols: set[str] = set()
    for index, raw_binding in enumerate(raw_bindings):
        if not isinstance(raw_binding, Mapping):
            raise ValueError(f"app export binding {index} must be an object")
        binding = dict(raw_binding)
        name = binding.get("name")
        qualified_name = binding.get("qualified_name")
        disposition = binding.get("disposition")
        symbol = binding.get("symbol")
        reason = binding.get("reason")
        kind = binding.get("kind")
        origin = binding.get("origin")
        if not isinstance(name, str) or not name or name in seen_names:
            raise ValueError(f"app export binding {index} has invalid/duplicate name")
        seen_names.add(name)
        if qualified_name != f"{entry_module}.{name}":
            raise ValueError(f"app export binding {name!r} has invalid qualified_name")
        if kind not in {"sync", "async", "gen", "asyncgen"}:
            raise ValueError(f"app export binding {name!r} has invalid function kind")
        if not isinstance(origin, str) or not origin:
            raise ValueError(f"app export binding {name!r} has invalid origin")
        if disposition not in {"export", "excluded"}:
            raise ValueError(f"app export binding {name!r} has invalid disposition")
        if symbol is not None and (not isinstance(symbol, str) or not symbol):
            raise ValueError(f"app export binding {name!r} has invalid symbol")
        if disposition == "export":
            if not isinstance(symbol, str) or symbol in seen_export_symbols:
                raise ValueError(f"exported app binding {name!r} has invalid symbol")
            if reason is not None:
                raise ValueError(f"exported app binding {name!r} cannot have a reason")
            seen_export_symbols.add(symbol)
        elif not isinstance(reason, str) or not reason:
            raise ValueError(f"excluded app binding {name!r} requires a reason")
        superseded = binding.get("superseded_symbols")
        if not isinstance(superseded, list) or not all(
            isinstance(item, str) and item for item in superseded
        ):
            raise ValueError(
                f"app export binding {name!r} superseded_symbols must be strings"
            )
        if len(superseded) != len(set(superseded)) or symbol in superseded:
            raise ValueError(
                f"app export binding {name!r} has duplicate/current superseded symbol"
            )
        imported_from = binding.get("imported_from")
        if imported_from is not None and (
            not isinstance(imported_from, str) or not imported_from
        ):
            raise ValueError(f"app export binding {name!r} has invalid imported_from")
        bindings.append(binding)
    return bindings


def _contract_digest(payload: Mapping[str, object]) -> str:
    canonical = json.dumps(
        payload,
        sort_keys=True,
        separators=(",", ":"),
        ensure_ascii=True,
    ).encode("utf-8")
    return hashlib.sha256(canonical).hexdigest()


def build_app_export_contract(
    *,
    entry_module: str,
    ir: Mapping[str, object],
    registry_digest: str,
) -> dict[str, object]:
    if not entry_module:
        raise ValueError("app export contract requires an entry module")
    if len(registry_digest) != 64 or any(
        ch not in "0123456789abcdef" for ch in registry_digest
    ):
        raise ValueError(
            "app export contract requires a lowercase SHA-256 registry digest"
        )

    entries = _frontend_resolved_bindings(ir, entry_module=entry_module)

    payload: dict[str, object] = {
        "schema": APP_EXPORT_CONTRACT_SCHEMA,
        "registry_digest": registry_digest,
        "entry_module": entry_module,
        "call_abi": _canonical_call_abi(),
        "bindings": entries,
    }
    payload["contract_digest"] = _contract_digest(payload)
    return payload


def validate_app_export_contract(payload: Mapping[str, object]) -> dict[str, object]:
    if payload.get("schema") != APP_EXPORT_CONTRACT_SCHEMA:
        raise ValueError(
            f"app export contract schema must be {APP_EXPORT_CONTRACT_SCHEMA}"
        )
    registry_digest = payload.get("registry_digest")
    if not isinstance(registry_digest, str) or len(registry_digest) != 64:
        raise ValueError("app export contract registry_digest must be SHA-256")
    entry_module = payload.get("entry_module")
    if not isinstance(entry_module, str) or not entry_module:
        raise ValueError("app export contract entry_module must be non-empty")
    _validated_call_abi(payload.get("call_abi"))
    raw_bindings = payload.get("bindings")
    if not isinstance(raw_bindings, list):
        raise ValueError("app export contract bindings must be a list")
    _validated_binding_rows(raw_bindings, entry_module=entry_module)
    supplied_digest = payload.get("contract_digest")
    unsigned = dict(payload)
    unsigned.pop("contract_digest", None)
    expected_digest = _contract_digest(unsigned)
    if supplied_digest != expected_digest:
        raise ValueError(
            "app export contract digest mismatch: "
            f"payload has {supplied_digest!r}, expected {expected_digest!r}"
        )
    return dict(payload)


def load_app_export_contract(path: Path) -> dict[str, object]:
    try:
        payload = json.loads(path.read_text(encoding="utf-8"))
    except (OSError, json.JSONDecodeError) as exc:
        raise ValueError(f"cannot read app export contract {path}: {exc}") from exc
    if not isinstance(payload, Mapping):
        raise ValueError(f"app export contract {path} must contain an object")
    return validate_app_export_contract(payload)


def exported_app_symbols(contract: Mapping[str, object]) -> tuple[str, ...]:
    validated = validate_app_export_contract(contract)
    bindings = validated["bindings"]
    assert isinstance(bindings, list)
    return tuple(
        binding["symbol"]
        for binding in bindings
        if isinstance(binding, Mapping)
        and binding.get("disposition") == "export"
        and isinstance(binding.get("symbol"), str)
    )


def app_export_call_abi(contract: Mapping[str, object]) -> dict[str, object]:
    validated = validate_app_export_contract(contract)
    return _validated_call_abi(validated["call_abi"])


def excluded_app_symbols(contract: Mapping[str, object]) -> tuple[str, ...]:
    validated = validate_app_export_contract(contract)
    exported = set(exported_app_symbols(validated))
    bindings = validated["bindings"]
    assert isinstance(bindings, list)
    excluded: list[str] = []
    for binding in bindings:
        if not isinstance(binding, Mapping):
            continue
        if binding.get("disposition") == "excluded":
            symbol = binding.get("symbol")
            if isinstance(symbol, str):
                excluded.append(symbol)
        superseded = binding.get("superseded_symbols")
        if isinstance(superseded, list):
            excluded.extend(item for item in superseded if isinstance(item, str))
    return tuple(symbol for symbol in dict.fromkeys(excluded) if symbol not in exported)
