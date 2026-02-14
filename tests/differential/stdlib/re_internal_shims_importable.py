"""Purpose: ensure CPython-style internal re modules are importable."""

import re
import re._casefix  # noqa: F401
import re._compiler  # noqa: F401
import re._constants  # noqa: F401
import re._parser  # noqa: F401

print(bool(getattr(re, "compile", None)))
print(bool(getattr(re._compiler, "_compile", None)))
