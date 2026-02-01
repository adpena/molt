import molt_buffer


def mat_mul() -> int:
    a = molt_buffer.new(2, 2, 0)
    b = molt_buffer.new(2, 2, 0)
    molt_buffer.set(a, 0, 0, 1)
    molt_buffer.set(a, 0, 1, 2)
    molt_buffer.set(a, 1, 0, 3)
    molt_buffer.set(a, 1, 1, 4)
    molt_buffer.set(b, 0, 0, 5)
    molt_buffer.set(b, 0, 1, 6)
    molt_buffer.set(b, 1, 0, 7)
    molt_buffer.set(b, 1, 1, 8)
    out = molt_buffer.new(2, 2, 0)
    i = 0
    res = 0
    while i < 20000:
        for row in range(2):
            for col in range(2):
                acc = 0
                for k in range(2):
                    acc = acc + molt_buffer.get(a, row, k) * molt_buffer.get(b, k, col)
                molt_buffer.set(out, row, col, acc)
        res = res + molt_buffer.get(out, 0, 0)
        i = i + 1
    return res


def main() -> None:
    print(mat_mul())


if __name__ == "__main__":
    main()
