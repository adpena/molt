"""
microGPT inference — pure Python, no dependencies beyond math and random.
Designed to run on Cloudflare Workers via molt (Python-to-WASM).

Architecture: 1-layer GPT with 4-head attention, 16-dim embeddings.
Total params: ~4192. Inference only — no autograd, no Value class.
All operations on plain list[float].
"""

import math
import random
import json

# ---------------------------------------------------------------------------
# Model hyperparameters (must match training)
# ---------------------------------------------------------------------------
N_LAYER: int = 1
N_EMBD: int = 16
BLOCK_SIZE: int = 16
N_HEAD: int = 4
HEAD_DIM: int = N_EMBD // N_HEAD


# ---------------------------------------------------------------------------
# Core operations on list[float]
# ---------------------------------------------------------------------------
def linear(x: list[float], w: list[list[float]]) -> list[float]:
    """Matrix-vector multiply: w @ x, where w is (out, in)."""
    out: list[float] = []
    for row in w:
        s: float = 0.0
        for i in range(len(x)):
            s += row[i] * x[i]
        out.append(s)
    return out


def softmax(logits: list[float]) -> list[float]:
    """Numerically stable softmax over a list of floats."""
    max_val: float = logits[0]
    for v in logits:
        if v > max_val:
            max_val = v
    exps: list[float] = []
    total: float = 0.0
    for v in logits:
        e: float = math.exp(v - max_val)
        exps.append(e)
        total += e
    return [e / total for e in exps]


def rmsnorm(x: list[float]) -> list[float]:
    """Root-mean-square layer normalization."""
    n: int = len(x)
    ms: float = 0.0
    for v in x:
        ms += v * v
    ms = ms / n
    scale: float = 1.0 / math.sqrt(ms + 1e-5)
    return [v * scale for v in x]


def relu(x: list[float]) -> list[float]:
    """Element-wise ReLU."""
    return [v if v > 0.0 else 0.0 for v in x]


# ---------------------------------------------------------------------------
# GPT forward pass — single token, with KV cache
# ---------------------------------------------------------------------------
def gpt(
    token: int,
    pos: int,
    keys: list[list[list[float]]],
    values: list[list[list[float]]],
    sd: dict[str, list[list[float]]],
) -> list[float]:
    """Run one forward step of the GPT, returning logits."""
    # Token + positional embedding
    tok_emb: list[float] = sd["wte"][token]
    pos_emb: list[float] = sd["wpe"][pos]
    x: list[float] = [t + p for t, p in zip(tok_emb, pos_emb)]
    x = rmsnorm(x)

    for li in range(N_LAYER):
        x_res: list[float] = x
        x = rmsnorm(x)

        # QKV projections
        q: list[float] = linear(x, sd["layer" + str(li) + ".attn_wq"])
        k: list[float] = linear(x, sd["layer" + str(li) + ".attn_wk"])
        v: list[float] = linear(x, sd["layer" + str(li) + ".attn_wv"])
        keys[li].append(k)
        values[li].append(v)

        # Multi-head attention
        x_attn: list[float] = []
        for h in range(N_HEAD):
            hs: int = h * HEAD_DIM
            he: int = hs + HEAD_DIM
            q_h: list[float] = q[hs:he]

            # Attention scores against all cached keys
            seq_len: int = len(keys[li])
            attn_logits: list[float] = []
            inv_scale: float = 1.0 / math.sqrt(float(HEAD_DIM))
            for t in range(seq_len):
                dot: float = 0.0
                for j in range(HEAD_DIM):
                    dot += q_h[j] * keys[li][t][hs + j]
                attn_logits.append(dot * inv_scale)

            attn_w: list[float] = softmax(attn_logits)

            # Weighted sum of values
            for j in range(HEAD_DIM):
                s: float = 0.0
                for t in range(seq_len):
                    s += attn_w[t] * values[li][t][hs + j]
                x_attn.append(s)

        x = linear(x_attn, sd["layer" + str(li) + ".attn_wo"])
        x = [a + b for a, b in zip(x, x_res)]

        # MLP block
        x_res = x
        x = rmsnorm(x)
        x = linear(x, sd["layer" + str(li) + ".mlp_fc1"])
        x = relu(x)
        x = linear(x, sd["layer" + str(li) + ".mlp_fc2"])
        x = [a + b for a, b in zip(x, x_res)]

    logits: list[float] = linear(x, sd["lm_head"])
    return logits


# ---------------------------------------------------------------------------
# Load weights and generate
# ---------------------------------------------------------------------------
def load_weights(path: str) -> tuple[dict[str, list[list[float]]], list[str], int]:
    """Load weights from JSON, return (state_dict, uchars, vocab_size)."""
    with open(path, "r") as f:
        raw: dict = json.load(f)
    meta: dict = raw["_meta"]
    uchars: list[str] = meta["uchars"]
    vocab_size: int = meta["vocab_size"]
    sd: dict[str, list[list[float]]] = {}
    for k in raw:
        if k != "_meta":
            sd[k] = raw[k]
    return sd, uchars, vocab_size


def generate(
    sd: dict[str, list[list[float]]],
    uchars: list[str],
    vocab_size: int,
    temperature: float = 0.5,
    num_samples: int = 10,
) -> list[str]:
    """Generate names from the trained model."""
    bos: int = vocab_size - 1
    results: list[str] = []

    for _ in range(num_samples):
        keys: list[list[list[float]]] = [[] for _ in range(N_LAYER)]
        values: list[list[list[float]]] = [[] for _ in range(N_LAYER)]
        token: int = bos
        chars: list[str] = []

        for pos in range(BLOCK_SIZE):
            logits: list[float] = gpt(token, pos, keys, values, sd)
            scaled: list[float] = [l / temperature for l in logits]
            probs: list[float] = softmax(scaled)
            population: list[int] = list(range(vocab_size))
            token = random.choices(population, weights=probs)[0]
            if token == bos:
                break
            chars.append(uchars[token])

        results.append("".join(chars))
    return results


def main() -> None:
    random.seed(1337)
    # Try bundle path first (WASM), then local path
    path: str = "weights.json"
    try:
        sd, uchars, vocab_size = load_weights("/bundle/weights.json")
        path = "/bundle/weights.json"
    except Exception:
        sd, uchars, vocab_size = load_weights("examples/microgpt/weights.json")
        path = "examples/microgpt/weights.json"

    print("microGPT inference")
    print("weights: " + path)
    print("vocab: " + "".join(uchars))
    print("---")

    names: list[str] = generate(sd, uchars, vocab_size)
    for i in range(len(names)):
        print("  " + str(i + 1) + ": " + names[i])


main()
