"""Purpose: differential coverage for mutable sys runtime state."""

import sys


def trace_func(frame, event, arg):
    return None


def profile_func(frame, event, arg):
    return None


old_trace = sys.gettrace()
old_profile = sys.getprofile()
old_switch = sys.getswitchinterval()
old_digits = sys.get_int_max_str_digits()

events = []


def audit_hook(event, args):
    if event == "molt.audit.check":
        events.append((event, args))


try:
    sys.settrace(trace_func)
    print("trace same:", sys.gettrace() is trace_func)
    sys.settrace(None)
    print("trace none:", sys.gettrace() is None)

    sys.setprofile(profile_func)
    print("profile same:", sys.getprofile() is profile_func)
    sys.setprofile(None)
    print("profile none:", sys.getprofile() is None)

    sys.setswitchinterval(0.02)
    print("switch stored:", sys.getswitchinterval() == 0.02)

    sys.set_int_max_str_digits(0)
    print("digits zero:", sys.get_int_max_str_digits())
    try:
        sys.set_int_max_str_digits(10)
    except ValueError as exc:
        print("digits invalid:", type(exc).__name__, "maxdigits" in str(exc))

    sys.addaudithook(audit_hook)
    sys.audit("molt.audit.check", 7, "x")
    print("audit events:", events)
finally:
    sys.settrace(old_trace)
    sys.setprofile(old_profile)
    sys.setswitchinterval(old_switch)
    sys.set_int_max_str_digits(old_digits)
