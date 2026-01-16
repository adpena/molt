rows = []
i = 0
while i < 200:
    qty = (i * 7) % 50
    price = (i * 19) % 1000
    active = "1" if (i % 3) != 0 else "0"
    line = str(i) + "," + str(qty) + "," + str(price) + "," + active
    if i % 10 == 0:
        line = line + ",note"
    rows.append(line)
    i = i + 1

data = "id,qty,price,active,tag\n" + "\n".join(rows)
lines = data.split("\n")

total = 0
outer = 0
while outer < 200:
    idx = 1
    while idx < len(lines):
        line = lines[idx]
        if line:
            fields = line.split(",")
            qty = int(fields[1])
            price = int(fields[2])
            if fields[3] == "1":
                total = total + (qty * price)
            if len(fields) > 4:
                total = total + len(fields[4])
        idx = idx + 1
    outer = outer + 1

print(total)
