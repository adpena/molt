import asyncio


def make_gen(scale):
    def gen(start, *, count=3, bump=0):
        total = 0
        for i in range(count):
            total += (start + i + bump) * scale
            yield total
        return total

    return gen


def make_async(scale):
    async def worker(start, *, count=2):
        total = 0
        for i in range(count):
            total += (start + i) * scale
            await asyncio.sleep(0)
        return total

    return worker


def consume(gen):
    out = []
    for val in gen:
        out.append(val)
    return out


print("--- generator task ---")
gen_factory = make_gen(2)
gen_vals = consume(gen_factory(1, count=4, bump=1))
print(gen_vals)

print("--- async task ---")


async def main():
    worker = make_async(3)
    return await worker(2, count=3)


print(asyncio.run(main()))
