use crate::{
    alloc_object, buffer2d_ptr, obj_from_bits, object_type_id, raise_exception, to_i64,
    Buffer2D, MoltHeader, MoltObject, TYPE_ID_BUFFER2D,
};

#[no_mangle]
pub extern "C" fn molt_buffer2d_new(rows_bits: u64, cols_bits: u64, init_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
    let rows = match to_i64(obj_from_bits(rows_bits)) {
        Some(val) if val >= 0 => val as usize,
        _ => return raise_exception::<_>(_py, "TypeError", "rows must be a non-negative int"),
    };
    let cols = match to_i64(obj_from_bits(cols_bits)) {
        Some(val) if val >= 0 => val as usize,
        _ => return raise_exception::<_>(_py, "TypeError", "cols must be a non-negative int"),
    };
    let init = match obj_from_bits(init_bits).as_int() {
        Some(val) => val,
        None => return raise_exception::<_>(_py, "TypeError", "init must be an int"),
    };
    let total = std::mem::size_of::<MoltHeader>() + std::mem::size_of::<*mut Buffer2D>();
    let ptr = alloc_object(_py, total, TYPE_ID_BUFFER2D);
    if ptr.is_null() {
        return MoltObject::none().bits();
    }
    let size = rows.saturating_mul(cols);
    let buf = Box::new(Buffer2D {
        rows,
        cols,
        data: vec![init; size],
    });
    let buf_ptr = Box::into_raw(buf);
    unsafe {
        *(ptr as *mut *mut Buffer2D) = buf_ptr;
    }
    MoltObject::from_ptr(ptr).bits()

    })
}

#[no_mangle]
pub extern "C" fn molt_buffer2d_get(obj_bits: u64, row_bits: u64, col_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
    let row = match to_i64(obj_from_bits(row_bits)) {
        Some(val) if val >= 0 => val as usize,
        _ => return raise_exception::<_>(_py, "TypeError", "row must be a non-negative int"),
    };
    let col = match to_i64(obj_from_bits(col_bits)) {
        Some(val) if val >= 0 => val as usize,
        _ => return raise_exception::<_>(_py, "TypeError", "col must be a non-negative int"),
    };
    let obj = obj_from_bits(obj_bits);
    if let Some(ptr) = obj.as_ptr() {
        unsafe {
            if object_type_id(ptr) != TYPE_ID_BUFFER2D {
                return MoltObject::none().bits();
            }
            let buf = buffer2d_ptr(ptr);
            if buf.is_null() {
                return MoltObject::none().bits();
            }
            let buf = &*buf;
            if row >= buf.rows || col >= buf.cols {
                return raise_exception::<_>(_py, "IndexError", "buffer2d index out of range");
            }
            let idx = row * buf.cols + col;
            return MoltObject::from_int(buf.data[idx]).bits();
        }
    }
    MoltObject::none().bits()

    })
}

#[no_mangle]
pub extern "C" fn molt_buffer2d_set(
    obj_bits: u64,
    row_bits: u64,
    col_bits: u64,
    val_bits: u64,
) -> u64 {
    crate::with_gil_entry!(_py, {
    let row = match to_i64(obj_from_bits(row_bits)) {
        Some(val) if val >= 0 => val as usize,
        _ => return raise_exception::<_>(_py, "TypeError", "row must be a non-negative int"),
    };
    let col = match to_i64(obj_from_bits(col_bits)) {
        Some(val) if val >= 0 => val as usize,
        _ => return raise_exception::<_>(_py, "TypeError", "col must be a non-negative int"),
    };
    let val = match obj_from_bits(val_bits).as_int() {
        Some(v) => v,
        None => return raise_exception::<_>(_py, "TypeError", "value must be an int"),
    };
    let obj = obj_from_bits(obj_bits);
    if let Some(ptr) = obj.as_ptr() {
        unsafe {
            if object_type_id(ptr) != TYPE_ID_BUFFER2D {
                return MoltObject::none().bits();
            }
            let buf = buffer2d_ptr(ptr);
            if buf.is_null() {
                return MoltObject::none().bits();
            }
            let buf = &mut *buf;
            if row >= buf.rows || col >= buf.cols {
                return raise_exception::<_>(_py, "IndexError", "buffer2d index out of range");
            }
            let idx = row * buf.cols + col;
            buf.data[idx] = val;
            return MoltObject::none().bits();
        }
    }
    MoltObject::none().bits()

    })
}

#[no_mangle]
pub extern "C" fn molt_buffer2d_matmul(a_bits: u64, b_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
    let a = obj_from_bits(a_bits);
    let b = obj_from_bits(b_bits);
    let (a_ptr, b_ptr) = match (a.as_ptr(), b.as_ptr()) {
        (Some(ap), Some(bp)) => (ap, bp),
        _ => return raise_exception::<_>(_py, "TypeError", "matmul expects buffer2d operands"),
    };
    unsafe {
        if object_type_id(a_ptr) != TYPE_ID_BUFFER2D || object_type_id(b_ptr) != TYPE_ID_BUFFER2D {
            return raise_exception::<_>(_py, "TypeError", "matmul expects buffer2d operands");
        }
        let a_buf = buffer2d_ptr(a_ptr);
        let b_buf = buffer2d_ptr(b_ptr);
        if a_buf.is_null() || b_buf.is_null() {
            return MoltObject::none().bits();
        }
        let a_buf = &*a_buf;
        let b_buf = &*b_buf;
        if a_buf.cols != b_buf.rows {
            return raise_exception::<_>(_py, "ValueError", "matmul dimension mismatch");
        }
        let rows = a_buf.rows;
        let cols = b_buf.cols;
        let inner = a_buf.cols;
        let total = std::mem::size_of::<MoltHeader>() + std::mem::size_of::<*mut Buffer2D>();
        let ptr = alloc_object(_py, total, TYPE_ID_BUFFER2D);
        if ptr.is_null() {
            return MoltObject::none().bits();
        }
        let mut data = vec![0i64; rows.saturating_mul(cols)];
        for i in 0..rows {
            for j in 0..cols {
                let mut acc = 0i64;
                for k in 0..inner {
                    let left = a_buf.data[i * inner + k];
                    let right = b_buf.data[k * cols + j];
                    acc = acc.wrapping_add(left.wrapping_mul(right));
                }
                data[i * cols + j] = acc;
            }
        }
        let buf = Box::new(Buffer2D { rows, cols, data });
        let buf_ptr = Box::into_raw(buf);
        *(ptr as *mut *mut Buffer2D) = buf_ptr;
        MoltObject::from_ptr(ptr).bits()
    }

    })
}
