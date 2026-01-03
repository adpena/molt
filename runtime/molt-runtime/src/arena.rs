use std::mem::{align_of, size_of};

pub struct TempArena {
    chunk_size: usize,
    chunks: Vec<Vec<u8>>,
    offset: usize,
}

impl TempArena {
    pub fn new(chunk_size: usize) -> Self {
        let size = chunk_size.max(1024);
        Self {
            chunk_size: size,
            chunks: vec![vec![0u8; size]],
            offset: 0,
        }
    }

    pub fn reset(&mut self) {
        self.chunks.truncate(1);
        self.offset = 0;
    }

    pub fn alloc_slice<T>(&mut self, len: usize) -> *mut T {
        if len == 0 {
            return std::ptr::null_mut();
        }
        let align = align_of::<T>();
        let size = match len.checked_mul(size_of::<T>()) {
            Some(val) => val,
            None => return std::ptr::null_mut(),
        };
        let aligned = (self.offset + (align - 1)) & !(align - 1);
        let needed = aligned.saturating_add(size);
        if needed > self.chunks.last().unwrap().len() {
            let new_size = self.chunk_size.max(size);
            self.chunks.push(vec![0u8; new_size]);
            self.offset = 0;
            return self.alloc_slice::<T>(len);
        }
        let ptr = unsafe { self.chunks.last_mut().unwrap().as_mut_ptr().add(aligned) };
        self.offset = needed;
        ptr as *mut T
    }
}
