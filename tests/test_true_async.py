import asyncio


def test_true_async_order(capsys):
    async def main():
        print(1)
        await asyncio.sleep(0)
        print(2)

    asyncio.run(main())
    captured = capsys.readouterr()
    assert captured.out == "1\n2\n"
