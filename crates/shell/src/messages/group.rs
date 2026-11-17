// SPDX-License-Identifier: GPL-3.0-or-later

//! A source's notifications: one card with the edges of up to two more
//! showing under it, or all of them under a heading. Port of
//! `NotificationMessageGroup` and `MessageGroupExpanderLayout` in
//! `js/ui/messageList.js`.

use std::cell::{Cell, RefCell};

use gtk::prelude::*;
use gtk::subclass::prelude::*;
use gtk::{gdk, glib, graphene, gsk};

/// messageList.js `MAX_VISIBLE_STACKED_MESSAGES`.
const MAX_VISIBLE_STACKED_MESSAGES: usize = 3;
/// messageList.js `WIDTH_OFFSET_STACKED`: how much narrower each card under
/// the first is, on each side.
const WIDTH_OFFSET_STACKED: i32 = 6;
/// messageList.js `HEIGHT_OFFSET_STACKED`: how far the second card shows
/// under the first.
const HEIGHT_OFFSET_STACKED: f64 = 10.0;
/// messageList.js `HEIGHT_OFFSET_REDUCTION_STACKED`: each further card shows
/// this many times less.
const HEIGHT_OFFSET_REDUCTION_STACKED: f64 = 1.4;
/// messageList.js `ADDITIONAL_BOTTOM_MARGIN_EXPANDED_GROUP`.
const ADDITIONAL_BOTTOM_MARGIN_EXPANDED_GROUP: i32 = 15;
/// messageList.js `GROUP_EXPENSION_TIME`
const GROUP_EXPANSION_TIME: u32 = 200;

mod imp {
    use super::*;

    #[derive(Default)]
    pub struct MessageGroup {
        /// `.message-group-header`, shown while expanded.
        pub(super) header: RefCell<Option<gtk::Widget>>,
        /// Top first.
        pub(super) messages: RefCell<Vec<gtk::Widget>>,
        pub(super) expanded: Cell<bool>,
        /// `MessageGroupExpanderLayout.expansion`: 0 a stack, 1 open, and
        /// between while it opens or closes.
        pub(super) expansion: Cell<f64>,
        pub(super) easing: RefCell<Option<crate::widgets::motion::Eased>>,
        /// Corner radius of a card, for cutting the cards under it to
        /// shape.
        pub(super) radius: Cell<f32>,
    }

    #[glib::object_subclass]
    impl ObjectSubclass for MessageGroup {
        const NAME: &'static str = "GniriMessageGroup";
        type Type = super::MessageGroup;
        type ParentType = gtk::Widget;
    }

    impl ObjectImpl for MessageGroup {
        fn dispose(&self) {
            if let Some(header) = self.header.take() {
                header.unparent();
            }
            for message in self.messages.take() {
                message.unparent();
            }
        }
    }

    fn height(widget: &gtk::Widget, width: i32) -> (i32, i32) {
        let (min, natural, _, _) = widget.measure(gtk::Orientation::Vertical, width);
        (min, natural)
    }

    impl MessageGroup {
        /// Shown as a stack: closed, and more than one card.
        pub(super) fn stacked(&self) -> bool {
            !self.expanded.get() && self.messages.borrow().len() > 1
        }

        /// Cards may overlap: a stack, or one opening or closing.
        fn overlapping(&self) -> bool {
            self.messages.borrow().len() > 1 && (self.stacked() || self.expansion.get() < 1.0)
        }

        /// The stack's height, and the open group's.
        fn heights(&self, for_size: i32) -> ((i32, i32), (i32, i32)) {
            let messages = self.messages.borrow();
            let stacked = match messages.first() {
                Some(top) => {
                    let (min, natural) = height(top, for_size);
                    let under: i32 = Self::stack_offsets(messages.len()).iter().sum();
                    (min + under, natural + under)
                }
                None => (0, 0),
            };
            let (mut min, mut natural) = (
                ADDITIONAL_BOTTOM_MARGIN_EXPANDED_GROUP,
                ADDITIONAL_BOTTOM_MARGIN_EXPANDED_GROUP,
            );
            let header = self.header.borrow();
            for child in header.iter().chain(messages.iter()) {
                let (child_min, child_natural) = height(child, for_size);
                min += child_min;
                natural += child_natural;
            }
            (stacked, (min, natural))
        }

