"""
Generate inference_wasm.py with weights embedded as Python literals.
This version has zero file I/O — suitable for WASM / Cloudflare Workers.
"""

import json

with open("weights.json") as f:
    raw = json.load(f)

meta = raw["_meta"]
uchars = meta["uchars"]
vocab_size = meta["vocab_size"]

# Collect weight matrices
weight_keys = [k for k in raw if k != "_meta"]

lines = []
lines.append('"""')
lines.append("microGPT inference — weights embedded, zero I/O.")
lines.append("Runs on Cloudflare Workers via molt (Python-to-WASM).")
lines.append('"""')
lines.append("")
lines.append("import math")
lines.append("import random")
lines.append("")
lines.append("# --- Config ---")
lines.append(f"N_LAYER: int = {meta['n_layer']}")
lines.append(f"N_EMBD: int = {meta['n_embd']}")
lines.append(f"BLOCK_SIZE: int = {meta['block_size']}")
lines.append(f"N_HEAD: int = {meta['n_head']}")
lines.append(f"HEAD_DIM: int = {meta['n_embd'] // meta['n_head']}")
lines.append(f"VOCAB_SIZE: int = {vocab_size}")
lines.append(f'UCHARS: str = "{"".join(uchars)}"')
lines.append("")

# Emit weights as Python literals, rounded to 6 decimal places
lines.append("# --- Weights (trained, embedded) ---")
lines.append("SD: dict[str, list[list[float]]] = {")
for k in weight_keys:
    mat = raw[k]
    lines.append(f'    "{k}": [')
    for row in mat:
        rounded = [round(v, 6) for v in row]
        lines.append(f"        {rounded},")
    lines.append("    ],")
lines.append("}")
lines.append("")

# Emit the rest of the inference code
lines.append("""
def linear(x: list[float], w: list[list[float]]) -> list[float]:
    out: list[float] = []
    for row in w:
        s: float = 0.0
        for i in range(len(x)):
            s += row[i] * x[i]
        out.append(s)
    return out


def softmax(logits: list[float]) -> list[float]:
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
    n: int = len(x)
    ms: float = 0.0
    for v in x:
        ms += v * v
    ms = ms / n
    scale: float = 1.0 / math.sqrt(ms + 1e-5)
    return [v * scale for v in x]


def relu(x: list[float]) -> list[float]:
    return [v if v > 0.0 else 0.0 for v in x]


def gpt(
    token: int,
    pos: int,
    keys: list[list[list[float]]],
    values: list[list[list[float]]],
) -> list[float]:
    tok_emb: list[float] = SD["wte"][token]
    pos_emb: list[float] = SD["wpe"][pos]
    x: list[float] = [t + p for t, p in zip(tok_emb, pos_emb)]
    x = rmsnorm(x)

    for li in range(N_LAYER):
        x_res: list[float] = x
        x = rmsnorm(x)
        q: list[float] = linear(x, SD["layer" + str(li) + ".attn_wq"])
        k: list[float] = linear(x, SD["layer" + str(li) + ".attn_wk"])
        v: list[float] = linear(x, SD["layer" + str(li) + ".attn_wv"])
        keys[li].append(k)
        values[li].append(v)

        x_attn: list[float] = []
        for h in range(N_HEAD):
            hs: int = h * HEAD_DIM
            he: int = hs + HEAD_DIM
            q_h: list[float] = q[hs:he]
            seq_len: int = len(keys[li])
            attn_logits: list[float] = []
            inv_scale: float = 1.0 / math.sqrt(float(HEAD_DIM))
            for t in range(seq_len):
                dot: float = 0.0
                for j in range(HEAD_DIM):
                    dot += q_h[j] * keys[li][t][hs + j]
                attn_logits.append(dot * inv_scale)
            attn_w: list[float] = softmax(attn_logits)
            for j in range(HEAD_DIM):
                s: float = 0.0
                for t in range(seq_len):
                    s += attn_w[t] * values[li][t][hs + j]
                x_attn.append(s)

        x = linear(x_attn, SD["layer" + str(li) + ".attn_wo"])
        x = [a + b for a, b in zip(x, x_res)]

        x_res = x
        x = rmsnorm(x)
        x = linear(x, SD["layer" + str(li) + ".mlp_fc1"])
        x = relu(x)
        x = linear(x, SD["layer" + str(li) + ".mlp_fc2"])
        x = [a + b for a, b in zip(x, x_res)]

    logits: list[float] = linear(x, SD["lm_head"])
    return logits


def generate(temperature: float, num_samples: int) -> list[str]:
    bos: int = VOCAB_SIZE - 1
    results: list[str] = []
    for _ in range(num_samples):
        keys: list[list[list[float]]] = [[] for _ in range(N_LAYER)]
        values: list[list[list[float]]] = [[] for _ in range(N_LAYER)]
        token: int = bos
        chars: list[str] = []
        for pos in range(BLOCK_SIZE):
            logits: list[float] = gpt(token, pos, keys, values)
            scaled: list[float] = [l / temperature for l in logits]
            probs: list[float] = softmax(scaled)
            population: list[int] = list(range(VOCAB_SIZE))
            token = random.choices(population, weights=probs)[0]
            if token == bos:
                break
            chars.append(UCHARS[token])
        results.append("".join(chars))
    return results


def main() -> None:
    random.seed(1337)
    print("microGPT inference (WASM-ready)")
    print("vocab: " + UCHARS)
    print("---")
    names: list[str] = generate(0.5, 10)
    for i in range(len(names)):
        print("  " + str(i + 1) + ": " + names[i])


main()
""".rstrip())

with open("inference_wasm.py", "w") as f:
    f.write("\n".join(lines) + "\n")

print(f"Written inference_wasm.py ({len(lines)} lines)")
