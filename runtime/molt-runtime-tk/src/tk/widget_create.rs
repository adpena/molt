use super::*;

pub(super) fn handle_widget_create_command(
    py: &PyToken,
    handle: i64,
    widget_command: &str,
    args: &[u64],
) -> Result<u64, u64> {
    if args.len() < 2 {
        return Err(raise_tcl_for_handle(
            py,
            handle,
            "widget creation requires a widget path",
        ));
    }
    let widget_path = get_string_arg(py, handle, args[1], "widget path")?;
    if !widget_path.starts_with('.') {
        return Err(raise_tcl_for_handle(
            py,
            handle,
            "widget path must start with '.'",
        ));
    }
    if !(args.len() - 2).is_multiple_of(2) {
        return Err(raise_tcl_for_handle(
            py,
            handle,
            "widget configure options must be key/value pairs",
        ));
    }
    let mut option_names = Vec::with_capacity((args.len() - 2) / 2);
    for idx in (2..args.len()).step_by(2) {
        option_names.push(get_string_arg(py, handle, args[idx], "widget option name")?);
    }
    let mut registry = tk_registry().lock().unwrap();
    let app = app_mut_from_registry(py, &mut registry, handle)?;
    if let Some(old_widget) = app.widgets.remove(&widget_path) {
        clear_widget_refs(py, old_widget);
    }
    let mut options = HashMap::new();
    for (idx, option_name) in option_names.into_iter().enumerate() {
        let value_bits = args[3 + idx * 2];
        inc_ref_bits(py, value_bits);
        if let Some(old_bits) = options.insert(option_name, value_bits) {
            dec_ref_bits(py, old_bits);
        }
    }
    app.widgets.insert(
        widget_path.clone(),
        TkWidgetState {
            widget_command: widget_command.to_string(),
            options,
            wm: (widget_command == "toplevel").then(TkWmState::default),
            treeview: (widget_command == "ttk::treeview").then(TkTreeviewState::default),
            ..TkWidgetState::default()
        },
    );
    app.last_error = None;
    drop(registry);
    alloc_string_bits(py, &widget_path)
}

pub(super) fn is_widget_constructor_command(command: &str) -> bool {
    matches!(
        command,
        "button"
            | "canvas"
            | "checkbutton"
            | "entry"
            | "frame"
            | "label"
            | "labelframe"
            | "listbox"
            | "menu"
            | "menubutton"
            | "message"
            | "panedwindow"
            | "radiobutton"
            | "scale"
            | "scrollbar"
            | "spinbox"
            | "text"
            | "toplevel"
            | "ttk::widget"
            | "ttk::button"
            | "ttk::checkbutton"
            | "ttk::combobox"
            | "ttk::entry"
            | "ttk::frame"
            | "ttk::label"
            | "ttk::labelframe"
            | "ttk::menubutton"
            | "ttk::notebook"
            | "ttk::panedwindow"
            | "ttk::progressbar"
            | "ttk::radiobutton"
            | "ttk::scale"
            | "ttk::scrollbar"
            | "ttk::separator"
            | "ttk::sizegrip"
            | "ttk::spinbox"
            | "ttk::treeview"
            | "tixBalloon"
            | "tixButtonBox"
            | "tixCObjView"
            | "tixCheckList"
            | "tixComboBox"
            | "tixControl"
            | "tixDialogShell"
            | "tixDirList"
            | "tixDirSelectBox"
            | "tixDirSelectDialog"
            | "tixDirTree"
            | "tixExFileSelectBox"
            | "tixExFileSelectDialog"
            | "tixFileEntry"
            | "tixFileSelectBox"
            | "tixFileSelectDialog"
            | "tixForm"
            | "tixGrid"
            | "tixHList"
            | "tixItemizedWidget"
            | "tixLabelEntry"
            | "tixLabelFrame"
            | "tixListNoteBook"
            | "tixMainWindow"
            | "tixMeter"
            | "tixNoteBook"
            | "tixNoteBookFrame"
            | "tixOptionMenu"
            | "tixPanedWindow"
            | "tixPopupMenu"
            | "tixResizeHandle"
            | "tixScrolledGrid"
            | "tixScrolledHList"
            | "tixScrolledListBox"
            | "tixScrolledTList"
            | "tixScrolledText"
            | "tixScrolledWindow"
            | "tixSelect"
            | "tixShell"
            | "tixStdButtonBox"
            | "tixTList"
            | "tixTree"
    )
}
