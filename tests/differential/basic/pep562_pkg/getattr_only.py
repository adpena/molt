"""PEP 562 corner fixture: a module that defines ``__getattr__`` but NOT
``__dir__``. ``dir(module)`` must fall back to the default module behaviour (its
namespace) WITHOUT consulting ``__getattr__`` for the name ``"__dir__"`` — i.e.
the ``__dir__`` lookup goes straight to the module ``__dict__``, never through
the module-level ``__getattr__`` hook.
"""

_dir_probe_calls = 0
present = 1


def __getattr__(name):
    # Records every miss so the test can prove dir() never routed a "__dir__"
    # lookup through here.
    global _dir_probe_calls
    _dir_probe_calls += 1
    raise AttributeError(f"module 'pep562_pkg.getattr_only' has no attribute {name!r}")
