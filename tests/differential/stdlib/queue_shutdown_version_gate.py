"""Purpose: differential coverage for queue shutdown semantics with version gates."""

import queue
import sys


version = tuple(sys.version_info[:3])
try:
    queue.ShutDown
except AttributeError:
    has_shutdown_exc = False
else:
    has_shutdown_exc = True

try:
    queue.Queue.shutdown
except AttributeError:
    has_shutdown_method = False
else:
    has_shutdown_method = True

if has_shutdown_exc and not has_shutdown_method:
    raise AssertionError((version, "ShutDown-present shutdown-missing"))
if has_shutdown_method and not has_shutdown_exc:
    raise AssertionError((version, "shutdown-present ShutDown-missing"))

print("version", version)
print("has_shutdown_exc", has_shutdown_exc)
print("has_shutdown_method", has_shutdown_method)

if has_shutdown_method:
    q = queue.Queue()
    q.put("queued")
    q.shutdown()

    try:
        q.put("after-shutdown")
    except Exception as exc:  # noqa: BLE001
        print("put_after_shutdown_exc", type(exc).__name__)
        assert isinstance(exc, queue.ShutDown), type(exc)
    else:
        raise AssertionError("queue.Queue.put() should raise ShutDown after shutdown()")

    print("get_before_drain", q.get_nowait())

    try:
        q.get_nowait()
    except Exception as exc:  # noqa: BLE001
        print("get_after_drain_exc", type(exc).__name__)
        assert isinstance(exc, queue.ShutDown), type(exc)
    else:
        raise AssertionError("queue.Queue.get_nowait() should raise ShutDown when drained")

    immediate = queue.Queue()
    immediate.put("queued")
    immediate.shutdown(immediate=True)
    try:
        immediate.get_nowait()
    except Exception as exc:  # noqa: BLE001
        print("get_after_immediate_exc", type(exc).__name__)
        assert isinstance(exc, queue.ShutDown), type(exc)
    else:
        raise AssertionError(
            "queue.Queue.get_nowait() should raise ShutDown after immediate shutdown"
        )
