# MOLT_META: min_py=3.14
"""Purpose: stdlib import smoke for 3.14-only modules."""

import annotationlib
import compression

modules = [
    annotationlib,
    compression,
]
print([mod.__name__ for mod in modules])
