use super::callbacks::{
    after_callback_name_from_token, filehandler_command_name, filehandler_event_name,
    filehandler_poll_events, filehandler_revents_to_mask, lookup_after_command_for_token,
    lookup_after_kind_for_token, next_after_token, register_after_command_token,
    remove_after_events_for_tokens, schedule_after_timer_token, sort_after_info_tokens,
    tokens_for_after_command, unregister_after_command_token,
};
use super::commands::{tkwait_visibility_reached_in_app, tkwait_window_exists};
use super::dialogs::{
    apply_default_extension, clamp_dialog_selection, commondialog_allowed_options,
    commondialog_is_supported_command, commondialog_supports_parent,
    filedialog_is_supported_command, join_dialog_path, messagebox_icon_is_supported,
    normalize_color_literal, resolve_messagebox_selection,
};
use super::dispatch::pop_next_ready_event;
use super::event_commands::{
    event_generate_binding_sequences, parse_bind_script_commands,
    remove_bind_script_command_invocations, treeview_event_target_item,
};
use super::parsing::{
    first_missing_treeview_item, parse_bool_text, parse_expr_literal, parse_notebook_index_strict,
    parse_tcl_script_commands, parse_treeview_column_offset, parse_treeview_index_strict,
    parse_ttk_insert_index_strict, treeview_hit_item_by_y, treeview_item_is_descendant_of,
    treeview_visible_items,
};
use super::state::{
    TK_FILE_EVENT_EXCEPTION, TK_FILE_EVENT_READABLE, TK_FILE_EVENT_WRITABLE, TkAppState, TkEvent,
    TkExprLiteral, TkFileHandlerRegistration, TkGateState, TkOperation, TkRegistry,
    TkTraceRegistration, TkTreeviewItem, TkTreeviewState, TkWidgetState, TkWmState,
    format_permission_error_message, format_tk_unavailable_message,
    has_platform_preflight_blockers, wm_state_for_path, wm_state_for_path_mut,
};
#[cfg(all(not(target_arch = "wasm32"), feature = "native-tcl"))]
use super::tcl::{get, new, tcl_find_executable_arg};
use super::trace_commands::{
    bump_variable_version, collect_trace_callbacks_for_operation, normalize_trace_mode_name,
    split_array_variable_reference, trace_callback_command_words, trace_mode_matches,
    variable_version,
};
use std::collections::HashMap;

#[test]
fn permission_message_single_capability_stays_stable() {
    let state = TkGateState {
        missing_gui_window: true,
        ..TkGateState::default()
    };
    assert_eq!(
        format_permission_error_message(&state),
        "missing gui.window capability"
    );
}

#[test]
fn permission_message_multi_capability_stays_ordered() {
    let state = TkGateState {
        missing_gui_window: true,
        missing_process_spawn: true,
        ..TkGateState::default()
    };
    assert_eq!(
        format_permission_error_message(&state),
        "missing capabilities: gui.window, process.spawn"
    );
}

#[test]
fn unavailable_message_native_blockers_exclude_backend_not_implemented() {
    let state = TkGateState {
        wasm_unsupported: false,
        backend_unimplemented: false,
        missing_gui_window: true,
        missing_process_spawn: true,
        ..TkGateState::default()
    };
    assert_eq!(
        format_tk_unavailable_message(TkOperation::AppNew, &state),
        "tkinter runtime support is not implemented yet (molt_tk_app_new) [blockers: capability.gui.window, capability.process.spawn]"
    );
}

#[test]
fn unavailable_message_includes_platform_preflight_blockers() {
    let state = TkGateState {
        missing_linux_display: true,
        missing_macos_main_thread: true,
        ..TkGateState::default()
    };
    assert_eq!(
        format_tk_unavailable_message(TkOperation::AppNew, &state),
        "tkinter runtime support is not implemented yet (molt_tk_app_new) [blockers: platform.linux.display, platform.macos.main_thread]"
    );
}

