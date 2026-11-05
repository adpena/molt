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

    # One SSA variable alternates between an immediate and a heap owner. Observe
    # each last-owner release directly, without a collector masking leaks.
    def run_mixed_representation():
        seen.clear()
        value = Item(10)
        for index in range(4):
            alias = value
            value = index if index % 2 == 0 else Item(20 + index)
            print("mixed_held", index, seen)
            del alias
            print("mixed_released", index, seen)
        del value
        print("mixed_finished", seen)

    def annotated_none_identity(value: None):
        alias = value
        return alias

    def run_annotation_and_suspension():
        seen.clear()
        value = Item(30)
        alias = annotated_none_identity(value)
        del value
        print("annotation_held", seen)
        del alias
        print("annotation_released", seen)

        def suspended():
            value = Item(40)
            yield value
            value = None
            yield value

        generator = suspended()
        value = next(generator)
        del value
        print("suspension_held", seen)
        print("suspension_resumed", next(generator), seen)
        generator.close()
        print("suspension_closed", seen)

    run_mixed_representation()
    run_annotation_and_suspension()
