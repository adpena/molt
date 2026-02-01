def parse_int(text: str) -> int:
    value = 0
    i = 0
    while i < len(text):
        value = value * 10 + (ord(text[i]) - 48)
        i += 1
    return value


def main() -> None:
    rows: list[str] = []
    i = 0
    while i < 200:
        qty: int = (i * 7) % 50
        price: int = (i * 19) % 1000
        active: str = "1" if (i % 3) != 0 else "0"
        line: str = str(i) + "," + str(qty) + "," + str(price) + "," + active
        if i % 10 == 0:
            line = line + ",note"
        rows.append(line)
        i += 1

    data: str = "id,qty,price,active,tag\n" + "\n".join(rows)
    lines: list[str] = data.split("\n")

    total = 0
    outer = 0
    while outer < 120:
        idx = 1
        while idx < len(lines):
            line: str = lines[idx]
            if line:
                fields: list[str] = line.split(",")
                qty = parse_int(fields[1])
                price = parse_int(fields[2])
                if fields[3] == "1":
                    total = total + (qty * price)
                if len(fields) > 4:
                    total = total + len(fields[4])
            idx += 1
        outer += 1

    print(total)


if __name__ == "__main__":
    main()
