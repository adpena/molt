use std::collections::VecDeque;

use molt_obj_model::MoltObject;

use crate::object::ops::{as_float_extended, float_result_bits};
use crate::{
    PyToken, TYPE_ID_LIST, TYPE_ID_LIST_BOOL, TYPE_ID_LIST_INT, TYPE_ID_TUPLE, alloc_list,
    alloc_tuple, dec_ref_bits, is_truthy, obj_from_bits, object_type_id, raise_exception,
    seq_vec_ref,
};

const INF: f64 = 1.0e30;

fn scipy_type_error(_py: &PyToken<'_>, op: &str) -> u64 {
    let message = format!("{op} requires a rectangular 2D list/tuple input");
    raise_exception::<u64>(_py, "TypeError", &message)
}

fn scipy_value_error(_py: &PyToken<'_>, op: &str, message: &str) -> u64 {
    let message = format!("{op} {message}");
    raise_exception::<u64>(_py, "ValueError", &message)
}

fn scalar_to_f64(bits: u64) -> Option<f64> {
    let obj = obj_from_bits(bits);
    if let Some(value) = as_float_extended(obj) {
        return Some(value);
    }
    if let Some(value) = obj.as_int() {
        return Some(value as f64);
    }
    obj.as_bool().map(|value| if value { 1.0 } else { 0.0 })
}

fn scalar_to_i64(bits: u64) -> Option<i64> {
    let obj = obj_from_bits(bits);
    if let Some(value) = obj.as_int() {
        return Some(value);
    }
    obj.as_bool().map(|value| if value { 1 } else { 0 })
}

fn bool_row_from_bits(
    _py: &PyToken<'_>,
    row_bits: u64,
    expected_width: Option<usize>,
    op: &str,
) -> Result<Vec<bool>, u64> {
    let Some(row_ptr) = obj_from_bits(row_bits).as_ptr() else {
        return Err(scipy_type_error(_py, op));
    };
    let type_id = unsafe { object_type_id(row_ptr) };
    let row: Vec<bool> = match type_id {
        TYPE_ID_LIST | TYPE_ID_TUPLE => unsafe {
            seq_vec_ref(row_ptr)
                .iter()
                .map(|&bits| is_truthy(_py, obj_from_bits(bits)))
                .collect()
        },
        TYPE_ID_LIST_BOOL => unsafe {
            crate::object::layout::list_bool_vec_ref(row_ptr)
                .iter()
                .map(|&value| value != 0)
                .collect()
        },
        TYPE_ID_LIST_INT => unsafe {
            crate::object::layout::list_int_vec_ref(row_ptr)
                .iter()
                .map(|&value| value != 0)
                .collect()
        },
        _ => return Err(scipy_type_error(_py, op)),
    };
    if let Some(width) = expected_width
        && row.len() != width
    {
        return Err(scipy_value_error(
            _py,
            op,
            "requires a rectangular 2D input",
        ));
    }
    Ok(row)
}

fn bool_grid_from_bits(
    _py: &PyToken<'_>,
    input_bits: u64,
    op: &str,
) -> Result<Vec<Vec<bool>>, u64> {
    let Some(input_ptr) = obj_from_bits(input_bits).as_ptr() else {
        return Err(scipy_type_error(_py, op));
    };
    let type_id = unsafe { object_type_id(input_ptr) };
    if type_id != TYPE_ID_LIST && type_id != TYPE_ID_TUPLE {
        return Err(scipy_type_error(_py, op));
    }
    let rows = unsafe { seq_vec_ref(input_ptr) };
    let mut out = Vec::with_capacity(rows.len());
    let mut width = None;
    for &row_bits in rows {
        let row = bool_row_from_bits(_py, row_bits, width, op)?;
        if width.is_none() {
            width = Some(row.len());
        }
        out.push(row);
    }
    Ok(out)
}

