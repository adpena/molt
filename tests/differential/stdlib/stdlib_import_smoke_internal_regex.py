"""Purpose: stdlib import smoke for internal regex helpers."""

import sre_compile
import sre_constants
import sre_parse

modules = [
    sre_compile,
    sre_constants,
    sre_parse,
]
print([mod.__name__ for mod in modules])
