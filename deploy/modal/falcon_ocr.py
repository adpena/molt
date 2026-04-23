"""
Modal GPU deployment for Falcon-OCR.

Provides GPU inference for API agents that can't use browser WebGPU.
Uses vLLM serving with the Falcon-OCR model weights pulled from HuggingFace.

Usage:
    modal run deploy/modal/falcon_ocr.py        # smoke test
    modal deploy deploy/modal/falcon_ocr.py     # persistent deployment
    modal shell deploy/modal/falcon_ocr.py      # interactive debug

Endpoint:
    POST /ocr  {"image_base64": "...", "category": "plain"|"table"|"chart"}
"""

import modal

app = modal.App("falcon-ocr")

# Build image with vLLM + Falcon-OCR model baked in.
# Using a slim base with vLLM pre-installed avoids cold-start model downloads.
falcon_ocr_image = (
    modal.Image.debian_slim(python_version="3.12")
    .pip_install(
        "vllm>=0.8.0",
        "transformers>=4.48.0",
        "httpx>=0.28.0",
        "pillow>=11.0",
        "fastapi[standard]",
    )
    .run_commands(
        # Pre-download model weights into the image layer so cold starts
        # only pay the container boot cost, not a HuggingFace download.
        'python3 -c "'
        "from huggingface_hub import snapshot_download; "
        "snapshot_download('tiiuae/falcon-ocr', local_dir='/models/falcon-ocr')"
        '"'
    )
)


@app.cls(
    image=falcon_ocr_image,
    gpu="A10G",  # 24 GB VRAM — fits 300M VLM comfortably with INT8
    timeout=120,
    scaledown_window=300,  # 5 min idle before scale-to-zero
)
@modal.concurrent(max_inputs=16)
class FalconOCR:
    """Persistent vLLM-backed Falcon-OCR inference server."""

    @modal.enter()
    def start_engine(self) -> None:
        """Boot vLLM engine once per container lifetime."""
        from vllm import LLM, SamplingParams  # noqa: F401

        self.llm = LLM(
            model="/models/falcon-ocr",
            dtype="half",
            max_model_len=2048,
            gpu_memory_utilization=0.85,
            enforce_eager=True,  # avoid CUDA graph overhead for VLM
        )
        self.sampling_params = SamplingParams(
            max_tokens=2048,
            temperature=0.0,
        )

    @modal.method()
    async def ocr(self, image_base64: str, category: str = "plain") -> dict:
        """
        Run Falcon-OCR inference on a single image.

        Args:
            image_base64: Base64-encoded PNG or JPEG image.
            category: OCR hint — "plain", "table", or "chart".

        Returns:
            {"text": str, "tokens": int, "latency_ms": float}
        """
        import base64
        import io
        import time

        from PIL import Image

        start = time.perf_counter()

        # Decode and validate input image
        raw = base64.b64decode(image_base64)
        img = Image.open(io.BytesIO(raw)).convert("RGB")

        prompt = f"Extract text.\n<|OCR_{category.upper()}|>"

        outputs = self.llm.generate(
            [
                {
                    "prompt": prompt,
                    "multi_modal_data": {"image": img},
                }
            ],
            self.sampling_params,
        )

        text = outputs[0].outputs[0].text
        tokens = len(outputs[0].outputs[0].token_ids)
        latency_ms = (time.perf_counter() - start) * 1000

        return {
            "text": text,
            "tokens": tokens,
            "latency_ms": round(latency_ms, 1),
        }


# --- Web endpoint for direct HTTP access ---


@app.function(
    image=falcon_ocr_image,
    gpu="A10G",
    timeout=120,
    scaledown_window=300,
)
@modal.fastapi_endpoint(method="POST", docs=True)
async def ocr_endpoint(item: dict) -> dict:
    """
    HTTP POST endpoint for OCR inference.

    Body: {"image_base64": "...", "category": "plain"}
    """
    image_base64 = item.get("image_base64", "")
    category = item.get("category", "plain")

    if not image_base64:
        return {"error": "image_base64 is required"}

    srv = FalconOCR()
    return await srv.ocr.remote(image_base64, category)


@app.local_entrypoint()
def main():
    """Smoke test: run OCR on a minimal test image."""
    import base64
    import io

    from PIL import Image

    # Create a trivial 64x64 white image with "TEST" — just validates the pipeline
    img = Image.new("RGB", (64, 64), (255, 255, 255))
    buf = io.BytesIO()
    img.save(buf, format="PNG")
    test_b64 = base64.b64encode(buf.getvalue()).decode()

    srv = FalconOCR()
    result = srv.ocr.remote(test_b64, "plain")
    print(f"Result: {result}")
