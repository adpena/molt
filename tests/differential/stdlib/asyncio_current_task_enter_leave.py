"""Purpose: differential coverage for _asyncio.current_task,
_asyncio._enter_task, _asyncio._leave_task, _asyncio._get_running_loop,
_asyncio._set_running_loop."""

import _asyncio

# current_task outside of async context raises RuntimeError
try:
    ct = _asyncio.current_task()
    print("current_task outside:", ct)
except RuntimeError:
    print("current_task outside: RuntimeError (no running event loop)")
# Test _get_running_loop / _set_running_loop
rl = _asyncio._get_running_loop()
print("running_loop initially:", rl)
