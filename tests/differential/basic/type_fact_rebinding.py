"""Assignment inference must not become a declaration for an entire scope."""

module_value = 7
module_value = "module rebound"


def choose(flag):
    value = 3
    if flag:
        value = "branch rebound"
    return value


def containers():
    value = [1, 2]
    value = {"answer": 3}
    return value["answer"]


def annotated_sum(count: int) -> int:
    total: int = 0
    for index in range(count):
        total += index
    return total


print(module_value)
print(choose(False), choose(True))
print(containers())
print(annotated_sum(5))
