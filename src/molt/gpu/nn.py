"""
molt.gpu.nn — Neural network layers for inference.

Provides PyTorch-style layer API for building and running neural network
models in inference mode (no autograd / backpropagation).

Usage:
    from molt.gpu.nn import Linear, ReLU, Sequential
    from molt.gpu.tensor import Tensor

    model = Sequential(
        Linear(784, 256),
        ReLU(),
        Linear(256, 10),
    )
    model.load_weights(weights_dict)
    output = model(input_tensor)
"""

import math
from .tensor import Tensor, zeros, randn, _product


# ── Activation layers ─────────────────────────────────────────────────

class ReLU:
    """Rectified linear unit activation."""

    def __call__(self, x: Tensor) -> Tensor:
        return x.relu()

    def __repr__(self):
        return "ReLU()"


class Sigmoid:
    """Logistic sigmoid activation."""

    def __call__(self, x: Tensor) -> Tensor:
        return x.sigmoid()

    def __repr__(self):
        return "Sigmoid()"


class Tanh:
    """Hyperbolic tangent activation."""

    def __call__(self, x: Tensor) -> Tensor:
        return x.tanh()

    def __repr__(self):
        return "Tanh()"


class Softmax:
    """Softmax activation along a given axis."""

    def __init__(self, axis=-1):
        self.axis = axis

    def __call__(self, x: Tensor) -> Tensor:
        return x.softmax(self.axis)

    def __repr__(self):
        return f"Softmax(axis={self.axis})"


class GELU:
    """Gaussian Error Linear Unit (approximate)."""

    def __call__(self, x: Tensor) -> Tensor:
        # GELU(x) = x * 0.5 * (1 + tanh(sqrt(2/pi) * (x + 0.044715 * x^3)))
        data = x._data_list()
        coeff = math.sqrt(2.0 / math.pi)
        result = []
        for v in data:
            inner = coeff * (v + 0.044715 * v * v * v)
            result.append(0.5 * v * (1.0 + math.tanh(inner)))
        return x._from_flat(result, x.shape)

    def __repr__(self):
        return "GELU()"


# ── Core layers ───────────────────────────────────────────────────────

class Linear:
    """Fully connected layer: y = x @ W^T + b.

    Args:
        in_features: size of each input sample
        out_features: size of each output sample
        bias: if True, adds a learnable bias (default: True)
    """

    def __init__(self, in_features: int, out_features: int, bias: bool = True):
        self.in_features = in_features
        self.out_features = out_features
        self.has_bias = bias

        # Gaussian initialization scaled by 1/sqrt(in_features)
        bound = 1.0 / math.sqrt(in_features)
        self.weight = randn(out_features, in_features, seed=hash((in_features, out_features)) & 0xFFFFFFFF) * bound
        if bias:
            self.bias = zeros(out_features) * 0.0
        else:
            self.bias = None

    def __call__(self, x: Tensor) -> Tensor:
        """Forward pass: y = x @ W^T + b.

        Args:
            x: input tensor of shape (..., in_features)

        Returns:
            output tensor of shape (..., out_features)
        """
        # x: (batch, in_features) or (in_features,)
        # weight: (out_features, in_features)
        # result: (batch, out_features) or (out_features,)
        squeezed = False
        if x.ndim == 1:
            x = x.reshape(1, x.size)
            squeezed = True

        # x @ W^T
        out = x @ self.weight.T

        if self.has_bias and self.bias is not None:
            out = out + self.bias

        if squeezed:
            out = out.reshape(self.out_features)

        return out

    def load_weights(self, weight, bias=None):
        """Load pre-trained weights.

        Args:
            weight: Tensor or nested list of shape (out_features, in_features)
            bias: Tensor or list of shape (out_features,), or None
        """
        if not isinstance(weight, Tensor):
            weight = Tensor(weight)
        self.weight = weight
        if bias is not None:
            if not isinstance(bias, Tensor):
                bias = Tensor(bias)
            self.bias = bias

    def parameters(self) -> list:
        """Return list of parameter tensors."""
        params = [self.weight]
        if self.bias is not None:
            params.append(self.bias)
        return params

    def __repr__(self):
        return (f"Linear(in_features={self.in_features}, "
                f"out_features={self.out_features}, "
                f"bias={self.has_bias})")


