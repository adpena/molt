"""Purpose: CPython 3.12+ builtins semantic probe for symbol `vars`."""
import builtins
import contextlib
import hashlib
import inspect
import io
import json
import math
import sys
import tempfile
from pathlib import Path


NAME = 'vars'
SYMBOL = getattr(builtins, NAME)


def _safe_signature(value: object) -> str | None:
    if not callable(value):
        return None
    try:
        return str(inspect.signature(value))
    except Exception:  # noqa: BLE001
        return None


def _normalize(value: object, depth: int = 0) -> object:
    if depth > 2:
        return {"type": type(value).__name__}
    if value is None or isinstance(value, (bool, int, str)):
        return value
    if isinstance(value, float):
        if math.isnan(value):
            return "nan"
        if math.isinf(value):
            return "inf" if value > 0 else "-inf"
        if value == 0.0 and math.copysign(1.0, value) < 0:
            return "-0.0"
        return value
    if isinstance(value, complex):
        return {"real": value.real, "imag": value.imag}
    if isinstance(value, bytes):
        return {"type": "bytes", "len": len(value), "hex": value[:24].hex()}
    if isinstance(value, bytearray):
        return {"type": "bytearray", "len": len(value), "hex": bytes(value[:24]).hex()}
    if isinstance(value, dict):
        rows = []
        for key in sorted(value.keys(), key=lambda item: repr(item)):
            rows.append([_normalize(key, depth + 1), _normalize(value[key], depth + 1)])
        return rows
    if isinstance(value, (list, tuple)):
        return [_normalize(item, depth + 1) for item in value[:10]]
    if isinstance(value, (set, frozenset)):
        normalized = [_normalize(item, depth + 1) for item in value]
        return sorted(normalized, key=lambda item: json.dumps(item, sort_keys=True, default=str))
    if isinstance(value, BaseException):
        return {"type": type(value).__name__, "args": _normalize(value.args, depth + 1)}
    return {"type": type(value).__name__}


cases: list[dict[str, object]] = []


def _record(label: str, fn) -> None:  # type: ignore[no-untyped-def]
    try:
        result = fn()
        cases.append({"label": label, "status": "ok", "result": _normalize(result)})
    except BaseException as exc:  # noqa: BLE001
        cases.append(
            {
                "label": label,
                "status": "error",
                "error_type": type(exc).__name__,
                "error_text": str(exc),
            }
        )


def _complex_workflow() -> dict[str, object]:
    orders = [
        {"region": "us-east", "hours": 3, "rate": 120.0},
        {"region": "us-west", "hours": 2, "rate": 140.0},
        {"region": "us-east", "hours": 4, "rate": 110.0},
        {"region": "eu", "hours": 5, "rate": 130.0},
    ]
    totals: dict[str, float] = {}
    for order in orders:
        region = str(order["region"])
        amount = float(order["hours"]) * float(order["rate"])
        totals[region] = totals.get(region, 0.0) + amount

    scratch = Path(tempfile.gettempdir()) / f"molt_builtin_{NAME}.tmp"
    note = io.StringIO()
    note.write(scratch.name)
    with contextlib.nullcontext():
        workflow_note = note.getvalue()

    ranked = sorted((key, round(val, 2)) for key, val in totals.items())
    probes = [orders[0]["hours"], -3, 0, "7", [1, 2, 3], None]
    transformed: list[object] = []
    if callable(SYMBOL):
        for value in probes:
            try:
                transformed.append(_normalize(SYMBOL(value)))
            except BaseException as exc:  # noqa: BLE001
                transformed.append({"error": type(exc).__name__})
    else:
        transformed.append(_normalize(SYMBOL))

    payload = json.dumps(
        {"ranked": ranked, "transformed": transformed, "note": workflow_note},
        sort_keys=True,
        ensure_ascii=True,
    )
    return {
        "ranked": ranked,
        "transformed": transformed,
        "workflow_note": workflow_note,
        "digest": hashlib.sha256(payload.encode("utf-8")).hexdigest()[:16],
    }


def _edge_matrix() -> list[dict[str, object]]:
    vectors: list[tuple[object, ...]] = [
        (),
        (0,),
        (-1,),
        ("9",),
        ([1, 2],),
        ({"k": 1},),
        (None,),
        (float("nan"),),
    ]
    out: list[dict[str, object]] = []
    if not callable(SYMBOL):
        return [{"status": "non_callable", "value": _normalize(SYMBOL)}]
    for args in vectors:
        try:
            result = SYMBOL(*args)
            out.append({"args": _normalize(args), "status": "ok", "result": _normalize(result)})
        except BaseException as exc:  # noqa: BLE001
            out.append(
                {
                    "args": _normalize(args),
                    "status": "error",
                    "error_type": type(exc).__name__,
                }
            )
    return out


def _category_specific() -> object:
    if isinstance(SYMBOL, type) and issubclass(SYMBOL, BaseException):
        instance = SYMBOL("workflow-failure")
        chain = None
        try:
            try:
                raise ValueError("inner")
            except ValueError as exc:
                raise SYMBOL("outer") from exc
        except BaseException as raised:  # noqa: BLE001
            chain = {
                "raised": type(raised).__name__,
                "cause": type(raised.__cause__).__name__ if raised.__cause__ is not None else None,
            }
        return {
            "kind": "exception",
            "args": _normalize(getattr(instance, "args", ())),
            "chain": chain,
        }
    if isinstance(SYMBOL, type):
        created = None
        try:
            created = SYMBOL()
        except BaseException:
            created = None
        return {
            "kind": "type",
            "created_type": type(created).__name__ if created is not None else None,
            "mro_head": [cls.__name__ for cls in SYMBOL.__mro__[:3]],
        }
    if callable(SYMBOL):
        return {"kind": "callable", "signature": _safe_signature(SYMBOL)}
    return {"kind": "value", "value": _normalize(SYMBOL)}


def _exec_class_body_case() -> object:
    namespace: dict[str, object] = {}
    SYMBOL(
        "class Probe:\n"
        "    label = 'daily'\n"
        "    score = 42\n",
        namespace,
        namespace,
    )
    probe = namespace["Probe"]
    return {"label": getattr(probe, "label"), "score": getattr(probe, "score")}


def _exit_case() -> object:
    try:
        SYMBOL(7)
    except SystemExit as exc:
        return {"code": exc.code, "type": type(exc).__name__}
    return {"code": None}


_record(
    "metadata",
    lambda: {
        "name": NAME,
        "type": type(SYMBOL).__name__,
        "callable": bool(callable(SYMBOL)),
        "module": getattr(SYMBOL, "__module__", None),
        "signature": _safe_signature(SYMBOL),
        "py_major": sys.version_info[0],
        "py_minor": sys.version_info[1],
    },
)
_record("complex_workflow", _complex_workflow)
_record("edge_matrix", _edge_matrix)
_record("category_specific", _category_specific)

summary_payload = json.dumps(cases, sort_keys=True, ensure_ascii=True)
summary = {
    "name": NAME,
    "case_count": len(cases),
    "digest": hashlib.sha256(summary_payload.encode("utf-8")).hexdigest()[:20],
}
print(json.dumps(summary, sort_keys=True, ensure_ascii=True))
for row in cases:
    print(json.dumps(row, sort_keys=True, ensure_ascii=True))