#[test]
fn platform_preflight_blockers_helper_matches_state() {
    let mut state = TkGateState::default();
    assert!(!has_platform_preflight_blockers(&state));
    state.missing_linux_display = true;
    assert!(has_platform_preflight_blockers(&state));
    state.missing_linux_display = false;
    state.missing_macos_main_thread = true;
    assert!(has_platform_preflight_blockers(&state));
}

#[cfg(all(not(target_arch = "wasm32"), feature = "native-tcl"))]
#[test]
fn tcl_find_executable_arg_prefers_non_empty_path() {
    let arg = tcl_find_executable_arg();
    assert!(!arg.as_bytes().is_empty());
}

#[test]
fn tcl_script_parser_handles_quotes_braces_and_commands() {
    assert_eq!(
        parse_tcl_script_commands("  set   answer   42  "),
        vec![vec![
            "set".to_string(),
            "answer".to_string(),
            "42".to_string()
        ]]
    );
    assert_eq!(
        parse_tcl_script_commands("set a {x y}; set b \"quoted value\""),
        vec![
            vec!["set".to_string(), "a".to_string(), "x y".to_string()],
            vec![
                "set".to_string(),
                "b".to_string(),
                "quoted value".to_string()
            ],
        ]
    );
    assert!(parse_tcl_script_commands(" \t\n ").is_empty());
}

#[test]
fn trace_callback_words_parses_command_prefix_words() {
    assert_eq!(
        trace_callback_command_words("::__molt_trace_cb arg1 arg2"),
        vec![
            "::__molt_trace_cb".to_string(),
            "arg1".to_string(),
            "arg2".to_string(),
        ]
    );
}

#[test]
fn trace_callback_words_preserves_single_braced_word() {
    assert_eq!(
        trace_callback_command_words("{::__molt trace cb}"),
        vec!["::__molt trace cb".to_string()]
    );
}

#[test]
fn bind_script_parser_extracts_if_wrapper_command_words() {
    let script = "if {\"[::__molt_cb %# %x %y]\" == \"break\"} break";
    assert_eq!(
        parse_bind_script_commands(script),
        vec![vec![
            "::__molt_cb".to_string(),
            "%#".to_string(),
            "%x".to_string(),
            "%y".to_string(),
        ]]
    );
}

#[test]
fn bind_script_remove_command_drops_matching_if_wrapper_lines() {
    let script = concat!(
        "if {\"[::__molt_keep %#]\" == \"break\"} break\n",
        "if {\"[::__molt_drop %#]\" == \"break\"} break\n",
    );
    assert_eq!(
        remove_bind_script_command_invocations(script, "::__molt_drop"),
        "if {\"[::__molt_keep %#]\" == \"break\"} break\n"
    );
}

#[test]
fn bind_script_remove_command_preserves_non_matching_commands() {
    let script = "+::__molt_drop %x %y\n+::__molt_keep %x %y";
    assert_eq!(
        remove_bind_script_command_invocations(script, "::__molt_drop"),
        "+::__molt_keep %x %y"
    );
}

#[test]
fn bind_script_remove_command_handles_plus_prefixed_if_wrapper() {
    let script = concat!(
        "+if {\"[::__molt_drop %#]\" == \"break\"} break\n",
        "+if {\"[::__molt_keep %#]\" == \"break\"} break\n",
    );
    assert_eq!(
        remove_bind_script_command_invocations(script, "::__molt_drop"),
        "+if {\"[::__molt_keep %#]\" == \"break\"} break\n"
    );
}

#[test]
fn expr_literal_parsing_handles_int_and_float() {
    assert_eq!(parse_expr_literal("123"), Some(TkExprLiteral::Int(123)));
    assert_eq!(parse_expr_literal("3.5"), Some(TkExprLiteral::Float(3.5)));
    assert_eq!(parse_expr_literal("x + 1"), None);
}

#[test]
fn after_token_generation_is_deterministic() {
    let mut next_after_id = 0;
    assert_eq!(next_after_token(&mut next_after_id), "after#1");
    assert_eq!(next_after_token(&mut next_after_id), "after#2");
    assert_eq!(next_after_token(&mut next_after_id), "after#3");
}

