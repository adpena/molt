# Behavior: async methods and async generator methods reached via dynamic dispatch
# (bound method indirection) must still allocate task frames with correct closure size.
# Why: Molt relies on task-aware trampolines for async call paths; missing closure metadata
# breaks await/asyncgen correctness for class methods that capture free vars.
# Pitfalls: relies on asyncio; ensure this stays host CPython compatible and avoid
# non-deterministic scheduling.
import asyncio


def make_instance():
    bonus = 3

    class C:
        async def add(self, x):
            return x + bonus

        async def agen(self, x):
            yield x + bonus + 1

    return C()


async def run_async():
    obj = make_instance()
    f = obj.add
    print(await f(4))
    g = obj.agen
    ag = g(5)
    print(await ag.__anext__())


def main():
    asyncio.run(run_async())


if __name__ == "__main__":
    main()
