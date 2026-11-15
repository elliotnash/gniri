//! What the tier 0 tests share: reading the GNOME reference dumps and the
//! theme's tokens in comparable forms.

#![allow(dead_code)] // Each test file uses its own part.

pub use gniri_theme::token;
use serde_json::Value;

pub fn reference(name: &str) -> Value {
    let path = format!(
        "{}/../../tests/visual/reference/gnome-51.1/metrics/{name}.json",
        env!("CARGO_MANIFEST_DIR")
    );
    let text = std::fs::read_to_string(&path).unwrap_or_else(|error| panic!("{path}: {error}"));
    serde_json::from_str(&text).expect("valid JSON")
}

/// The first actor, depth first, for which `matches` holds.
pub fn find<'a>(node: &'a Value, matches: &dyn Fn(&Value) -> bool) -> Option<&'a Value> {
    if matches(node) {
        return Some(node);
    }
    node["children"]
        .as_array()?
        .iter()
        .find_map(|child| find(child, matches))
}

pub fn by_name<'a>(tree: &'a Value, name: &str) -> &'a Value {
    find(tree, &|node| node["name"] == name).unwrap_or_else(|| panic!("no actor named {name}"))
}

pub fn by_class<'a>(tree: &'a Value, class: &str) -> &'a Value {
    find(tree, &|node| {
        node["classes"]
            .as_str()
            .is_some_and(|classes| classes.split(' ').any(|other| other == class))
    })
    .unwrap_or_else(|| panic!("no actor with class {class}"))
}

pub fn by_type<'a>(tree: &'a Value, type_name: &str) -> &'a Value {
    find(tree, &|node| node["type"] == type_name)
        .unwrap_or_else(|| panic!("no actor of type {type_name}"))
}

pub fn numbers(value: &Value) -> Vec<f64> {
    value
        .as_array()
        .unwrap_or_else(|| panic!("expected an array, got {value}"))
        .iter()
        .map(|number| number.as_f64().expect("a number"))
        .collect()
}

/// A token colour as `[r, g, b, a]` with every channel 0 to 255, the form
/// the reference dump uses.
pub fn colour(name: &str) -> Vec<f64> {
    let value = token(name).unwrap_or_else(|| panic!("no token {name}"));
    if let Some(hex) = value.strip_prefix('#') {
        let channel = |index: usize| {
            f64::from(u8::from_str_radix(&hex[index..index + 2], 16).expect("hex digits"))
        };
        return vec![channel(0), channel(2), channel(4), 255.0];
    }
    let inner = value
        .strip_prefix("rgba(")
        .and_then(|rest| rest.strip_suffix(')'))
        .unwrap_or_else(|| panic!("token {name} is not a colour: {value}"));
    let parts: Vec<f64> = inner
        .split(',')
        .map(|part| part.trim().parse().expect("a number"))
        .collect();
    vec![parts[0], parts[1], parts[2], (parts[3] * 255.0).round()]
}

/// The size of the font GNOME used, in pixels: what 1em is.
pub fn em(tree: &Value) -> f64 {
    let font = tree["style"]["font"].as_str().expect("a font description");
    font.rsplit(' ')
        .next()
        .and_then(|size| size.strip_suffix("px"))
        .and_then(|size| size.parse().ok())
        .unwrap_or_else(|| panic!("no pixel size in {font:?}"))
}

pub fn length(name: &str, em: f64) -> f64 {
    let value = token(name).unwrap_or_else(|| panic!("no token {name}"));
    if let Some(ems) = value.strip_suffix("em") {
        ems.parse::<f64>().expect("a number") * em
    } else if let Some(pixels) = value.strip_suffix("px") {
        pixels.parse::<f64>().expect("a number")
    } else {
        panic!("token {name} is not a length: {value}")
    }
}

/// A token length in whole pixels as GTK lays it out when it is a padding,
/// margin, border or icon size: rounded down.
pub fn pixels(name: &str, em: f64) -> f64 {
    length(name, em).floor()
}

/// The same for a minimum size, which GTK rounds up.
pub fn minimum_pixels(name: &str, em: f64) -> f64 {
    length(name, em).ceil()
}
