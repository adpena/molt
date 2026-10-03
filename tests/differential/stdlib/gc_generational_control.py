"""Purpose: CPython parity for generational GC controls and statistics."""

import gc


original = gc.get_threshold()
gc.disable()
try:
    gc.set_threshold(5)
    print("threshold_optional", gc.get_threshold())
    print(
        "count_shape",
        len(gc.get_count()),
        all(isinstance(v, int) for v in gc.get_count()),
    )
    stats = gc.get_stats()
    print("stats_generations", len(stats))
    print(
        "stats_shape",
        all(
            sorted(generation) == ["collected", "collections", "uncollectable"]
            and all(isinstance(value, int) for value in generation.values())
            for generation in stats
        ),
    )
    for generation in (-1, 3):
        try:
            gc.collect(generation)
        except Exception as exc:
            print("invalid_generation", type(exc).__name__)

    # Automatic collections would make these observation points nondeterministic.
    # Mutation never untracks a dictionary; full-GC demotion is version-specific.
    for mode in ("replace", "delete", "pop", "popitem", "clear", "update"):
        mapping = {"edge": []}
        before = gc.is_tracked(mapping)
        if mode == "replace":
            mapping["edge"] = 0
        elif mode == "delete":
            del mapping["edge"]
        elif mode == "pop":
            mapping.pop("edge")
        elif mode == "popitem":
            mapping.popitem()
        elif mode == "clear":
            mapping.clear()
        else:
            mapping.update(edge=0)
        after = gc.is_tracked(mapping)
        gc.collect(0)
        young = gc.is_tracked(mapping)
        gc.collect(1)
        middle = gc.is_tracked(mapping)
        gc.collect(2)
        print("mutation", mode, before, after, young, middle, gc.is_tracked(mapping))

    child = {}
    parent = {"child": child}
    gc.collect()
    print("mutable_child", gc.is_tracked(child), gc.is_tracked(parent))
    child["parent"] = parent
    del child, parent
    # Compare the known isolated cycle, not all interpreter-owned garbage.
    print("late_back_edge", gc.collect() >= 2)

    mapping = {"edge": []}
    atomic_tuple = tuple([1])
    mapping["edge"] = atomic_tuple
    gc.collect()
    print(
        "tuple_before_dict_demotion",
        gc.is_tracked(atomic_tuple),
        gc.is_tracked(mapping),
    )
    duplicate = dict([("edge", []), ("edge", 0)])
    print("duplicate_construction", gc.is_tracked(duplicate))

    for empty in (False, True):
        source = {"edge": []}
        source["edge"] = 0
        if empty:
            source.clear()
        updated = {}
        updated.update(source)
        occupied = {"other": 1}
        occupied.update(source)
        print(
            "copy_tracking",
            empty,
            gc.is_tracked(source.copy()),
            gc.is_tracked(dict(source)),
            gc.is_tracked(updated),
            gc.is_tracked(occupied),
            gc.is_tracked({**source}),
        )

    observations = []

    class Observer:
        def __del__(self):
            observations.append((sorted(published), gc.is_tracked(published)))

    for mode in ("replace", "delete", "clear", "update"):
        published = {"edge": Observer()}
        if mode == "replace":
            published["edge"] = 0
        elif mode == "delete":
            del published["edge"]
        elif mode == "clear":
            published.clear()
        else:
            published.update(edge=0)
        print("published_before_finalizer", mode, observations.pop())
finally:
    gc.set_threshold(*original)
    gc.enable()
