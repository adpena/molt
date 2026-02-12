"""Purpose: differential coverage for select backend object surface/failures."""

from __future__ import annotations

import select


def _safe_close(obj) -> None:
    close = getattr(obj, "close", None)
    if callable(close):
        close()


def _has_attr(obj, name: str) -> int:
    try:
        getattr(obj, name)
        return 1
    except AttributeError:
        return 0


def _probe_poll() -> tuple[str, object]:
    if not hasattr(select, "poll"):
        return ("poll", "missing")
    return ("poll", "present")


def _probe_epoll() -> tuple[str, object]:
    if not hasattr(select, "epoll"):
        return ("epoll", "missing")
    ep = select.epoll()
    try:
        surface = (
            _has_attr(ep, "close"),
            _has_attr(ep, "fileno"),
            _has_attr(ep, "closed"),
        )
        default_register = "ok"
        try:
            ep.register(0)  # default eventmask parity probe
        except Exception as exc:
            default_register = type(exc).__name__
        try:
            ep.unregister(0)
        except Exception:
            pass
        try:
            ep.poll(0.0, 0)
            maxevents_err = "ok"
        except Exception as exc:  # parity by exception class
            maxevents_err = type(exc).__name__
    finally:
        _safe_close(ep)
    return ("epoll", surface, default_register, maxevents_err)


def _probe_devpoll() -> tuple[str, object]:
    if not hasattr(select, "devpoll"):
        return ("devpoll", "missing")
    dev = select.devpoll()
    try:
        surface = (
            _has_attr(dev, "close"),
            _has_attr(dev, "fileno"),
            _has_attr(dev, "closed"),
        )
        default_register = "ok"
        try:
            dev.register(0)  # default eventmask parity probe
        except Exception as exc:
            default_register = type(exc).__name__
        try:
            dev.unregister(0)
        except Exception:
            pass
    finally:
        _safe_close(dev)
    return ("devpoll", surface, default_register)


def _probe_kqueue() -> tuple[str, object]:
    if not (hasattr(select, "kqueue") and hasattr(select, "kevent")):
        return ("kqueue", "missing")
    kq = select.kqueue()
    initial_closed = bool(getattr(kq, "closed", False))
    try:
        try:
            kq.control([], -1, 0.0)
            neg_maxevents = "ok"
        except Exception as exc:
            neg_maxevents = type(exc).__name__
        try:
            kq.control([], 0, -1.0)
            neg_timeout = "ok"
        except Exception as exc:
            neg_timeout = type(exc).__name__
        try:
            kq.control([1], 0, 0.0)
            bad_change = "ok"
        except Exception as exc:
            bad_change = type(exc).__name__
    finally:
        _safe_close(kq)
    final_closed = bool(getattr(kq, "closed", False))
    return (
        "kqueue",
        initial_closed,
        final_closed,
        neg_maxevents,
        neg_timeout,
        bad_change,
    )


print([_probe_poll(), _probe_epoll(), _probe_devpoll(), _probe_kqueue()])
