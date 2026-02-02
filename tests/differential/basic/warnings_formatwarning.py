"""Purpose: differential coverage for warnings formatwarning."""

import warnings


text = warnings.formatwarning("hi", UserWarning, "file.py", 5)
print("file.py" in text and "UserWarning" in text)