fn numeric_row_from_bits(
    _py: &PyToken<'_>,
    row_bits: u64,
    expected_width: Option<usize>,
    op: &str,
) -> Result<Vec<f64>, u64> {
    let Some(row_ptr) = obj_from_bits(row_bits).as_ptr() else {
        return Err(scipy_type_error(_py, op));
    };
    let type_id = unsafe { object_type_id(row_ptr) };
    let row: Vec<f64> = match type_id {
        TYPE_ID_LIST | TYPE_ID_TUPLE => {
            let elems = unsafe { seq_vec_ref(row_ptr) };
            let mut out = Vec::with_capacity(elems.len());
            for &bits in elems {
                let Some(value) = scalar_to_f64(bits) else {
                    return Err(scipy_type_error(_py, op));
                };
                out.push(value);
            }
            out
        }
        TYPE_ID_LIST_BOOL => unsafe {
            crate::object::layout::list_bool_vec_ref(row_ptr)
                .iter()
                .map(|&value| if value != 0 { 1.0 } else { 0.0 })
                .collect()
        },
        TYPE_ID_LIST_INT => unsafe {
            crate::object::layout::list_int_vec_ref(row_ptr)
                .iter()
                .map(|&value| value as f64)
                .collect()
        },
        _ => return Err(scipy_type_error(_py, op)),
    };
    if let Some(width) = expected_width
        && row.len() != width
    {
        return Err(scipy_value_error(
            _py,
            op,
            "requires a rectangular 2D input",
        ));
    }
    Ok(row)
}

fn numeric_grid_from_bits(
    _py: &PyToken<'_>,
    input_bits: u64,
    op: &str,
) -> Result<Vec<Vec<f64>>, u64> {
    let Some(input_ptr) = obj_from_bits(input_bits).as_ptr() else {
        return Err(scipy_type_error(_py, op));
    };
    let type_id = unsafe { object_type_id(input_ptr) };
    if type_id != TYPE_ID_LIST && type_id != TYPE_ID_TUPLE {
        return Err(scipy_type_error(_py, op));
    }
    let rows = unsafe { seq_vec_ref(input_ptr) };
    let mut out = Vec::with_capacity(rows.len());
    let mut width = None;
    for &row_bits in rows {
        let row = numeric_row_from_bits(_py, row_bits, width, op)?;
        if width.is_none() {
            width = Some(row.len());
        }
        out.push(row);
    }
    Ok(out)
}

fn positive_odd_size_from_bits(_py: &PyToken<'_>, size_bits: u64, op: &str) -> Result<usize, u64> {
    let Some(size) = scalar_to_i64(size_bits) else {
        return Err(scipy_value_error(
            _py,
            op,
            "requires a positive odd integer size",
        ));
    };
    if size <= 0 || size % 2 == 0 {
        return Err(scipy_value_error(
            _py,
            op,
            "requires a positive odd integer size",
        ));
    }
    Ok(size as usize)
}

fn scalar_sigma_from_bits(_py: &PyToken<'_>, sigma_bits: u64) -> Result<f64, u64> {
    let Some(sigma) = scalar_to_f64(sigma_bits) else {
        return Err(scipy_value_error(
            _py,
            "gaussian_filter",
            "requires a non-negative scalar sigma",
        ));
    };
    if !sigma.is_finite() || sigma < 0.0 {
        return Err(scipy_value_error(
            _py,
            "gaussian_filter",
            "requires a non-negative scalar sigma",
        ));
    }
    Ok(sigma)
}

fn edt_1d(sources: &[f64]) -> Vec<f64> {
    let n = sources.len();
    if n == 0 {
        return Vec::new();
    }
    let mut sites = vec![0usize; n];
    let mut breaks = vec![0.0f64; n + 1];
    let mut out = vec![0.0f64; n];
    let mut k = 0usize;
    sites[0] = 0;
    breaks[0] = -INF;
    breaks[1] = INF;
    for q in 1..n {
        let mut s;
        loop {
            let p = sites[k];
            let numerator = (sources[q] + (q * q) as f64) - (sources[p] + (p * p) as f64);
            let denom = (2 * q) as f64 - (2 * p) as f64;
            s = numerator / denom;
            if s > breaks[k] {
                break;
            }
            if k == 0 {
                break;
            }
            k -= 1;
        }
        k += 1;
        sites[k] = q;
        breaks[k] = s;
        breaks[k + 1] = INF;
    }
    k = 0;
    for (q, slot) in out.iter_mut().enumerate() {
        while breaks[k + 1] < q as f64 {
            k += 1;
        }
        let p = sites[k];
        let delta = q as isize - p as isize;
        *slot = (delta * delta) as f64 + sources[p];
    }
    out
}

fn distance_transform_edt_bool_grid(mask: &[Vec<bool>]) -> Vec<Vec<f64>> {
    let height = mask.len();
    if height == 0 {
        return Vec::new();
    }
    let width = mask[0].len();
    if width == 0 {
        return vec![Vec::new(); height];
    }
    let has_background = mask.iter().any(|row| row.iter().any(|&value| !value));
    if !has_background {
        let mut result = Vec::with_capacity(height);
        for row in 0..height {
            let mut out_row = Vec::with_capacity(width);
            for col in 0..width {
                out_row.push(((row + 1) as f64).hypot(col as f64));
            }
            result.push(out_row);
        }
        return result;
    }

    let mut column_pass = vec![vec![0.0f64; width]; height];
    for col in 0..width {
        let mut sources = Vec::with_capacity(height);
        for row in mask.iter().take(height) {
            sources.push(if row[col] { INF } else { 0.0 });
        }
        let dist = edt_1d(&sources);
        for row in 0..height {
            column_pass[row][col] = dist[row];
        }
    }

    let mut result = vec![vec![0.0f64; width]; height];
    for row in 0..height {
        let dist = edt_1d(&column_pass[row]);
        for col in 0..width {
            result[row][col] = dist[col].sqrt();
        }
    }
    result
}

