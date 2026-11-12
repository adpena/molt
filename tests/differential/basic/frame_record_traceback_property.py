"""Purpose: seeded random call shapes produce the same traceback entries and caller chains as CPython."""

import sys
import traceback


class Rng:
    def __init__(self, seed):
        self.state = seed

    def below(self, bound):
        self.state = (self.state * 1103515245 + 12345) % 2147483648
        return self.state % bound


def chain_names():
    names = []
    # Up to this program's own entry: what runs the module is the host's.
    frame = sys._getframe(1)
    while frame is not None and frame.f_code.co_name != "run":
        names.append(frame.f_code.co_name)
        frame = frame.f_back
    return names


def alpha(rng, depth):
    marker = depth
    if depth == 0:
        return finish(rng)
    return step(rng, depth - 1) + marker - marker


def beta(rng, depth):
    held = [depth]
    if depth == 0:
        return finish(rng)
    value = step(rng, depth - 1)
    return value + len(held) - 1


def gamma(rng, depth):
    if depth == 0:
        return finish(rng)
    try:
        return step(rng, depth - 1)
    finally:
        depth = None


def step(rng, depth):
    return (alpha, beta, gamma)[rng.below(3)](rng, depth)


def finish(rng):
    choice = rng.below(4)
    if choice == 0:
        raise ValueError("value " + str(len(chain_names())))
    if choice == 1:
        return [][rng.below(3)]
    if choice == 2:
        return {}["missing"]
    return len(chain_names())


def run(seed):
    rng = Rng(seed)
    depth = 1 + rng.below(40)
    try:
        print("seed", seed, "depth", depth, "returned", step(rng, depth))
    except Exception as exc:
        print("seed", seed, "depth", depth, type(exc).__name__, exc)
        for entry in traceback.extract_tb(exc.__traceback__):
            print(" ", entry.name, entry.lineno)
        frames = []
        tb = exc.__traceback__
        while tb is not None:
            frames.append((tb.tb_frame.f_code.co_name, tb.tb_lineno))
            tb = tb.tb_next
        print("frames", len(frames), frames[-1])

if __name__ == "__main__":
    for seed in range(1, 25):
        run(seed)