        /// `MessageGroupExpanderLayout.vfunc_allocate` part way open.
        fn allocate_between(&self, width: i32, height_limit: i32, expansion: f64) {
            let messages = self.messages.borrow();
            let mut y = 0.0;
            if let Some(header) = &*self.header.borrow()
                && header.is_visible()
            {
                let (header_height, _) = height(header, width);
                header.size_allocate(&gtk::Allocation::new(0, 0, width, header_height), -1);
                y = expansion * f64::from(header_height);
            }
            let Some(top) = messages.first() else {
                return;
            };
            let (top_height, _) = height(top, width);
            top.size_allocate(
                &gtk::Allocation::new(0, y.round() as i32, width, top_height),
                -1,
            );
            let (mut bottom, mut inset) = (y + f64::from(top_height), 0.0);
            let mut offset = HEIGHT_OFFSET_STACKED;
            let limit = f64::from(height_limit);
            for message in &messages[1..] {
                inset += (1.0 - expansion) * f64::from(WIDTH_OFFSET_STACKED);
                let child_width = (f64::from(width) - 2.0 * inset).max(0.0).round() as i32;
                let (child_height, _) = height(message, child_width);
                // "Stack children with a small reveal when collapsed"
                bottom += offset + expansion * (f64::from(child_height) - offset);
                if bottom > limit {
                    bottom = limit;
                } else {
                    offset /= HEIGHT_OFFSET_REDUCTION_STACKED;
                }
                message.size_allocate(
                    &gtk::Allocation::new(
                        inset.round() as i32,
                        (bottom - f64::from(child_height)).round() as i32,
                        child_width,
                        child_height,
                    ),
                    -1,
                );
            }
        }

        /// How far the bottom edge of each card under the first lies below
        /// the one above it, whole pixels as St would place them.
        fn stack_offsets(count: usize) -> Vec<i32> {
            let (mut total, mut offset) = (0.0, HEIGHT_OFFSET_STACKED);
            let mut offsets = Vec::new();
            for _ in 1..count.min(MAX_VISIBLE_STACKED_MESSAGES) {
                let before = total as i32;
                total += offset;
                offsets.push(total as i32 - before);
                offset /= HEIGHT_OFFSET_REDUCTION_STACKED;
            }
            offsets
        }
    }

    impl WidgetImpl for MessageGroup {
        fn request_mode(&self) -> gtk::SizeRequestMode {
            gtk::SizeRequestMode::HeightForWidth
        }

        fn measure(&self, orientation: gtk::Orientation, for_size: i32) -> (i32, i32, i32, i32) {
            let messages = self.messages.borrow();
            if orientation == gtk::Orientation::Horizontal {
                let (min, natural) = messages.iter().fold((0, 0), |(min, natural), message| {
                    let (child_min, child_natural, _, _) =
                        message.measure(gtk::Orientation::Horizontal, -1);
                    (min.max(child_min), natural.max(child_natural))
                });
                return (min, natural, -1, -1);
            }
            drop(messages);
            let ((stacked_min, stacked_natural), (open_min, open_natural)) = self.heights(for_size);
            let expansion = self.expansion.get();
            let between = |stacked: i32, open: i32| {
                (f64::from(stacked) + expansion * f64::from(open - stacked)).round() as i32
            };
            (
                between(stacked_min, open_min),
                between(stacked_natural, open_natural),
                -1,
                -1,
            )
        }

        fn size_allocate(&self, width: i32, height_limit: i32, _baseline: i32) {
            let expansion = self.expansion.get();
            if expansion > 0.0 && expansion < 1.0 {
                self.allocate_between(width, height_limit, expansion);
                return;
            }
            let messages = self.messages.borrow();
            let header = self.header.borrow();
            if expansion >= 1.0 {
                let mut y = 0;
                for child in header.iter().chain(messages.iter()) {
                    let (child_height, _) = height(child, width);
                    child.size_allocate(&gtk::Allocation::new(0, y, width, child_height), -1);
                    y += child_height;
                }
                return;
            }
            let Some(top) = messages.first() else {
                return;
            };
            let (top_height, _) = height(top, width);
            top.size_allocate(&gtk::Allocation::new(0, 0, width, top_height), -1);
            let (mut bottom, mut inset) = (top_height, 0);
            for (message, offset) in messages[1..]
                .iter()
                .zip(Self::stack_offsets(messages.len()))
            {
                inset += WIDTH_OFFSET_STACKED;
                bottom += offset;
                let child_width = (width - 2 * inset).max(0);
                let (child_height, _) = height(message, child_width);
                message.size_allocate(
                    &gtk::Allocation::new(inset, bottom - child_height, child_width, child_height),
                    -1,
                );
            }
        }

