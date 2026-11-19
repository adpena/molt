"""Purpose: CPython parity for `%` string-formatting diagnostics on every target.

CPython 3.15 rewrote them (Objects/unicode_format.c): conversion errors name
their argument, count errors give the counts and parse errors give positions.
"""


class Box:
    pass


cases = [
 ('"%(a)s %s" % {"a": 1}', lambda: "%(a)s %s" % {"a": 1}),
 ('"%s %(a)s" % {"a": 1}', lambda: "%s %(a)s" % {"a": 1}),
 ('"%(a)*d" % {"a": 1}', lambda: "%(a)*d" % {"a": 1}),
 ('"%(a" % {"a": 1}', lambda: "%(a" % {"a": 1}),
 ('"%" % ()', lambda: "%" % ()),
 ('"abc %" % ()', lambda: "abc %" % ()),
 ('"%q" % 1', lambda: "%q" % 1),
 ('"%\'" % 1', lambda: "%'" % 1),
 ('"%é" % 1', lambda: "%é" % 1),
 ('"%\\u20ac" % 1', lambda: "%€" % 1),
 ('"%s %s" % (1,)', lambda: "%s %s" % (1,)),
 ('"%s" % (1, 2)', lambda: "%s" % (1, 2)),
 ('"%d" % "x"', lambda: "%d" % "x"),
 ('"%d %d" % (1, "x")', lambda: "%d %d" % (1, "x")),
 ('"%(k)d" % {"k": "x"}', lambda: "%(k)d" % {"k": "x"}),
 ('"%f" % "x"', lambda: "%f" % "x"),
 ('"%x" % 1.5', lambda: "%x" % 1.5),
 ('"%c" % "ab"', lambda: "%c" % "ab"),
 ('"%c" % -1', lambda: "%c" % -1),
 ('"%*d" % ("x", 1)', lambda: "%*d" % ("x", 1)),
 ('"%.*d" % ("x", 1)', lambda: "%.*d" % ("x", 1)),
 ('"%*d" % (1 << 70, 1)', lambda: "%*d" % (1 << 70, 1)),
 ('"%.*d" % (1 << 40, 1)', lambda: "%.*d" % (1 << 40, 1)),
 ('"%99999999999999999999d" % 1', lambda: "%99999999999999999999d" % 1),
 ('"%.99999999999d" % 1', lambda: "%.99999999999d" % 1),
 ('"%(a)s" % (1,)', lambda: "%(a)s" % (1,)),
 ('"%(a)s" % 1', lambda: "%(a)s" % 1),
 ('"%s" % {}', lambda: "%s" % {}),
 ('"x" % {"a": 1}', lambda: "x" % {"a": 1}),
 ('"x" % 1', lambda: "x" % 1),
 ('"%s %s" % [1]', lambda: "%s %s" % [1]),
 ('"%5" % 1', lambda: "%5" % 1),
 ('"%.5" % 1', lambda: "%.5" % 1),
 ('"%l" % 1', lambda: "%l" % 1),
 ('"%i" % None', lambda: "%i" % None),
 ('"%o" % "s"', lambda: "%o" % "s"),
 ('"%c" % 1.5', lambda: "%c" % 1.5),
 ('"%c" % (1 << 70)', lambda: "%c" % (1 << 70)),
 ('"%a %r" % (1,)', lambda: "%a %r" % (1,)),
]
for label, f in cases:
    try: r = f(); print(label, "->", repr(r))
    except Exception as e: print(label, "!", type(e).__name__, e)
more = [
 ('"%d" % Box()', lambda: "%d" % Box()),
 ('"%(k)x" % {"k": Box()}', lambda: "%(k)x" % {"k": Box()}),
 ('"%s %*d" % (1, Box(), 2)', lambda: "%s %*d" % (1, Box(), 2)),
 ('"%c" % "\\ud800x"', lambda: "%c" % "\ud800x"),
 ('"%(a)s %(b)d" % {"a": 1, "b": "x"}', lambda: "%(a)s %(b)d" % {"a": 1, "b": "x"}),
 ('"%*d" % 5', lambda: "%*d" % 5),
 ('"%(a)s %.*d" % {"a": 1}', lambda: "%(a)s %.*d" % {"a": 1}),
 ('"%\\x07" % 1', lambda: "%\x07" % 1),
 ('"ab%(k" % {}', lambda: "ab%(k" % {}),
 ('"é%q" % 1', lambda: "é%q" % 1),
]
for label, f in more:
    try: r = f(); print(label, "->", repr(r))
    except Exception as e: print(label, "!", type(e).__name__, e)
