// SPDX-License-Identifier: GPL-3.0-or-later

//! Short names for keyboard layouts, as shown in the panel ("en", "de").
//!
//! niri reports layouts by their XKB description ("English (US)"). The short
//! form comes from the XKB registry file, which is where GNOME's input-source
//! indicator gets it too.

use std::collections::HashMap;
use std::path::Path;

const REGISTRY: &str = "/usr/share/X11/xkb/rules/evdev.xml";

/// Layout description to short name.
#[derive(Clone, Debug, Default)]
pub struct LayoutNames {
    short: HashMap<String, String>,
}

impl LayoutNames {
    /// Reads the system's XKB registry. If it is missing or unreadable the
    /// result is empty and [`LayoutNames::short_names`] falls back to
    /// guessing.
    pub fn load_system() -> Self {
        Self::load(Path::new(REGISTRY)).unwrap_or_default()
    }

    fn load(path: &Path) -> Option<Self> {
        Self::parse(&std::fs::read_to_string(path).ok()?)
    }

    fn parse(xml: &str) -> Option<Self> {
        let options = roxmltree::ParsingOptions {
            allow_dtd: true,
            ..Default::default()
        };
        let document = roxmltree::Document::parse_with_options(xml, options).ok()?;
        let child_text = |node: roxmltree::Node, name: &str| -> Option<String> {
            node.children()
                .find(|child| child.has_tag_name(name))
                .and_then(|child| child.text())
                .map(str::to_owned)
        };
        let config_item = |node: roxmltree::Node| {
            node.children()
                .find(|child| child.has_tag_name("configItem"))
                .map(|item| {
                    (
                        child_text(item, "description"),
                        child_text(item, "shortDescription"),
                        child_text(item, "name"),
                    )
                })
        };

        let mut short = HashMap::new();
        for layout in document
            .descendants()
            .filter(|node| node.has_tag_name("layout"))
        {
            let Some((description, layout_short, name)) = config_item(layout) else {
                continue;
            };
            // A layout without a short description is shown by its code.
            let Some(layout_short) = layout_short.or(name) else {
                continue;
            };
            if let Some(description) = description {
                short.insert(description, layout_short.clone());
            }
            // Variants use their own short name, or else the layout's.
            for variant in layout
                .descendants()
                .filter(|node| node.has_tag_name("variant"))
            {
                if let Some((Some(description), variant_short, _)) = config_item(variant) {
                    short.insert(description, variant_short.unwrap_or(layout_short.clone()));
                }
            }
        }
        Some(Self { short })
    }

    /// The panel text for each of `layouts`.
    ///
    /// Layouts that share a short name get a subscript number, as in GNOME:
    /// "en₁", "en₂".
    pub fn short_names(&self, layouts: &[String]) -> Vec<String> {
        let plain: Vec<String> = layouts
            .iter()
            .map(|description| match self.short.get(description) {
                Some(short) => short.clone(),
                // Unknown to the registry: the first two letters, lowercase.
                None => description
                    .chars()
                    .filter(|character| character.is_alphabetic())
                    .take(2)
                    .flat_map(char::to_lowercase)
                    .collect(),
            })
            .collect();

        let mut seen: HashMap<&str, u32> = HashMap::new();
        plain
            .iter()
            .map(|name| {
                let total = plain.iter().filter(|other| *other == name).count();
                if total == 1 {
                    return name.clone();
                }
                let number = seen.entry(name).or_insert(0);
                *number += 1;
                format!("{name}{}", subscript(*number))
            })
            .collect()
    }
}

fn subscript(number: u32) -> String {
    number
        .to_string()
        .chars()
        .map(|digit| char::from_u32(0x2080 + digit.to_digit(10).unwrap_or(0)).unwrap_or(digit))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE xkbConfigRegistry SYSTEM "xkb.dtd">
<xkbConfigRegistry version="1.1">
  <layoutList>
    <layout>
      <configItem>
        <name>us</name>
        <shortDescription>en</shortDescription>
        <description>English (US)</description>
      </configItem>
      <variantList>
        <variant>
          <configItem>
            <name>dvorak</name>
            <description>English (Dvorak)</description>
          </configItem>
        </variant>
        <variant>
          <configItem>
            <name>chr</name>
            <shortDescription>chr</shortDescription>
            <description>Cherokee</description>
          </configItem>
        </variant>
      </variantList>
    </layout>
    <layout>
      <configItem>
        <name>de</name>
        <shortDescription>de</shortDescription>
        <description>German</description>
      </configItem>
    </layout>
  </layoutList>
</xkbConfigRegistry>"#;

    fn names(layouts: &[&str]) -> Vec<String> {
        let layouts: Vec<String> = layouts.iter().map(|name| (*name).to_owned()).collect();
        LayoutNames::parse(SAMPLE).unwrap().short_names(&layouts)
    }

    #[test]
    fn looks_up_layouts_and_variants() {
        assert_eq!(names(&["English (US)", "German"]), ["en", "de"]);
        assert_eq!(names(&["Cherokee"]), ["chr"]);
    }

    #[test]
    fn numbers_layouts_that_share_a_name() {
        assert_eq!(
            names(&["English (US)", "German", "English (Dvorak)"]),
            ["en₁", "de", "en₂"]
        );
    }

    #[test]
    fn guesses_when_the_registry_does_not_know() {
        assert_eq!(names(&["Klingon (pIqaD)"]), ["kl"]);
        assert_eq!(
            LayoutNames::default().short_names(&["German".to_owned()]),
            ["ge"]
        );
    }

    #[test]
    fn reads_the_system_registry_if_present() {
        if !Path::new(REGISTRY).exists() {
            return;
        }
        let names = LayoutNames::load_system();
        assert_eq!(
            names.short_names(&["English (US)".to_owned(), "German".to_owned()]),
            ["en", "de"]
        );
    }
}
