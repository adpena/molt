"""Purpose: differential coverage for threading.settrace/setprofile."""

try:
    import threading
except Exception as exc:
    print(type(exc).__name__, exc)
else:
    trace_events: list[str] = []
    profile_events: list[str] = []

    def tracer(frame, event, arg):
        trace_events.append(event)
        return tracer

    def profiler(frame, event, arg):
        profile_events.append(event)

    try:
        threading.settrace(tracer)
        threading.setprofile(profiler)
    except Exception as exc:
        print(type(exc).__name__, exc)
    else:
        def worker() -> None:
            return None

        t = threading.Thread(target=worker)
        t.start()
        t.join(timeout=1.0)
        print("trace", len(trace_events) > 0)
        print("profile", len(profile_events) > 0)
    finally:
        threading.settrace(None)
        threading.setprofile(None)
