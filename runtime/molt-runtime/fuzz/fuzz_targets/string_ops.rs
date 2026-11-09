#![no_main]

use libfuzzer_sys::fuzz_target;
use molt_obj_model::MoltObject;
use molt_runtime::{
    lifecycle, molt_dec_ref, molt_string_count, molt_string_endswith, molt_string_find,
    molt_string_from_bytes, molt_string_replace, molt_string_startswith,
};

unsafe extern "C" {
    fn molt_attached_context() -> u64;
    fn molt_exception_clear(context: u64);
}

molt_runtime::declare_app_bootstrap!(molt_runtime::AppBootstrapProvider::Unavailable(
    "molt-runtime string_ops fuzz target"
));

fuzz_target!(|data: &[u8]| {
    assert_eq!(lifecycle::init(), 1);
    lifecycle::with_ready_execution(|| unsafe {
        let context = molt_attached_context();
        let mut owners = [MoltObject::none().bits(); 8];
        molt_exception_clear(context);
        (|| {
            if molt_string_from_bytes(context, data.as_ptr(), data.len() as u64, &mut owners[0])
                != 0
            {
                return;
            }
            let (left, right) = data.split_at(data.len() / 2);
            if molt_string_from_bytes(context, left.as_ptr(), left.len() as u64, &mut owners[1])
                != 0
            {
                return;
            }
            if molt_string_from_bytes(context, right.as_ptr(), right.len() as u64, &mut owners[2])
                != 0
            {
                return;
            }
            owners[3] = molt_string_find(context, owners[0], owners[1]);
            owners[4] = molt_string_startswith(context, owners[0], owners[1]);
            owners[5] = molt_string_endswith(context, owners[0], owners[1]);
            owners[6] = molt_string_count(context, owners[0], owners[1]);
            owners[7] = molt_string_replace(
                context,
                owners[0],
                owners[1],
                owners[2],
                MoltObject::from_int(-1).bits(),
            );
        })();
        for bits in owners.into_iter().rev() {
            if let Some(ptr) = MoltObject::from_bits(bits).as_ptr() {
                molt_dec_ref(context, ptr);
            }
        }
        molt_exception_clear(context);
    })
    .expect("fuzz input requires ready runtime execution");
});
