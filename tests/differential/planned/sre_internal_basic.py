"""Purpose: differential coverage for internal sre helpers."""

import sre_compile
import sre_constants
import sre_parse

print(hasattr(sre_compile, "compile"))
print(hasattr(sre_parse, "parse"))
print(hasattr(sre_constants, "OPCODES"))
