"""Purpose: globals() as a first-class callable."""

g = globals
print(g is globals)
print(g() is globals())
print("__name__" in g())

# Reference acquisition must not manufacture a module-local Python function.
print(type(g).__name__, g.__name__, g.__qualname__, g.__module__)


def captured(fn=globals):
    return fn is g, fn() is globals()


print(captured())


def cross_module():
    import builtins
    import globals_callable_support as support

    print(g is builtins.globals, g is support.alias)
    # The caller's frame, not the module that acquired the callable, owns globals.
    print(support.alias() is globals())
    print(support.invoke(g) is support.__dict__)
    print(support.invoke(support.alias) is support.__dict__)


cross_module()
