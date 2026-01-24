"""Purpose: differential coverage for warnings simplefilter."""

import warnings


with warnings.catch_warnings(record=True) as rec:
    warnings.simplefilter("ignore")
    warnings.warn("skip")
    print(len(rec))

with warnings.catch_warnings(record=True) as rec:
    warnings.simplefilter("always")
    warnings.warn("keep")
    print(len(rec))
