"""Purpose: rebinding a local drops only that binding, not live aliases.

CPython's STORE_FAST overwrites the target local after evaluating the RHS and
decrefs the old target binding. If another local still references the old
object, the finalizer must not run until that alias is deleted.
"""

try:
    import gc
except Exception as exc:
    print(type(exc).__name__, exc)
else:
    seen = []

    class Item:
        def __init__(self, tag: int) -> None:
            self.tag = tag

        def __del__(self) -> None:
            seen.append(self.tag)

    def run() -> None:
        first = Item(1)
        alias = first
        first = Item(2)
        gc.collect()
        print("after_rebind", sorted(seen), alias.tag)
        del alias
        del first
        gc.collect()
        print("after_delete", sorted(seen))

    def run_scope_exit() -> None:
        seen.clear()

        def inner() -> None:
            first = Item(3)
            alias = first
            gc.collect()
            print("scope_inside", sorted(seen), alias.tag)

        inner()
        gc.collect()
        print("scope_after", sorted(seen))

    run()
    run_scope_exit()
