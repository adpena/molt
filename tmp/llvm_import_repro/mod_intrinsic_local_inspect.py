from _intrinsics import require_intrinsic as _require_intrinsic


def f():
    g = _require_intrinsic("molt_future_features")
    return type(g).__name__


value = f()
