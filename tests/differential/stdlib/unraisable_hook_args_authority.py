"""Purpose: exact internal UnraisableHookArgs struct-sequence authority."""

import atexit
import sys


captured: list[object] = []


def boom() -> None:
    raise RuntimeError("boom")


old_hook = sys.unraisablehook
atexit._clear()
atexit.register(boom)
sys.unraisablehook = captured.append
atexit._run_exitfuncs()
sys.unraisablehook = old_hook

args = captured[0]
args_type = type(args)
print("not-exported", not hasattr(sys, "UnraisableHookArgs"))
print("type", args_type.__name__, args_type.__module__)
print("sequence", isinstance(args, tuple), len(args), tuple(args)[:2])
print(
    "metadata",
    args_type.n_fields,
    args_type.n_sequence_fields,
    args_type.n_unnamed_fields,
)
print("match-args", args_type.__match_args__)
print(
    "attributes",
    args.exc_type is RuntimeError,
    isinstance(args.exc_value, RuntimeError),
    args.exc_traceback is not None,
    hasattr(args, "err_msg"),
    hasattr(args, "object"),
)

simple = args_type((1, 2, None, "message", "object"))
print("constructed", repr(simple), tuple(simple))
print(
    "constructed-empty-fields",
    args_type((1, 2, None, "message", "object"), {}) == simple,
)
try:
    args_type((1, 2, None, "message", "object"), {"object": "duplicate"})
except BaseException as exc:
    print("reject-fields", type(exc).__name__, str(exc))
try:
    simple.err_msg = "changed"
except BaseException as exc:
    print("immutable", type(exc).__name__)
try:
    type("UnraisableHookArgsSubclass", (args_type,), {})
except BaseException as exc:
    print("reject-subclass", type(exc).__name__, str(exc))


class UnraisableHookArgs(tuple):
    exc_type = RuntimeError
    exc_value = RuntimeError("forged")
    exc_traceback = None
    err_msg = None
    object = None


UnraisableHookArgs.__module__ = "builtins"
for value in ((), UnraisableHookArgs()):
    try:
        sys.__unraisablehook__(value)
    except BaseException as exc:
        print("reject-nonexact", type(exc).__name__, str(exc))
