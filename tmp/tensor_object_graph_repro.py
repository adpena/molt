from molt.gpu.tensor import zeros

class Linear:
    def __init__(self, weight):
        self.weight = weight

class Block:
    def __init__(self, idx):
        self.idx = idx
        self.attn = Linear(zeros(8))
        self.ffn = Linear(zeros(8))

class Model:
    def __init__(self):
        self.layers = []
        for i in range(22):
            self.layers.append(Block(i))
        self.out = Linear(zeros(8))

m = Model()
print(type(m).__name__)
print(len(m.layers))
print(type(m.layers[0].attn.weight).__name__)
