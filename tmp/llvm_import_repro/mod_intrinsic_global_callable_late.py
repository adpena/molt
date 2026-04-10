from _intrinsics import require_intrinsic as _require_intrinsic

g = _require_intrinsic("molt_sys_modules")


def f():
    return g()
