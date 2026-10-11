import colorsys


def fmt_triplet(values):
    return tuple(round(v, 6) for v in values)


print("rgb_to_hls_red", fmt_triplet(colorsys.rgb_to_hls(1.0, 0.0, 0.0)))
print("rgb_to_hls_gray", fmt_triplet(colorsys.rgb_to_hls(0.25, 0.25, 0.25)))
print("hls_to_rgb_red", fmt_triplet(colorsys.hls_to_rgb(0.0, 0.5, 1.0)))
print("hls_to_rgb_wrap", fmt_triplet(colorsys.hls_to_rgb(1.2, 0.5, 0.5)))

print("rgb_to_hsv_sample", fmt_triplet(colorsys.rgb_to_hsv(0.2, 0.4, 0.6)))
print("hsv_to_rgb_sample", fmt_triplet(colorsys.hsv_to_rgb(0.5833333333, 0.6666666667, 0.6)))
print("hsv_to_rgb_wrap", fmt_triplet(colorsys.hsv_to_rgb(-0.1, 0.5, 0.75)))

print("rgb_to_yiq_sample", fmt_triplet(colorsys.rgb_to_yiq(0.2, 0.4, 0.6)))
print("yiq_to_rgb_sample", fmt_triplet(colorsys.yiq_to_rgb(0.3, 0.1, -0.1)))

# CPython returns the original brightness/lightness at zero saturation. No
# numeric conversion of either that object or the unused hue is permitted.
marker = object()
for operation, arguments in (
    (colorsys.hls_to_rgb, (object(), marker, 0)),
    (colorsys.hsv_to_rgb, (object(), 0, marker)),
):
    result = operation(*arguments)
    assert all(value is marker for value in result)
    print(operation.__name__, "identity", len(result))

assert tuple(type(value).__name__ for value in colorsys.hsv_to_rgb(0, 0, 7)) == (
    "int", "int", "int"
)
assert type(colorsys.rgb_to_hsv(1, 1, 1)[2]) is int
print("integer output types", colorsys.hsv_to_rgb(0, 0, 7))

# A traceable arithmetic carrier exposes the exact public expression order.
# It deliberately has no __float__: the removed eager coercion cannot pass.
events = []


class Trace:
    def __init__(self, name):
        self.name = name

    def __rmul__(self, other):
        name = "(" + str(other) + "*" + self.name + ")"
        events.append(name)
        return Trace(name)

    def __add__(self, other):
        name = "(" + self.name + "+" + other.name + ")"
        events.append(name)
        return Trace(name)

    def __sub__(self, other):
        name = "(" + self.name + "-" + other.name + ")"
        events.append(name)
        return Trace(name)


result = colorsys.rgb_to_yiq(Trace("r"), Trace("g"), Trace("b"))
assert events[:5] == [
    "(0.3*r)", "(0.59*g)", "((0.3*r)+(0.59*g))",
    "(0.11*b)", "(((0.3*r)+(0.59*g))+(0.11*b))",
]
assert len(events) == 15
assert all(type(value) is Trace for value in result)
print("yiq arithmetic", events)
