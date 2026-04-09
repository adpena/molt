from molt.gpu.interop import load_safetensors

state = load_safetensors(
    "/Users/adpena/Projects/enjoice/experiments/tinygrad-molt/falcon-ocr/weights/model.safetensors"
)
print(len(state), state["tok_embeddings.weight"].shape)
