def main() -> None:
    rows = 80
    cols = 60
    depth = 40
    inner_limit = 5

    data: list[list[int]] = []
    r_build = 0
    while r_build < rows:
        row: list[int] = []
        c_build = 0
        while c_build < cols:
            row.append(c_build)
            c_build += 1
        data.append(row)
        r_build += 1

    total = 0
    d = 0
    while d < depth:
        r = 0
        while r < rows:
            row = data[r]
            c = 0
            while c < cols:
                base = row[c] + d
                inner = 0
                while inner < inner_limit:
                    total = total + (base ^ inner)
                    inner += 1
                c += 1
            r += 1
        d += 1

    print(total)


if __name__ == "__main__":
    main()
