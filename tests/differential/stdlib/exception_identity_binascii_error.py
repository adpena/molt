"""Purpose: runtime decode failures raise the owning module's binascii.Error.

Each failing decode must raise the class bound by ``binascii`` (a ValueError
subclass), so ``except binascii.Error`` and ``except ValueError`` both catch it.
"""

import base64
import binascii


def probe(label, func, *args, **kwargs):
    try:
        func(*args, **kwargs)
    except binascii.Error as exc:
        print(
            label,
            "binascii.Error",
            type(exc) is binascii.Error,
            isinstance(exc, ValueError),
            type(exc).__module__,
        )
    except ValueError as exc:
        print(label, "ValueError only", type(exc).__name__)
    except Exception as exc:  # noqa: BLE001 - the observation is the escape
        print(label, "escaped", type(exc).__name__)
    else:
        print(label, "no error")


def main():
    probe("b64decode validate", base64.b64decode, "ab!c", validate=True)
    probe("b64decode padding", base64.b64decode, "abc")
    probe("b32decode", base64.b32decode, "A")
    probe("b16decode", base64.b16decode, "0")
    probe("a2b_base64", binascii.a2b_base64, b"a")
    probe("unhexlify odd", binascii.unhexlify, b"abc")
    probe("unhexlify digit", binascii.unhexlify, b"zz")
    probe("a2b_hex", binascii.a2b_hex, b"0")
    print("Error base", binascii.Error.__mro__[1].__name__)
    print("Error module", binascii.Error.__module__, binascii.Error.__qualname__)


if __name__ == "__main__":
    main()
