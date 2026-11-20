//! Connection colours: the named choices the dialogs offer and the colour strings saved in
//! `connections.toml`. A saved colour is a `#rrggbb` string.

use gpui::{Hsla, rgb};

/// The colour choices in dialog order; the first inherits from the folder.
pub const CHOICES: [(&str, Option<&str>); 9] = [
    ("Inherit from folder", None),
    ("Red", Some("#d13438")),
    ("Orange", Some("#ca5010")),
    ("Yellow", Some("#c19c00")),
    ("Green", Some("#107c10")),
    ("Teal", Some("#038387")),
    ("Blue", Some("#0078d4")),
    ("Purple", Some("#8764b8")),
    ("Grey", Some("#69797e")),
];

/// The choice for a saved colour.
pub fn known(colour: Option<&str>) -> Option<&'static str> {
    CHOICES
        .iter()
        .find_map(|(_, value)| value.filter(|v| Some(*v) == colour))
}

/// The choice's name for a saved colour, or `None` when it has none or isn't one of ours.
pub fn name(colour: Option<&str>) -> Option<&'static str> {
    let colour = known(colour)?;
    CHOICES
        .iter()
        .find(|(_, value)| *value == Some(colour))
        .map(|(label, _)| *label)
}

/// The colour to draw for a saved `#rrggbb` string; `None` if it isn't one.
pub fn hsla(colour: &str) -> Option<Hsla> {
    let hex = colour.strip_prefix('#').filter(|h| h.len() == 6)?;
    let value = u32::from_str_radix(hex, 16).ok()?;
    Some(rgb(value).into())
}
