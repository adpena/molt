"""Purpose: differential coverage for call_indirect non-callable deopt/type-error lane."""

import types

ns = types.SimpleNamespace()
ns.fn = 7

try:
    ns.fn()
except Exception as exc:
    print(type(exc).__name__)
