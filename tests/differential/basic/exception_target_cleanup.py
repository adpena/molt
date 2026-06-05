"""Purpose: `except ... as e` implicitly deletes its target at handler exit
(CPython unconditionally `del`s the binding even when the handler escapes), so a
later read of the name is an unbound *name* lookup.  At module scope a bare Name
read has LOAD_GLOBAL semantics, so it must raise NameError ("name 'e' is not
defined") — NOT the AttributeError that a module-attribute access would yield.
The same rule applies to an explicit `del` of a module global.  This regression
pins the module-scope flavour (NameError) across the except-as and del triggers,
plus the conditional-delete and re-bind-after-delete cases where the live module
dict still holds the value.
"""

# 1. except-as read after the handler block -> NameError.
try:
    raise ValueError("boom")
except Exception as err:
    print("inside", type(err).__name__, str(err))
try:
    err
except NameError as ne:
    print("after-except", type(ne).__name__, str(ne))

# 2. except-as where the handler re-raises a new exception (target still deleted).
try:
    try:
        raise KeyError("k")
    except KeyError as exc:
        raise RuntimeError("rethrown") from exc
except RuntimeError:
    pass
try:
    exc
except NameError as ne:
    print("after-rethrow", type(ne).__name__, str(ne))

# 3. plain `del` of a module global -> NameError on read.
value = 10
del value
try:
    value
except NameError as ne:
    print("after-del", type(ne).__name__, str(ne))

# 4. nested except reusing the same name; both bindings are deleted at exit.
try:
    raise ValueError("outer")
except Exception as g:
    try:
        raise TypeError("inner")
    except Exception as g:
        print("nested-inner", str(g))
try:
    g
except NameError as ne:
    print("after-nested", type(ne).__name__, str(ne))

# 5. delete on a (runtime-dead) conditional branch: the live dict still has the
#    value, so the bare read returns it rather than raising.
keep = 99
if False:
    del keep
print("conditional-keep", keep)

# 6. re-bind after delete: the read sees the new binding.
rebound = 1
del rebound
rebound = 2
print("rebound", rebound)
