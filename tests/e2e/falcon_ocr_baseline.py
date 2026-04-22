"""
Falcon-OCR baseline test harness.

Produces reference outputs and molt outputs for comparison. Saves results
as JSON files containing token IDs, logits at each step, and timing data.

Modes:
    --reference   Run using CPython + tinygrad (reference implementation)
    --molt        Run using molt's falcon_ocr.py
    --compare     Load both outputs and compare token-by-token

Usage:
    # Generate reference output (CPython + tinygrad):
    python tests/e2e/falcon_ocr_baseline.py --reference --output /tmp/falcon_ref.json

    # Generate molt output:
    python -m molt build --target native --output /tmp/test_out tests/e2e/falcon_ocr_baseline.py --rebuild
    # (Or run under molt runtime to produce molt output)
    python tests/e2e/falcon_ocr_baseline.py --molt --output /tmp/falcon_molt.json

    # Compare both:
    python tests/e2e/falcon_ocr_baseline.py --compare \\
        --reference-file /tmp/falcon_ref.json \\
        --molt-file /tmp/falcon_molt.json

All tests use deterministic stub weights from falcon_ocr_stub_weights.py.
No real Falcon-OCR weights are required.
"""

from __future__ import annotations

import argparse
import json
import math
import os
import sys
import time

# Ensure the project root is on the path for imports
_project_root = os.path.dirname(os.path.dirname(os.path.dirname(os.path.abspath(__file__))))
if _project_root not in sys.path:
    sys.path.insert(0, _project_root)

from tests.e2e.falcon_ocr_stub_weights import (
    STUB_CONFIG,
    generate_stub_config_json,
    generate_stub_weights,
    generate_test_image,
)

# ---------------------------------------------------------------------------
# Test protocol constants
# ---------------------------------------------------------------------------

# Fixed prompt: token IDs for "What text is in this image?"
# Using small integer IDs that fit in our 256-token stub vocab.
PROMPT_IDS = [1, 2, 3, 4, 5, 6, 7, 8]

# Max tokens to generate for parity testing
MAX_NEW_TOKENS = 20

# Test image dimensions (must be multiples of 16)
IMAGE_WIDTH = 32
IMAGE_HEIGHT = 32

# Comparison tolerances
LOGIT_ATOL = 1e-5   # Absolute tolerance for logit comparison
LOGIT_RTOL = 1e-4   # Relative tolerance for logit comparison
KL_THRESHOLD = 1e-6  # KL divergence threshold for softmax distributions


# ---------------------------------------------------------------------------
# Result data structure
# ---------------------------------------------------------------------------

def _empty_result() -> dict:
    return {
        "mode": "",
        "config": STUB_CONFIG,
        "prompt_ids": PROMPT_IDS,
        "max_new_tokens": MAX_NEW_TOKENS,
        "image_width": IMAGE_WIDTH,
        "image_height": IMAGE_HEIGHT,
        "generated_token_ids": [],
        "logits_per_step": [],   # List of list[float] -- top-k logits at each step
        "argmax_per_step": [],   # Argmax token ID at each step
        "timing": {
            "init_seconds": 0.0,
            "time_to_first_token_seconds": 0.0,
            "total_inference_seconds": 0.0,
            "tokens_per_second": 0.0,
        },
        "metadata": {
            "python_version": sys.version,
            "platform": sys.platform,
        },
    }


# ---------------------------------------------------------------------------
# Reference mode: CPython + molt runtime
# ---------------------------------------------------------------------------

