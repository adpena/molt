"""Purpose: differential coverage for string format errors."""
# ruff: noqa: F521, F523, F524


def _print_error(label: str, func) -> None:
    try:
        func()
    except Exception as exc:  # noqa: BLE001
        print(label, type(exc).__name__, exc)


def _print_result(label: str, func) -> None:
    try:
        print(label, repr(func()))
    except Exception as exc:  # noqa: BLE001
        print(label, type(exc).__name__, exc)


def _template_contract() -> None:
    from string import Template

    for label, text in (
        ("bare", "$"),
        ("invalid_digit", "$9"),
        ("invalid_punctuation", "prefix $- suffix"),
        ("empty_brace", "${}"),
        ("unclosed_brace", "${name"),
        ("partial_identifier", "${a-b}"),
        ("nested_placeholder", "${bad$name}"),
        ("unicode_identifier", "${caf\u00e9}"),
        ("unicode_literal", "\u00e9\U0001f980 $-"),
        ("multiline", "first\r\n\u00e9 $-"),
        ("line_separator", "first\u2028 $-"),
    ):
        template = Template(text)
        _print_result(label + "_strict", lambda: template.substitute(name="N"))
        _print_result(label + "_safe", lambda: template.safe_substitute(name="N"))
        print(label + "_valid", template.is_valid(), template.get_identifiers())

    escaped = Template("$$/$name/${name}/$$/$_A9/$caf\u00e9")
    print("escaped", escaped.substitute(name="N", _A9="A", caf="C"))
    print("escaped_ids", escaped.get_identifiers())
    _print_result("missing_strict", lambda: Template("$missing").substitute({}))
    print("missing_safe", Template("${missing}").safe_substitute({}))
    print(
        "keywords_first", Template("$x/$y").substitute({"x": "map", "y": "Y"}, x="kw")
    )

    class MappingFailure(Exception):
        pass

    class Lookup:
        def __init__(self, events, error=None, value="VALUE"):
            self.events = events
            self.error = error
            self.value = value

        def __getitem__(self, key):
            self.events.append("get:" + key)
            if self.error is not None:
                raise self.error
            return self.value

    class StringFailure:
        def __init__(self, events, error):
            self.events = events
            self.error = error

        def __str__(self):
            self.events.append("str")
            raise self.error

    for safe in (False, True):
        for where in ("lookup", "str"):
            for kind in ("key", "other"):
                events = []
                error = (
                    KeyError("sentinel")
                    if kind == "key"
                    else MappingFailure("sentinel")
                )
                mapping = (
                    Lookup(events, error)
                    if where == "lookup"
                    else Lookup(events, value=StringFailure(events, error))
                )
                try:
                    template = Template("${value}")
                    result = (
                        template.safe_substitute(mapping)
                        if safe
                        else template.substitute(mapping)
                    )
                    print("callback", safe, where, kind, repr(result), events)
                except Exception as exc:  # noqa: BLE001
                    print(
                        "callback",
                        safe,
                        where,
                        kind,
                        type(exc).__name__,
                        exc is error,
                        events,
                    )

    events = []
    _print_result(
        "invalid_before_lookup",
        lambda: Template("$-/$value").substitute(Lookup(events)),
    )
    print("invalid_before_lookup_calls", events)
    events = []
    missing = KeyError("before-syntax")
    try:
        Template("$value/$-").substitute(Lookup(events, missing))
    except Exception as exc:  # noqa: BLE001
        print("lookup_before_invalid", type(exc).__name__, exc is missing, events)

    class UnicodeDelimiter(Template):
        delimiter = "\u00a7"

    class MultiDelimiter(Template):
        delimiter = "::"

    class MultiUnicodeDelimiter(Template):
        delimiter = "\u203b\u203b"

    class EmptyDelimiter(Template):
        delimiter = ""

    for cls in (UnicodeDelimiter, MultiDelimiter, MultiUnicodeDelimiter):
        delimiter = cls.delimiter
        text = "\u00e9/" + delimiter * 2 + "/" + delimiter + "{item}/" + delimiter + "?"
        template = cls(text)
        _print_result(cls.__name__ + "_strict", lambda: template.substitute(item="OK"))
        print(cls.__name__ + "_safe", template.safe_substitute(item="OK"))
        print(cls.__name__ + "_ids", template.is_valid(), template.get_identifiers())
    empty = EmptyDelimiter("\u00e9 $name ${bad}")
    print(
        "empty_delimiter",
        empty.substitute(name="N", bad="B"),
        empty.is_valid(),
        empty.get_identifiers(),
    )
    _print_result("empty_missing_strict", lambda: empty.substitute(name="N"))
    print("empty_missing_safe", empty.safe_substitute(name="N"))
    print("empty_text", EmptyDelimiter("").substitute())

    class MutatingDelimiter:
        def __init__(self, template, events):
            self.template = template
            self.events = events

        def __getitem__(self, key):
            self.events.append(key)
            self.template.delimiter = "|"
            return "VALUE"

    for safe in (False, True):
        template = EmptyDelimiter("a!")
        events = []
        mapping = MutatingDelimiter(template, events)
        result = (
            template.safe_substitute(mapping) if safe else template.substitute(mapping)
        )
        print("empty_escape_mutation", safe, repr(result), events)

    repeated = Template("$v " * 512 + "${v}")
    print("repeated", repeated.get_identifiers(), len(repeated.substitute(v="x")))
    identifiers = Template(" ".join("$v" + str(index) for index in range(256)))
    names = identifiers.get_identifiers()
    print("ordered_identifiers", len(names), names[0], names[-1])
    unclosed = Template("${" * 1024 + "$x")
    print(
        "repeated_unclosed",
        unclosed.is_valid(),
        unclosed.get_identifiers(),
        len(unclosed.safe_substitute(x="X")),
    )


def main() -> None:
    _print_error("single_open", lambda: "{".format(1))
    _print_error("single_close", lambda: "}".format(1))
    _print_error("unclosed", lambda: "{0".format(1))
    _print_error("bad_conv", lambda: "{0!x}".format(1))
    _print_error("no_args", lambda: "{0}".format())
    _print_error("missing_kw", lambda: "{foo}".format())
    _print_error("bad_follow", lambda: "{0[1]x}".format([1, 2]))
    _print_error("empty_attr", lambda: "{0.}".format(object()))
    _print_error("big_index", lambda: "{999999999999999999999999999999}".format(1))
    _print_error("spec_unmatched", lambda: "{0:{".format(1))
    _print_error("conv_unmatched", lambda: "{!}".format(1))
    _template_contract()


if __name__ == "__main__":
    main()