        fn snapshot(&self, snapshot: &gtk::Snapshot) {
            let widget = self.obj();
            let messages = self.messages.borrow();
            if !self.overlapping() {
                if let Some(header) = &*self.header.borrow() {
                    widget.snapshot_child(header, snapshot);
                }
                for message in messages.iter() {
                    widget.snapshot_child(message, snapshot);
                }
                return;
            }

            // Each card under the first shows only where the card above
            // does not cover it. The cards are translucent, so the one
            // above cannot simply be painted over it: its shape is cut out
            // of the one below instead, as blur-my-shell does.
            // A stack shows three; a group on its way open or closed, all
            // it shows.
            if let Some(header) = &*self.header.borrow()
                && header.is_visible()
            {
                widget.snapshot_child(header, snapshot);
            }
            let shown = if self.stacked() {
                messages.len().min(MAX_VISIBLE_STACKED_MESSAGES)
            } else {
                messages.len()
            };
            let bounds = |child: &gtk::Widget| child.compute_bounds(&*widget);
            let top_edge = bounds(&messages[0]).map_or(0.0, |top| top.y());
            let below_top = graphene::Rect::new(
                0.0,
                top_edge,
                widget.width() as f32,
                (widget.height() as f32 - top_edge).max(0.0),
            );
            for index in (1..shown).rev() {
                let Some(above) = bounds(&messages[index - 1]) else {
                    continue;
                };
                snapshot.push_clip(&below_top);
                snapshot.push_mask(gsk::MaskMode::InvertedAlpha);
                snapshot.push_rounded_clip(&gsk::RoundedRect::from_rect(above, self.radius.get()));
                snapshot.append_color(&gdk::RGBA::BLACK, &above);
                snapshot.pop();
                snapshot.pop(); // The mask is drawn; what follows is cut by it.
                widget.snapshot_child(&messages[index], snapshot);
                snapshot.pop();
                snapshot.pop();
            }
            widget.snapshot_child(&messages[0], snapshot);
        }
    }
}

glib::wrapper! {
    pub struct MessageGroup(ObjectSubclass<imp::MessageGroup>)
        @extends gtk::Widget,
        @implements gtk::Accessible, gtk::Buildable, gtk::ConstraintTarget;
}

impl MessageGroup {
    /// `radius` is the corner radius the theme gives a card in the list.
    pub fn new(header: &impl IsA<gtk::Widget>, radius: f32) -> Self {
        let group: Self = glib::Object::new();
        group.add_css_class("message-notification-group");
        group.imp().radius.set(radius);
        header.set_parent(&group);
        header.set_visible(false);
        group.imp().header.replace(Some(header.clone().upcast()));
        group
    }

    /// Replaces the cards, given top first. A widget that stays keeps its
    /// state.
    pub fn set_messages(&self, messages: &[gtk::Widget]) {
        let imp = self.imp();
        if *imp.messages.borrow() == messages {
            return;
        }
        let old = imp.messages.replace(messages.to_vec());
        for message in &old {
            if !messages.contains(message) {
                message.unparent();
            }
        }
        // Kept in order after the heading, so that keyboard focus moves
        // from the top card down.
        let mut previous = imp.header.borrow().clone();
        for message in messages {
            if message.parent().is_none() {
                message.set_parent(self);
            }
            message.insert_after(self, previous.as_ref());
            previous = Some(message.clone());
        }
        self.update_stack();
        self.queue_resize();
    }

    pub fn is_expanded(&self) -> bool {
        self.imp().expanded.get()
    }

    /// Whether the group shows as a stack of cards: closed, with more than
    /// one in it.
    pub fn is_stacked(&self) -> bool {
        self.imp().stacked()
    }

    /// `NotificationMessageGroup.expand` and `collapse`: the cards move
    /// apart under the heading, or together, which then goes.
    pub fn set_expanded(&self, expanded: bool) {
        let imp = self.imp();
        if imp.expanded.replace(expanded) == expanded {
            return;
        }
        let header = imp.header.borrow().clone();
        if expanded && let Some(header) = &header {
            header.set_visible(true);
        }
        self.update_stack();
        let mut easing = imp.easing.borrow_mut();
        let easing = easing.get_or_insert_with(|| {
            let group = self.downgrade();
            crate::widgets::motion::Eased::new(self, imp.expansion.get(), move |expansion| {
                if let Some(group) = group.upgrade() {
                    group.imp().expansion.set(expansion);
                    group.queue_resize();
                }
            })
        });
        easing.ease(
            if expanded { 1.0 } else { 0.0 },
            GROUP_EXPANSION_TIME,
            adw::Easing::EaseOutQuad,
            move || {
                if !expanded && let Some(header) = &header {
                    header.set_visible(false);
                }
            },
        );
    }

    /// `_updateStackedMessagesFade`: marks the cards under the first of a
    /// stack, and takes them out of the pointer's and the keyboard's way.
    fn update_stack(&self) {
        let imp = self.imp();
        let stacked = imp.stacked();
        for (index, message) in imp.messages.borrow().iter().enumerate() {
            let under = stacked && index > 0;
            message.set_child_visible(!under || index < MAX_VISIBLE_STACKED_MESSAGES);
            message.set_can_target(!under);
            message.set_can_focus(!under);
            for (class, wanted) in [
                ("second-in-stack", under && index == 1),
                ("lower-in-stack", under && index > 1),
            ] {
                if wanted {
                    message.add_css_class(class);
                } else {
                    message.remove_css_class(class);
                }
            }
        }
        self.queue_draw();
    }
}
