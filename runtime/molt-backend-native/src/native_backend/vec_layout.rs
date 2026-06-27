//! Runtime `Vec<u64>` field-layout probing for the native JIT (moved verbatim
//! from the inline `vec_layout` module in lib.rs).

use std::sync::OnceLock;

#[derive(Clone, Copy, Debug)]
pub(crate) struct VecLayout {
    /// Offset (in bytes) of the data pointer within Vec<u64>.
    pub data_offset: i32,
    /// Offset (in bytes) of the length field within Vec<u64>.
    pub len_offset: i32,
}

static LAYOUT: OnceLock<VecLayout> = OnceLock::new();

pub(crate) fn vec_u64_layout() -> VecLayout {
    *LAYOUT.get_or_init(|| {
        // Create a Vec with unique sentinel values for len and cap.
        let mut v: Vec<u64> = Vec::with_capacity(13);
        // Push exactly 7 elements so len=7, cap=13.
        for i in 0u64..7 {
            v.push(i + 0xDEAD_0000);
        }
        assert_eq!(v.len(), 7);
        assert_eq!(v.capacity(), 13);

        let vec_bytes: &[u8; std::mem::size_of::<Vec<u64>>()] =
            unsafe { &*(&v as *const Vec<u64> as *const [u8; std::mem::size_of::<Vec<u64>>()]) };

        let data_ptr = v.as_ptr() as usize;

        // Vec<u64> has exactly 3 usize-sized fields (ptr, len, cap) = 24 bytes on 64-bit.
        assert_eq!(std::mem::size_of::<Vec<u64>>(), 24);

        let mut data_offset: Option<i32> = None;
        let mut len_offset: Option<i32> = None;
        let mut cap_offset: Option<i32> = None;

        for field_idx in 0..3 {
            let byte_offset = field_idx * 8;
            let val =
                usize::from_ne_bytes(vec_bytes[byte_offset..byte_offset + 8].try_into().unwrap());
            if val == 7 {
                assert!(len_offset.is_none(), "Vec layout: duplicate len field");
                len_offset = Some(byte_offset as i32);
            } else if val == 13 {
                assert!(cap_offset.is_none(), "Vec layout: duplicate cap field");
                cap_offset = Some(byte_offset as i32);
            } else if val == data_ptr {
                assert!(data_offset.is_none(), "Vec layout: duplicate data field");
                data_offset = Some(byte_offset as i32);
            } else {
                panic!(
                    "Vec layout probe: unexpected value {val:#x} at offset {byte_offset}; \
                         expected data_ptr={data_ptr:#x}, len=7, or cap=13"
                );
            }
        }

        // Intentionally NOT forgetting v — drop it normally.
        VecLayout {
            data_offset: data_offset.expect("Vec layout: data pointer field not found"),
            len_offset: len_offset.expect("Vec layout: length field not found"),
        }
    })
}
