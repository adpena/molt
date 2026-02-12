"""Purpose: ensure _asyncio running-loop hooks are runtime intrinsic-backed."""

import _asyncio
import asyncio


print("initial-none", _asyncio._get_running_loop() is None)


async def main() -> None:
    loop = asyncio.get_running_loop()
    print("during-run-main", _asyncio.get_running_loop() is loop)
    print("during-run-hidden", _asyncio._get_running_loop() is loop)
    print("during-run-event-loop", _asyncio.get_event_loop() is loop)


asyncio.run(main())

print("after-run-none", _asyncio._get_running_loop() is None)


token = object()
_asyncio._set_running_loop(token)
print("manual-set", _asyncio._get_running_loop() is token)
print("asyncio-view", asyncio._get_running_loop() is token)
_asyncio._set_running_loop(None)

try:
    _asyncio.get_running_loop()
except Exception as exc:
    print("get-running-loop-error", type(exc).__name__)

loop = _asyncio.get_event_loop()
print("event-loop-type", type(loop).__name__)
