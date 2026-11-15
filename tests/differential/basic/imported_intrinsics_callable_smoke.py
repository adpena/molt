"""Purpose: imported intrinsic resolver aliases must remain callable.

`_intrinsics` is Molt's private bootstrap module. A compiled program binds its
resolver to the runtime's native builtin (call argument binding contract,
section 4); under CPython the `src/_intrinsics.py` shim supplies a Python
function. The resolver's type is not a CPython fact, so this test checks the
contract both share: every alias form is a callable value, and calling it with
an unknown intrinsic raises RuntimeError.
"""

from _intrinsics import require_intrinsic
from _intrinsics import require_intrinsic as ri


def resolve_through_default(name, _resolver=require_intrinsic):
    return _resolver(name)


alias = ri
unknown = "".join(["molt_", "no_such_intrinsic"])

for resolver in (require_intrinsic, ri, alias):
    print(callable(resolver))

for call in (
    lambda: alias(unknown),
    lambda: resolve_through_default(unknown),
):
    try:
        call()
    except RuntimeError:
        print("RuntimeError")
    else:
        print("resolved")
