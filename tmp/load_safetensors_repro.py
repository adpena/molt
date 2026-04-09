from molt.gpu.interop import load_safetensors
print('stage: import')
weights = load_safetensors('tmp/tiny_safetensors_one.safetensors')
print(type(weights).__name__)
print(sorted(weights.keys())[0])
print(type(weights['w']).__name__)
