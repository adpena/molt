"""Purpose: differential coverage for warnings filter category."""

import warnings


class CustomWarning(UserWarning):
    pass


with warnings.catch_warnings(record=True) as rec:
    warnings.simplefilter("always")
    warnings.filterwarnings("ignore", category=CustomWarning)
    warnings.warn("skip", CustomWarning)
    warnings.warn("keep", UserWarning)
    print(len(rec))
    print(type(rec[0].message).__name__)