#[test]
fn after_callback_name_derivation_is_deterministic() {
    assert_eq!(
        after_callback_name_from_token("after#7"),
        "::__molt_after_callback_7"
    );
    assert_eq!(
        after_callback_name_from_token("custom_token"),
        "::__molt_after_callback_custom_token"
    );
}

#[test]
fn after_helpers_remove_expected_tokens() {
    let mut app = TkAppState::default();
    register_after_command_token(&mut app, "after#1", "cmd_one", "timer");
    register_after_command_token(&mut app, "after#2", "cmd_one", "idle");
    register_after_command_token(&mut app, "after#3", "cmd_two", "timer");
    schedule_after_timer_token(&mut app, "after#1", 5);

    app.event_queue.push_back(TkEvent::Callback {
        token: "after#1".to_string(),
    });
    app.event_queue.push_back(TkEvent::Script {
        token: "after#2".to_string(),
        commands: vec![vec![
            "set".to_string(),
            "value".to_string(),
            "1".to_string(),
        ]],
    });
    app.event_queue.push_back(TkEvent::Callback {
        token: "after#3".to_string(),
    });

    assert_eq!(
        lookup_after_command_for_token(&app, "after#1").as_deref(),
        Some("cmd_one")
    );
    let tokens = tokens_for_after_command(&app, "cmd_one");
    assert_eq!(tokens.len(), 2);
    remove_after_events_for_tokens(&mut app, &tokens);

    assert_eq!(app.event_queue.len(), 1);
    let Some(TkEvent::Callback { token }) = app.event_queue.front() else {
        panic!("expected callback event");
    };
    assert_eq!(token, "after#3");

    unregister_after_command_token(&mut app, "after#3");
    assert!(lookup_after_kind_for_token(&app, "after#3").is_none());
    assert!(!app.after_due_at_ms.contains_key("after#3"));
}

#[test]
fn after_scheduler_waits_for_due_tokens_and_prefers_non_idle() {
    let mut app = TkAppState::default();
    register_after_command_token(&mut app, "after#timer", "timer_cmd", "timer");
    register_after_command_token(&mut app, "after#idle", "idle_cmd", "idle");
    schedule_after_timer_token(&mut app, "after#timer", 3);

    app.event_queue.push_back(TkEvent::Script {
        token: "after#idle".to_string(),
        commands: vec![vec!["set".to_string(), "idle".to_string(), "1".to_string()]],
    });
    app.event_queue.push_back(TkEvent::Script {
        token: "after#timer".to_string(),
        commands: vec![vec![
            "set".to_string(),
            "timer".to_string(),
            "1".to_string(),
        ]],
    });

    let first = pop_next_ready_event(&mut app).expect("first event");
    assert!(matches!(first, TkEvent::Script { token, .. } if token == "after#idle"));

    let second = pop_next_ready_event(&mut app);
    assert!(second.is_none());

    let third = pop_next_ready_event(&mut app).expect("third event");
    assert!(matches!(third, TkEvent::Script { token, .. } if token == "after#timer"));
}

#[test]
fn after_info_token_sorting_prefers_newest_after_ids() {
    let mut tokens = vec![
        "after#2".to_string(),
        "after#10".to_string(),
        "after#1".to_string(),
        "custom".to_string(),
    ];
    sort_after_info_tokens(&mut tokens);
    assert_eq!(
        tokens,
        vec![
            "after#10".to_string(),
            "after#2".to_string(),
            "after#1".to_string(),
            "custom".to_string(),
        ]
    );
}

#[test]
fn tkwait_window_exists_handles_root_handle_and_widget_paths() {
    let mut registry = TkRegistry::default();
    registry.apps.insert(7, TkAppState::default());
    assert!(tkwait_window_exists(&registry, 7, "."));
    assert!(!tkwait_window_exists(&registry, 8, "."));
    {
        let app = registry.apps.get_mut(&7).expect("app handle");
        app.widgets.insert(
            ".w".to_string(),
            TkWidgetState {
                widget_command: "frame".to_string(),
                ..TkWidgetState::default()
            },
        );
    }
    assert!(tkwait_window_exists(&registry, 7, ".w"));
    assert!(!tkwait_window_exists(&registry, 7, ".missing"));
}

