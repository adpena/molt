import random

rng = random.Random(123)
print("randbytes", rng.randbytes(4).hex())

random.seed(123)
print("module", random.randbytes(4).hex())
