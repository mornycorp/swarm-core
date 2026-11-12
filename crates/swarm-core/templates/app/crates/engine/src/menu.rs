//! Where a command sits in a menu and the keys that run it (docs/app-architecture.md › The minimum
//! interfaces): plain text in its spec, so no layer below the UI names a toolkit type. The UI maps
//! them to its toolkit's menus and keys; [`Registry::register`](crate::Registry::register) refuses a
//! path or a shortcut that breaks the forms below, and one another command has.
//!
//! - **A menu path** is the item's place: the menus from the menu bar down, then the item's own
//!   label, joined by `/` (`File/Open recent`, `Help/App info`). Two to [`MAX_DEPTH`] parts, each
//!   text of at most [`MAX_PART`] characters with no control character and no space at either end.
//! - **A shortcut** is portable: modifiers, then one key, joined by `+` (`Mod+O`, `Mod+Shift+P`,
//!   `F5`). `Mod` is the platform's command key (Cmd on macOS, Ctrl elsewhere); the modifiers are
//!   [`MODIFIERS`], each at most once and in that order. The key is a letter `A`-`Z`, a digit,
//!   `F1`-`F24` or one of [`NAMED_KEYS`].

/// The most parts a menu path has: the menus and the item.
pub const MAX_DEPTH: usize = 4;

/// The longest part of a menu path, in characters.
pub const MAX_PART: usize = 40;

/// A shortcut's modifiers, in the order a shortcut names them.
pub const MODIFIERS: [&str; 4] = ["Mod", "Ctrl", "Alt", "Shift"];

/// The keys a shortcut names by a word, separated by spaces.
pub const NAMED_KEYS: &str = "Enter Escape Tab Space Backspace Delete Insert Home End PageUp PageDown Up Down Left Right Plus Minus Comma Period Slash";

/// What is wrong with the menu path `path`, if anything.
pub fn menu_problem(path: &str) -> Option<&'static str> {
    let parts: Vec<&str> = path.split('/').collect();
    if parts.len() < 2 {
        return Some("it needs a menu and an item, `Menu/Item`");
    }
    if parts.len() > MAX_DEPTH {
        return Some("it has more than 4 parts");
    }
    parts.into_iter().find_map(part_problem)
}

fn part_problem(part: &str) -> Option<&'static str> {
    if part.trim().is_empty() {
        Some("a part is empty")
    } else if part.trim() != part {
        Some("a part has a space at an end")
    } else if part.chars().any(char::is_control) {
        Some("a part has a control character")
    } else if part.chars().count() > MAX_PART {
        Some("a part is longer than 40 characters")
    } else {
        None
    }
}

/// What is wrong with the shortcut `shortcut`, if anything.
pub fn shortcut_problem(shortcut: &str) -> Option<&'static str> {
    let mut parts: Vec<&str> = shortcut.split('+').collect();
    let key = parts.pop().unwrap_or_default();
    let mut next = 0;
    for modifier in parts {
        match MODIFIERS.iter().skip(next).position(|m| *m == modifier) {
            Some(at) => next += at + 1,
            None if MODIFIERS.contains(&modifier) => return Some("each modifier is named at most once, in the order Mod, Ctrl, Alt, Shift"),
            None => return Some("a modifier is one of Mod, Ctrl, Alt, Shift"),
        }
    }
    if is_key(key) { None } else { Some("the key is a letter A-Z, a digit, F1-F24 or a named key (Enter, Escape, Tab, ...)") }
}

/// Whether `key` is a key a shortcut may name.
fn is_key(key: &str) -> bool {
    let mut chars = key.chars();
    match (chars.next(), chars.next()) {
        (Some(c), None) => c.is_ascii_uppercase() || c.is_ascii_digit(),
        _ => NAMED_KEYS.split(' ').any(|named| named == key) || is_function_key(key),
    }
}

/// `F1` to `F24`, with no leading zero.
fn is_function_key(key: &str) -> bool {
    key.strip_prefix('F').is_some_and(|n| !n.starts_with('0') && n.parse::<u8>().is_ok_and(|n| (1..=24).contains(&n)))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_menu_path_is_menus_then_the_item() {
        for good in ["File/Open recent", "Help/App info", "View/Panels/Left/Show"] {
            assert_eq!(menu_problem(good), None, "{good}");
        }
        for bad in ["", "File", "File/", "/Open", "File//Open", "File/ Open", "File/Open\n", "A/B/C/D/E", &format!("File/{}", "x".repeat(41))] {
            assert!(menu_problem(bad).is_some(), "{bad:?}");
        }
    }

    #[test]
    fn a_shortcut_is_modifiers_in_order_then_one_key() {
        for good in ["Mod+O", "Mod+Shift+P", "Ctrl+Alt+Delete", "F5", "Shift+F12", "Mod+1", "Alt+Enter", "Mod+Plus"] {
            assert_eq!(shortcut_problem(good), None, "{good}");
        }
        for bad in ["", "+", "Mod+", "Mod+o", "Cmd+O", "Shift+Mod+O", "Mod+Mod+O", "F0", "F25", "F05", "Mod+OO", "Mod +O", "Mod+O+P", "Mod+é"] {
            assert!(shortcut_problem(bad).is_some(), "{bad:?}");
        }
    }
}
