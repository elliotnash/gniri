// SPDX-License-Identifier: GPL-3.0-or-later

//! The text of a notification, made safe to show.
//!
//! Applications may send a little markup in a body: `<b>`, `<i>`, `<u>` and
//! the five XML entities. Anything else is shown as the characters it is.
//! Ports of `fixMarkup` (`js/misc/util.js`) and of the trimming in
//! `Message` (`js/ui/messageList.js`).

/// messageList.js `MAX_TITLE_LENGTH`.
pub const MAX_TITLE_LENGTH: usize = 150;
/// messageList.js `MAX_BODY_LENGTH`.
pub const MAX_BODY_LENGTH: usize = 850;

const ENTITIES: [&str; 5] = ["amp;", "quot;", "apos;", "lt;", "gt;"];
const TAGS: [&str; 6] = ["b>", "i>", "u>", "/b>", "/i>", "/u>"];

/// Escapes every character markup gives a meaning to.
pub fn escape(text: &str) -> String {
    let mut escaped = String::with_capacity(text.len());
    for character in text.chars() {
        match character {
            '&' => escaped.push_str("&amp;"),
            '<' => escaped.push_str("&lt;"),
            '>' => escaped.push_str("&gt;"),
            '\'' => escaped.push_str("&apos;"),
            '"' => escaped.push_str("&quot;"),
            other => escaped.push(other),
        }
    }
    escaped
}

/// Whether the `<b>`, `<i>` and `<u>` tags of `text` open and close in
/// order: what Pango would accept.
fn well_formed(text: &str) -> bool {
    let mut open = Vec::new();
    let mut rest = text;
    while let Some(start) = rest.find('<') {
        rest = &rest[start + 1..];
        let Some(tag) = TAGS.iter().find(|tag| rest.starts_with(**tag)) else {
            return false;
        };
        match tag.strip_prefix('/') {
            Some(name) => {
                if open.pop() != Some(name) {
                    return false;
                }
            }
            None => open.push(*tag),
        }
        rest = &rest[tag.len()..];
    }
    open.is_empty()
}

/// Pango markup that shows `text`: with its `<b>`, `<i>`, `<u>` and
/// entities kept if `allow_markup` and they are well formed, and as plain
/// characters otherwise.
pub fn fix(text: &str, allow_markup: bool) -> String {
    if allow_markup {
        let mut fixed = String::with_capacity(text.len());
        let mut rest = text;
        while let Some(start) = rest.find(['&', '<']) {
            fixed.push_str(&rest[..start]);
            let (mark, after) = rest[start..].split_at(1);
            let known: &[&str] = if mark == "&" { &ENTITIES } else { &TAGS };
            if known.iter().any(|known| after.starts_with(known)) {
                fixed.push_str(mark);
            } else {
                fixed.push_str(if mark == "&" { "&amp;" } else { "&lt;" });
            }
            rest = after;
        }
        fixed.push_str(rest);
        if well_formed(&fixed) {
            return fixed;
        }
    }
    escape(text)
}

/// At most `limit` characters of `text`, on one line.
fn one_line(text: &str, limit: usize) -> String {
    text.chars()
        .take(limit)
        .map(|character| if character == '\n' { ' ' } else { character })
        .collect()
}

/// A notification's title as markup (`Message.title`).
pub fn title(text: &str) -> String {
    escape(&one_line(text, MAX_TITLE_LENGTH))
}

/// A notification's body as markup (`Message.body`).
pub fn body(text: &str) -> String {
    fix(&one_line(text, MAX_BODY_LENGTH), true)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn allowed_markup_is_kept() {
        assert_eq!(
            fix("<b>Bold</b> and <i>it</i>", true),
            "<b>Bold</b> and <i>it</i>"
        );
        assert_eq!(fix("a &amp; b &lt;c&gt;", true), "a &amp; b &lt;c&gt;");
        assert_eq!(fix("<u><b>both</b></u>", true), "<u><b>both</b></u>");
    }

    #[test]
    fn anything_else_is_shown_as_typed() {
        assert_eq!(fix("Tom & Jerry", true), "Tom &amp; Jerry");
        assert_eq!(fix("1 < 2", true), "1 &lt; 2");
        assert_eq!(
            fix("<a href=\"x\">link</a>", true),
            "&lt;a href=\"x\">link&lt;/a>"
        );
        assert_eq!(fix("&nbsp;", true), "&amp;nbsp;");
        assert_eq!(fix("<b>Bold</b>", false), "&lt;b&gt;Bold&lt;/b&gt;");
    }

    #[test]
    fn broken_markup_is_escaped_whole() {
        assert_eq!(fix("<b>never closed", true), "&lt;b&gt;never closed");
        assert_eq!(
            fix("<b><i>crossed</b></i>", true),
            escape("<b><i>crossed</b></i>")
        );
        assert_eq!(fix("closed</b>", true), "closed&lt;/b&gt;");
    }

    #[test]
    fn titles_and_bodies_are_one_trimmed_line() {
        assert_eq!(title("Two\nlines <b>"), "Two lines &lt;b&gt;");
        assert_eq!(body("Two\n<b>lines</b>"), "Two <b>lines</b>");
        assert_eq!(title(&"x".repeat(400)).len(), MAX_TITLE_LENGTH);
        assert_eq!(body(&"é".repeat(2000)).chars().count(), MAX_BODY_LENGTH);
    }
}