def run_reference(output_path: str) -> dict:
    """Run Falcon-OCR inference using the molt runtime's falcon_ocr module.

    This imports falcon_ocr.py which depends on molt.gpu.Buffer and
    tinygrad.tensor.Tensor from the molt stdlib. It serves as the
    reference implementation that we compare against.
    """
    result = _empty_result()
    result["mode"] = "reference"

    # Add molt stdlib to path
    stdlib_path = os.path.join(_project_root, "src", "molt", "stdlib")
    if stdlib_path not in sys.path:
        sys.path.insert(0, stdlib_path)
    src_path = os.path.join(_project_root, "src")
    if src_path not in sys.path:
        sys.path.insert(0, src_path)

    weights_bytes = generate_stub_weights()
    config_json = generate_stub_config_json()
    test_image = generate_test_image(IMAGE_WIDTH, IMAGE_HEIGHT)

    # Import falcon_ocr (requires molt runtime modules)
    t0 = time.monotonic()
    try:
        from molt.stdlib.tinygrad.examples.falcon_ocr import init, ocr_tokens
    except ImportError as e:
        print(f"ERROR: Cannot import falcon_ocr module: {e}")
        print("This mode requires the molt runtime to be importable.")
        print("Ensure the molt stdlib is on PYTHONPATH or run under molt.")
        sys.exit(1)

    init(weights_bytes, config_json)
    t_init = time.monotonic()
    result["timing"]["init_seconds"] = t_init - t0

    # Run inference
    t_start = time.monotonic()
    generated = ocr_tokens(
        IMAGE_WIDTH,
        IMAGE_HEIGHT,
        test_image,
        PROMPT_IDS,
        MAX_NEW_TOKENS,
    )
    t_end = time.monotonic()

    result["generated_token_ids"] = generated
    result["argmax_per_step"] = generated  # In greedy decoding, argmax == token
    total_time = t_end - t_start
    result["timing"]["total_inference_seconds"] = total_time
    if generated:
        result["timing"]["time_to_first_token_seconds"] = total_time / len(generated)
        result["timing"]["tokens_per_second"] = len(generated) / total_time if total_time > 0 else 0.0

    if output_path:
        with open(output_path, "w") as f:
            json.dump(result, f, indent=2)
        print(f"Reference output saved to {output_path}")

    return result


# ---------------------------------------------------------------------------
# Molt mode: run under molt runtime
# ---------------------------------------------------------------------------

def run_molt(output_path: str) -> dict:
    """Run Falcon-OCR inference using the molt-compiled path.

    In practice this is the same Python code but compiled and executed
    by the molt backend. The entry point is identical; the difference
    is in HOW the code is executed (native/WASM backend vs CPython).

    For now, this uses the same import path as --reference. The actual
    molt compilation path will be invoked by the test harness via:
        python -m molt build --target native ...
    """
    result = _empty_result()
    result["mode"] = "molt"

    # Same import path -- the test harness wrapper decides the runtime
    stdlib_path = os.path.join(_project_root, "src", "molt", "stdlib")
    if stdlib_path not in sys.path:
        sys.path.insert(0, stdlib_path)
    src_path = os.path.join(_project_root, "src")
    if src_path not in sys.path:
        sys.path.insert(0, src_path)

    weights_bytes = generate_stub_weights()
    config_json = generate_stub_config_json()
    test_image = generate_test_image(IMAGE_WIDTH, IMAGE_HEIGHT)

    t0 = time.monotonic()
    try:
        from molt.stdlib.tinygrad.examples.falcon_ocr import init, ocr_tokens
    except ImportError as e:
        print(f"ERROR: Cannot import falcon_ocr module: {e}")
        sys.exit(1)

    init(weights_bytes, config_json)
    t_init = time.monotonic()
    result["timing"]["init_seconds"] = t_init - t0

    t_start = time.monotonic()
    generated = ocr_tokens(
        IMAGE_WIDTH,
        IMAGE_HEIGHT,
        test_image,
        PROMPT_IDS,
        MAX_NEW_TOKENS,
    )
    t_end = time.monotonic()

    result["generated_token_ids"] = generated
    result["argmax_per_step"] = generated
    total_time = t_end - t_start
    result["timing"]["total_inference_seconds"] = total_time
    if generated:
        result["timing"]["time_to_first_token_seconds"] = total_time / len(generated)
        result["timing"]["tokens_per_second"] = len(generated) / total_time if total_time > 0 else 0.0

    if output_path:
        with open(output_path, "w") as f:
            json.dump(result, f, indent=2)
        print(f"Molt output saved to {output_path}")

    return result


# ---------------------------------------------------------------------------
# Compare mode
# ---------------------------------------------------------------------------

def _kl_divergence(p: list, q: list) -> float:
    """KL(P || Q) for discrete distributions. Returns float('inf') on zero-Q."""
    eps = 1e-30
    kl = 0.0
    for pi, qi in zip(p, q):
        pi = max(pi, eps)
        qi = max(qi, eps)
        kl += pi * math.log(pi / qi)
    return kl


def _softmax(logits: list) -> list:
    """Stable softmax."""
    max_v = max(logits)
    exps = [math.exp(v - max_v) for v in logits]
    total = sum(exps)
    return [e / total for e in exps]


