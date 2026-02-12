"""Purpose: differential coverage for warnings catch reentrancy."""

import warnings


with warnings.catch_warnings(record=True) as rec1:
    warnings.simplefilter("always")
    warnings.warn("outer")
    with warnings.catch_warnings(record=True) as rec2:
        warnings.simplefilter("always")
        warnings.warn("inner")
    print(len(rec2), rec2[0].message)

print(len(rec1), rec1[0].message)
