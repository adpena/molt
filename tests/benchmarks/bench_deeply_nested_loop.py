rows = 120
cols = 80
depth = 100
inner_limit = 5

data = []
r_build = 0
while r_build < rows:
    row = []
    c_build = 0
    while c_build < cols:
        row.append(c_build)
        c_build = c_build + 1
    data.append(row)
    r_build = r_build + 1

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
                inner = inner + 1
            c = c + 1
        r = r + 1
    d = d + 1

print(total)
