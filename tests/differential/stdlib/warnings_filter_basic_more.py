"""Purpose: differential coverage for warnings filters."""

import warnings

warnings.simplefilter("always")
warnings.warn("hello", RuntimeWarning)
print(True)
