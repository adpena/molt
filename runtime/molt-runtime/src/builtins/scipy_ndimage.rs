use molt_obj_model::MoltObject;

use crate::{
    PyToken, TYPE_ID_LIST, TYPE_ID_LIST_BOOL, TYPE_ID_TUPLE, alloc_list, dec_ref_bits, is_truthy,
    obj_from_bits, object_type_id, raise_exception, seq_vec_ref,
};

const INF: f64 = 1.0e30;

fn unsupported_input(_py: &PyToken<'_>) -> u64 {
    raise_exception::<u64>(
        _py,
        "TypeError",
        "distance_transform_edt requires a rectangular 2D list/tuple input",
    )
}

fn rectangular_input(_py: &PyToken<'_>) -> u64 {
    raise_exception::<u64>(
        _py,
        "ValueError",
        "distance_transform_edt requires a rectangular 2D input",
    )
}

fn bool_row_from_bits(
    _py: &PyToken<'_>,
    row_bits: u64,
    expected_width: Option<usize>,
) -> Result<Vec<bool>, u64> {
    let Some(row_ptr) = obj_from_bits(row_bits).as_ptr() else {
        return Err(unsupported_input(_py));
    };
    let type_id = unsafe { object_type_id(row_ptr) };
    let row: Vec<bool> = if type_id == TYPE_ID_LIST || type_id == TYPE_ID_TUPLE {
        unsafe {
            seq_vec_ref(row_ptr)
                .iter()
                .map(|&bits| is_truthy(_py, obj_from_bits(bits)))
                .collect()
        }
    } else if type_id == TYPE_ID_LIST_BOOL {
        unsafe {
            crate::object::layout::list_bool_vec_ref(row_ptr)
                .iter()
                .map(|&value| value != 0)
                .collect()
        }
    } else {
        return Err(unsupported_input(_py));
    };
    if let Some(width) = expected_width
        && row.len() != width
    {
        return Err(rectangular_input(_py));
    }
    Ok(row)
}

fn bool_grid_from_bits(_py: &PyToken<'_>, input_bits: u64) -> Result<Vec<Vec<bool>>, u64> {
    let Some(input_ptr) = obj_from_bits(input_bits).as_ptr() else {
        return Err(unsupported_input(_py));
    };
    let type_id = unsafe { object_type_id(input_ptr) };
    if type_id != TYPE_ID_LIST && type_id != TYPE_ID_TUPLE {
        return Err(unsupported_input(_py));
    }
    let rows = unsafe { seq_vec_ref(input_ptr) };
    let mut out = Vec::with_capacity(rows.len());
    let mut width = None;
    for &row_bits in rows {
        let row = bool_row_from_bits(_py, row_bits, width)?;
        if width.is_none() {
            width = Some(row.len());
        }
        out.push(row);
    }
    Ok(out)
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

fn float_grid_to_list(_py: &PyToken<'_>, values: &[Vec<f64>]) -> u64 {
    let mut row_bits = Vec::with_capacity(values.len());
    for row in values {
        let elems: Vec<u64> = row
            .iter()
            .map(|value| MoltObject::from_float(*value).bits())
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
        let grid = match bool_grid_from_bits(_py, input_bits) {
            Ok(grid) => grid,
            Err(err_bits) => return err_bits,
        };
        let distances = distance_transform_edt_bool_grid(&grid);
        float_grid_to_list(_py, &distances)
    })
}

#[cfg(test)]
mod tests {
    use super::distance_transform_edt_bool_grid;

    fn assert_grid_close(actual: &[Vec<f64>], expected: &[&[f64]]) {
        assert_eq!(actual.len(), expected.len());
        for (actual_row, expected_row) in actual.iter().zip(expected.iter()) {
            assert_eq!(actual_row.len(), expected_row.len());
            for (actual, expected) in actual_row.iter().zip(expected_row.iter()) {
                assert!(
                    (actual - expected).abs() <= f64::EPSILON,
                    "actual={actual} expected={expected}"
                );
            }
        }
    }

    #[test]
    fn edt_all_background_is_zero() {
        let actual = distance_transform_edt_bool_grid(&[
            vec![false, false, false],
            vec![false, false, false],
        ]);
        assert_grid_close(&actual, &[&[0.0, 0.0, 0.0], &[0.0, 0.0, 0.0]]);
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
        );
    }
}
