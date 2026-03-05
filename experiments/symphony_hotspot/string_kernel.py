"""Minimal string processing kernel — isolates string.strip/lower/sort."""
import time


def normalize_labels(names: list[str]) -> tuple[str, ...]:
    seen: dict[str, bool] = {}
    clean: list[str] = []
    for name in names:
        stripped = name.strip()
        if stripped == "":
            continue
        lowered = stripped.lower()
        if lowered not in seen:
            seen[lowered] = True
            clean.append(lowered)
    clean.sort()
    return tuple(clean)


def run() -> None:
    labels = [
        "  area:runtime  ",
        " owner:compiler ",
        "  P1  ",
        "  milestone:M2  ",
        " area:runtime ",
    ]

    start = time.perf_counter_ns()
    last: tuple[str, ...] = ()
    i = 0
    while i < 500000:
        last = normalize_labels(labels)
        i += 1
    end = time.perf_counter_ns()

    elapsed_ms = (end - start) / 1000000.0
    per_call_ns = (end - start) / 500000.0
    print("Result: " + str(last))
    print("INTERNAL_TIME_MS=" + str(int(elapsed_ms)))
    print("PER_CALL_NS=" + str(int(per_call_ns)))


run()