fn reflect_index(mut index: isize, len: usize) -> usize {
    if len <= 1 {
        return 0;
    }
    let len = len as isize;
    while index < 0 || index >= len {
        if index < 0 {
            index = -index - 1;
        } else {
            index = 2 * len - index - 1;
        }
    }
    index as usize
}

fn gaussian_kernel(sigma: f64) -> Vec<f64> {
    if sigma == 0.0 {
        return vec![1.0];
    }
    let radius = (4.0 * sigma + 0.5).floor() as isize;
    let mut kernel = Vec::with_capacity((radius * 2 + 1) as usize);
    let sigma2 = sigma * sigma;
    for offset in -radius..=radius {
        kernel.push((-0.5 * (offset * offset) as f64 / sigma2).exp());
    }
    let norm: f64 = kernel.iter().sum();
    for weight in &mut kernel {
        *weight /= norm;
    }
    kernel
}

fn convolve_rows_reflect(input: &[Vec<f64>], kernel: &[f64]) -> Vec<Vec<f64>> {
    let height = input.len();
    if height == 0 {
        return Vec::new();
    }
    let width = input[0].len();
    let radius = (kernel.len() / 2) as isize;
    let mut out = vec![vec![0.0f64; width]; height];
    for row in 0..height {
        for col in 0..width {
            let mut acc = 0.0;
            for (k, weight) in kernel.iter().enumerate() {
                let src_col = reflect_index(col as isize + k as isize - radius, width);
                acc += input[row][src_col] * weight;
            }
            out[row][col] = acc;
        }
    }
    out
}

fn convolve_cols_reflect(input: &[Vec<f64>], kernel: &[f64]) -> Vec<Vec<f64>> {
    let height = input.len();
    if height == 0 {
        return Vec::new();
    }
    let width = input[0].len();
    let radius = (kernel.len() / 2) as isize;
    let mut out = vec![vec![0.0f64; width]; height];
    for row in 0..height {
        for col in 0..width {
            let mut acc = 0.0;
            for (k, weight) in kernel.iter().enumerate() {
                let src_row = reflect_index(row as isize + k as isize - radius, height);
                acc += input[src_row][col] * weight;
            }
            out[row][col] = acc;
        }
    }
    out
}

fn gaussian_filter_grid(input: &[Vec<f64>], sigma: f64) -> Vec<Vec<f64>> {
    if input.is_empty() {
        return Vec::new();
    }
    let kernel = gaussian_kernel(sigma);
    let rows = convolve_rows_reflect(input, &kernel);
    convolve_cols_reflect(&rows, &kernel)
}

fn extremum_filter_grid(input: &[Vec<f64>], size: usize, find_max: bool) -> Vec<Vec<f64>> {
    let height = input.len();
    if height == 0 {
        return Vec::new();
    }
    let width = input[0].len();
    let radius = (size / 2) as isize;
    let mut out = vec![vec![0.0f64; width]; height];
    for row in 0..height {
        for col in 0..width {
            let mut best = input[reflect_index(row as isize - radius, height)]
                [reflect_index(col as isize - radius, width)];
            for dr in -radius..=radius {
                let src_row = reflect_index(row as isize + dr, height);
                for dc in -radius..=radius {
                    let value = input[src_row][reflect_index(col as isize + dc, width)];
                    if (find_max && value > best) || (!find_max && value < best) {
                        best = value;
                    }
                }
            }
            out[row][col] = best;
        }
    }
    out
}

