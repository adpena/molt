import asyncio


async def work(count: int) -> int:
    total = 0
    i = 0
    while i < count:
        await asyncio.sleep(0)
        total += i
        i += 1
    return total


def main() -> None:
    print(asyncio.run(work(1_000)))


if __name__ == "__main__":
    main()
