//! Tells the test harness when the shell has drawn itself.
//!
//! The harness cannot screenshot at an arbitrary moment: it needs to know
//! every surface has had a frame presented. When the `GNIRI_READY_FILE`
//! environment variable is set, gniri writes that file once each watched
//! surface has painted at least one frame. The file carries what the harness
//! needs to talk to the nested compositor.

use std::cell::Cell;
use std::rc::Rc;

use gtk::prelude::*;

#[derive(Default)]
struct State {
    /// Surfaces that have not painted yet.
    pending: Cell<usize>,
    /// Set once all surfaces are registered, so an early paint cannot
    /// announce readiness while more surfaces are still being built.
    armed: Cell<bool>,
    announced: Cell<bool>,
}

impl State {
    fn maybe_announce(&self) {
        if !self.armed.get() || self.pending.get() > 0 || self.announced.replace(true) {
            return;
        }
        let Some(path) = std::env::var_os("GNIRI_READY_FILE") else {
            return;
        };
        let var = |name: &str| std::env::var(name).unwrap_or_default();
        let contents = format!(
            "pid={}\nniri_socket={}\nwayland_display={}\n",
            std::process::id(),
            var("NIRI_SOCKET"),
            var("WAYLAND_DISPLAY"),
        );
        // Write to a sibling and rename, so a reader never sees half a file.
        let path = std::path::PathBuf::from(path);
        let partial = path.with_extension("partial");
        let written =
            std::fs::write(&partial, contents).and_then(|()| std::fs::rename(&partial, &path));
        if let Err(error) = written {
            eprintln!("gniri: cannot write ready file {}: {error}", path.display());
        }
    }
}

pub struct ReadyTracker {
    state: Rc<State>,
}

impl ReadyTracker {
    pub fn new() -> Self {
        Self {
            state: Rc::new(State::default()),
        }
    }

    /// Waits for `window` to paint its first frame.
    pub fn watch(&self, window: &impl IsA<gtk::Widget>) {
        let state = Rc::clone(&self.state);
        state.pending.set(state.pending.get() + 1);

        let painted = Cell::new(false);
        let on_first_paint = move || {
            if !painted.replace(true) {
                state.pending.set(state.pending.get() - 1);
                state.maybe_announce();
            }
        };

        // The frame clock exists once the widget is realized, which
        // `present()` has already done for our layer surfaces.
        match window.as_ref().frame_clock() {
            Some(clock) => {
                clock.connect_after_paint(move |_| on_first_paint());
            }
            None => on_first_paint(),
        }
    }

    /// Call after the last `watch`.
    pub fn arm(&self) {
        self.state.armed.set(true);
        self.state.maybe_announce();
    }
}
