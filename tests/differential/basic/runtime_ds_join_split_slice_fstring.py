"""Join, split, list slice and f-string build: edge cases of the runtime fast paths."""


def observe(label, operation):
    try:
        result = operation()
    except Exception as error:
        result = (type(error).__name__, str(error))
    print(label, repr(result))


class Text(str):
    pass


class Index:
    def __init__(self, value, action=None):
        self.value = value
        self.action = action

    def __index__(self):
        if self.action is not None:
            self.action()
        return self.value


def gen(items):
    for item in items:
        yield item


# --- join -------------------------------------------------------------------
observe("join empty list", lambda: ",".join([]))
observe("join empty tuple", lambda: ",".join(()))
observe("join one", lambda: ",".join(["solo"]))
solo = "solo value"
observe("join one identity", lambda: ",".join([solo]) is solo)
sub = Text("sub")
observe("join one subclass", lambda: (type("".join([sub])).__name__, "".join([sub])))
observe("join one subclass identity", lambda: "".join([sub]) is sub)
observe("join two", lambda: ", ".join(["a", "bc"]))
observe("join tuple", lambda: "-".join(("x", "", "yz")))
observe("join empty sep", lambda: "".join(["ab", "cd", "ef"]))
observe("join long sep", lambda: "<=>".join(["1", "2", "3"]))
observe("join unicode", lambda: "é".join(["caf", "\U0001f600", "中文"]))
observe("join repeated", lambda: "-".join(["ab"] * 9))
observe("join repeated empty", lambda: "-".join([""] * 5))
observe("join repeated empty sep", lambda: "".join(["xyz"] * 7))
same = "word"
observe("join repeated len", lambda: len("::".join([same for _ in range(1000)])))
observe("join mixed subclass", lambda: "+".join([Text("a"), "b", Text("c")]))
observe("join generator", lambda: "|".join(gen(["g1", "g2", "g3"])))
observe("join str arg", lambda: ".".join("abc"))
observe("join dict keys", lambda: ",".join({"k1": 1, "k2": 2}))
observe("join ints", lambda: ",".join([1, 2, 3]))
observe("join negative ints", lambda: ",".join([-1, -2]))
observe("join range list", lambda: ",".join(list(range(3))))
observe("join bools", lambda: ",".join([True, False]))
observe("join empty int list", lambda: ",".join(list(range(0))))
observe("join mixed", lambda: ",".join(["a", 2, "c"]))
observe("join none item", lambda: ",".join(["a", "b", None]))
observe("join bytes item", lambda: ",".join(["a", b"b"]))
observe("join single int", lambda: ",".join([7]))
observe("join single none tuple", lambda: ",".join((None,)))
observe("join non iterable", lambda: ",".join(5))
observe("join big", lambda: len(",".join([str(i) for i in range(5000)])))
parts = [str(i) for i in range(50)]
joined = " ".join(parts)
observe("join roundtrip", lambda: joined.split(" ") == parts)

# --- split ------------------------------------------------------------------
observe("split basic", lambda: "alpha,beta,gamma,delta".split(","))
observe("split repeated", lambda: "ab,ab,ab,cd,ab".split(","))
observe("split empties", lambda: ",a,,b,".split(","))
observe("split singles", lambda: "a,b,a,b".split(","))
observe("split absent", lambda: "no separator".split(","))
observe("split empty hay", lambda: "".split(","))
observe("split multi sep", lambda: "one::two::::three".split("::"))
observe("split max", lambda: "a,b,c,d".split(",", 2))
observe("split max zero", lambda: "a,b,c".split(",", 0))
observe("rsplit max", lambda: "a,b,c,d".rsplit(",", 2))
observe("split whitespace", lambda: "  many   spaced\twords\nhere ".split())
observe("split unicode", lambda: "café;\U0001f600;中文".split(";"))
observe("split long field", lambda: [len(p) for p in ("x" * 40 + "," + "x" * 40).split(",")])
many = ",".join("f%d" % (i % 12) for i in range(40))
observe("split many distinct", lambda: many.split(","))
fields = "name,value,name,value".split(",")
table = {"name": 1, "value": 2}
observe("split parts as dict keys", lambda: [table[f] for f in fields])
observe("split parts equal literals", lambda: fields[0] == "name" and fields[2] == fields[0])
observe("split parts hash", lambda: hash(fields[1]) == hash("value"))
observe("split parts in set", lambda: sorted(set(fields)))
observe("split upper", lambda: [f.upper() for f in fields])
observe("split getattr", lambda: getattr(Text("zz"), "upper")())
observe("split empty sep", lambda: "abc".split(""))
observe("split bad sep", lambda: "abc".split(1))

