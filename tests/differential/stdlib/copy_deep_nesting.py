"""CPython 3.12 deepcopy charges nesting to the recursion limit.

Each nested _deepcopy_list/_dict/_tuple frame counts against the limit, so deep
nesting raises RecursionError. Molt's native exact-container walk must charge
the same limit instead of overflowing the native stack. Native cells only: on
WASM the host stack runs out before the limit (HF-442). Depth 2,000 stays below
the separate deallocation-depth limit (HF-660).
"""
import copy


def nest(depth, wrap):
    value = []
    for _ in range(depth):
        value = wrap(value)
    return value


for name, wrap in (("list", lambda inner: [inner]),
                   ("dict", lambda inner: {"k": inner}),
                   ("tuple", lambda inner: (inner, []))):
    for depth in (50, 2_000):
        nested = nest(depth, wrap)
        try:
            print("nested", name, depth, copy.deepcopy(nested) is not nested)
        except RecursionError:
            print("nested", name, depth, "RecursionError")
        del nested
