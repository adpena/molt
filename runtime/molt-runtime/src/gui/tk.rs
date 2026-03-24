//! Re-export bridge: delegates to `molt_runtime_tk::tk`.
//!
//! The canonical implementation lives in the extracted `molt_runtime_tk` crate.
//! This file provides `#[unsafe(no_mangle)]` entry points so the linker
//! exports them with the expected symbol names.

#[unsafe(no_mangle)]
pub extern "C" fn molt_tk_available() -> u64 {
    molt_runtime_tk::tk::molt_tk_available()
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_tk_app_new(_options_bits: u64) -> u64 {
    molt_runtime_tk::tk::molt_tk_app_new(_options_bits)
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_tk_quit(app_bits: u64) -> u64 {
    molt_runtime_tk::tk::molt_tk_quit(app_bits)
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_tk_mainloop(app_bits: u64) -> u64 {
    molt_runtime_tk::tk::molt_tk_mainloop(app_bits)
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_tk_do_one_event(app_bits: u64, flags_bits: u64) -> u64 {
    molt_runtime_tk::tk::molt_tk_do_one_event(app_bits, flags_bits)
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_tk_after(app_bits: u64, delay_ms_bits: u64, callback_bits: u64) -> u64 {
    molt_runtime_tk::tk::molt_tk_after(app_bits, delay_ms_bits, callback_bits)
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_tk_after_idle(app_bits: u64, callback_bits: u64) -> u64 {
    molt_runtime_tk::tk::molt_tk_after_idle(app_bits, callback_bits)
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_tk_after_cancel(app_bits: u64, identifier_bits: u64) -> u64 {
    molt_runtime_tk::tk::molt_tk_after_cancel(app_bits, identifier_bits)
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_tk_after_info(app_bits: u64, identifier_bits: u64) -> u64 {
    molt_runtime_tk::tk::molt_tk_after_info(app_bits, identifier_bits)
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_tk_call(app_bits: u64, argv_bits: u64) -> u64 {
    molt_runtime_tk::tk::molt_tk_call(app_bits, argv_bits)
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_tk_trace_add(app_bits: u64, variable_name_bits: u64, mode_bits: u64, callback_bits: u64) -> u64 {
    molt_runtime_tk::tk::molt_tk_trace_add(app_bits, variable_name_bits, mode_bits, callback_bits)
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_tk_trace_remove(app_bits: u64, variable_name_bits: u64, mode_bits: u64, cbname_bits: u64) -> u64 {
    molt_runtime_tk::tk::molt_tk_trace_remove(app_bits, variable_name_bits, mode_bits, cbname_bits)
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_tk_trace_info(app_bits: u64, variable_name_bits: u64) -> u64 {
    molt_runtime_tk::tk::molt_tk_trace_info(app_bits, variable_name_bits)
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_tk_trace_clear(app_bits: u64, variable_name_bits: u64) -> u64 {
    molt_runtime_tk::tk::molt_tk_trace_clear(app_bits, variable_name_bits)
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_tk_tkwait_variable(app_bits: u64, variable_name_bits: u64) -> u64 {
    molt_runtime_tk::tk::molt_tk_tkwait_variable(app_bits, variable_name_bits)
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_tk_tkwait_window(app_bits: u64, target_bits: u64) -> u64 {
    molt_runtime_tk::tk::molt_tk_tkwait_window(app_bits, target_bits)
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_tk_tkwait_visibility(app_bits: u64, target_bits: u64) -> u64 {
    molt_runtime_tk::tk::molt_tk_tkwait_visibility(app_bits, target_bits)
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_tk_bind_callback_register(app_bits: u64, target_bits: u64, sequence_bits: u64, callback_bits: u64, add_bits: u64) -> u64 {
    molt_runtime_tk::tk::molt_tk_bind_callback_register(app_bits, target_bits, sequence_bits, callback_bits, add_bits)
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_tk_bind_callback_unregister(app_bits: u64, target_bits: u64, sequence_bits: u64, command_name_bits: u64) -> u64 {
    molt_runtime_tk::tk::molt_tk_bind_callback_unregister(app_bits, target_bits, sequence_bits, command_name_bits)
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_tk_widget_bind_callback_register(app_bits: u64, widget_path_bits: u64, bind_target_bits: u64, sequence_bits: u64, callback_bits: u64, add_bits: u64) -> u64 {
    molt_runtime_tk::tk::molt_tk_widget_bind_callback_register(app_bits, widget_path_bits, bind_target_bits, sequence_bits, callback_bits, add_bits)
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_tk_widget_bind_callback_unregister(app_bits: u64, widget_path_bits: u64, bind_target_bits: u64, sequence_bits: u64, command_name_bits: u64) -> u64 {
    molt_runtime_tk::tk::molt_tk_widget_bind_callback_unregister(app_bits, widget_path_bits, bind_target_bits, sequence_bits, command_name_bits)
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_tk_text_tag_bind_callback_register(app_bits: u64, widget_path_bits: u64, tagname_bits: u64, sequence_bits: u64, callback_bits: u64, add_bits: u64) -> u64 {
    molt_runtime_tk::tk::molt_tk_text_tag_bind_callback_register(app_bits, widget_path_bits, tagname_bits, sequence_bits, callback_bits, add_bits)
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_tk_text_tag_bind_callback_unregister(app_bits: u64, widget_path_bits: u64, tagname_bits: u64, sequence_bits: u64, command_name_bits: u64) -> u64 {
    molt_runtime_tk::tk::molt_tk_text_tag_bind_callback_unregister(app_bits, widget_path_bits, tagname_bits, sequence_bits, command_name_bits)
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_tk_treeview_tag_bind_callback_register(app_bits: u64, widget_path_bits: u64, tagname_bits: u64, sequence_bits: u64, callback_bits: u64) -> u64 {
    molt_runtime_tk::tk::molt_tk_treeview_tag_bind_callback_register(app_bits, widget_path_bits, tagname_bits, sequence_bits, callback_bits)
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_tk_treeview_tag_bind_callback_unregister(app_bits: u64, widget_path_bits: u64, tagname_bits: u64, sequence_bits: u64, command_name_bits: u64) -> u64 {
    molt_runtime_tk::tk::molt_tk_treeview_tag_bind_callback_unregister(app_bits, widget_path_bits, tagname_bits, sequence_bits, command_name_bits)
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_tk_bind_command(app_bits: u64, name_bits: u64, callback_bits: u64) -> u64 {
    molt_runtime_tk::tk::molt_tk_bind_command(app_bits, name_bits, callback_bits)
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_tk_unbind_command(app_bits: u64, name_bits: u64) -> u64 {
    molt_runtime_tk::tk::molt_tk_unbind_command(app_bits, name_bits)
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_tk_filehandler_create(app_bits: u64, fd_bits: u64, mask_bits: u64, callback_bits: u64, file_obj_bits: u64) -> u64 {
    molt_runtime_tk::tk::molt_tk_filehandler_create(app_bits, fd_bits, mask_bits, callback_bits, file_obj_bits)
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_tk_filehandler_delete(app_bits: u64, fd_bits: u64) -> u64 {
    molt_runtime_tk::tk::molt_tk_filehandler_delete(app_bits, fd_bits)
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_tk_destroy_widget(app_bits: u64, widget_path_bits: u64) -> u64 {
    molt_runtime_tk::tk::molt_tk_destroy_widget(app_bits, widget_path_bits)
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_tk_last_error(app_bits: u64) -> u64 {
    molt_runtime_tk::tk::molt_tk_last_error(app_bits)
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_tk_getboolean(value_bits: u64) -> u64 {
    molt_runtime_tk::tk::molt_tk_getboolean(value_bits)
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_tk_getdouble(value_bits: u64) -> u64 {
    molt_runtime_tk::tk::molt_tk_getdouble(value_bits)
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_tk_splitlist(value_bits: u64) -> u64 {
    molt_runtime_tk::tk::molt_tk_splitlist(value_bits)
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_tk_event_subst_parse(_widget_path_bits: u64, event_args_bits: u64) -> u64 {
    molt_runtime_tk::tk::molt_tk_event_subst_parse(_widget_path_bits, event_args_bits)
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_tk_bind_script_remove_command(script_bits: u64, command_name_bits: u64) -> u64 {
    molt_runtime_tk::tk::molt_tk_bind_script_remove_command(script_bits, command_name_bits)
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_tk_errorinfo_append(app_bits: u64, message_bits: u64) -> u64 {
    molt_runtime_tk::tk::molt_tk_errorinfo_append(app_bits, message_bits)
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_tk_dialog_show(app_bits: u64, master_path_bits: u64, title_bits: u64, text_bits: u64, bitmap_bits: u64, default_index_bits: u64, strings_bits: u64) -> u64 {
    molt_runtime_tk::tk::molt_tk_dialog_show(app_bits, master_path_bits, title_bits, text_bits, bitmap_bits, default_index_bits, strings_bits)
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_tk_commondialog_show(app_bits: u64, master_path_bits: u64, command_bits: u64, options_bits: u64) -> u64 {
    molt_runtime_tk::tk::molt_tk_commondialog_show(app_bits, master_path_bits, command_bits, options_bits)
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_tk_messagebox_show(app_bits: u64, master_path_bits: u64, options_bits: u64) -> u64 {
    molt_runtime_tk::tk::molt_tk_messagebox_show(app_bits, master_path_bits, options_bits)
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_tk_filedialog_show(app_bits: u64, master_path_bits: u64, command_bits: u64, options_bits: u64) -> u64 {
    molt_runtime_tk::tk::molt_tk_filedialog_show(app_bits, master_path_bits, command_bits, options_bits)
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_tk_simpledialog_query(app_bits: u64, parent_path_bits: u64, title_bits: u64, prompt_bits: u64, initial_value_bits: u64, query_kind_bits: u64, min_value_bits: u64, max_value_bits: u64) -> u64 {
    molt_runtime_tk::tk::molt_tk_simpledialog_query(app_bits, parent_path_bits, title_bits, prompt_bits, initial_value_bits, query_kind_bits, min_value_bits, max_value_bits)
}
