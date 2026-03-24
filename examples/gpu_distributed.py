"""Distributed GPU DataFrame + kernel fusion example.

Demonstrates:
1. Distributing a DataFrame across multiple partitions
2. Parallel map/filter/collect operations
3. Kernel fusion for zero-intermediate-buffer compute
4. Per-channel vs per-tensor quantization comparison
"""
from molt.gpu.dataframe import DataFrame
from molt.gpu.distributed import Cluster, DistributedDataFrame
from molt.gpu.fusion import FusedPipeline, fused_map_reduce, fused_filter_reduce
from molt.gpu import ops


def demo_distributed():
    """Distributed DataFrame: partition, transform, collect."""
    print("=== Distributed DataFrame ===")

    # Create data
    df = DataFrame({
        "price": [float(i) * 0.5 for i in range(1000)],
        "quantity": [i * 10 for i in range(1000)],
    })

    # Distribute across 4 partitions
    ddf = DistributedDataFrame.from_dataframe(df, n_partitions=4)
    print(f"Distributed: {ddf}")

    # Map-reduce across partitions: filter rows where price > 100
    result = ddf.map_partitions(lambda p: p.filter(p["price"] > 100.0))
    local = result.collect()
    print(f"Filtered: {len(local)} rows with price > 100")

    # Repartition
    repart = result.repartition(2)
    print(f"Repartitioned: {repart}")

    # Head
    top = ddf.head(3)
    print(f"Head(3): {top.to_dict()}")
    print()


def demo_fusion():
    """Kernel fusion: single-pass compound operations."""
    print("=== Kernel Fusion ===")

    data = ops.arange(1.0, 10001.0)

    # Fused map+reduce vs. separate ops
    fused_result = fused_map_reduce(lambda x: x * x, 'sum', data)
    regular_result = ops.reduce(ops.map(lambda x: x * x, data), 'sum')
    print(f"Fused map+reduce:   {fused_result:.0f}")
    print(f"Regular map+reduce: {regular_result:.0f}")
    print(f"Results match: {abs(fused_result - regular_result) < 1e-6}")

    # Fused filter+reduce
    positive_sum = fused_filter_reduce(lambda x: x > 5000, 'sum', data)
    print(f"Sum of values > 5000: {positive_sum:.0f}")

    # Pipeline API
    pipeline_result = (FusedPipeline(data)
        .map(lambda x: x * x)
        .filter(lambda x: x > 1000000)
        .reduce('count'))
    print(f"Count of squares > 1M: {pipeline_result}")
    print()


def demo_quantization():
    """Per-channel vs per-tensor quantization accuracy."""
    print("=== Per-Channel Quantization ===")

    from molt.gpu.tensor import Tensor
    from molt.gpu.nn import Linear
    from molt.gpu.quantize import (
        quantize_model, quantize_model_per_channel
    )

    # Create a linear layer with varied weight magnitudes per row.
    # Per-channel quantization shines when rows have different dynamic ranges:
    # per-tensor must use a single scale for the whole matrix, wasting bits
    # on rows with small values when the global range is dominated by large ones.
    layer = Linear(8, 4, bias=False)
    weights = [
        [0.1, 0.2, 0.15, 0.3, 0.1, 0.2, 0.15, 0.25],    # small range
        [5.0, 10.0, 7.5, 12.0, 5.0, 10.0, 7.5, 12.0],    # medium range
        [0.5, 0.6, 0.4, 0.7, 0.5, 0.6, 0.4, 0.7],        # small-medium
        [50.0, 100.0, 75.0, 120.0, 50.0, 100.0, 75.0, 120.0],  # large range
    ]
    layer.weight = Tensor(weights)

    x = Tensor([1.0, 1.0, 1.0, 1.0, 1.0, 1.0, 1.0, 1.0])
    expected = layer(x)

    from molt.gpu.nn import Sequential
    model = Sequential(layer)

    exp = expected._data_list()
    print(f"Expected output:    {[round(v, 4) for v in exp]}")

    for bits in (8, 4):
        print(f"\n  --- INT{bits} ---")
        q_model = quantize_model(model, bits=bits)
        per_tensor_out = q_model(x).flatten()

        qc_model = quantize_model_per_channel(model, bits=bits)
        per_channel_out = qc_model(x).flatten()

        pt = per_tensor_out._data_list()
        pc = per_channel_out._data_list()

        print(f"  Per-tensor:     {[round(v, 4) for v in pt]}")
        print(f"  Per-channel:    {[round(v, 4) for v in pc]}")

        pt_err = sum(abs(a - b) for a, b in zip(exp, pt)) / len(exp)
        pc_err = sum(abs(a - b) for a, b in zip(exp, pc)) / len(exp)
        print(f"  Per-tensor MAE:  {pt_err:.4f}")
        print(f"  Per-channel MAE: {pc_err:.4f}")
    print()


def main():
    demo_distributed()
    demo_fusion()
    demo_quantization()
    print("All demos completed.")


if __name__ == "__main__":
    main()
