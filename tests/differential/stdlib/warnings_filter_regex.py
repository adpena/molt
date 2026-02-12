"""Purpose: differential coverage for warnings filter regex."""

import warnings


with warnings.catch_warnings(record=True) as rec:
    warnings.simplefilter("always")
    warnings.filterwarnings("ignore", message=r"skip.*")
    warnings.warn("skip this")
    warnings.warn("keep this")
    print(len(rec))
    print(rec[0].message)
