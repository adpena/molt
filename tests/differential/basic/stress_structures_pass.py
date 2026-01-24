"""Purpose: differential coverage for stress structures pass."""


def safe_print(obj):
    if isinstance(obj, list):
        print("[")
        for item in obj:
            safe_print(item)
            print(",")
        print("]")
    elif isinstance(obj, dict):
        print("{")
        for k in sorted(obj.keys()):
            print(f"{k!r}: ")
            safe_print(obj[k])
            print(",")
        print("}")
    elif isinstance(obj, (set, frozenset)):
        print("{" + ", ".join(sorted(map(str, obj))) + "}")
    else:
        print(repr(obj))


def run():
    # 1. Nested Structures (List of Dict of Set)
    print("--- Nested Structures ---")
    complex_list = []
    for i in range(5):
        d = {"id": i, "tags": {f"tag_{i}", f"tag_{i + 1}"}, "meta": {"nested": i * 10}}
        complex_list.append(d)

    safe_print(complex_list)

    # 2. Mutation in Loop (Without dynamic set.add)
    print("--- Mutation ---")
    for item in complex_list:
        # Intentionally skipping dynamic set.add here to test other features
        item["meta"]["nested"] += 1
        if item["id"] % 2 == 0:
            item["extra"] = [1, 2, 3]

    safe_print(complex_list)

    # 3. Deep Equality
    print("--- Equality ---")
    clone = []
    for item in complex_list:
        # Manual deep copyish
        new_d = {}
        for k, v in item.items():
            if k == "tags":
                new_d[k] = set(v)
            elif k == "meta":
                new_d[k] = dict(v)
            elif k == "extra":
                new_d[k] = list(v)
            else:
                new_d[k] = v
        clone.append(new_d)

    print(complex_list == clone)

    # 4. Sorting (Mixed Types - should fail or succeed depending on type)
    print("--- Sorting ---")
    int_list = [5, 1, 9, 3]
    int_list.sort()
    print(int_list)

    # Sorting list of dicts by key
    complex_list.sort(key=lambda x: x["id"], reverse=True)
    print([x["id"] for x in complex_list])

    # 5. Set Algebra with Mixed Types
    print("--- Set Algebra ---")
    s1 = {1, 2, 3}
    s2 = {3, 4, 5}
    # Print sorted lists of results
    print(sorted(list(s1 | s2)))
    print(sorted(list(s1 & s2)))
    print(sorted(list(s1 - s2)))
    print(sorted(list(s1 ^ s2)))

    try:
        # Frozenset interaction
        fs = frozenset([3, 4, 6])
        res1 = s1 | fs
        print(type(res1).__name__, sorted(list(res1)))

        res2 = fs | s1
        print(type(res2).__name__, sorted(list(res2)))
    except TypeError as e:
        print(e)


if __name__ == "__main__":
    run()
