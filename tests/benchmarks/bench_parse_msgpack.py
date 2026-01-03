import msgpack

from molt_msgpack import parse


payload = msgpack.packb(
    [{"k": i, "v": i + 1} for i in range(16)],
    use_bin_type=True,
)

total = 0
for _ in range(2000):
    obj = parse(payload)
    total = total + obj[0]["v"]

print(total)
