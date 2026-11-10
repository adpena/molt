"""Code literals survive branches, alias deletion, rebinding and exception exits."""

def choose(flag):
    text = "literal-identity-\u03bb-\x00-tail"
    data = b"literal-identity-\x00-\xff-tail"
    integer = 123456789012345678901234567890123456789
    if flag:
        other_text = "literal-identity-\u03bb-\x00-tail"
        other_data = b"literal-identity-\x00-\xff-tail"
        other_integer = 123456789012345678901234567890123456789
    else:
        other_text = text
        other_data = data
        other_integer = integer
    saved = [text, data, integer]
    print("identity", flag, text is other_text, data is other_data, integer is other_integer)
    del text, data, integer
    print("alive", other_text == saved[0], other_data == saved[1], other_integer == saved[2])
    other_text = "replacement"
    other_data = b"replacement"
    other_integer = -123456789012345678901234567890123456789
    print("rebound", saved[0], saved[1], saved[2], other_text, other_data, other_integer)
    return saved


def exceptions(flag):
    first = b"exception-literal-\x00-\xfe"
    values = [first]
    try:
        if flag:
            repeated = b"exception-literal-\x00-\xfe"
            values.append(repeated)
            raise ValueError("literal-error")
        values.append(b"exception-literal-\x00-\xfe")
    except ValueError as error:
        print("caught", str(error), values[0] is values[1])
    finally:
        del first
        print("finally", values[0], values[1], values[0] is values[1])
    return values


for condition in (False, True):
    escaped = choose(condition)
    observed = exceptions(condition)
    print("escaped", escaped, observed)


def repeat_literals():
    strings = []
    buffers = []
    integers = []
    for index in range(4):
        strings.append("repeated-loop-\u03bb-\x00")
        buffers.append(b"repeated-loop-\xff-\x00")
        integers.append(987654321098765432109876543210987654321)
    print("loop identity", strings[0] is strings[-1], buffers[0] is buffers[-1], integers[0] is integers[-1])
    print("loop values", strings, buffers, integers)


repeat_literals()
