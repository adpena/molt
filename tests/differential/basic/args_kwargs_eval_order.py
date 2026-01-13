events = []


def tag(label):
    events.append(label)
    return label


def f(*args, **kwargs):
    return (args, kwargs)


f(a=tag("kw_a"), *[tag("star_a")])
print(events)

events = []
f(tag("pos"), b=tag("kw_b"), *[tag("star_b")], **{"c": tag("kw_c")}, d=tag("kw_d"))
print(events)
