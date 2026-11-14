//! A single-threaded observable state container.
//!
//! Every service owns one `Store<S>`. Backends change the state through
//! [`Store::update`]; widgets observe it through [`Store::subscribe`] or
//! [`Store::select`] and never write to it.
//!
//! Subscribers are not called from inside `update`. The store asks its
//! [`Scheduler`] to run a flush later, so several updates in one main-loop
//! iteration cause one notification, and a subscriber can call `update`
//! itself without re-entering the store.

use std::cell::{Cell, RefCell};
use std::collections::VecDeque;
use std::rc::{Rc, Weak};

/// Runs a task later on the thread that owns the store.
///
/// The shell supplies one backed by the GLib main loop. Tests use
/// [`ManualScheduler`] to decide exactly when notifications happen.
pub trait Scheduler {
    fn schedule(&self, task: Box<dyn FnOnce()>);
}

/// Runs each task at once. Handy for code with no main loop.
#[derive(Default)]
pub struct ImmediateScheduler;

impl Scheduler for ImmediateScheduler {
    fn schedule(&self, task: Box<dyn FnOnce()>) {
        task();
    }
}

/// Queues tasks until [`ManualScheduler::run_pending`] is called.
#[derive(Default)]
pub struct ManualScheduler {
    queue: RefCell<VecDeque<Box<dyn FnOnce()>>>,
}

impl ManualScheduler {
    pub fn new() -> Rc<Self> {
        Rc::new(Self::default())
    }

    /// Runs queued tasks, including any they queue, and returns how many ran.
    pub fn run_pending(&self) -> usize {
        let mut ran = 0;
        loop {
            // Take the task out before running it: it may schedule more.
            let task = self.queue.borrow_mut().pop_front();
            match task {
                Some(task) => {
                    task();
                    ran += 1;
                }
                None => return ran,
            }
        }
    }
}

impl Scheduler for ManualScheduler {
    fn schedule(&self, task: Box<dyn FnOnce()>) {
        self.queue.borrow_mut().push_back(task);
    }
}

type Callback<S> = Rc<dyn Fn(&S)>;

struct Inner<S> {
    state: RefCell<S>,
    subscribers: RefCell<Vec<(u64, Callback<S>)>>,
    next_id: Cell<u64>,
    flush_queued: Cell<bool>,
    scheduler: Rc<dyn Scheduler>,
}

/// Observable state. Cloning gives another handle to the same state.
pub struct Store<S> {
    inner: Rc<Inner<S>>,
}

impl<S> Clone for Store<S> {
    fn clone(&self) -> Self {
        Self {
            inner: Rc::clone(&self.inner),
        }
    }
}

impl<S: Clone + PartialEq + 'static> Store<S> {
    pub fn new(initial: S, scheduler: Rc<dyn Scheduler>) -> Self {
        Self {
            inner: Rc::new(Inner {
                state: RefCell::new(initial),
                subscribers: RefCell::new(Vec::new()),
                next_id: Cell::new(0),
                flush_queued: Cell::new(false),
                scheduler,
            }),
        }
    }

    /// A copy of the current state.
    pub fn get(&self) -> S {
        self.inner.state.borrow().clone()
    }

    /// Reads the current state without copying it.
    ///
    /// `read` must not call [`Store::update`] on the same store.
    pub fn with<R>(&self, read: impl FnOnce(&S) -> R) -> R {
        read(&self.inner.state.borrow())
    }

    /// Changes the state. Subscribers hear about it on the next flush, and
    /// only if the state ends up different from what they last saw.
    pub fn update(&self, change: impl FnOnce(&mut S)) {
        let changed = {
            let mut state = self.inner.state.borrow_mut();
            let before = state.clone();
            change(&mut state);
            *state != before
        };
        if changed {
            self.queue_flush();
        }
    }

    /// Calls `callback` after every change, until the subscription is dropped.
    #[must_use = "the callback stops when the Subscription is dropped"]
    pub fn subscribe(&self, callback: impl Fn(&S) + 'static) -> Subscription {
        let id = self.inner.next_id.get();
        self.inner.next_id.set(id + 1);
        self.inner
            .subscribers
            .borrow_mut()
            .push((id, Rc::new(callback)));

        let inner: Weak<Inner<S>> = Rc::downgrade(&self.inner);
        Subscription {
            cancel: Some(Box::new(move || {
                if let Some(inner) = inner.upgrade() {
                    inner
                        .subscribers
                        .borrow_mut()
                        .retain(|(other, _)| *other != id);
                }
            })),
        }
    }

    /// Watches one derived value.
    ///
    /// `callback` runs once straight away with the current value, then again
    /// each time a change to the state makes `selector` return something
    /// different. This is what widgets use: bind once, stay in sync.
    #[must_use = "the callback stops when the Subscription is dropped"]
    pub fn select<T: PartialEq + 'static>(
        &self,
        selector: impl Fn(&S) -> T + 'static,
        callback: impl Fn(&T) + 'static,
    ) -> Subscription {
        let current = self.with(&selector);
        callback(&current);
        let last = RefCell::new(current);
        self.subscribe(move |state| {
            let value = selector(state);
            if *last.borrow() != value {
                callback(&value);
                *last.borrow_mut() = value;
            }
        })
    }

    fn queue_flush(&self) {
        if self.inner.flush_queued.replace(true) {
            return;
        }
        let inner = Rc::downgrade(&self.inner);
        self.inner.scheduler.schedule(Box::new(move || {
            if let Some(inner) = inner.upgrade() {
                Inner::flush(&inner);
            }
        }));
    }
}

