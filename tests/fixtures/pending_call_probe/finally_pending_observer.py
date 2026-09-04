from pending_call_probe._native import arm_runtime_error


PLAIN_ORIGINAL = ValueError("plain original")
PLAIN_REPLACEMENT = TypeError("plain replacement")
MARKED_ORIGINAL = LookupError("marked original")


def raise_plain_replacement() -> None:
    try:
        raise PLAIN_ORIGINAL
    finally:
        # No call occurs in this finalbody, so its pending observer must remain
        # the plain exception-state read rather than acquiring poll semantics.
        raise PLAIN_REPLACEMENT


def raise_pending_replacement() -> None:
    try:
        raise MARKED_ORIGINAL
    finally:
        # This call queues the replacement. The existing post-call marker must
        # make the final observer service pending work before reconciliation.
        arm_runtime_error()


try:
    raise_plain_replacement()
except TypeError as exc:
    assert exc is PLAIN_REPLACEMENT
    assert exc.__context__ is PLAIN_ORIGINAL
    print(
        "plain",
        type(exc).__name__,
        str(exc),
        type(exc.__context__).__name__,
        str(exc.__context__),
    )

try:
    raise_pending_replacement()
except RuntimeError as exc:
    assert str(exc) == "pending replacement"
    assert exc.__context__ is MARKED_ORIGINAL
    print(
        "marked",
        type(exc).__name__,
        str(exc),
        type(exc.__context__).__name__,
        str(exc.__context__),
    )
