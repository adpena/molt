import re

pattern = re.compile("FOO", re.IGNORECASE)
parser = re._Parser("FOO")
node, groups = parser.parse()


def my_match_node(node, text, pos, end, origin, groups, flags):
    if isinstance(node, re._Empty):
        yield pos, groups
        return
    if isinstance(node, re._Literal):
        length = len(node.text)
        if pos + length <= end:
            segment = text[pos : pos + length]
            if flags & re.IGNORECASE:
                if re._casefold(segment) == re._casefold(node.text):
                    yield pos + length, groups
            else:
                if segment == node.text:
                    yield pos + length, groups
        return
    raise re.error("unsupported pattern node")


print(
    "builtin_match",
    list(re._match_node(node, "foo", 0, len("foo"), 0, (None,), pattern.flags)),
)
print(
    "custom_match",
    list(my_match_node(node, "foo", 0, len("foo"), 0, (None,), pattern.flags)),
)