#[test]
fn tkwait_visibility_tracks_root_wm_state_and_widget_manager() {
    let mut app = TkAppState::default();
    assert!(tkwait_visibility_reached_in_app(&app, "."));

    app.wm.state = "withdrawn".to_string();
    assert!(!tkwait_visibility_reached_in_app(&app, "."));

    app.wm.state = "normal".to_string();
    app.widgets.insert(
        ".w".to_string(),
        TkWidgetState {
            widget_command: "frame".to_string(),
            manager: Some("pack".to_string()),
            ..TkWidgetState::default()
        },
    );
    assert!(tkwait_visibility_reached_in_app(&app, ".w"));
    assert!(!tkwait_visibility_reached_in_app(&app, ".missing"));
}

#[test]
fn wm_state_is_isolated_per_toplevel_path() {
    let mut app = TkAppState::default();
    app.wm.title = "root".to_string();
    app.wm
        .protocols
        .insert("WM_DELETE_WINDOW".to_string(), "root_cb".to_string());
    app.widgets.insert(
        ".dialog".to_string(),
        TkWidgetState {
            widget_command: "toplevel".to_string(),
            wm: Some(TkWmState::default()),
            ..TkWidgetState::default()
        },
    );

    let dialog_wm = wm_state_for_path_mut(&mut app, ".dialog").expect("dialog wm");
    dialog_wm.title = "dialog".to_string();
    dialog_wm.transient = Some(".".to_string());
    dialog_wm
        .protocols
        .insert("WM_DELETE_WINDOW".to_string(), "dialog_cb".to_string());

    let root_wm = wm_state_for_path(&app, ".").expect("root wm");
    assert_eq!(root_wm.title, "root");
    assert_eq!(
        root_wm
            .protocols
            .get("WM_DELETE_WINDOW")
            .map(String::as_str),
        Some("root_cb")
    );
    assert!(root_wm.transient.is_none());

    let dialog_wm = wm_state_for_path(&app, ".dialog").expect("dialog wm");
    assert_eq!(dialog_wm.title, "dialog");
    assert_eq!(dialog_wm.transient.as_deref(), Some("."));
    assert_eq!(
        dialog_wm
            .protocols
            .get("WM_DELETE_WINDOW")
            .map(String::as_str),
        Some("dialog_cb")
    );
}

#[test]
fn split_array_variable_reference_handles_array_element_names() {
    assert_eq!(
        split_array_variable_reference("name"),
        ("name".to_string(), None)
    );
    assert_eq!(
        split_array_variable_reference("arr(key)"),
        ("arr".to_string(), Some("key".to_string()))
    );
    assert_eq!(
        split_array_variable_reference("(broken)"),
        ("(broken)".to_string(), None)
    );
}

#[test]
fn trace_mode_normalization_and_matching_are_stable() {
    assert_eq!(normalize_trace_mode_name("write").as_deref(), Ok("write"));
    assert_eq!(
        normalize_trace_mode_name("write read").as_deref(),
        Ok("read write")
    );
    assert_eq!(
        normalize_trace_mode_name("w, read, read, u").as_deref(),
        Ok("read write unset")
    );
    assert!(normalize_trace_mode_name("bogus").is_err());
    assert!(normalize_trace_mode_name("").is_err());

    assert!(trace_mode_matches("write", "write"));
    assert!(trace_mode_matches("read write", "read"));
    assert!(trace_mode_matches("read write", "write"));
    assert!(trace_mode_matches("unset", "unset"));
    assert!(!trace_mode_matches("read", "write"));
    assert!(!trace_mode_matches("", "read"));
}

