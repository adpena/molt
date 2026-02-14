def branch_capture_stability(x):
    if isinstance(x, (list, tuple)):
        return [item for item in x[:3]]
    return {"x_repr": repr(x), "type": type(x).__name__}


print(branch_capture_stability(type(3)))
print(branch_capture_stability([1, 2, 3, 4]))
