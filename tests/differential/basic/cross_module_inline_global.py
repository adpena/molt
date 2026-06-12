"""Purpose: cross-module method-inline must not mis-resolve callee globals.

Regression for the from-import / typed-receiver devirtualization bug where a
trivially-inlinable method (e.g. ``array.tolist`` -> ``_MOLT_ARRAY_TOLIST(...)``)
was spliced into the *caller's* module scope, causing its bare reference to a
*callee-module* global to resolve against the caller's globals -> ``NameError``
at runtime. The fix refuses to inline a body that reads a defining-module global
across a module boundary (the call falls through to a real CALL, which reads the
callee's globals correctly). Same-module inlines are unaffected.

Both spellings are exercised: the ``from <mod> import Name`` form (which binds a
typed local and is the form that triggered the original miscompile) and the
``import <mod> as alias`` full-import form. CPython prints identical output for
both; molt must match byte-for-byte.
"""
# ruff: noqa: E402

import os
import sys

sys.path.insert(0, os.path.dirname(__file__))

from cross_module_inline_global_mod import Greeter, Lookup, Widget, scale

# from-import spelling: typed locals + inlinable-method calls reading callee
# globals (constant, constant dict, __all__-adjacent helper).
w = Widget(3)
print("from_scaled", w.scaled())

x = Lookup("b")
print("from_lookup", x.get())

g = Greeter("world")
print("from_greet", g.greet())

print("from_scale_fn", scale(5))

# full-import spelling: same calls through the module alias.
import cross_module_inline_global_mod as m

w2 = m.Widget(7)
print("full_scaled", w2.scaled())

x2 = m.Lookup("c")
print("full_lookup", x2.get())

g2 = m.Greeter("there")
print("full_greet", g2.greet())

print("full_scale_fn", m.scale(9))
