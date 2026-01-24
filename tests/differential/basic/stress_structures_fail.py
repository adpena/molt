"""Purpose: differential coverage for stress structures fail."""


def run():
    # 1. Nested Structures (List of Dict of Set)
    print("--- Nested Structures ---")
    complex_list = []
    for i in range(5):
        d = {"id": i, "tags": {f"tag_{i}", f"tag_{i + 1}"}, "meta": {"nested": i * 10}}
        complex_list.append(d)

    print(complex_list)

    # 2. Mutation in Loop
    print("--- Mutation ---")
    for item in complex_list:
        item["tags"].add("new_tag")
        item["meta"]["nested"] += 1
        if item["id"] % 2 == 0:
            item["extra"] = [1, 2, 3]

    print(complex_list)

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
    clone[0]["tags"].add("diff")
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
    print(s1 | s2)
    print(s1 & s2)
    print(s1 - s2)
    print(s1 ^ s2)

    try:
        # Frozenset interaction
        fs = frozenset([3, 4, 6])
        print(s1 | fs)  # Result is set
        print(fs | s1)  # Result is frozenset
    except TypeError as e:
        print(e)


if __name__ == "__main__":
    run()
