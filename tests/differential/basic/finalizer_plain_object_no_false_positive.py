# MOLT_META: stderr=exact
"""Purpose: non-finalizer objects must not enter the __del__ dispatch path."""

try:
    import gc
except Exception as exc:
    print(type(exc).__name__, exc)
else:

    class Plain:
        pass

    class InstanceOnly:
        pass

    def run_plain() -> None:
        obj = Plain()
        obj.tag = "plain"
        del obj
        gc.collect()

    def run_instance_only() -> None:
        obj = InstanceOnly()
        obj.__del__ = "not a type finalizer"
        del obj
        gc.collect()

    run_plain()
    run_instance_only()
    print("plain-finalizer-clean")