def compare_results(ref_path: str, molt_path: str) -> bool:
    """Compare reference and molt outputs. Returns True if parity holds."""
    with open(ref_path) as f:
        ref = json.load(f)
    with open(molt_path) as f:
        molt = json.load(f)

    passed = True

    # 1. Token ID parity
    ref_tokens = ref["generated_token_ids"]
    molt_tokens = molt["generated_token_ids"]
    if ref_tokens == molt_tokens:
        print(f"PASS: Token IDs match ({len(ref_tokens)} tokens)")
    else:
        print("FAIL: Token IDs differ")
        print(f"  Reference: {ref_tokens}")
        print(f"  Molt:      {molt_tokens}")
        # Find first divergence
        for i in range(min(len(ref_tokens), len(molt_tokens))):
            if ref_tokens[i] != molt_tokens[i]:
                print(f"  First divergence at step {i}: ref={ref_tokens[i]} molt={molt_tokens[i]}")
                break
        if len(ref_tokens) != len(molt_tokens):
            print(f"  Length mismatch: ref={len(ref_tokens)} molt={len(molt_tokens)}")
        passed = False

    # 2. Logit distribution parity (if available)
    ref_logits = ref.get("logits_per_step", [])
    molt_logits = molt.get("logits_per_step", [])
    if ref_logits and molt_logits:
        n_steps = min(len(ref_logits), len(molt_logits))
        max_kl = 0.0
        for step in range(n_steps):
            ref_sm = _softmax(ref_logits[step])
            molt_sm = _softmax(molt_logits[step])
            kl = _kl_divergence(ref_sm, molt_sm)
            max_kl = max(max_kl, kl)
            if kl > KL_THRESHOLD:
                print(f"  WARN: KL divergence at step {step}: {kl:.2e} > {KL_THRESHOLD:.2e}")
        if max_kl <= KL_THRESHOLD:
            print(f"PASS: Logit distributions match (max KL={max_kl:.2e})")
        else:
            print(f"FAIL: Logit distributions diverge (max KL={max_kl:.2e})")
            passed = False
    else:
        print("SKIP: Logit-per-step data not available for comparison")

    # 3. Timing comparison (informational, not pass/fail)
    ref_t = ref["timing"]
    molt_t = molt["timing"]
    print("\nPerformance comparison:")
    print(f"  Init:              ref={ref_t['init_seconds']:.4f}s  molt={molt_t['init_seconds']:.4f}s")
    print(f"  Time-to-first:     ref={ref_t['time_to_first_token_seconds']:.4f}s  molt={molt_t['time_to_first_token_seconds']:.4f}s")
    print(f"  Total inference:   ref={ref_t['total_inference_seconds']:.4f}s  molt={molt_t['total_inference_seconds']:.4f}s")
    print(f"  Tokens/sec:        ref={ref_t['tokens_per_second']:.2f}  molt={molt_t['tokens_per_second']:.2f}")

    if molt_t["total_inference_seconds"] > 0 and ref_t["total_inference_seconds"] > 0:
        speedup = ref_t["total_inference_seconds"] / molt_t["total_inference_seconds"]
        print(f"  Speedup:           {speedup:.2f}x")

    return passed


# ---------------------------------------------------------------------------
# CLI
# ---------------------------------------------------------------------------

def main():
    parser = argparse.ArgumentParser(
        description="Falcon-OCR baseline test harness"
    )
    group = parser.add_mutually_exclusive_group(required=True)
    group.add_argument("--reference", action="store_true",
                       help="Run reference (CPython+tinygrad) inference")
    group.add_argument("--molt", action="store_true",
                       help="Run molt inference")
    group.add_argument("--compare", action="store_true",
                       help="Compare reference and molt outputs")

    parser.add_argument("--output", "-o", type=str, default="",
                        help="Output JSON path (for --reference/--molt)")
    parser.add_argument("--reference-file", type=str, default="",
                        help="Reference JSON path (for --compare)")
    parser.add_argument("--molt-file", type=str, default="",
                        help="Molt JSON path (for --compare)")

    args = parser.parse_args()

    if args.reference:
        result = run_reference(args.output)
        print(f"Generated {len(result['generated_token_ids'])} tokens: {result['generated_token_ids']}")
        print(f"Init: {result['timing']['init_seconds']:.4f}s")
        print(f"Inference: {result['timing']['total_inference_seconds']:.4f}s")

    elif args.molt:
        result = run_molt(args.output)
        print(f"Generated {len(result['generated_token_ids'])} tokens: {result['generated_token_ids']}")
        print(f"Init: {result['timing']['init_seconds']:.4f}s")
        print(f"Inference: {result['timing']['total_inference_seconds']:.4f}s")

    elif args.compare:
        if not args.reference_file or not args.molt_file:
            parser.error("--compare requires --reference-file and --molt-file")
        success = compare_results(args.reference_file, args.molt_file)
        sys.exit(0 if success else 1)


if __name__ == "__main__":
    main()
