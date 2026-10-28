# MOLT_META: backends=native
# Source lines come from the compiled-source registry, which only native entry
# stubs fill; the WASI runner preopens only the guest's output directory.
"""Purpose: a formatted warning shows the source line at its location."""

import io
import os
import warnings


def caller_line():
    warnings.warn("from the caller", UserWarning, stacklevel=2)


def show(text, filename):
    print(text.replace(filename, os.path.basename(filename)), end="")


with warnings.catch_warnings(record=True) as records:
    warnings.simplefilter("always")
    caller_line()
    warnings.warn("at module level", UserWarning)

for record in records:
    text = warnings.formatwarning(
        record.message, record.category, record.filename, record.lineno
    )
    show(text, record.filename)
    stream = io.StringIO()
    warnings.showwarning(
        record.message, record.category, record.filename, record.lineno, file=stream
    )
    show(stream.getvalue(), record.filename)

print(warnings.formatwarning("given", UserWarning, "nowhere.py", 3, "  text  "), end="")
print(warnings.formatwarning("blank", UserWarning, "nowhere.py", 3, "   "), end="")
print(warnings.formatwarning("absent", UserWarning, "nowhere.py", 3), end="")
