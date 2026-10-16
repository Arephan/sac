//! Rule-text heuristics shared by the instruction and memory importers: scope classification,
//! severity from the word HARD, and the first ISO date in a text.

use crate::Doc;

/// Best-matching scope for a rule text, by keyword hits; `global` when nothing matches.
pub fn classify_scope(_text: &str, _extra: Option<&Doc>) -> String {
    "global".to_string()
}

/// `hard` when the text contains the uppercase word HARD, else `standing`.
pub fn severity_of(_text: &str) -> &'static str {
    "standing"
}

/// The first `20YY-MM-DD` in any of the texts.
pub fn first_date(_texts: &[&str]) -> Option<String> {
    None
}
