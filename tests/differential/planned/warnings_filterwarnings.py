"""Purpose: differential coverage for warnings filterwarnings."""

import warnings


with warnings.catch_warnings(record=True) as rec:
    warnings.simplefilter("default")
    warnings.filterwarnings("ignore", message="skip")
    warnings.warn("skip")
    warnings.warn("keep")
    print(len(rec))
    print(rec[0].message)
