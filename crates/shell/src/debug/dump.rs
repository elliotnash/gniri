// SPDX-License-Identifier: GPL-3.0-or-later

//! A description of every mapped widget, for the visual tests.
//!
//! `gniri msg debug dump-tree` prints it as JSON. `cargo xtask diff` compares
//! the boxes with the actor boxes captured from GNOME Shell.

use gtk::graphene;
use gtk::prelude::*;
use gtk4_layer_shell::{Edge, LayerShell};
use serde::Serialize;

#[derive(Serialize)]
pub struct Dump {
    pub surfaces: Vec<Surface>,
}

#[derive(Serialize)]
pub struct Surface {
    /// Layer-shell namespace, such as "gniri-panel".
    pub namespace: String,
    /// Connector name of the output the surface is on.
    pub output: Option<String>,
    /// Logical pixels.
    pub width: i32,
    pub height: i32,
    /// Physical pixels per logical pixel.
    pub scale: f64,
    /// The output edges the surface is anchored to, of "top", "bottom",
    /// "left" and "right". Opposite edges both anchored means it spans the
    /// output between them; neither means it is centred.
    pub anchors: Vec<&'static str>,
    pub tree: Node,
}

#[derive(Serialize)]
pub struct Node {
    /// GObject type name, such as "GtkButton".
    #[serde(rename = "type")]
    pub type_name: String,
    /// Widget name (the CSS id), empty if none was set.
    pub name: String,
    pub classes: Vec<String>,
    /// Of "hover", "active", "checked", "insensitive".
    pub states: Vec<&'static str>,
    /// Border box as `[x, y, width, height]` in the surface's logical pixels.
    #[serde(rename = "box")]
    pub bounds: [f32; 4],
    /// Natural `[width, height]`: what the widget asks for, which its box
    /// can exceed when the parent stretches it.
    pub natural: [i32; 2],
    pub opacity: f64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub text: Option<String>,
    /// Where a label's text sits, same coordinates as `box`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub text_box: Option<[f32; 4]>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub icon: Option<String>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub children: Vec<Node>,
}

/// The style class of a window that is a session-lock surface.
pub const SESSION_LOCK_CLASS: &str = "gniri-session-lock";

/// Describes every mapped surface of `app`: its layer surfaces, and the
/// lock screen's, which cover their outputs and go by their widget name.
pub fn dump(app: &impl IsA<gtk::Application>) -> Dump {
    const EDGES: [(Edge, &str); 4] = [
        (Edge::Top, "top"),
        (Edge::Bottom, "bottom"),
        (Edge::Left, "left"),
        (Edge::Right, "right"),
    ];
    let mut surfaces: Vec<Surface> = app
        .windows()
        .iter()
        .filter(|window| window.is_mapped())
        .map(|window| {
            // The layer shell library takes a session-lock surface for one
            // of its own and then reads what is not there: such a window
            // says what it is, and is never asked.
            let layer = !window.has_css_class(SESSION_LOCK_CLASS) && window.is_layer_window();
            let monitor = if layer {
                window.monitor()
            } else {
                window
                    .surface()
                    .and_then(|surface| surface.display().monitor_at_surface(&surface))
            };
            Surface {
                namespace: if layer {
                    window.namespace().map(Into::into).unwrap_or_default()
                } else {
                    window.widget_name().to_string()
                },
                output: monitor
                    .and_then(|monitor| monitor.connector())
                    .map(Into::into),
                width: window.width(),
                height: window.height(),
                scale: window.surface().map_or(1.0, |surface| surface.scale()),
                anchors: EDGES
                    .into_iter()
                    .filter(|(edge, _)| !layer || window.is_anchor(*edge))
                    .map(|(_, name)| name)
                    .collect(),
                tree: node(window.upcast_ref(), window.upcast_ref()),
            }
        })
        .collect();
    surfaces.sort_by(|a, b| (&a.output, &a.namespace).cmp(&(&b.output, &b.namespace)));
    Dump { surfaces }
}

fn rect(bounds: &graphene::Rect) -> [f32; 4] {
    [bounds.x(), bounds.y(), bounds.width(), bounds.height()]
}

fn node(widget: &gtk::Widget, root: &gtk::Widget) -> Node {
    let flags = widget.state_flags();
    let states = [
        (gtk::StateFlags::PRELIGHT, "hover"),
        (gtk::StateFlags::ACTIVE, "active"),
        (gtk::StateFlags::CHECKED, "checked"),
        (gtk::StateFlags::INSENSITIVE, "insensitive"),
    ]
    .into_iter()
    .filter(|(flag, _)| flags.contains(*flag))
    .map(|(_, name)| name)
    .collect();

    let (text, text_box) = match widget.downcast_ref::<gtk::Label>() {
        Some(label) => (Some(label.text().to_string()), text_box(label, root)),
        // What is typed in a field, unless the field hides it: a password
        // is never written anywhere.
        None => match widget.downcast_ref::<gtk::Text>() {
            Some(field) if field.property::<bool>("visibility") => {
                (Some(field.text().to_string()), None)
            }
            _ => (None, None),
        },
    };
    let icon = widget
        .downcast_ref::<gtk::Image>()
        .and_then(|image| image.icon_name())
        .map(Into::into);

    let mut children = Vec::new();
    let mut child = widget.first_child();
    while let Some(current) = child {
        if current.is_mapped() {
            children.push(node(&current, root));
        }
        child = current.next_sibling();
    }

    Node {
        type_name: widget.type_().name().to_owned(),
        name: widget.widget_name().to_string(),
        classes: widget
            .css_classes()
            .iter()
            .map(ToString::to_string)
            .collect(),
        states,
        bounds: widget
            .compute_bounds(root)
            .map_or([0.0; 4], |bounds| rect(&bounds)),
        natural: [
            widget.measure(gtk::Orientation::Horizontal, -1).1,
            widget.measure(gtk::Orientation::Vertical, -1).1,
        ],
        opacity: widget.opacity(),
        text,
        text_box,
        icon,
        children,
    }
}

/// The logical extents of a label's text, in `root` coordinates.
fn text_box(label: &gtk::Label, root: &gtk::Widget) -> Option<[f32; 4]> {
    let (_, logical) = label.layout().pixel_extents();
    let (x, y) = label.layout_offsets();
    let origin = label.compute_point(
        root,
        &graphene::Point::new((x + logical.x()) as f32, (y + logical.y()) as f32),
    )?;
    Some([
        origin.x(),
        origin.y(),
        logical.width() as f32,
        logical.height() as f32,
    ])
}
