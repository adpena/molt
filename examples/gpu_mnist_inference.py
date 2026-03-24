"""MNIST inference example using Molt GPU compute.

Demonstrates: build model -> run inference -> classify digit.
Works in interpreted mode (CPU) and compiled mode (GPU).

Usage:
    python examples/gpu_mnist_inference.py
    molt run examples/gpu_mnist_inference.py       # interpreted
    molt compile examples/gpu_mnist_inference.py   # compiled (GPU)
"""
from molt.gpu.tensor import Tensor
from molt.gpu.nn import Linear, ReLU, Sequential, Softmax


def make_fake_digit(digit_type="seven"):
    """Create a fake 28x28 grayscale image as a flat list.

    Returns a 784-element list representing a hand-drawn digit.
    """
    image = [0.0] * 784

    if digit_type == "seven":
        # Draw a "7": horizontal top bar + diagonal stroke
        for col in range(8, 22):
            image[2 * 28 + col] = 1.0   # top bar (row 2)
            image[3 * 28 + col] = 0.8   # top bar (row 3)
        for row in range(4, 26):
            col = 20 - int((row - 4) * 0.6)
            if 0 <= col < 28:
                image[row * 28 + col] = 1.0
                if col + 1 < 28:
                    image[row * 28 + col + 1] = 0.5

    elif digit_type == "one":
        # Draw a "1": center vertical line
        for row in range(2, 26):
            image[row * 28 + 14] = 1.0
            image[row * 28 + 13] = 0.3

    elif digit_type == "zero":
        # Draw a "0": oval outline
        for row in range(4, 24):
            for col in range(8, 20):
                # Distance from center of oval
                cy, cx = 14, 14
                ry, rx = 10, 5
                dy = (row - cy) / ry
                dx = (col - cx) / rx
                dist = dy * dy + dx * dx
                if 0.6 < dist < 1.2:
                    image[row * 28 + col] = 1.0
    else:
        # Default: center column active (generic vertical stroke)
        for row in range(28):
            image[row * 28 + 14] = 1.0

    return image


def main():
    print("MNIST Digit Classification (Molt GPU Compute)")
    print("=" * 50)

    # Build a simple 2-layer MLP for MNIST
    # Architecture: 784 -> 128 (ReLU) -> 10 (Softmax)
    model = Sequential(
        Linear(784, 128),
        ReLU(),
        Linear(128, 10),
        Softmax(),
    )

    print(f"\nModel architecture:")
    print(model)

    # Count parameters
    total_params = 0
    for param in model.parameters():
        total_params += param.size
    print(f"\nTotal parameters: {total_params:,}")

    # Test with several fake digits
    for digit_name in ["seven", "one", "zero"]:
        image = make_fake_digit(digit_name)

        # Normalize pixel values (simulating standard MNIST preprocessing)
        mean_val = sum(image) / len(image)
        max_val = max(image) if max(image) > 0 else 1.0
        normalized = [(x - mean_val) / max_val for x in image]

        # Create input tensor: shape (1, 784) for batch of 1
        input_tensor = Tensor(normalized, shape=(1, 784))

        # Forward pass
        output = model(input_tensor)

        # Get probabilities
        probs = output.to_list()
        if isinstance(probs[0], list):
            probs = probs[0]  # Unwrap batch dimension

        # Find predicted digit
        predicted = 0
        max_prob = probs[0]
        for i in range(1, 10):
            if probs[i] > max_prob:
                max_prob = probs[i]
                predicted = i

        print(f"\nInput: fake '{digit_name}' digit")
        print(f"  Predicted class: {predicted}")
        print(f"  Confidence: {max_prob:.4f}")
        print(f"  All probabilities: {['%.3f' % p for p in probs]}")

    print("\n" + "=" * 50)
    print("Note: With random weights, predictions are arbitrary.")
    print("Load trained weights with model.load_weights() for real inference.")
    print("This example verifies the full forward-pass pipeline works correctly.")


if __name__ == "__main__":
    main()