fn label_grid(mask: &[Vec<bool>]) -> (Vec<Vec<i64>>, i64) {
    let height = mask.len();
    if height == 0 {
        return (Vec::new(), 0);
    }
    let width = mask[0].len();
    let mut labels = vec![vec![0i64; width]; height];
    let mut next_label = 0i64;
    let mut queue = VecDeque::new();
    for row in 0..height {
        for col in 0..width {
            if !mask[row][col] || labels[row][col] != 0 {
                continue;
            }
            next_label += 1;
            labels[row][col] = next_label;
            queue.push_back((row, col));
            while let Some((cur_row, cur_col)) = queue.pop_front() {
                let neighbors = [
                    cur_row.checked_sub(1).map(|r| (r, cur_col)),
                    (cur_row + 1 < height).then_some((cur_row + 1, cur_col)),
                    cur_col.checked_sub(1).map(|c| (cur_row, c)),
                    (cur_col + 1 < width).then_some((cur_row, cur_col + 1)),
                ];
                for neighbor in neighbors.into_iter().flatten() {
                    let (next_row, next_col) = neighbor;
                    if mask[next_row][next_col] && labels[next_row][next_col] == 0 {
                        labels[next_row][next_col] = next_label;
                        queue.push_back((next_row, next_col));
                    }
                }
            }
        }
    }
    (labels, next_label)
}

fn float_grid_to_list(_py: &PyToken<'_>, values: &[Vec<f64>]) -> u64 {
    let mut row_bits = Vec::with_capacity(values.len());
    for row in values {
        let elems: Vec<u64> = row
            .iter()
            .map(|value| float_result_bits(_py, *value))
            .collect();
        let row_ptr = alloc_list(_py, elems.as_slice());
        if row_ptr.is_null() {
            for bits in row_bits {
                dec_ref_bits(_py, bits);
            }
            return MoltObject::none().bits();
        }
        row_bits.push(MoltObject::from_ptr(row_ptr).bits());
    }
    let outer_ptr = alloc_list(_py, row_bits.as_slice());
    for bits in row_bits {
        dec_ref_bits(_py, bits);
    }
    if outer_ptr.is_null() {
        return MoltObject::none().bits();
    }
    MoltObject::from_ptr(outer_ptr).bits()
}

