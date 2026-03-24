"""Transformer text generation example using Molt GPU compute."""

from molt.gpu.tensor import Tensor
from molt.gpu.transformer import TransformerDecoder
from molt.gpu.generate import greedy_decode, top_k_sample


def main():
    # Tiny transformer for demonstration
    vocab_size = 256  # byte-level
    model = TransformerDecoder(
        vocab_size=vocab_size,
        embed_dim=64,
        num_heads=4,
        num_layers=2,
        max_seq_len=128,
    )

    # Generate from a prompt (random weights = random text)
    prompt = [ord(c) for c in "Hello"]
    print(f"Prompt: {''.join(chr(t) for t in prompt)}")

    # Greedy decode
    output = greedy_decode(model, prompt, max_new_tokens=20)
    print(f"Greedy: {''.join(chr(min(t, 127)) for t in output)}")

    # Top-k sampling
    output = top_k_sample(model, prompt, max_new_tokens=20, k=10, temperature=0.8)
    print(f"Top-k:  {''.join(chr(min(t, 127)) for t in output)}")

    print("\nNote: With random weights, output is gibberish.")
    print("Load trained weights for meaningful generation.")


if __name__ == "__main__":
    main()
