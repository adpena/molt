# Parity test: stdlib FFI panic-contract under panic=abort.
# All output via print() for diff comparison against CPython.
#
# Under `--build-profile release` the runtime staticlib is compiled with the
# `release-output` profile (panic = "abort"), where catch_unwind is a no-op.
# Each operation below hits a Python-level error condition inside a
# `with_gil_entry!`-wrapped stdlib FFI entry point.  Those entry points
# propagate the error explicitly (pending exception + sentinel), so the errors
# MUST be catchable here rather than aborting the process.  If any op aborted
# instead of raising, this program would terminate abnormally and the captured
# output would diverge from CPython, failing the parity diff.

import base64
import binascii
import decimal
import json
import struct


def expect(exc_types, fn, label):
    try:
        fn()
    except exc_types as e:
        print(f"caught {label}: {type(e).__name__}")
        return
    print(f"FAIL {label}: did not raise")


# decimal: division by zero with the default context traps -> raises.
expect(
    (decimal.DivisionByZero, decimal.InvalidOperation, ArithmeticError),
    lambda: decimal.Decimal(1) / decimal.Decimal(0),
    "decimal_div_by_zero",
)

# decimal: malformed literal -> InvalidOperation.
expect(
    (decimal.InvalidOperation, ValueError, ArithmeticError),
    lambda: decimal.Decimal("not-a-number!!"),
    "decimal_bad_literal",
)

# struct: bad format char -> struct.error.
expect(
    (struct.error, ValueError),
    lambda: struct.pack("Z", 1),
    "struct_bad_format",
)

# struct: unpack buffer size mismatch -> struct.error.
expect(
    (struct.error, ValueError),
    lambda: struct.unpack("i", b"\x00"),
    "struct_short_buffer",
)

# json: invalid document -> ValueError / JSONDecodeError.
expect(
    (ValueError,),
    lambda: json.loads("{not valid json"),
    "json_invalid",
)

# base64: invalid input with validate -> binascii.Error.
expect(
    (binascii.Error, ValueError),
    lambda: base64.b64decode("@@@@notbase64@@@@", validate=True),
    "base64_invalid",
)

print("ALL_CAUGHT")
