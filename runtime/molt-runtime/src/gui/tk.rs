//! Re-export bridge: delegates to `molt_runtime_tk::tk`.
//!
//! The canonical implementation lives in the extracted `molt_runtime_tk` crate.
//! This file owns only the exported C symbol names required by generated
//! intrinsic resolvers.

macro_rules! forward_tk_intrinsic {
    ($name:ident()) => {
        #[unsafe(no_mangle)]
        pub extern "C" fn $name() -> u64 {
            molt_runtime_tk::tk::$name()
        }
    };
    ($name:ident($($arg:ident),+ $(,)?)) => {
        #[unsafe(no_mangle)]
        pub extern "C" fn $name($($arg: u64),+) -> u64 {
            molt_runtime_tk::tk::$name($($arg),+)
        }
    };
}

forward_tk_intrinsic!(molt_tk_available());
forward_tk_intrinsic!(molt_tk_app_new(_options_bits));
forward_tk_intrinsic!(molt_tk_quit(app_bits));
forward_tk_intrinsic!(molt_tk_mainloop(app_bits));
forward_tk_intrinsic!(molt_tk_do_one_event(app_bits, flags_bits));
forward_tk_intrinsic!(molt_tk_after(app_bits, delay_ms_bits, callback_bits));
forward_tk_intrinsic!(molt_tk_after_idle(app_bits, callback_bits));
forward_tk_intrinsic!(molt_tk_after_cancel(app_bits, identifier_bits));
forward_tk_intrinsic!(molt_tk_after_info(app_bits, identifier_bits));
forward_tk_intrinsic!(molt_tk_call(app_bits, argv_bits));
forward_tk_intrinsic!(molt_tk_trace_add(
    app_bits,
    variable_name_bits,
    mode_bits,
    callback_bits,
));
forward_tk_intrinsic!(molt_tk_trace_remove(
    app_bits,
    variable_name_bits,
    mode_bits,
    cbname_bits,
));
forward_tk_intrinsic!(molt_tk_trace_info(app_bits, variable_name_bits));
forward_tk_intrinsic!(molt_tk_trace_clear(app_bits, variable_name_bits));
forward_tk_intrinsic!(molt_tk_tkwait_variable(app_bits, variable_name_bits));
forward_tk_intrinsic!(molt_tk_tkwait_window(app_bits, target_bits));
forward_tk_intrinsic!(molt_tk_tkwait_visibility(app_bits, target_bits));
forward_tk_intrinsic!(molt_tk_bind_callback_register(
    app_bits,
    target_bits,
    sequence_bits,
    callback_bits,
    add_bits,
));
forward_tk_intrinsic!(molt_tk_bind_callback_unregister(
    app_bits,
    target_bits,
    sequence_bits,
    command_name_bits,
));
forward_tk_intrinsic!(molt_tk_widget_bind_callback_register(
    app_bits,
    widget_path_bits,
    bind_target_bits,
    sequence_bits,
    callback_bits,
    add_bits,
));
forward_tk_intrinsic!(molt_tk_widget_bind_callback_unregister(
    app_bits,
    widget_path_bits,
    bind_target_bits,
    sequence_bits,
    command_name_bits,
));
forward_tk_intrinsic!(molt_tk_text_tag_bind_callback_register(
    app_bits,
    widget_path_bits,
    tagname_bits,
    sequence_bits,
    callback_bits,
    add_bits,
));
forward_tk_intrinsic!(molt_tk_text_tag_bind_callback_unregister(
    app_bits,
    widget_path_bits,
    tagname_bits,
    sequence_bits,
    command_name_bits,
));
forward_tk_intrinsic!(molt_tk_treeview_tag_bind_callback_register(
    app_bits,
    widget_path_bits,
    tagname_bits,
    sequence_bits,
    callback_bits,
));
forward_tk_intrinsic!(molt_tk_treeview_tag_bind_callback_unregister(
    app_bits,
    widget_path_bits,
    tagname_bits,
    sequence_bits,
    command_name_bits,
));
forward_tk_intrinsic!(molt_tk_bind_command(app_bits, name_bits, callback_bits));
forward_tk_intrinsic!(molt_tk_unbind_command(app_bits, name_bits));
forward_tk_intrinsic!(molt_tk_filehandler_create(
    app_bits,
    fd_bits,
    mask_bits,
    callback_bits,
    file_obj_bits,
));
forward_tk_intrinsic!(molt_tk_filehandler_delete(app_bits, fd_bits));
forward_tk_intrinsic!(molt_tk_destroy_widget(app_bits, widget_path_bits));
forward_tk_intrinsic!(molt_tk_last_error(app_bits));
forward_tk_intrinsic!(molt_tk_getboolean(value_bits));
forward_tk_intrinsic!(molt_tk_getdouble(value_bits));
forward_tk_intrinsic!(molt_tk_splitlist(value_bits));
forward_tk_intrinsic!(molt_tk_event_subst_parse(
    _widget_path_bits,
    event_args_bits
));
forward_tk_intrinsic!(molt_tk_bind_script_remove_command(
    script_bits,
    command_name_bits,
));
forward_tk_intrinsic!(molt_tk_errorinfo_append(app_bits, message_bits));
forward_tk_intrinsic!(molt_tk_dialog_show(
    app_bits,
    master_path_bits,
    title_bits,
    text_bits,
    bitmap_bits,
    default_index_bits,
    strings_bits,
));
forward_tk_intrinsic!(molt_tk_commondialog_show(
    app_bits,
    master_path_bits,
    command_bits,
    options_bits,
));
forward_tk_intrinsic!(molt_tk_messagebox_show(
    app_bits,
    master_path_bits,
    options_bits,
));
forward_tk_intrinsic!(molt_tk_filedialog_show(
    app_bits,
    master_path_bits,
    command_bits,
    options_bits,
));
forward_tk_intrinsic!(molt_tk_simpledialog_query(
    app_bits,
    parent_path_bits,
    title_bits,
    prompt_bits,
    initial_value_bits,
    query_kind_bits,
    min_value_bits,
    max_value_bits,
));