class Conv2d:
    """2D convolution layer for image models.

    Expects input shape: (batch, in_channels, height, width)
    Output shape: (batch, out_channels, out_h, out_w)

    Args:
        in_channels: number of input channels
        out_channels: number of output channels (filters)
        kernel_size: size of the convolution kernel (int or tuple)
        stride: convolution stride (default: 1)
        padding: zero-padding added to both sides (default: 0)
    """

    def __init__(self, in_channels: int, out_channels: int,
                 kernel_size: int, stride: int = 1, padding: int = 0):
        self.in_channels = in_channels
        self.out_channels = out_channels
        self.kernel_size = kernel_size if isinstance(kernel_size, tuple) else (kernel_size, kernel_size)
        self.stride = stride if isinstance(stride, tuple) else (stride, stride)
        self.padding = padding if isinstance(padding, tuple) else (padding, padding)

        kh, kw = self.kernel_size
        bound = 1.0 / math.sqrt(in_channels * kh * kw)
        self.weight = randn(
            out_channels, in_channels, kh, kw,
            seed=hash((in_channels, out_channels, kh, kw)) & 0xFFFFFFFF
        ) * bound
        self.bias = zeros(out_channels) * 0.0

    def __call__(self, x: Tensor) -> Tensor:
        """Forward pass for 2D convolution.

        Uses direct convolution (no im2col) for simplicity.
        GPU compilation will optimize this.
        """
        if x.ndim == 3:
            # Add batch dimension
            x = x.reshape(1, *x.shape)

        batch, in_c, in_h, in_w = x.shape
        kh, kw = self.kernel_size
        sh, sw = self.stride
        ph, pw = self.padding

        out_h = (in_h + 2 * ph - kh) // sh + 1
        out_w = (in_w + 2 * pw - kw) // sw + 1

        x_data = x._data_list()
        w_data = self.weight._data_list()
        b_data = self.bias._data_list() if self.bias is not None else None

        result = [0.0] * (batch * self.out_channels * out_h * out_w)

        for b in range(batch):
            for oc in range(self.out_channels):
                for oh in range(out_h):
                    for ow in range(out_w):
                        val = 0.0
                        for ic in range(in_c):
                            for fh in range(kh):
                                for fw in range(kw):
                                    ih = oh * sh - ph + fh
                                    iw = ow * sw - pw + fw
                                    if 0 <= ih < in_h and 0 <= iw < in_w:
                                        x_idx = ((b * in_c + ic) * in_h + ih) * in_w + iw
                                        w_idx = ((oc * in_c + ic) * kh + fh) * kw + fw
                                        val += x_data[x_idx] * w_data[w_idx]
                        if b_data is not None:
                            val += b_data[oc]
                        r_idx = ((b * self.out_channels + oc) * out_h + oh) * out_w + ow
                        result[r_idx] = val

        return Tensor(result, shape=(batch, self.out_channels, out_h, out_w))

    def load_weights(self, weight, bias=None):
        """Load pre-trained convolution weights."""
        if not isinstance(weight, Tensor):
            weight = Tensor(weight)
        self.weight = weight
        if bias is not None:
            if not isinstance(bias, Tensor):
                bias = Tensor(bias)
            self.bias = bias

    def parameters(self) -> list:
        params = [self.weight]
        if self.bias is not None:
            params.append(self.bias)
        return params

    def __repr__(self):
        return (f"Conv2d({self.in_channels}, {self.out_channels}, "
                f"kernel_size={self.kernel_size}, stride={self.stride}, "
                f"padding={self.padding})")


class MaxPool2d:
    """2D max pooling layer.

    Args:
        kernel_size: size of the pooling window
        stride: stride of the pooling window (default: kernel_size)
    """

    def __init__(self, kernel_size: int, stride: int = None):
        self.kernel_size = kernel_size if isinstance(kernel_size, tuple) else (kernel_size, kernel_size)
        if stride is None:
            self.stride = self.kernel_size
        else:
            self.stride = stride if isinstance(stride, tuple) else (stride, stride)

    def __call__(self, x: Tensor) -> Tensor:
        if x.ndim == 3:
            x = x.reshape(1, *x.shape)

        batch, channels, in_h, in_w = x.shape
        kh, kw = self.kernel_size
        sh, sw = self.stride

        out_h = (in_h - kh) // sh + 1
        out_w = (in_w - kw) // sw + 1

        x_data = x._data_list()
        result = [0.0] * (batch * channels * out_h * out_w)

        for b in range(batch):
            for c in range(channels):
                for oh in range(out_h):
                    for ow in range(out_w):
                        max_val = float('-inf')
                        for fh in range(kh):
                            for fw in range(kw):
                                ih = oh * sh + fh
                                iw = ow * sw + fw
                                idx = ((b * channels + c) * in_h + ih) * in_w + iw
                                if x_data[idx] > max_val:
                                    max_val = x_data[idx]
                        r_idx = ((b * channels + c) * out_h + oh) * out_w + ow
                        result[r_idx] = max_val

        return Tensor(result, shape=(batch, channels, out_h, out_w))

    def __repr__(self):
        return f"MaxPool2d(kernel_size={self.kernel_size}, stride={self.stride})"


