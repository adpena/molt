# MOLT_META: stderr=exact
"""Differential coverage for loop exits and their enclosing lexical regions."""

values = []
for i in range(5):
    if i == 2:
        break
    values.append(i)
print(values)

values = []
for i in range(5):
    if i == 2:
        continue
    values.append(i)
print(values)

i = 0
while True:
    i += 1
    if i == 3:
        break
print(i)

i = 0
total = 0
while i < 5:
    i += 1
    if i % 2 == 0:
        continue
    total += i
print(total)


def terminal_loop(enabled, values):
    if not enabled:
        return "disabled"
    else:
        for value in values:
            return ("found", value)
        return "empty"


def nested_terminal_loop(groups):
    for group in groups:
        for value in group:
            if value < 0:
                break
            return value
    return "empty"


for enabled in (False, True):
    for items in ([], [7, 8]):
        print("terminal", enabled, items, terminal_loop(enabled, items))
for groups in ([], [[]], [[-1], [9]], [[5, 6]]):
    print("nested-terminal", groups, nested_terminal_loop(groups))
