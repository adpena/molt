def parse_int(text: str) -> int:
    value = 0
    i = 0
    while i < len(text):
        value = value * 10 + (ord(text[i]) - 48)
        i += 1
    return value


def main() -> None:
    names: list[str] = ["alpha", "beta", "gamma", "delta", "epsilon"]
    regions: list[str] = ["us", "eu", "apac"]
    products: list[str] = ["widget", "gizmo", "sprocket", "doodad"]

    rows: list[str] = []
    i = 0
    while i < 400:
        name: str = names[i % len(names)]
        city: str = "city" + str(i % 50)
        product: str = products[i % len(products)]
        qty: int = (i * 7) % 80
        price: int = (i * 19) % 10000
        active: str = "1" if (i % 5) != 0 else "0"
        tag: str = "promo" if (i % 7) == 0 else "std"
        note: str = "note" + str(i % 13)
        code: str = "C" + str((i * 3) % 1000)
        region: str = regions[i % len(regions)]
        extra: str = "x" + str((i * 11) % 997)
        if i % 4 == 0:
            name = '"' + name + '"'
            note = '"' + note + '"'
        line = (
            str(i)
            + ","
            + name
            + ","
            + city
            + ","
            + product
            + ","
            + str(qty)
            + ","
            + str(price)
            + ","
            + active
            + ","
            + tag
            + ","
            + note
            + ","
            + code
            + ","
            + region
            + ","
            + extra
        )
        rows.append(line)
        i += 1

    data: str = (
        "id,name,city,product,qty,price,active,tag,note,code,region,extra\n"
        + "\n".join(rows)
    )
    lines: list[str] = data.split("\n")

    total = 0
    outer = 0
    while outer < 80:
        idx = 1
        while idx < len(lines):
            line: str = lines[idx]
            if line:
                fields: list[str] = line.split(",")
                qty = parse_int(fields[4])
                price = parse_int(fields[5])
                if fields[6] == "1":
                    total = total + (qty * price)
                name = fields[1]
                if name and name[0] == '"':
                    name = name[1:-1]
                note = fields[8]
                if note and note[0] == '"':
                    note = note[1:-1]
                if note.startswith("note1"):
                    total = total + 1
                if fields[10].lower() == "eu":
                    total = total + len(name)
                total = (
                    total
                    + len(fields[2])
                    + len(fields[3])
                    + len(fields[9])
                    + len(fields[11])
                )
            idx += 1
        outer += 1

    print(total)


if __name__ == "__main__":
    main()
