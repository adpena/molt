from _intrinsics import require_intrinsic as _require_intrinsic


def f():
    try:
        g = _require_intrinsic("molt_future_features")
    except Exception:
        return [("fallback",)]
    try:
        result = g()
        if result is None:
            return [("fallback",)]
        rows = list(result)
        if not rows:
            return [("fallback",)]
        return rows
    except Exception:
        return [("fallback",)]


rows = f()