#[test]
fn trace_callbacks_preserve_registration_order() {
    let mut app = TkAppState::default();
    app.traces.insert(
        "trace_var".to_string(),
        vec![
            TkTraceRegistration {
                mode_name: "write".to_string(),
                callback_name: "cb_write".to_string(),
                order: 20,
            },
            TkTraceRegistration {
                mode_name: "write".to_string(),
                callback_name: "cb_w".to_string(),
                order: 10,
            },
            TkTraceRegistration {
                mode_name: "read".to_string(),
                callback_name: "cb_read".to_string(),
                order: 30,
            },
        ],
    );

    let write_callbacks = collect_trace_callbacks_for_operation(&app, "trace_var", "write", None);
    assert_eq!(
        write_callbacks,
        vec![
            ("cb_w".to_string(), "write".to_string()),
            ("cb_write".to_string(), "write".to_string())
        ]
    );
    let read_callbacks = collect_trace_callbacks_for_operation(&app, "trace_var", "read", None);
    assert_eq!(
        read_callbacks,
        vec![("cb_read".to_string(), "read".to_string())]
    );
}

#[test]
fn trace_callbacks_include_array_mode_for_element_access() {
    let mut app = TkAppState::default();
    app.traces.insert(
        "arr".to_string(),
        vec![
            TkTraceRegistration {
                mode_name: "array".to_string(),
                callback_name: "cb_array".to_string(),
                order: 2,
            },
            TkTraceRegistration {
                mode_name: "write".to_string(),
                callback_name: "cb_write".to_string(),
                order: 1,
            },
        ],
    );

    let callbacks =
        collect_trace_callbacks_for_operation(&app, "arr(index)", "write", Some("index"));
    assert_eq!(
        callbacks,
        vec![
            ("cb_write".to_string(), "write".to_string()),
            ("cb_array".to_string(), "array".to_string()),
        ]
    );
}

#[test]
fn event_generate_binding_sequences_include_virtual_aliases() {
    let mut app = TkAppState::default();
    app.virtual_events.insert(
        "<<ProbeVirtual>>".to_string(),
        vec!["<KeyPress>".to_string(), "<Button-1>".to_string()],
    );
    let key_sequences = event_generate_binding_sequences(&app, "<KeyPress>");
    assert_eq!(
        key_sequences,
        vec!["<KeyPress>".to_string(), "<<ProbeVirtual>>".to_string()]
    );
    let virtual_sequences = event_generate_binding_sequences(&app, "<<ProbeVirtual>>");
    assert_eq!(virtual_sequences, vec!["<<ProbeVirtual>>".to_string()]);
}

#[test]
fn treeview_descendant_detection_handles_parent_chains() {
    let mut treeview = TkTreeviewState::default();
    treeview.items.insert(
        "root_child".to_string(),
        TkTreeviewItem {
            parent: "".to_string(),
            ..TkTreeviewItem::default()
        },
    );
    treeview.items.insert(
        "leaf".to_string(),
        TkTreeviewItem {
            parent: "root_child".to_string(),
            ..TkTreeviewItem::default()
        },
    );
    treeview.items.insert(
        "deep_leaf".to_string(),
        TkTreeviewItem {
            parent: "leaf".to_string(),
            ..TkTreeviewItem::default()
        },
    );

    assert!(treeview_item_is_descendant_of(
        &treeview,
        "deep_leaf",
        "root_child"
    ));
    assert!(treeview_item_is_descendant_of(
        &treeview,
        "leaf",
        "root_child"
    ));
    assert!(!treeview_item_is_descendant_of(
        &treeview,
        "root_child",
        "deep_leaf"
    ));
}

#[test]
fn treeview_event_target_item_priority_is_stable() {
    let mut treeview = TkTreeviewState::default();
    treeview
        .items
        .insert("i1".to_string(), TkTreeviewItem::default());
    treeview
        .items
        .insert("i2".to_string(), TkTreeviewItem::default());
    treeview
        .items
        .insert("i3".to_string(), TkTreeviewItem::default());
    treeview.focus = Some("i2".to_string());
    treeview.selection = vec!["i3".to_string()];

    let mut options = HashMap::new();
    options.insert("-item".to_string(), "i1".to_string());
    assert_eq!(
        treeview_event_target_item(&treeview, &options).as_deref(),
        Some("i1")
    );

    options.clear();
    assert_eq!(
        treeview_event_target_item(&treeview, &options).as_deref(),
        Some("i2")
    );

    treeview.focus = None;
    assert_eq!(
        treeview_event_target_item(&treeview, &options).as_deref(),
        Some("i3")
    );
}

