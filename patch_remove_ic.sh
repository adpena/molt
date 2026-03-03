cat << 'INNER_EOF' > remove_ic.patch
--- runtime/molt-runtime/src/builtins/attributes.rs
+++ runtime/molt-runtime/src/builtins/attributes.rs
@@ -4657,59 +4657,11 @@
     attr_name_len_bits: u64,
     site_bits: u64,
 ) -> i64 {
     unsafe {
         crate::with_gil_entry!(_py, {
             let Some(site_id) = ic_site_from_bits(site_bits) else {
                 return molt_get_attr_object(obj_bits, attr_name_ptr, attr_name_len_bits);
             };
-
-            let obj = obj_from_bits(obj_bits);
-            let mut class_bits = 0;
-            let mut version = 0;
-            let mut ptr_opt = None;
-
-            if let Some(ptr) = obj.as_ptr() {
-                ptr_opt = Some(ptr);
-                if object_type_id(ptr) == TYPE_ID_OBJECT {
-                    let cb = object_class_bits(ptr);
-                    if cb != 0 {
-                        if let Some(class_ptr) = obj_from_bits(cb).as_ptr() {
-                            if object_type_id(class_ptr) == TYPE_ID_TYPE {
-                                class_bits = cb;
-                                version = class_layout_version_bits(class_ptr);
-                            }
-                        }
-                    }
-                }
-            }
-
-            if class_bits != 0 {
-                if let Some(entry) = ic_tls_lookup(_py, site_id, class_bits, version) {
-                    if entry.kind == ATTR_IC_KIND_INSTANCE_DICT {
-                        let dict_bits = instance_dict_bits(ptr_opt.unwrap());
-                        if dict_bits != 0 {
-                            if let Some(dict_ptr) = obj_from_bits(dict_bits).as_ptr() {
-                                if object_type_id(dict_ptr) == TYPE_ID_DICT {
-                                    if let Some(val_bits) = dict_get_in_place(_py, dict_ptr, entry.name_bits) {
-                                        inc_ref_bits(_py, val_bits);
-                                        profile_hit_unchecked(&ATTR_SITE_NAME_CACHE_HIT_COUNT);
-                                        return val_bits as i64;
-                                    }
-                                }
-                            }
-                        }
-                    }
-                }
-            }

             let attr_name_len = usize_from_bits(attr_name_len_bits);
             let slice = std::slice::from_raw_parts(attr_name_ptr, attr_name_len);
@@ -4718,22 +4670,6 @@

             let out = molt_get_attr_name(obj_bits, name_bits);

-            if out != MoltObject::none().bits() && class_bits != 0 && !exception_pending(_py) {
-                let ptr = ptr_opt.unwrap();
-                let dict_bits = instance_dict_bits(ptr);
-                if dict_bits != 0 {
-                    if let Some(dict_ptr) = obj_from_bits(dict_bits).as_ptr() {
-                        if object_type_id(dict_ptr) == TYPE_ID_DICT {
-                            if let Some(val_bits) = dict_get_in_place(_py, dict_ptr, name_bits) {
-                                if val_bits == out {
-                                    ic_tls_insert(_py, site_id, AttrIcEntry {
-                                        vm_epoch: 0,
-                                        class_bits,
-                                        version,
-                                        kind: ATTR_IC_KIND_INSTANCE_DICT,
-                                        name_bits,
-                                    });
-                                }
-                            }
-                        }
-                    }
-                }
-            }
-
             dec_ref_bits(_py, name_bits);
INNER_EOF
patch runtime/molt-runtime/src/builtins/attributes.rs < remove_ic.patch
