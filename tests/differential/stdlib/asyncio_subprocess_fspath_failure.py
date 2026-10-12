# MOLT_ENV: MOLT_CAPABILITIES=fs,env,time,random,process.exec
"""Process path conversion must finish or fail before starting a child."""

import asyncio
import os


async def main():
    if os.name == "nt":
        print("POSIX path conversion controls")
        return

    for location in ("argument", "environment", "cwd"):
        error = RuntimeError("path callback failure")
        calls = []

        class FailingPath:
            def __fspath__(self):
                calls.append(location)
                raise error

        program = ["/molt-fspath-regression-no-such-executable"]
        options = {}
        if location == "argument":
            program.append(FailingPath())
        elif location == "environment":
            options["env"] = {"MOLT_TEST_PATH": FailingPath()}
        else:
            options["cwd"] = FailingPath()
        try:
            await asyncio.create_subprocess_exec(*program, **options)
        except RuntimeError as actual:
            print("callback identity", location, actual is error, calls)
            assert actual is error
        else:
            raise AssertionError("path callback error was lost")

    environment = {}
    calls = []

    class MutatingPath:
        def __fspath__(self):
            calls.append("key")
            environment.clear()
            return "MOLT_TEST_PATH"

    class RetainedValue:
        def __fspath__(self):
            assert "retired" not in calls
            calls.append("value")
            return "still alive"

        def __del__(self):
            calls.append("retired")

    environment[MutatingPath()] = RetainedValue()
    environment["SECOND"] = "value"
    try:
        await asyncio.create_subprocess_exec(
            "/molt-fspath-regression-no-such-executable", env=environment
        )
    except RuntimeError as error:
        conversions = [event for event in calls if event != "retired"]
        print("environment mutation", str(error), conversions)
        assert str(error) == "dictionary changed size during iteration"
        assert conversions == ["key", "value"]
    else:
        raise AssertionError("dictionary mutation was concealed")


asyncio.run(main())
