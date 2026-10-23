from dataclasses import FrozenInstanceError, dataclass


@dataclass(frozen=True)
class FrozenPoint:
    x: int


fp = FrozenPoint(1)

try:
    fp.x = 2
except FrozenInstanceError:
    print("assign", "FrozenInstanceError")
except Exception as exc:
    print("assign", type(exc).__name__)

try:
    del fp.x
except FrozenInstanceError:
    print("delete", "FrozenInstanceError")
except Exception as exc:
    print("delete", type(exc).__name__)


# A caught runtime-constructed error must not retain its payload or traceback.
import gc
import weakref


class ErrorMarker:
    pass


def frozen_error_marker(delete):
    marker = ErrorMarker()
    observed = weakref.ref(marker)
    point = FrozenPoint(3)
    try:
        if delete:
            del point.x
        else:
            point.x = 4
    except FrozenInstanceError as error:
        error.marker = marker
    else:
        raise AssertionError("frozen mutation did not raise")
    return observed


assigned_marker = frozen_error_marker(False)
deleted_marker = frozen_error_marker(True)
gc.collect()
assert assigned_marker() is None
assert deleted_marker() is None
print("frozen error owners released", True)
