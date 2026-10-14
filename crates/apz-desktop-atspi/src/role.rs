//! AT-SPI role names, normalised to the vocabulary `apz_desktop::UiNode::role` uses.

/// The normalised role of an AT-SPI role name (`atspi::Role::name()`). Roles the table does not name keep their
/// AT-SPI name in snake case, so a new AT-SPI role still reads sensibly rather than vanishing.
pub(crate) fn apz_role(atspi: &str) -> String {
    let named = match atspi {
        // Both spellings exist: newer libatspi says "push button", the atspi crate's table says "button".
        "push button" | "button" => "button",
        "toggle button" => "toggle_button",
        "check box" => "check_box",
        "radio button" => "radio_button",
        "check menu item" => "check_menu_item",
        "radio menu item" => "radio_menu_item",
        "menu item" | "tearoff menu item" => "menu_item",
        "menu" => "menu",
        "menu bar" => "menu_bar",
        "popup menu" => "popup_menu",
        "combo box" => "combo_box",
        "spin button" => "spin_button",
        "slider" => "slider",
        // AT-SPI "text" is an editable text control; "static" and "label" are text that is only shown.
        "text" | "entry" => "text_field",
        "password text" => "password_field",
        "static" | "label" => "text",
        "frame" | "window" => "window",
        "link" => "link",
        "image" | "icon" => "image",
        "list" | "list box" => "list",
        "list item" => "list_item",
        "page tab" => "tab",
        "page tab list" => "tab_list",
        "table" => "table",
        "table cell" => "table_cell",
        "table row" => "table_row",
        "tree" => "tree",
        "tree item" => "tree_item",
        "progress bar" => "progress_bar",
        "scroll bar" => "scroll_bar",
        "tool bar" => "tool_bar",
        "status bar" => "status_bar",
        "tool tip" => "tool_tip",
        "panel" => "panel",
        "filler" | "grouping" | "section" => "group",
        "heading" => "heading",
        "paragraph" => "paragraph",
        "separator" => "separator",
        "terminal" => "terminal",
        "document frame"
        | "document text"
        | "document web"
        | "document email"
        | "document spreadsheet"
        | "document presentation" => "document",
        _ => return atspi.replace(' ', "_"),
    };
    named.to_owned()
}

#[cfg(test)]
mod tests {
    use super::apz_role;

    #[test]
    fn the_common_controls_map_to_the_names_the_desktop_uses() {
        let cases = [
            ("push button", "button"),
            ("button", "button"),
            ("check box", "check_box"),
            ("radio button", "radio_button"),
            ("text", "text_field"),
            ("entry", "text_field"),
            ("password text", "password_field"),
            ("label", "text"),
            ("static", "text"),
            ("menu item", "menu_item"),
            ("frame", "window"),
            ("window", "window"),
            ("application", "application"),
            ("combo box", "combo_box"),
        ];
        for (atspi, apz) in cases {
            assert_eq!(apz_role(atspi), apz, "AT-SPI role {atspi:?}");
        }
    }

    #[test]
    fn a_role_the_table_does_not_name_keeps_its_name_in_snake_case() {
        assert_eq!(apz_role("dial"), "dial");
        assert_eq!(apz_role("color chooser"), "color_chooser");
    }
}
