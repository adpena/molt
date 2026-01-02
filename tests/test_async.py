import asyncio


def test_async_prints_value(capsys):
    async def main():
        print(42)

    asyncio.run(main())
    captured = capsys.readouterr()
    assert captured.out == "42\n"
