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

    class DigitsIndex:
        def __index__(self):
            return 700

    for digits in [DigitsIndex(), 2**31 - 1, 2**31, 2**63, 2**80, -(2**40), 700.0]:
        before = sys.get_int_max_str_digits()
        try:
            sys.set_int_max_str_digits(digits)
            print("digits index:", sys.get_int_max_str_digits())
        except (TypeError, OverflowError) as exc:
            print("digits range:", type(exc).__name__, sys.get_int_max_str_digits() == before)

    sys.addaudithook(audit_hook)
    sys.audit("molt.audit.check", 7, "x")
    print("audit events:", events)
finally:
    sys.settrace(old_trace)
    sys.setprofile(old_profile)
    sys.setswitchinterval(old_switch)
    sys.set_int_max_str_digits(old_digits)

# Integer output shares this runtime's live policy. Compiler literals and
# nondecimal conversions do not acquire the decimal conversion limit.
import csv
import io
import pprint
import urllib.parse


def digit_failure(label, action):
    try:
        action()
        print(label, "accepted")
    except ValueError as exc:
        print(label, type(exc).__name__, str(exc))


old_digits = sys.get_int_max_str_digits()
try:
    sys.set_int_max_str_digits(640)
    allowed = 10**639
    rejected = 10**640
    print("decimal boundary", len(str(allowed)), len(str(-allowed)))
    for label, action in [
        ("str", lambda: str(rejected)),
        ("repr", lambda: repr(-rejected)),
        ("format-empty", lambda: format(rejected, "")),
        ("format-d", lambda: format(rejected, "+0800,d")),
        ("format-n", lambda: format(rejected, "n")),
        ("percent-d", lambda: "%d" % rejected),
        ("percent-i", lambda: "%i" % rejected),
        ("percent-u", lambda: "%u" % rejected),
        ("percent-s", lambda: "%s" % rejected),
        ("percent-r", lambda: "%r" % rejected),
        ("percent-a", lambda: "%a" % rejected),
        ("list", lambda: repr([rejected])),
        ("tuple", lambda: repr((rejected,))),
        ("dict", lambda: repr({"n": rejected})),
        ("set", lambda: repr({rejected})),
        ("range", lambda: repr(range(0, rejected))),
        ("exception", lambda: str(ValueError(rejected))),
        ("pprint", lambda: pprint.pformat([rejected])),
    ]:
        digit_failure(label, action)
    # Format admission precedes decimal conversion even for oversized values.
    for spec in [".1d", "zd", "q"]:
        digit_failure("specifier precedence " + spec, lambda: format(rejected, spec))
    try:
        "%.*d" % (2147483647, rejected)
    except (ValueError, OverflowError) as exc:
        print("percent precision precedence", type(exc).__name__, str(exc))
    print("padding excluded", len(format(7, "0800d")), len("%.800d" % 7))
    print("grouping excluded", len(format(allowed, ",d")), len(format(-allowed, "_d")))
    print("nondecimal exempt", int(format(rejected, "x"), 16) == rejected,
          int("%o" % rejected, 8) == rejected,
          int(format(-rejected, "b"), 2) == -rejected)

    class IntChangesLimit:
        def __init__(self, limit):
            self.limit = limit

        def __int__(self):
            sys.set_int_max_str_digits(self.limit)
            return rejected

    print("limit after conversion", len("%d" % IntChangesLimit(700)))
    digit_failure("lowered during conversion", lambda: "%d" % IntChangesLimit(640))

    events = []

    class LaterText:
        def __str__(self):
            events.append("later")
            return "later"

    output = io.StringIO()
    digit_failure("print failure", lambda: print(rejected, file=output))
    print("print output empty", output.getvalue() == "")
    writer = csv.writer(output)
    digit_failure("csv failure", lambda: writer.writerow([rejected, LaterText()]))
    print("csv output empty", output.getvalue() == "", events)
    digit_failure("urlencode failure", lambda: urllib.parse.urlencode([("n", rejected), ("later", LaterText())]))
    print("urlencode callback order", events)
    sys.set_int_max_str_digits(0)
    print("disabled output", len(str(rejected)))
    sys.set_int_max_str_digits(640)
    digit_failure("reenabled output", lambda: str(rejected))
finally:
    sys.set_int_max_str_digits(old_digits)
