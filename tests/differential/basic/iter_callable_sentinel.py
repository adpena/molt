"""Purpose: differential coverage for iter(callable, sentinel)."""

count = 0


def next_value():
    global count
    count += 1
    return count


if __name__ == "__main__":
    iterator = iter(next_value, 3)
    print("values", list(iterator))
    print("count", count)
