"""Purpose: differential coverage for bisect public API surface."""

import bisect


expected_public = [
    "bisect",
    "bisect_left",
    "bisect_right",
    "insort",
    "insort_left",
    "insort_right",
]
print(hasattr(bisect, "__all__"))
print(all(hasattr(bisect, name) and callable(getattr(bisect, name)) for name in expected_public))
print(bisect.bisect is bisect.bisect_right)
print(bisect.insort is bisect.insort_right)
