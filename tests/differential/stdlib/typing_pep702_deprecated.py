"""Purpose: verify PEP 702 warnings.deprecated decorator (Python 3.13+)."""

import sys
import warnings

if sys.version_info >= (3, 13):
    from warnings import deprecated

    @deprecated("use new_func instead")
    def old_func():
        return 42

    print(hasattr(old_func, "__deprecated__"))
    print(old_func.__deprecated__)

    with warnings.catch_warnings(record=True) as w:
        warnings.simplefilter("always")
        result = old_func()
        print(result)
        print(len(w))
        print(issubclass(w[0].category, DeprecationWarning))
        print("use new_func instead" in str(w[0].message))
else:
    print("pep702_skipped_below_313")
