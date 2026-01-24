"""Purpose: differential coverage for warnings resetwarnings."""

import warnings


warnings.filterwarnings("ignore", message="skip")
warnings.resetwarnings()

with warnings.catch_warnings(record=True) as rec:
    warnings.warn("skip")
    print(len(rec))
