def main() -> None:
    s = {1, 2, 2, 3}
    print(len(s))
    print(2 in s, 5 in s)
    s.add(5)
    print(len(s))
    s.discard(2)
    print(2 in s)
    try:
        s.remove(99)
    except KeyError:
        print("KeyError")
    t = set([1, 1, 2])
    print(len(t))
    total = 0
    for x in t:
        total += x
    print(total)
    v = {10, 20}
    popped = v.pop()
    print(popped in {10, 20})
    print(len(v))
    empty = set()
    print(len(empty))
    empty.add(7)
    print(7 in empty)


if __name__ == "__main__":
    main()
