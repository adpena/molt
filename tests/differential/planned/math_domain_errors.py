"""Purpose: differential coverage for math domain errors."""

import math

for fn in (math.sqrt, math.log):
    try:
        fn(-1)
    except Exception as exc:
        print(type(exc).__name__)