# --- list slices ------------------------------------------------------------
ints = list(range(10))
observe("int slice", lambda: ints[2:7])
observe("int slice empty", lambda: ints[7:2])
observe("int slice negative", lambda: ints[-4:-1])
observe("int slice open", lambda: (ints[:3], ints[7:], ints[:]))
observe("int slice clamp", lambda: ints[-100:100])
observe("int slice step", lambda: (ints[::2], ints[1::3], ints[8:1:-2], ints[::-1]))
observe("int slice big step", lambda: (ints[::100], ints[::-100], ints[3:4:7]))
copy = ints[2:5]
copy.append(99)
copy[0] = -1
observe("int slice independent", lambda: (copy, ints))
big = [1, 2**70, 3, -(2**65), 5]
observe("bigint slice", lambda: (big[1:4], big[::2]))
bools = [True, False, True, True]
observe("bool slice", lambda: (bools[1:3], bools[::-1], bools[::2]))
objs = ["a", "bb", None, 3.5, ("t",), ["l"]]
observe("obj slice", lambda: objs[1:5])
observe("obj slice step", lambda: objs[::-2])
observe("obj slice identity", lambda: objs[4:6][1] is objs[5])
observe("slice index protocol", lambda: objs[Index(1) : Index(3)])
shrink = [0, 1, 2, 3, 4, 5]
observe("slice index shrinks list", lambda: shrink[Index(1, shrink.clear) : Index(4)])
observe("after shrink", lambda: shrink)
grow = ["a", "b"]
observe("slice index grows list", lambda: grow[Index(0, lambda: grow.extend("cd")) : 10])
observe("slice bad index", lambda: objs[1.5:3])
observe("tuple slice", lambda: tuple(objs)[1:3])
observe("str slice", lambda: "café \U0001f600 ok"[2:7])


class Probe:
    live = 0

    def __init__(self):
        Probe.live += 1

    def __del__(self):
        Probe.live -= 1


probes = [Probe() for _ in range(6)]
window = probes[1:4]
del probes
observe("slice keeps items alive", lambda: Probe.live)
del window
observe("slice releases items", lambda: Probe.live)

# --- f-string build ---------------------------------------------------------
observe("fstring ints", lambda: [f"{i}:{i & 7}" for i in (0, 7, 8, -1, 123456, -987654321)])
observe("fstring extremes", lambda: f"{2**62}|{-2**62}|{2**63}|{-2**63}|{2**200}")
observe("fstring single int", lambda: (f"{42}", f"{-0}", f"{10**18}"))
observe("fstring bool", lambda: f"{True}/{False}/{1}/{0}")
observe("fstring mixed", lambda: f"<{1}|{'s'}|{2.5}|{None}|{[1, 2]}|{Text('t')}>")
nine = (1, 22, 333, 4444, 55555, 666666, 7777777, 88888888, 999999999)
observe("fstring nine ints", lambda: f"{nine[0]}{nine[1]}{nine[2]}{nine[3]}{nine[4]}{nine[5]}{nine[6]}{nine[7]}{nine[8]}")
wide = 10**18
observe(
    "fstring many wide ints",
    lambda: f"{wide},{-wide},{wide},{-wide},{wide},{-wide},{wide},{-wide},{wide},{-wide},{wide},{-wide}",
)
observe(
    "fstring twenty parts",
    lambda: f"{1}a{2}b{3}c{4}d{5}e{6}f{7}g{8}h{9}i{10}j{11}k{12}l{'x'}m{None}n",
)


class Loud:
    def __str__(self):
        raise ValueError("no str")


observe("fstring raising str", lambda: f"{1}-{'kept'}-{Loud()}-{2}")
built = []
for i in range(200):
    built.append(f"{i}:{i & 7}")
observe("fstring build join", lambda: (len(",".join(built)), built[0], built[-1]))

# --- leaf objects die and are reused ----------------------------------------
total = 0
for i in range(20000):
    s = f"row{i}".upper()
    b = s.encode()
    f = i * 1.5
    n = (1 << 70) + i
    xs = [i, i + 1][0:1]
    total += len(s) + len(b) + int(f) + (n & 1) + xs[0]
observe("leaf churn", lambda: total)
