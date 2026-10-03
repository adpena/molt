"""Purpose: a TAQ-style ingest loop reads its header flag as the loop does.

Both loop bodies below once had a bespoke fused ingest op; they now lower as
ordinary Python, and must stay exactly that. The ``if header:`` guard must
read the live binding on every iteration. At module scope the flag's home is
the module namespace, so reusing the value cached before the loop would treat
every line as the header. The function variant pins the same loop over frame
locals. Version-stable across CPython 3.12/3.13/3.14.
"""

BUCKET_SIZE = 1_000_000_000
LINES = [
    "timestamp|x|symbol|x|volume",
    "100|X|AAPL|X|200",
    "END|X|IGNORED|X|1",
    "2000000000|X|MSFT|X|400",
    "300|X|AAPL|X|ENDP",
    "3000000001|X|AAPL|X|500",
]

data = {}
header = True
for line in LINES:
    if header:
        header = False
        continue
    x = line.split("|")
    if x[0] == "END" or x[4] == "ENDP":
        continue
    timestamp = int(x[0])
    symbol = x[2]
    volume = int(x[4])
    series = data.setdefault(symbol, [])
    series.append((timestamp // BUCKET_SIZE, volume))
print("module", sorted(data.items()), header)


def ingest(lines):
    data = {}
    header = True
    for line in lines:
        if header:
            header = False
            continue
        x = line.split("|")
        if x[0] == "END" or x[4] == "ENDP":
            continue
        timestamp = int(x[0])
        symbol = x[2]
        volume = int(x[4])
        series = data.setdefault(symbol, [])
        series.append((timestamp // BUCKET_SIZE, volume))
    return sorted(data.items()), header


print("function", *ingest(LINES))
