"""Purpose: differential coverage for warnings warn explicit registry."""

import warnings


reg = {}
warnings.warn_explicit("hi", UserWarning, "file.py", 10, registry=reg)
warnings.warn_explicit("hi", UserWarning, "file.py", 10, registry=reg)
print(len(reg))
