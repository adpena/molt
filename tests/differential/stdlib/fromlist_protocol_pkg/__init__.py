"""Fresh package observing ordered fromlist preparation."""

events = []
failure = ValueError("fromlist child failure")


def __getattr__(name):
    events.append(("get", name))
    raise AttributeError(name)
