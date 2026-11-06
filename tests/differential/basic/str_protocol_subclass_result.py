"""str's object protocol preserves an overriding str subclass result."""


class Text(str):
    def lower(self):
        return "override-lower"

    def __len__(self):
        return 41

    def __add__(self, other):
        return "override-add"

    def __hash__(self):
        return 73

    def __contains__(self, other):
        return False

    def __format__(self, spec):
        return "override-format:" + spec


text = Text("payload")


class Source:
    def __str__(self):
        return text

    def __repr__(self):
        return text


source = Source()


def inspect_conversion(value):
    result = str(value)
    print(type(result) is Text, result is text)
    print(result.lower(), len(result), result + "!", hash(result), "pay" in result)
    print(str(value).lower(), len(str(value)), str(value) + "!")
    print(type(f"{value}") is Text, f"{value}".lower())
    print(f"{value!s}", f"{value!s:tag}")
    print(type(repr(value)) is Text, repr(value).lower(), f"{value!r}")
    print(type(ascii(value)) is Text, ascii(value).lower(), f"{value!a}")
    print(type(format(value)) is Text, format(value).lower())
    print("%s" % value, "{}".format(value), "{!s}".format(value))


inspect_conversion(source)
result = str(source)
print(type(result) is Text, result is text, result.lower(), len(result))
print(str(source).lower(), len(str(source)), str(source) + "!")
# Builtin str's own __str__ normalizes a subclass; the constructor does not
# normalize a different object's user-defined __str__ result a second time.
print(type(str(text)) is str, str(text).lower(), len(str(text)))
print(type(str("exact")) is str, str("EXACT").lower())
print(type(str(123)) is str, str(123), type(str(True)) is str)