#[test]
fn treeview_visible_order_and_hit_testing_are_deterministic() {
    let mut treeview = TkTreeviewState {
        root_children: vec!["r1".to_string(), "r2".to_string()],
        ..TkTreeviewState::default()
    };
    treeview.items.insert(
        "r1".to_string(),
        TkTreeviewItem {
            parent: String::new(),
            children: vec!["c1".to_string()],
            ..TkTreeviewItem::default()
        },
    );
    treeview.items.insert(
        "c1".to_string(),
        TkTreeviewItem {
            parent: "r1".to_string(),
            ..TkTreeviewItem::default()
        },
    );
    treeview.items.insert(
        "r2".to_string(),
        TkTreeviewItem {
            parent: String::new(),
            ..TkTreeviewItem::default()
        },
    );

    assert_eq!(
        treeview_visible_items(&treeview),
        vec!["r1".to_string(), "c1".to_string(), "r2".to_string()]
    );
    assert_eq!(treeview_hit_item_by_y(&treeview, 0).as_deref(), Some("r1"));
    assert_eq!(treeview_hit_item_by_y(&treeview, 20).as_deref(), Some("c1"));
    assert_eq!(treeview_hit_item_by_y(&treeview, 40).as_deref(), Some("r2"));
    assert_eq!(treeview_hit_item_by_y(&treeview, 60).as_deref(), None);
}

#[test]
fn treeview_column_offset_parser_accepts_hash_indices() {
    assert_eq!(parse_treeview_column_offset("#0"), Some(0));
    assert_eq!(parse_treeview_column_offset("#1"), Some(120));
    assert_eq!(parse_treeview_column_offset("#2"), Some(240));
    assert_eq!(parse_treeview_column_offset("#-1"), None);
    assert_eq!(parse_treeview_column_offset("1"), None);
    assert_eq!(parse_treeview_column_offset("bad"), None);
}

#[test]
fn treeview_strict_index_parser_rejects_non_integer_tokens() {
    assert_eq!(parse_treeview_index_strict("end", 4), Some(4));
    assert_eq!(parse_treeview_index_strict("2", 4), Some(2));
    assert_eq!(parse_treeview_index_strict("-7", 4), Some(0));
    assert_eq!(parse_treeview_index_strict("oops", 4), None);
}

#[test]
fn ttk_insert_index_parser_rejects_non_integer_tokens() {
    assert_eq!(parse_ttk_insert_index_strict("end", 3), Some(3));
    assert_eq!(parse_ttk_insert_index_strict("1", 3), Some(1));
    assert_eq!(parse_ttk_insert_index_strict("-2", 3), Some(0));
    assert_eq!(parse_ttk_insert_index_strict("bad", 3), None);
}

#[test]
fn notebook_index_parser_enforces_bounds() {
    assert_eq!(parse_notebook_index_strict("end", 2), Ok(2));
    assert_eq!(parse_notebook_index_strict("0", 2), Ok(0));
    assert_eq!(parse_notebook_index_strict("1", 2), Ok(1));
    assert!(parse_notebook_index_strict("-1", 2).is_err());
    assert!(parse_notebook_index_strict("2", 2).is_err());
    assert!(parse_notebook_index_strict("tabx", 2).is_err());
}

#[test]
fn treeview_missing_item_detection_reports_first_missing_id() {
    let mut treeview = TkTreeviewState::default();
    treeview
        .items
        .insert("i1".to_string(), TkTreeviewItem::default());
    treeview
        .items
        .insert("i2".to_string(), TkTreeviewItem::default());

    let items = vec!["i1".to_string(), "missing".to_string(), "i2".to_string()];
    assert_eq!(
        first_missing_treeview_item(&treeview, &items),
        Some("missing")
    );

    let existing = vec!["i1".to_string(), "i2".to_string()];
    assert_eq!(first_missing_treeview_item(&treeview, &existing), None);
}

