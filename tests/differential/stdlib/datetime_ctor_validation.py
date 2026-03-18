"""Purpose: datetime constructor validation parity, including fold support."""

import datetime as dt


def capture(label, fn):
    try:
        print(label, fn())
    except Exception as exc:
        print(label, type(exc).__name__, str(exc))


def human_reset_text(reset_at, *, now=None):
    current = now or dt.datetime.now(dt.timezone.utc)
    if reset_at.tzinfo is None:
        reset_at = reset_at.replace(tzinfo=dt.timezone.utc)
    diff = reset_at - current
    total_seconds = int(diff.total_seconds())
    if total_seconds <= 0:
        return "Resetting..."
    days, remainder = divmod(total_seconds, 86400)
    hours, remainder = divmod(remainder, 3600)
    minutes, seconds = divmod(remainder, 60)
    if days > 0:
        return f"Resets in {days}d {hours:02}:{minutes:02}:{seconds:02}"
    return f"Resets in {hours:02}:{minutes:02}:{seconds:02}"


capture("time-ok", lambda: dt.time(20, 30, 0, 123456).isoformat())
capture("datetime-ok", lambda: dt.datetime(2026, 3, 18, 20, 30, 0, 123456).isoformat())
capture(
    "datetime-replace-tz",
    lambda: dt.datetime(2026, 3, 18, 20, 30, 0, 123456)
    .replace(tzinfo=dt.timezone.utc)
    .isoformat(),
)
capture("fromordinal-epoch", lambda: dt.date.fromordinal(719163).isoformat())
capture("now-utc", lambda: dt.datetime.now(dt.timezone.utc).tzname())
capture("now-utc-iso", lambda: dt.datetime.now(dt.timezone.utc).isoformat())
capture("utcnow-iso", lambda: dt.datetime.utcnow().isoformat())
capture("timedelta-float", lambda: str(dt.timedelta(minutes=1.5)))
capture(
    "timedelta-total-seconds",
    lambda: (
        dt.datetime.fromisoformat("2026-03-20T00:00:00+00:00")
        - dt.datetime.now(dt.timezone.utc)
    ).total_seconds()
    > 0,
)
capture(
    "human-reset-text",
    lambda: human_reset_text(
        dt.datetime.fromisoformat("2026-03-20T00:00:00+00:00"),
        now=dt.datetime.now(dt.timezone.utc),
    ),
)
capture("bad-month", lambda: dt.datetime(2026, 13, 18))
capture("bad-fold", lambda: dt.time(20, 30, fold=2))
