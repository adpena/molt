"""Purpose: differential coverage for asyncio module/function surface."""

from __future__ import annotations

import asyncio


def _probe(name: str, getter) -> tuple[str, bool, str | None]:
    try:
        value = getter()
    except AttributeError:
        return (name, False, None)
    except Exception as exc:  # pragma: no cover - diagnostic only
        return (name, True, f"error:{type(exc).__name__}")
    return (name, True, type(value).__name__)


PROBES = [
    ("_get_running_loop", lambda: asyncio._get_running_loop),
    ("_set_running_loop", lambda: asyncio._set_running_loop),
    ("base_events", lambda: asyncio.base_events),
    ("base_futures", lambda: asyncio.base_futures),
    ("base_subprocess", lambda: asyncio.base_subprocess),
    ("base_tasks", lambda: asyncio.base_tasks),
    ("capture_call_graph", lambda: asyncio.capture_call_graph),
    ("constants", lambda: asyncio.constants),
    ("coroutines", lambda: asyncio.coroutines),
    ("create_eager_task_factory", lambda: asyncio.create_eager_task_factory),
    ("eager_task_factory", lambda: asyncio.eager_task_factory),
    ("events", lambda: asyncio.events),
    ("exceptions", lambda: asyncio.exceptions),
    ("format_helpers", lambda: asyncio.format_helpers),
    ("future_add_to_awaited_by", lambda: asyncio.future_add_to_awaited_by),
    ("future_discard_from_awaited_by", lambda: asyncio.future_discard_from_awaited_by),
    ("futures", lambda: asyncio.futures),
    ("get_child_watcher", lambda: asyncio.get_child_watcher),
    ("get_event_loop", lambda: asyncio.get_event_loop),
    ("iscoroutine", lambda: asyncio.iscoroutine),
    ("iscoroutinefunction", lambda: asyncio.iscoroutinefunction),
    ("isfuture", lambda: asyncio.isfuture),
    ("locks", lambda: asyncio.locks),
    ("log", lambda: asyncio.log),
    ("mixins", lambda: asyncio.mixins),
    ("open_unix_connection", lambda: asyncio.open_unix_connection),
    ("print_call_graph", lambda: asyncio.print_call_graph),
    ("protocols", lambda: asyncio.protocols),
    ("queues", lambda: asyncio.queues),
    ("runners", lambda: asyncio.runners),
    ("run_coroutine_threadsafe", lambda: asyncio.run_coroutine_threadsafe),
    ("selector_events", lambda: asyncio.selector_events),
    ("set_child_watcher", lambda: asyncio.set_child_watcher),
    ("sslproto", lambda: asyncio.sslproto),
    ("staggered", lambda: asyncio.staggered),
    ("start_unix_server", lambda: asyncio.start_unix_server),
    ("subprocess", lambda: asyncio.subprocess),
    ("streams", lambda: asyncio.streams),
    ("taskgroups", lambda: asyncio.taskgroups),
    ("tasks", lambda: asyncio.tasks),
    ("threads", lambda: asyncio.threads),
    ("timeouts", lambda: asyncio.timeouts),
    ("transports", lambda: asyncio.transports),
    ("trsock", lambda: asyncio.trsock),
    ("unix_events", lambda: asyncio.unix_events),
    ("windows_events", lambda: asyncio.windows_events),
    ("windows_utils", lambda: asyncio.windows_utils),
    ("wrap_future", lambda: asyncio.wrap_future),
]


def main() -> None:
    results = [_probe(name, getter) for name, getter in PROBES]
    print(results)


if __name__ == "__main__":
    main()
