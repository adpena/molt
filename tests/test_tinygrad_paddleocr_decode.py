from __future__ import annotations

from tests.helpers.tinygrad_stdlib_loader import tinygrad_stdlib_context


def _recognizer(paddleocr):
    recognizer = paddleocr.PaddleOCRRecognizer()
    recognizer.charset = ["", "A", "B"]
    return recognizer


def test_decode_ctc_uses_probability_inputs_without_second_softmax() -> None:
    with tinygrad_stdlib_context("paddleocr") as modules:
        Tensor = modules["tensor"].Tensor
        recognizer = _recognizer(modules["paddleocr"])
        probabilities = Tensor(
            [
                [
                    [0.10, 0.80, 0.10],
                    [0.20, 0.70, 0.10],
                    [0.90, 0.05, 0.05],
                    [0.10, 0.20, 0.70],
                ]
            ]
        )

        text, confidence = recognizer.decode_ctc(probabilities)

        assert text == "AB"
        assert abs(confidence - 0.75) < 1e-12


def test_decode_ctc_still_accepts_raw_logits() -> None:
    with tinygrad_stdlib_context("paddleocr") as modules:
        Tensor = modules["tensor"].Tensor
        recognizer = _recognizer(modules["paddleocr"])
        logits = Tensor(
            [
                [
                    [0.0, 4.0, 1.0],
                    [0.0, 3.0, 1.0],
                    [5.0, 0.0, 0.0],
                    [0.0, 1.0, 4.0],
                ]
            ]
        )

        text, confidence = recognizer.decode_ctc(logits)

        assert text == "AB"
        assert confidence > 0.9
