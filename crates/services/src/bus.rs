//! D-Bus: one connection and one thread for every backend that lives on the
//! same bus. UPower, NetworkManager, BlueZ and logind are on the system bus;
//! notifications and media players on the session bus.
//!
//! Each backend is an `async fn run(connection, events, commands)` that only
//! returns when its channels close or the bus goes away. They all run on the
//! same thread, interleaved; none of them blocks.
//!
//! `DBUS_SYSTEM_BUS_ADDRESS` and `DBUS_SESSION_BUS_ADDRESS` point the
//! connections somewhere else, which is how tests put stand-ins in place of
//! the real services.

use std::future::Future;
use std::pin::Pin;

use async_channel::{Receiver, Sender};
use std::task::{Context, Poll};

use futures_lite::Stream;
#[cfg(test)]
use futures_lite::stream;
use futures_lite::stream::StreamExt;

use crate::model::{Link, Model};

type Task = Box<dyn FnOnce(zbus::Connection) -> Pin<Box<dyn Future<Output = ()>>> + Send>;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Kind {
    System,
    Session,
}

/// Collects the backends of one bus, then starts them together.
pub struct Bus {
    kind: Kind,
    tasks: Vec<Task>,
}

impl Bus {
    pub fn system() -> Self {
        Self {
            kind: Kind::System,
            tasks: Vec::new(),
        }
    }

    pub fn session() -> Self {
        Self {
            kind: Kind::Session,
            tasks: Vec::new(),
        }
    }

    /// Adds a backend. `name` prefixes what it prints when it gives up.
    pub fn add<M, F, Fut>(&mut self, name: &'static str, run: F) -> Link<M>
    where
        M: Model,
        F: FnOnce(zbus::Connection, Sender<M::Event>, Receiver<M::Command>) -> Fut + Send + 'static,
        Fut: Future<Output = zbus::Result<()>> + 'static,
    {
        let (link, events, commands) = Link::new();
        self.tasks.push(Box::new(move |connection| {
            Box::pin(async move {
                if let Err(error) = run(connection, events, commands).await {
                    eprintln!("gniri: {name}: {error}");
                }
            })
        }));
        link
    }

    /// Connects and runs every added backend on a new thread.
    pub fn start(self) {
        if self.tasks.is_empty() {
            return;
        }
        let (name, thread) = match self.kind {
            Kind::System => ("system", "system-bus"),
            Kind::Session => ("session", "session-bus"),
        };
        let spawned = std::thread::Builder::new()
            .name(thread.to_owned())
            .spawn(move || {
                futures_lite::future::block_on(async move {
                    let connection = match self.kind {
                        Kind::System => zbus::Connection::system().await,
                        Kind::Session => zbus::Connection::session().await,
                    };
                    let connection = match connection {
                        Ok(connection) => connection,
                        Err(error) => {
                            eprintln!("gniri: cannot reach the {name} bus: {error}");
                            return;
                        }
                    };
                    let mut all: Pin<Box<dyn Future<Output = ()>>> = Box::pin(async {});
                    for task in self.tasks {
                        let next = task(connection.clone());
                        all = Box::pin(async move {
                            futures_lite::future::zip(all, next).await;
                        });
                    }
                    all.await;
                });
            });
        if let Err(error) = spawned {
            eprintln!("gniri: cannot start the {name} bus thread: {error}");
        }
    }
}

/// A boxed stream, for [`merge`].
pub type Wakes<T> = Pin<Box<dyn Stream<Item = T>>>;

/// Boxes `stream`, turning each item into `wake`.
pub fn wake<T: Clone + 'static>(stream: impl Stream + 'static, wake: T) -> Wakes<T> {
    Box::pin(stream.map(move |_| wake.clone()))
}

/// One stream that yields whatever any of `streams` yields, until all of
/// them have ended.
pub fn merge<T>(streams: Vec<Wakes<T>>) -> Merge<T> {
    Merge { streams }
}

pub struct Merge<T> {
    streams: Vec<Wakes<T>>,
}

impl<T> Stream for Merge<T> {
    type Item = T;

    fn poll_next(mut self: Pin<&mut Self>, context: &mut Context<'_>) -> Poll<Option<T>> {
        let mut index = 0;
        while index < self.streams.len() {
            match self.streams[index].as_mut().poll_next(context) {
                Poll::Ready(Some(item)) => {
                    // Start with the next stream next time, so a busy one
                    // cannot starve the rest.
                    self.streams.rotate_left(index + 1);
                    return Poll::Ready(Some(item));
                }
                Poll::Ready(None) => drop(self.streams.remove(index)),
                Poll::Pending => index += 1,
            }
        }
        if self.streams.is_empty() {
            Poll::Ready(None)
        } else {
            Poll::Pending
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn merge_yields_from_every_stream_and_ends_with_the_last() {
        let merged = merge(vec![
            Box::pin(stream::iter([1, 2])) as Wakes<i32>,
            Box::pin(stream::empty()),
            Box::pin(stream::iter([10, 20, 30])),
        ]);
        let mut items: Vec<i32> = futures_lite::future::block_on(merged.collect());
        items.sort_unstable();
        assert_eq!(items, [1, 2, 10, 20, 30]);
    }
}
