"""Purpose: CPython 3.12+ builtins API semantic probe."""

import builtins
import hashlib
import inspect
import json


def _safe_signature(value: object) -> str | None:
    if not callable(value):
        return None
    try:
        return str(inspect.signature(value))
    except Exception:  # noqa: BLE001
        return None


rows: list[dict[str, object]] = []
for name in sorted(n for n in dir(builtins) if not n.startswith("_")):
    value = getattr(builtins, name)
    rows.append(
        {
            "name": name,
            "type": type(value).__name__,
            "callable": bool(callable(value)),
            "module": getattr(value, "__module__", None),
            "signature": _safe_signature(value),
        }
    )

payload = json.dumps(rows, separators=(",", ":"), sort_keys=True, ensure_ascii=True)
summary = {
    "count": len(rows),
    "digest": hashlib.sha256(payload.encode("utf-8")).hexdigest()[:20],
}
print(json.dumps(summary, sort_keys=True, ensure_ascii=True))
for row in rows:
    print(json.dumps(row, sort_keys=True, ensure_ascii=True))
