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
finally:
    gc.set_threshold(*original)
    gc.enable()