#[test]
fn variable_version_progress_is_monotonic() {
    let mut app = TkAppState::default();
    assert_eq!(variable_version(&app, "name"), 0);
    bump_variable_version(&mut app, "name");
    assert_eq!(variable_version(&app, "name"), 1);
    bump_variable_version(&mut app, "name");
    assert_eq!(variable_version(&app, "name"), 2);
}

#[test]
fn commondialog_supported_command_allowlist_is_stable() {
    for command in [
        "tk_messageBox",
        "tk_getOpenFile",
        "tk_getSaveFile",
        "tk_chooseDirectory",
        "tk_chooseColor",
    ] {
        assert!(commondialog_is_supported_command(command));
        assert!(commondialog_supports_parent(command));
    }
    assert!(!commondialog_is_supported_command("tk_chooseFont"));
    assert!(!commondialog_supports_parent("tk_chooseFont"));
}

#[test]
fn commondialog_option_allowlists_cover_core_dialog_flags() {
    assert!(
        commondialog_allowed_options("tk_messageBox")
            .iter()
            .any(|name| name.eq_ignore_ascii_case("-type"))
    );
    assert!(
        commondialog_allowed_options("tk_getOpenFile")
            .iter()
            .any(|name| name.eq_ignore_ascii_case("-multiple"))
    );
    assert!(
        commondialog_allowed_options("tk_chooseDirectory")
            .iter()
            .any(|name| name.eq_ignore_ascii_case("-mustexist"))
    );
    assert!(
        !commondialog_allowed_options("tk_chooseColor")
            .iter()
            .any(|name| name.eq_ignore_ascii_case("-multiple"))
    );
}

#[test]
fn bool_text_parser_accepts_tcl_prefix_forms() {
    assert_eq!(parse_bool_text("t"), Some(true));
    assert_eq!(parse_bool_text("tr"), Some(true));
    assert_eq!(parse_bool_text("y"), Some(true));
    assert_eq!(parse_bool_text("f"), Some(false));
    assert_eq!(parse_bool_text("fa"), Some(false));
    assert_eq!(parse_bool_text("n"), Some(false));
    assert_eq!(parse_bool_text("o"), None);
    assert_eq!(parse_bool_text(""), None);
}

#[test]
fn filedialog_supported_command_allowlist_is_stable() {
    assert!(filedialog_is_supported_command("tk_getOpenFile"));
    assert!(filedialog_is_supported_command("tk_getSaveFile"));
    assert!(filedialog_is_supported_command("tk_chooseDirectory"));
    assert!(!filedialog_is_supported_command("tk_chooseColor"));
    assert!(!filedialog_is_supported_command("tk_chooseFont"));
}

#[test]
fn messagebox_selection_defaults_are_deterministic() {
    assert_eq!(
        resolve_messagebox_selection("ok", None).as_deref(),
        Ok("ok")
    );
    assert_eq!(
        resolve_messagebox_selection("yesnocancel", None).as_deref(),
        Ok("yes")
    );
    assert_eq!(
        resolve_messagebox_selection("yesno", Some("no")).as_deref(),
        Ok("no")
    );
    assert!(resolve_messagebox_selection("bogus", None).is_err());
    assert!(resolve_messagebox_selection("ok", Some("cancel")).is_err());
}

#[test]
fn messagebox_icon_validation_is_stable() {
    for icon in ["error", "info", "question", "warning", "ERROR"] {
        assert!(messagebox_icon_is_supported(icon));
    }
    assert!(!messagebox_icon_is_supported("bogus"));
}

#[test]
fn dialog_path_joining_handles_common_forms() {
    assert_eq!(join_dialog_path("", ""), "");
    assert_eq!(join_dialog_path("/tmp", "out.txt"), "/tmp/out.txt");
    assert_eq!(join_dialog_path("/tmp/", "out.txt"), "/tmp/out.txt");
    assert_eq!(
        join_dialog_path("C:\\Users\\me", "out.txt"),
        "C:\\Users\\me\\out.txt"
    );
}

