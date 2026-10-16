//! Terminal formatting helpers. Colour only when stdout is a terminal and `NO_COLOR` is unset.

use crate::layers::count_layers;
use crate::Doc;
use owo_colors::OwoColorize;
use std::io::IsTerminal;

pub fn color_enabled() -> bool {
    std::io::stdout().is_terminal() && std::env::var_os("NO_COLOR").is_none()
}

pub fn green(s: &str) -> String {
    if color_enabled() {
        s.green().to_string()
    } else {
        s.to_string()
    }
}
pub fn red(s: &str) -> String {
    if color_enabled() {
        s.red().bold().to_string()
    } else {
        s.to_string()
    }
}
pub fn yellow(s: &str) -> String {
    if color_enabled() {
        s.yellow().to_string()
    } else {
        s.to_string()
    }
}
pub fn dim(s: &str) -> String {
    if color_enabled() {
        s.dimmed().to_string()
    } else {
        s.to_string()
    }
}

/// `[PASS]` / `[FAIL]` / `[WARN]` tokens inside a text block get their colour.
pub fn paint_statuses(text: &str) -> String {
    if !color_enabled() {
        return text.to_string();
    }
    text.replace("[PASS]", &format!("[{}]", "PASS".green()))
        .replace("[FAIL]", &format!("[{}]", "FAIL".red().bold()))
        .replace("[WARN]", &format!("[{}]", "WARN".yellow()))
}

/// The per-layer counts table.
pub fn counts_block(doc: &Doc, title: &str) -> String {
    let mut lines = vec![title.to_string()];
    for (k, v) in count_layers(doc) {
        let parts: Vec<String> = v.per.iter().map(|(kk, n)| format!("{kk} {n}")).collect();
        lines.push(format!("  {:<13} {:>4}   {}", k, v.total, parts.join(", ")));
    }
    lines.join("\n")
}