class AvgPool2d:
    """2D average pooling layer."""

    def __init__(self, kernel_size: int, stride: int = None):
        self.kernel_size = kernel_size if isinstance(kernel_size, tuple) else (kernel_size, kernel_size)
        if stride is None:
            self.stride = self.kernel_size
        else:
            self.stride = stride if isinstance(stride, tuple) else (stride, stride)

    def __call__(self, x: Tensor) -> Tensor:
        if x.ndim == 3:
            x = x.reshape(1, *x.shape)

        batch, channels, in_h, in_w = x.shape
        kh, kw = self.kernel_size
        sh, sw = self.stride

        out_h = (in_h - kh) // sh + 1
        out_w = (in_w - kw) // sw + 1

        x_data = x._data_list()
        result = [0.0] * (batch * channels * out_h * out_w)
        pool_size = kh * kw

        for b in range(batch):
            for c in range(channels):
                for oh in range(out_h):
                    for ow in range(out_w):
                        total = 0.0
                        for fh in range(kh):
                            for fw in range(kw):
                                ih = oh * sh + fh
                                iw = ow * sw + fw
                                idx = ((b * channels + c) * in_h + ih) * in_w + iw
                                total += x_data[idx]
                        r_idx = ((b * channels + c) * out_h + oh) * out_w + ow
                        result[r_idx] = total / pool_size

        return Tensor(result, shape=(batch, channels, out_h, out_w))

    def __repr__(self):
        return f"AvgPool2d(kernel_size={self.kernel_size}, stride={self.stride})"


class BatchNorm1d:
    """Batch normalization (inference mode only).

    In inference mode, uses running mean/variance (not batch statistics).
    Until load_weights is called, acts as an identity.

    Args:
        num_features: number of features (channels)
        eps: numerical stability term (default: 1e-5)
    """

    def __init__(self, num_features: int, eps: float = 1e-5):
        self.num_features = num_features
        self.eps = eps
        # Default: identity transform
        self.weight = None  # gamma
        self.bias_param = None  # beta
        self.running_mean = None
        self.running_var = None

    def __call__(self, x: Tensor) -> Tensor:
        if self.running_mean is None:
            return x  # Not initialized — identity

        data = x._data_list()
        mean = self.running_mean._data_list()
        var = self.running_var._data_list()
        gamma = self.weight._data_list() if self.weight is not None else [1.0] * self.num_features
        beta = self.bias_param._data_list() if self.bias_param is not None else [0.0] * self.num_features

        # x shape: (batch, features) or (features,)
        if x.ndim == 1:
            result = []
            for i in range(self.num_features):
                val = (data[i] - mean[i]) / math.sqrt(var[i] + self.eps)
                result.append(gamma[i] * val + beta[i])
            return Tensor(result, shape=x.shape)
        else:
            batch = x.shape[0]
            result = [0.0] * len(data)
            for b in range(batch):
                for i in range(self.num_features):
                    idx = b * self.num_features + i
                    val = (data[idx] - mean[i]) / math.sqrt(var[i] + self.eps)
                    result[idx] = gamma[i] * val + beta[i]
            return Tensor(result, shape=x.shape)

    def load_weights(self, weight=None, bias=None, running_mean=None, running_var=None):
        """Load batch norm parameters."""
        if weight is not None:
            self.weight = weight if isinstance(weight, Tensor) else Tensor(weight)
        if bias is not None:
            self.bias_param = bias if isinstance(bias, Tensor) else Tensor(bias)
        if running_mean is not None:
            self.running_mean = running_mean if isinstance(running_mean, Tensor) else Tensor(running_mean)
        if running_var is not None:
            self.running_var = running_var if isinstance(running_var, Tensor) else Tensor(running_var)

    def __repr__(self):
        return f"BatchNorm1d({self.num_features})"


class LayerNorm:
    """Layer normalization.

    Args:
        normalized_shape: input shape from the last dimension
        eps: numerical stability (default: 1e-5)
    """

    def __init__(self, normalized_shape, eps=1e-5):
        if isinstance(normalized_shape, int):
            normalized_shape = (normalized_shape,)
        self.normalized_shape = tuple(normalized_shape)
        self.eps = eps
        norm_size = _product(self.normalized_shape)
        self.weight = Tensor([1.0] * norm_size, shape=self.normalized_shape)
        self.bias_param = Tensor([0.0] * norm_size, shape=self.normalized_shape)

    def __call__(self, x: Tensor) -> Tensor:
        data = x._data_list()
        norm_size = _product(self.normalized_shape)
        outer = x.size // norm_size
        gamma = self.weight._data_list()
        beta = self.bias_param._data_list()

        result = [0.0] * len(data)
        for o in range(outer):
            base = o * norm_size
            chunk = data[base:base + norm_size]
            mean = sum(chunk) / norm_size
            var = sum((v - mean) ** 2 for v in chunk) / norm_size
            inv_std = 1.0 / math.sqrt(var + self.eps)
            for i in range(norm_size):
                result[base + i] = gamma[i] * (chunk[i] - mean) * inv_std + beta[i]

        return Tensor(result, shape=x.shape)

    def load_weights(self, weight=None, bias=None):
        if weight is not None:
            self.weight = weight if isinstance(weight, Tensor) else Tensor(weight)
        if bias is not None:
            self.bias_param = bias if isinstance(bias, Tensor) else Tensor(bias)

    def __repr__(self):
        return f"LayerNorm({self.normalized_shape})"