fn int_grid_to_list(_py: &PyToken<'_>, values: &[Vec<i64>]) -> u64 {
    let mut row_bits = Vec::with_capacity(values.len());
    for row in values {
        let elems: Vec<u64> = row
            .iter()
            .map(|value| MoltObject::from_int(*value).bits())
            .collect();
        let row_ptr = alloc_list(_py, elems.as_slice());
        if row_ptr.is_null() {
            for bits in row_bits {
                dec_ref_bits(_py, bits);
            }
            return MoltObject::none().bits();
        }
        row_bits.push(MoltObject::from_ptr(row_ptr).bits());
    }
    let outer_ptr = alloc_list(_py, row_bits.as_slice());
    for bits in row_bits {
        dec_ref_bits(_py, bits);
    }
    if outer_ptr.is_null() {
        return MoltObject::none().bits();
    }
    MoltObject::from_ptr(outer_ptr).bits()
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_scipy_ndimage_distance_transform_edt(input_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let grid = match bool_grid_from_bits(_py, input_bits, "distance_transform_edt") {
            Ok(grid) => grid,
            Err(err_bits) => return err_bits,
        };
        let distances = distance_transform_edt_bool_grid(&grid);
        float_grid_to_list(_py, &distances)
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_scipy_ndimage_gaussian_filter(input_bits: u64, sigma_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let grid = match numeric_grid_from_bits(_py, input_bits, "gaussian_filter") {
            Ok(grid) => grid,
            Err(err_bits) => return err_bits,
        };
        let sigma = match scalar_sigma_from_bits(_py, sigma_bits) {
            Ok(sigma) => sigma,
            Err(err_bits) => return err_bits,
        };
        float_grid_to_list(_py, &gaussian_filter_grid(&grid, sigma))
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_scipy_ndimage_maximum_filter(input_bits: u64, size_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let grid = match numeric_grid_from_bits(_py, input_bits, "maximum_filter") {
            Ok(grid) => grid,
            Err(err_bits) => return err_bits,
        };
        let size = match positive_odd_size_from_bits(_py, size_bits, "maximum_filter") {
            Ok(size) => size,
            Err(err_bits) => return err_bits,
        };
        float_grid_to_list(_py, &extremum_filter_grid(&grid, size, true))
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_scipy_ndimage_minimum_filter(input_bits: u64, size_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let grid = match numeric_grid_from_bits(_py, input_bits, "minimum_filter") {
            Ok(grid) => grid,
            Err(err_bits) => return err_bits,
        };
        let size = match positive_odd_size_from_bits(_py, size_bits, "minimum_filter") {
            Ok(size) => size,
            Err(err_bits) => return err_bits,
        };
        float_grid_to_list(_py, &extremum_filter_grid(&grid, size, false))
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_scipy_ndimage_label(input_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let grid = match bool_grid_from_bits(_py, input_bits, "label") {
            Ok(grid) => grid,
            Err(err_bits) => return err_bits,
        };
        let (labels, count) = label_grid(&grid);
        let labels_bits = int_grid_to_list(_py, &labels);
        let count_bits = MoltObject::from_int(count).bits();
        let tuple_ptr = alloc_tuple(_py, &[labels_bits, count_bits]);
        dec_ref_bits(_py, labels_bits);
        if tuple_ptr.is_null() {
            return MoltObject::none().bits();
        }
        MoltObject::from_ptr(tuple_ptr).bits()
    })
}

#[cfg(test)]
mod tests {
    use super::{
        distance_transform_edt_bool_grid, extremum_filter_grid, gaussian_filter_grid, label_grid,
        reflect_index,
    };

    fn assert_grid_close(actual: &[Vec<f64>], expected: &[&[f64]], atol: f64) {
        assert_eq!(actual.len(), expected.len());
        for (actual_row, expected_row) in actual.iter().zip(expected.iter()) {
            assert_eq!(actual_row.len(), expected_row.len());
            for (actual, expected) in actual_row.iter().zip(expected_row.iter()) {
                assert!(
                    (actual - expected).abs() <= atol,
                    "actual={actual} expected={expected}"
                );
            }
        }
    }

    #[test]
    fn reflect_index_matches_scipy_half_sample_boundary() {
        let actual: Vec<usize> = (-6..10).map(|idx| reflect_index(idx, 4)).collect();
        assert_eq!(actual, vec![2, 3, 3, 2, 1, 0, 0, 1, 2, 3, 3, 2, 1, 0, 0, 1]);
    }

    #[test]
    fn edt_all_background_is_zero() {
        let actual = distance_transform_edt_bool_grid(&[
            vec![false, false, false],
            vec![false, false, false],
        ]);
        assert_grid_close(&actual, &[&[0.0, 0.0, 0.0], &[0.0, 0.0, 0.0]], 0.0);
    }

    #[test]
    fn edt_all_foreground_matches_scipy_axis_zero_degenerate() {
        let actual =
            distance_transform_edt_bool_grid(&[vec![true, true, true], vec![true, true, true]]);
        assert_grid_close(
            &actual,
            &[
                &[1.0, 2.0f64.sqrt(), 5.0f64.sqrt()],
                &[2.0, 5.0f64.sqrt(), 8.0f64.sqrt()],
            ],
            f64::EPSILON,
        );
    }

    #[test]
    fn edt_uses_nearest_background_from_either_axis() {
        let actual = distance_transform_edt_bool_grid(&[
            vec![true, true, false],
            vec![true, true, true],
            vec![false, true, true],
        ]);
        assert_grid_close(
            &actual,
            &[
                &[2.0, 1.0, 0.0],
                &[1.0, 2.0f64.sqrt(), 1.0],
                &[0.0, 1.0, 2.0],
            ],
            f64::EPSILON,
        );
    }

    #[test]
    fn gaussian_filter_smooths_with_reflect_boundaries() {
        let input = vec![vec![0.0, 1.0, 2.0], vec![3.0, 4.0, 5.0]];
        let actual = gaussian_filter_grid(&input, 1.0);
        assert_grid_close(
            &actual,
            &[
                &[1.4852304526268052, 2.0631566889496113, 2.641082925272417],
                &[2.358917074727583, 2.9368433110503882, 3.5147695473731943],
            ],
            1.0e-8,
        );
    }

    #[test]
    fn extremum_filters_use_square_reflect_footprints() {
        let input = vec![
            vec![5.0, 1.0, 7.0],
            vec![2.0, 9.0, 3.0],
            vec![4.0, 6.0, 8.0],
        ];
        let maxed = extremum_filter_grid(&input, 3, true);
        let mined = extremum_filter_grid(&input, 3, false);
        assert_grid_close(
            &maxed,
            &[&[9.0, 9.0, 9.0], &[9.0, 9.0, 9.0], &[9.0, 9.0, 9.0]],
            0.0,
        );
        assert_grid_close(
            &mined,
            &[&[1.0, 1.0, 1.0], &[1.0, 1.0, 1.0], &[2.0, 2.0, 3.0]],
            0.0,
        );
    }

    #[test]
    fn label_uses_default_four_connectivity() {
        let (labels, count) = label_grid(&[
            vec![true, false, true],
            vec![true, false, false],
            vec![false, true, true],
        ]);
        assert_eq!(count, 3);
        assert_eq!(labels, vec![vec![1, 0, 2], vec![1, 0, 0], vec![0, 3, 3]]);
    }
}
