//! Finding nodes in a widget or actor tree.
//!
//! Both gniri's dump (`gniri msg debug dump-tree`) and the GNOME reference
//! dumps are JSON trees of nodes with `type`, `name`, `classes`, `box` and
//! `children`. One selector syntax addresses both:
//!
//! ```text
//! selector := step (" " step)*          each step searches below the last
//! step     := [Type] ["#" name] ("." class)* [":" index]
//! ```
//!
//! A step matches depth first; `:index` picks the n-th match (default 0).
//! `#panelActivities .workspace-dot:1` is the second dot in the activities
//! button.

use anyhow::{Result, anyhow, bail};
use serde_json::Value;

struct Step<'a> {
    type_name: Option<&'a str>,
    name: Option<&'a str>,
    classes: Vec<&'a str>,
    index: usize,
}

fn parse_step(text: &str) -> Result<Step<'_>> {
    let (body, index) = match text.rsplit_once(':') {
        Some((body, index)) => (
            body,
            index
                .parse()
                .map_err(|_| anyhow!("bad index in selector step {text:?}"))?,
        ),
        None => (text, 0),
    };
    let first_mark = body.find(['#', '.']).unwrap_or(body.len());
    let type_name = Some(&body[..first_mark]).filter(|name| !name.is_empty());

    let mut name = None;
    let mut classes = Vec::new();
    let mut rest = &body[first_mark..];
    while !rest.is_empty() {
        let mark = &rest[..1];
        let end = rest[1..].find(['#', '.']).map_or(rest.len(), |at| at + 1);
        let word = &rest[1..end];
        if word.is_empty() {
            bail!("empty name in selector step {text:?}");
        }
        match mark {
            "#" => name = Some(word),
            _ => classes.push(word),
        }
        rest = &rest[end..];
    }
    Ok(Step {
        type_name,
        name,
        classes,
        index,
    })
}

/// A node's classes. gniri dumps a list, the GNOME dump one string.
pub fn classes(node: &Value) -> Vec<&str> {
    match &node["classes"] {
        Value::Array(list) => list.iter().filter_map(Value::as_str).collect(),
        Value::String(text) => text.split_whitespace().collect(),
        _ => Vec::new(),
    }
}

pub fn children(node: &Value) -> &[Value] {
    node["children"].as_array().map_or(&[], Vec::as_slice)
}

fn matches(node: &Value, step: &Step) -> bool {
    step.type_name
        .is_none_or(|type_name| node["type"] == type_name)
        && step.name.is_none_or(|name| node["name"] == name)
        && {
            let have = classes(node);
            step.classes.iter().all(|class| have.contains(class))
        }
}

fn collect<'a>(node: &'a Value, step: &Step, include_self: bool, found: &mut Vec<&'a Value>) {
    if include_self && matches(node, step) {
        found.push(node);
    }
    for child in children(node) {
        collect(child, step, true, found);
    }
}

/// The node `selector` picks below (or at) `root`.
pub fn select<'a>(root: &'a Value, selector: &str) -> Result<&'a Value> {
    let mut current = root;
    for (position, text) in selector.split_whitespace().enumerate() {
        let step = parse_step(text)?;
        let mut found = Vec::new();
        collect(current, &step, position == 0, &mut found);
        current = found.get(step.index).copied().ok_or_else(|| {
            anyhow!(
                "selector {selector:?}: step {text:?} matched {} node(s)",
                found.len()
            )
        })?;
    }
    Ok(current)
}

/// `[x, y, width, height]` stored under `key`.
pub fn rect(node: &Value, key: &str) -> Result<[f64; 4]> {
    let numbers: Option<Vec<f64>> = node[key]
        .as_array()
        .map(|list| list.iter().filter_map(Value::as_f64).collect());
    match numbers.as_deref() {
        Some(&[x, y, width, height]) => Ok([x, y, width, height]),
        _ => bail!("node has no rectangle {key:?}: {}", node[key]),
    }
}

/// Every node in the tree, depth first.
pub fn walk<'a>(node: &'a Value, visit: &mut dyn FnMut(&'a Value)) {
    visit(node);
    for child in children(node) {
        walk(child, visit);
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    fn sample() -> Value {
        json!({
            "type": "GtkCenterBox", "name": "panel", "classes": ["gniri-panel"],
            "box": [0, 0, 1280, 32],
            "children": [
                {"type": "GtkButton", "name": "panelActivities", "classes": ["panel-button"],
                 "children": [
                    {"type": "GtkBox", "name": "", "classes": ["workspace-dot"], "box": [16, 13, 6, 6]},
                    {"type": "GtkBox", "name": "", "classes": ["workspace-dot"], "box": [28, 12, 29, 8]},
                 ]},
                {"type": "StLabel", "name": "", "classes": "clock first-child", "box": [1, 2, 3, 4]},
            ]
        })
    }

    #[test]
    fn selects_by_name_class_type_and_index() {
        let tree = sample();
        assert_eq!(select(&tree, "#panel").unwrap()["type"], "GtkCenterBox");
        assert_eq!(
            rect(
                select(&tree, "#panelActivities .workspace-dot:1").unwrap(),
                "box"
            )
            .unwrap(),
            [28.0, 12.0, 29.0, 8.0]
        );
        assert_eq!(
            rect(select(&tree, "GtkBox.workspace-dot").unwrap(), "box").unwrap(),
            [16.0, 13.0, 6.0, 6.0]
        );
        // Classes given as one string, as in the GNOME dumps.
        assert_eq!(select(&tree, "StLabel.clock").unwrap()["type"], "StLabel");
        assert_eq!(
            select(&tree, ".first-child.clock").unwrap()["type"],
            "StLabel"
        );
    }

    #[test]
    fn a_later_step_never_matches_the_node_it_searches_below() {
        let tree = sample();
        assert!(select(&tree, "#panel #panel").is_err());
    }

    #[test]
    fn reports_what_went_wrong() {
        let tree = sample();
        let error = select(&tree, "#panelActivities .workspace-dot:2").unwrap_err();
        assert!(error.to_string().contains("matched 2 node(s)"), "{error}");
        assert!(select(&tree, ".workspace-dot:x").is_err());
        assert!(select(&tree, "GtkBox.").is_err());
        assert!(rect(select(&tree, "#panelActivities").unwrap(), "box").is_err());
    }
}