class Dropout:
    """Dropout layer — no-op in inference mode."""

    def __init__(self, p: float = 0.5):
        self.p = p

    def __call__(self, x: Tensor) -> Tensor:
        return x  # No-op in inference

    def __repr__(self):
        return f"Dropout(p={self.p})"


class Flatten:
    """Flatten all dimensions after start_dim into one."""

    def __init__(self, start_dim: int = 1):
        self.start_dim = start_dim

    def __call__(self, x: Tensor) -> Tensor:
        if self.start_dim >= x.ndim:
            return x
        new_shape = x.shape[:self.start_dim] + (_product(x.shape[self.start_dim:]),)
        return x.reshape(*new_shape)

    def __repr__(self):
        return f"Flatten(start_dim={self.start_dim})"


class Embedding:
    """Lookup table for token embeddings.

    Args:
        num_embeddings: size of the vocabulary
        embedding_dim: size of each embedding vector
    """

    def __init__(self, num_embeddings: int, embedding_dim: int):
        self.num_embeddings = num_embeddings
        self.embedding_dim = embedding_dim
        self.weight = randn(
            num_embeddings, embedding_dim,
            seed=hash((num_embeddings, embedding_dim)) & 0xFFFFFFFF
        ) * 0.02  # Small initialization

    def __call__(self, indices: Tensor) -> Tensor:
        """Look up embeddings for the given indices.

        Args:
            indices: integer tensor of any shape

        Returns:
            tensor of shape (*indices.shape, embedding_dim)
        """
        try:
            return self.weight.take_rows(indices, allow_negative=False)
        except IndexError as exc:
            raise IndexError(
                f"Embedding index out of range [0, {self.num_embeddings})"
            ) from exc

    def load_weights(self, weight):
        """Load pre-trained embedding weights."""
        if not isinstance(weight, Tensor):
            weight = Tensor(weight)
        self.weight = weight

    def parameters(self) -> list:
        return [self.weight]

    def __repr__(self):
        return f"Embedding({self.num_embeddings}, {self.embedding_dim})"


# ── Container layers ──────────────────────────────────────────────────

class Sequential:
    """Chain layers sequentially.

    Usage:
        model = Sequential(
            Linear(784, 256),
            ReLU(),
            Linear(256, 10),
        )
        output = model(input_tensor)
    """

    def __init__(self, *layers):
        self.layers = list(layers)

    def __call__(self, x: Tensor) -> Tensor:
        for layer in self.layers:
            x = layer(x)
        return x

    def load_weights(self, weights_dict: dict):
        """Load weights from a dictionary.

        Keys should follow PyTorch convention:
            "0.weight", "0.bias" for the first layer, etc.
        Or layer-name based:
            "layer_name.weight", "layer_name.bias"
        """
        for i, layer in enumerate(self.layers):
            prefix = str(i)
            w_key = f"{prefix}.weight"
            b_key = f"{prefix}.bias"

            if hasattr(layer, 'load_weights'):
                weight = weights_dict.get(w_key)
                bias = weights_dict.get(b_key)
                if weight is not None:
                    if not isinstance(weight, Tensor):
                        weight = Tensor(weight)
                    if bias is not None and not isinstance(bias, Tensor):
                        bias = Tensor(bias)
                    if isinstance(layer, (Linear, Conv2d)):
                        layer.load_weights(weight, bias)
                    elif isinstance(layer, BatchNorm1d):
                        rm_key = f"{prefix}.running_mean"
                        rv_key = f"{prefix}.running_var"
                        layer.load_weights(
                            weight=weight,
                            bias=bias,
                            running_mean=weights_dict.get(rm_key),
                            running_var=weights_dict.get(rv_key),
                        )
                    elif isinstance(layer, (Embedding, LayerNorm)):
                        layer.load_weights(weight, bias)

    def parameters(self) -> list:
        """Return all parameter tensors."""
        params = []
        for layer in self.layers:
            if hasattr(layer, 'parameters'):
                params.extend(layer.parameters())
        return params

    def __repr__(self):
        lines = ["Sequential("]
        for i, layer in enumerate(self.layers):
            lines.append(f"  ({i}): {layer}")
        lines.append(")")
        return "\n".join(lines)

    def __len__(self):
        return len(self.layers)

    def __getitem__(self, idx):
        return self.layers[idx]
