use super::*;

mod attention;
mod contiguous;
mod kernels;
mod objects;
mod tensor_methods;

#[cfg(test)]
mod tests;

use kernels::*;
use objects::*;

pub use attention::{
    molt_gpu_tensor__tensor_scaled_dot_product_attention,
    molt_gpu_tensor__tensor_scaled_dot_product_attention_attached_body,
    molt_gpu_turboquant_attention_packed, molt_gpu_turboquant_attention_packed_attached_body,
};
pub use contiguous::{
    molt_gpu_broadcast_binary_contiguous, molt_gpu_broadcast_binary_contiguous_attached_body,
    molt_gpu_buffer_to_list, molt_gpu_buffer_to_list_attached_body,
    molt_gpu_interop__load_safetensors, molt_gpu_interop__load_safetensors_attached_body,
    molt_gpu_linear_contiguous, molt_gpu_linear_contiguous_attached_body,
    molt_gpu_linear_split_last_dim_contiguous,
    molt_gpu_linear_split_last_dim_contiguous_attached_body,
    molt_gpu_linear_squared_relu_gate_interleaved_contiguous,
    molt_gpu_linear_squared_relu_gate_interleaved_contiguous_attached_body,
    molt_gpu_matmul_contiguous, molt_gpu_matmul_contiguous_attached_body,
    molt_gpu_permute_contiguous, molt_gpu_permute_contiguous_attached_body,
    molt_gpu_repeat_axis_contiguous, molt_gpu_repeat_axis_contiguous_attached_body,
    molt_gpu_rms_norm_last_axis_contiguous, molt_gpu_rms_norm_last_axis_contiguous_attached_body,
    molt_gpu_rope_apply_contiguous, molt_gpu_rope_apply_contiguous_attached_body,
    molt_gpu_softmax_last_axis_contiguous, molt_gpu_softmax_last_axis_contiguous_attached_body,
    molt_gpu_squared_relu_gate_interleaved_contiguous,
    molt_gpu_squared_relu_gate_interleaved_contiguous_attached_body, molt_gpu_tensor_from_buffer,
    molt_gpu_tensor_from_buffer_attached_body, molt_gpu_tensor_from_parts,
    molt_gpu_tensor_from_parts_attached_body,
};
pub use tensor_methods::{
    molt_gpu_tensor__tensor_concat_first_dim,
    molt_gpu_tensor__tensor_concat_first_dim_attached_body, molt_gpu_tensor__tensor_data_list,
    molt_gpu_tensor__tensor_data_list_attached_body, molt_gpu_tensor__tensor_linear,
    molt_gpu_tensor__tensor_linear_attached_body, molt_gpu_tensor__tensor_linear_split_last_dim,
    molt_gpu_tensor__tensor_linear_split_last_dim_attached_body,
    molt_gpu_tensor__tensor_linear_squared_relu_gate_interleaved,
    molt_gpu_tensor__tensor_linear_squared_relu_gate_interleaved_attached_body,
    molt_gpu_tensor__tensor_permute_dims, molt_gpu_tensor__tensor_permute_dims_attached_body,
    molt_gpu_tensor__tensor_reshape_view, molt_gpu_tensor__tensor_reshape_view_attached_body,
    molt_gpu_tensor__tensor_scatter_rows, molt_gpu_tensor__tensor_scatter_rows_attached_body,
    molt_gpu_tensor__tensor_softmax_last_axis,
    molt_gpu_tensor__tensor_softmax_last_axis_attached_body, molt_gpu_tensor__tensor_take_rows,
    molt_gpu_tensor__tensor_take_rows_attached_body, molt_gpu_tensor__zeros,
    molt_gpu_tensor__zeros_attached_body,
};