#[test]
fn default_extension_application_is_stable() {
    assert_eq!(apply_default_extension("", ".txt"), "");
    assert_eq!(
        apply_default_extension("/tmp/output", ".txt"),
        "/tmp/output.txt"
    );
    assert_eq!(
        apply_default_extension("/tmp/output", "log"),
        "/tmp/output.log"
    );
    assert_eq!(
        apply_default_extension("/tmp/output.txt", ".log"),
        "/tmp/output.txt"
    );
}

#[test]
fn color_literal_normalization_supports_short_and_long_hex() {
    assert_eq!(normalize_color_literal("#abc").as_deref(), Some("#aabbcc"));
    assert_eq!(
        normalize_color_literal("#A1b2C3").as_deref(),
        Some("#A1b2C3")
    );
    assert_eq!(normalize_color_literal("red").as_deref(), Some("red"));
    assert!(normalize_color_literal("#zzzzzz").is_none());
}

#[test]
fn dialog_selection_clamping_is_stable() {
    assert_eq!(clamp_dialog_selection(-2, 0), 0);
    assert_eq!(clamp_dialog_selection(-2, 3), 0);
    assert_eq!(clamp_dialog_selection(1, 3), 1);
    assert_eq!(clamp_dialog_selection(9, 3), 2);
}

#[cfg(all(not(target_arch = "wasm32"), feature = "native-tcl"))]
#[test]
fn filehandler_event_name_mapping_is_stable() {
    assert_eq!(
        filehandler_event_name(TK_FILE_EVENT_READABLE),
        Some("readable")
    );
    assert_eq!(
        filehandler_event_name(TK_FILE_EVENT_WRITABLE),
        Some("writable")
    );
    assert_eq!(
        filehandler_event_name(TK_FILE_EVENT_EXCEPTION),
        Some("exception")
    );
    assert_eq!(filehandler_event_name(0), None);
}

#[test]
fn filehandler_command_name_is_deterministic() {
    assert_eq!(
        filehandler_command_name(41, "readable"),
        "::__molt_filehandler_41_readable"
    );
    assert_eq!(
        filehandler_command_name(7, "exception"),
        "::__molt_filehandler_7_exception"
    );
}

#[cfg(all(unix, not(target_arch = "wasm32"), not(feature = "native-tcl")))]
#[test]
fn filehandler_poll_event_mask_is_stable() {
    let mut registration = TkFileHandlerRegistration {
        callback_bits: 0,
        file_obj_bits: 0,
        commands: HashMap::new(),
    };
    registration
        .commands
        .insert(TK_FILE_EVENT_READABLE, "r".to_string());
    registration
        .commands
        .insert(TK_FILE_EVENT_WRITABLE, "w".to_string());
    assert_eq!(
        filehandler_poll_events(&registration),
        libc::POLLIN | libc::POLLOUT
    );
    registration
        .commands
        .insert(TK_FILE_EVENT_EXCEPTION, "x".to_string());
    assert_eq!(
        filehandler_poll_events(&registration),
        libc::POLLIN | libc::POLLOUT | libc::POLLPRI
    );
}

#[cfg(all(unix, not(target_arch = "wasm32"), not(feature = "native-tcl")))]
#[test]
fn filehandler_revents_translation_is_stable() {
    assert_eq!(
        filehandler_revents_to_mask(libc::POLLIN),
        TK_FILE_EVENT_READABLE
    );
    assert_eq!(
        filehandler_revents_to_mask(libc::POLLOUT),
        TK_FILE_EVENT_WRITABLE
    );
    assert_eq!(
        filehandler_revents_to_mask(libc::POLLERR | libc::POLLNVAL),
        TK_FILE_EVENT_EXCEPTION
    );
    assert_eq!(
        filehandler_revents_to_mask(libc::POLLHUP | libc::POLLPRI),
        TK_FILE_EVENT_READABLE | TK_FILE_EVENT_EXCEPTION
    );
}
