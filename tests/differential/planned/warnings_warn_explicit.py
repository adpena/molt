"""Purpose: differential coverage for warnings warn explicit."""

import warnings


with warnings.catch_warnings(record=True) as rec:
    warnings.warn_explicit("hi", UserWarning, "file.py", 10)
    print(len(rec))
    print(rec[0].filename, rec[0].lineno)
