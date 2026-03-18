"""Purpose: datetime constructor validation parity, including fold support."""

import datetime as dt


def capture(label, fn):
    try:
        print(label, fn())
    except Exception as exc:
        print(label, type(exc).__name__, str(exc))


capture("time-ok", lambda: dt.time(20, 30, 0, 123456).isoformat())
capture("datetime-ok", lambda: dt.datetime(2026, 3, 18, 20, 30, 0, 123456).isoformat())
capture("now-utc", lambda: dt.datetime.now(dt.timezone.utc).tzname())
capture("timedelta-float", lambda: str(dt.timedelta(minutes=1.5)))
capture(
    "timedelta-total-seconds",
    lambda: (
        dt.datetime.fromisoformat("2026-03-19T00:00:00+00:00")
        - dt.datetime.now(dt.timezone.utc)
    ).total_seconds()
    > 0,
)
capture("bad-month", lambda: dt.datetime(2026, 13, 18))
capture("bad-fold", lambda: dt.time(20, 30, fold=2))
