import asyncio


def test_async_closure_value(capsys):
    async def main():
        x = 40
        await asyncio.sleep(0)
        print(x + 2)

    asyncio.run(main())
    captured = capsys.readouterr()
    assert captured.out == "42\n"
