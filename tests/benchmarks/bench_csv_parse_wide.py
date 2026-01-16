names = ["alpha", "beta", "gamma", "delta", "epsilon"]
regions = ["us", "eu", "apac"]
products = ["widget", "gizmo", "sprocket", "doodad"]

rows = []
i = 0
while i < 400:
    name = names[i % len(names)]
    city = "city" + str(i % 50)
    product = products[i % len(products)]
    qty = (i * 7) % 80
    price = (i * 19) % 10000
    active = "1" if (i % 5) != 0 else "0"
    tag = "promo" if (i % 7) == 0 else "std"
    note = "note" + str(i % 13)
    code = "C" + str((i * 3) % 1000)
    region = regions[i % len(regions)]
    extra = "x" + str((i * 11) % 997)
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
    i = i + 1

data = "id,name,city,product,qty,price,active,tag,note,code,region,extra\n" + "\n".join(
    rows
)
lines = data.split("\n")

total = 0
outer = 0
while outer < 120:
    idx = 1
    while idx < len(lines):
        line = lines[idx]
        if line:
            fields = line.split(",")
            qty = int(fields[4])
            price = int(fields[5])
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
        idx = idx + 1
    outer = outer + 1

print(total)
