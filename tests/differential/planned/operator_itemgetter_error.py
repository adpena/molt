"""Purpose: differential coverage for operator.itemgetter error behavior."""

import operator

getter = operator.itemgetter(0)
try:
    getter(5)
except Exception as exc:
    print(type(exc).__name__)