impl<S: Clone> Inner<S> {
    fn flush(inner: &Rc<Self>) {
        inner.flush_queued.set(false);
        // Snapshot both, so a callback may update the store or drop
        // subscriptions while we iterate.
        let state = inner.state.borrow().clone();
        let subscribers: Vec<(u64, Callback<S>)> = inner.subscribers.borrow().clone();
        for (id, callback) in subscribers {
            let still_subscribed = inner
                .subscribers
                .borrow()
                .iter()
                .any(|(other, _)| *other == id);
            if still_subscribed {
                callback(&state);
            }
        }
    }
}

/// Keeps a callback registered. Dropping it unsubscribes.
pub struct Subscription {
    cancel: Option<Box<dyn FnOnce()>>,
}

impl Drop for Subscription {
    fn drop(&mut self) {
        if let Some(cancel) = self.cancel.take() {
            cancel();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Clone, PartialEq, Debug, Default)]
    struct Audio {
        volume: u32,
        muted: bool,
    }

    fn recorder<T: Clone + 'static>() -> (Rc<RefCell<Vec<T>>>, impl Fn(&T) + 'static) {
        let seen = Rc::new(RefCell::new(Vec::new()));
        let sink = Rc::clone(&seen);
        (seen, move |value: &T| sink.borrow_mut().push(value.clone()))
    }

    #[test]
    fn several_updates_cause_one_notification() {
        let scheduler = ManualScheduler::new();
        let store = Store::new(Audio::default(), scheduler.clone());
        let (seen, record) = recorder::<Audio>();
        let _subscription = store.subscribe(record);

        store.update(|s| s.volume = 10);
        store.update(|s| s.volume = 20);
        store.update(|s| s.muted = true);
        assert!(
            seen.borrow().is_empty(),
            "nothing is delivered before the flush"
        );

        assert_eq!(scheduler.run_pending(), 1);
        assert_eq!(
            *seen.borrow(),
            vec![Audio {
                volume: 20,
                muted: true
            }]
        );
    }

    #[test]
    fn an_update_that_changes_nothing_is_silent() {
        let scheduler = ManualScheduler::new();
        let store = Store::new(Audio::default(), scheduler.clone());
        let (seen, record) = recorder::<Audio>();
        let _subscription = store.subscribe(record);

        store.update(|s| s.volume = 0);
        assert_eq!(scheduler.run_pending(), 0);
        assert!(seen.borrow().is_empty());
    }

    #[test]
    fn select_reports_the_current_value_then_only_its_own_changes() {
        let scheduler = ManualScheduler::new();
        let store = Store::new(
            Audio {
                volume: 5,
                muted: false,
            },
            scheduler.clone(),
        );
        let (seen, record) = recorder::<u32>();
        let _subscription = store.select(|s| s.volume, record);
        assert_eq!(*seen.borrow(), vec![5]);

        store.update(|s| s.muted = true);
        scheduler.run_pending();
        assert_eq!(
            *seen.borrow(),
            vec![5],
            "a change to another field is ignored"
        );

        store.update(|s| s.volume = 6);
        scheduler.run_pending();
        assert_eq!(*seen.borrow(), vec![5, 6]);
    }

    #[test]
    fn dropping_the_subscription_stops_the_callback() {
        let scheduler = ManualScheduler::new();
        let store = Store::new(Audio::default(), scheduler.clone());
        let (seen, record) = recorder::<Audio>();
        let subscription = store.subscribe(record);

        store.update(|s| s.volume = 1);
        drop(subscription);
        scheduler.run_pending();
        assert!(seen.borrow().is_empty());
    }

    #[test]
    fn a_subscriber_may_update_the_store() {
        let scheduler = ManualScheduler::new();
        let store = Store::new(Audio::default(), scheduler.clone());
        let (seen, record) = recorder::<Audio>();

        // Clamp the volume from inside a subscriber.
        let clamp_target = store.clone();
        let _clamp = store.subscribe(move |state| {
            if state.volume > 100 {
                clamp_target.update(|s| s.volume = 100);
            }
        });
        let _record = store.subscribe(record);

        store.update(|s| s.volume = 150);
        assert_eq!(
            scheduler.run_pending(),
            2,
            "the clamp queues a second flush"
        );
        assert_eq!(store.get().volume, 100);
        assert_eq!(seen.borrow().last().map(|s| s.volume), Some(100));
    }

    #[test]
    fn a_subscriber_dropped_during_a_flush_is_not_called() {
        let scheduler = ManualScheduler::new();
        let store = Store::new(Audio::default(), scheduler.clone());
        let (seen, record) = recorder::<Audio>();

        let victim: Rc<RefCell<Option<Subscription>>> = Rc::new(RefCell::new(None));
        let handle = Rc::clone(&victim);
        let _killer = store.subscribe(move |_| {
            handle.borrow_mut().take();
        });
        *victim.borrow_mut() = Some(store.subscribe(record));

        store.update(|s| s.volume = 1);
        scheduler.run_pending();
        assert!(seen.borrow().is_empty());
    }

    #[test]
    fn immediate_scheduler_delivers_inside_update() {
        let store = Store::new(Audio::default(), Rc::new(ImmediateScheduler));
        let (seen, record) = recorder::<Audio>();
        let _subscription = store.subscribe(record);

        store.update(|s| s.volume = 3);
        assert_eq!(seen.borrow().len(), 1);
    }
}
