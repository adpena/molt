"""Purpose: validate test.support.captured_output captures stdout/stderr."""

from molt.stdlib.test import support

with support.captured_output("stdout") as out, support.captured_output("stderr") as err:
    print("hello")
    print("oops", file=err)

print(out.getvalue().strip())
print(err.getvalue().strip())
