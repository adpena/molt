from __future__ import annotations

from concurrent.futures import ThreadPoolExecutor, wait
import time


def add(x: int, y: int) -> int:
    return x + y


with ThreadPoolExecutor(max_workers=2) as executor:
    fut1 = executor.submit(add, 1, 2)
    fut2 = executor.submit(add, 3, 4)
    print("submitted", fut1, fut2)
    print("type", type(fut1))
    print("field_offsets", getattr(type(fut1), "__molt_field_offsets__", None))
    print("dict", getattr(fut1, "__dict__", None))
    for name in (
        "_condition",
        "_done",
        "_running",
        "_cancelled",
        "_result",
        "_exception",
        "_callbacks",
    ):
        print("has", name, hasattr(fut1, name), getattr(fut1, name, None))
    done, pending = wait([fut1, fut2], timeout=2.0)
    print("done", len(done), "pending", len(pending))
    for name in (
        "_condition",
        "_done",
        "_running",
        "_cancelled",
        "_result",
        "_exception",
        "_callbacks",
    ):
        print("post-wait", name, hasattr(fut1, name), getattr(fut1, name, None))
    try:
        print("direct _cancelled", fut1._cancelled)
    except Exception as exc:
        print("direct _cancelled error", type(exc), exc)
    try:
        print("direct _result", fut1._result)
    except Exception as exc:
        print("direct _result error", type(exc), exc)
    for fut in sorted(done, key=lambda f: id(f)):
        try:
            print("result", fut.result())
        except Exception as exc:
            print("result error", type(exc), exc)
    if pending:
        print("pending", pending)
        time.sleep(0.5)
        done2, pending2 = wait([fut1, fut2], timeout=2.0)
        print("done2", len(done2), "pending2", len(pending2))
