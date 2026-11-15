// SPDX-License-Identifier: GPL-3.0-or-later

//! A private bus with python-dbusmock services on it, standing in for the
//! system bus.
//!
//! Tests that use it need `dbus-daemon` and the `dbusmock` Python module
//! (Fedora: `python3-dbusmock`). Without them [`MockBus::start`] returns
//! `None` and the test passes with a note, so `cargo test` still works on a
//! machine that has neither.

#![allow(dead_code)] // Each test file uses its own part.

use std::io::{BufRead, BufReader};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

use futures_lite::future::block_on;
use gniri_services::{Link, Model};

pub const TIMEOUT: Duration = Duration::from_secs(10);

pub struct MockBus {
    daemon: Child,
    mocks: Vec<Child>,
    address: String,
    /// For driving the mocks.
    pub control: zbus::Connection,
}

impl MockBus {
    pub fn start() -> Option<Self> {
        let has_dbusmock = Command::new("python3")
            .args(["-c", "import dbusmock"])
            .stderr(Stdio::null())
            .status()
            .is_ok_and(|status| status.success());
        if !has_dbusmock {
            eprintln!("skipped: python3 with the dbusmock module is not installed");
            return None;
        }
        Self::start_empty()
    }

    /// A private bus with nothing on it, for a backend that serves a name
    /// itself. Needs only `dbus-daemon`.
    pub fn start_empty() -> Option<Self> {
        let daemon = Command::new("dbus-daemon")
            .args(["--session", "--nofork", "--print-address=1"])
            .stdout(Stdio::piped())
            .spawn();
        let mut daemon = match daemon {
            Ok(daemon) => daemon,
            Err(error) => {
                eprintln!("skipped: cannot run dbus-daemon: {error}");
                return None;
            }
        };
        let mut address = String::new();
        BufReader::new(daemon.stdout.take().expect("stdout is piped"))
            .read_line(&mut address)
            .expect("dbus-daemon prints its address");
        let address = address.trim().to_owned();
        let control = connect(&address);
        Some(Self {
            daemon,
            mocks: Vec::new(),
            address,
            control,
        })
    }

    /// Where the bus listens, for programs that are pointed at it through
    /// `DBUS_SESSION_BUS_ADDRESS`.
    pub fn address(&self) -> &str {
        &self.address
    }

    /// Waits until `bus_name` has an owner.
    pub fn wait_for_name(&self, bus_name: &str) {
        let bus = block_on(zbus::fdo::DBusProxy::new(&self.control)).expect("the bus is there");
        let name = zbus::names::BusName::try_from(bus_name).expect("a valid bus name");
        let deadline = Instant::now() + TIMEOUT;
        while !block_on(bus.name_has_owner(name.clone())).unwrap_or(false) {
            assert!(Instant::now() < deadline, "nothing ever claimed {bus_name}");
            std::thread::sleep(Duration::from_millis(20));
        }
    }

    /// Starts a bare dbusmock object, with no template, and waits until it
    /// owns `bus_name`. Its properties and methods are added through the
    /// `org.freedesktop.DBus.Mock` interface.
    pub fn object(&mut self, bus_name: &str, path: &str, interface: &str) {
        let mock = Command::new("python3")
            .args(["-m", "dbusmock", bus_name, path, interface])
            .env("DBUS_SYSTEM_BUS_ADDRESS", &self.address)
            .env("DBUS_SESSION_BUS_ADDRESS", &self.address)
            .stdout(Stdio::null())
            .spawn()
            .expect("python3 was found a moment ago");
        self.mocks.push(mock);
        self.wait_for_name(bus_name);
    }

    /// Starts a dbusmock template and waits until it owns `bus_name`.
    /// `parameters` is the JSON dictionary the template's `load` receives.
    pub fn template(&mut self, template: &str, bus_name: &str, parameters: &str) {
        let mock = Command::new("python3")
            .args(["-m", "dbusmock", "--template", template])
            .args(["--parameters", parameters])
            // Templates for system services connect to "the system bus".
            .env("DBUS_SYSTEM_BUS_ADDRESS", &self.address)
            .env("DBUS_SESSION_BUS_ADDRESS", &self.address)
            .stdout(Stdio::null())
            .spawn()
            .expect("python3 was found a moment ago");
        self.mocks.push(mock);

        self.wait_for_name(bus_name);
    }

    /// Stops every template at once, as when a service crashes: its names
    /// lose their owner without a word from it.
    pub fn stop_templates(&mut self) {
        for mock in &mut self.mocks {
            let _ = mock.kill();
            let _ = mock.wait();
        }
        self.mocks.clear();
    }

    /// A new connection for the backend under test.
    pub fn connection(&self) -> zbus::Connection {
        connect(&self.address)
    }

    /// Calls a method on a mock and returns the reply.
    pub fn call<B>(
        &self,
        service: &str,
        path: &str,
        interface: &str,
        method: &str,
        body: &B,
    ) -> zbus::Message
    where
        B: serde::Serialize + zbus::zvariant::DynamicType,
    {
        block_on(
            self.control
                .call_method(Some(service), path, Some(interface), method, body),
        )
        .unwrap_or_else(|error| panic!("{interface}.{method} on {path} failed: {error}"))
    }

    /// Sets a property the way another client of the service would.
    pub fn set_property<'v>(
        &self,
        service: &str,
        path: &str,
        interface: &str,
        property: &str,
        value: impl Into<zbus::Variant<'v>>,
    ) {
        self.call(
            service,
            path,
            "org.freedesktop.DBus.Properties",
            "Set",
            &(interface, property, value.into()),
        );
    }

    pub fn property<T>(&self, service: &str, path: &str, interface: &str, property: &str) -> T
    where
        T: TryFrom<zbus::OwnedVariant>,
        T::Error: std::fmt::Debug,
    {
        let reply = self.call(
            service,
            path,
            "org.freedesktop.DBus.Properties",
            "Get",
            &(interface, property),
        );
        let value: zbus::OwnedVariant = reply.body().deserialize().expect("Get returns a variant");
        T::try_from(value).expect("the property has the expected type")
    }

    /// Runs a backend on its own thread, as `Bus` would.
    pub fn run<M, F, Fut>(&self, run: F) -> Link<M>
    where
        M: Model,
        F: FnOnce(
                zbus::Connection,
                async_channel::Sender<M::Event>,
                async_channel::Receiver<M::Command>,
            ) -> Fut
            + Send
            + 'static,
        Fut: Future<Output = zbus::Result<()>> + 'static,
    {
        let (link, events, commands) = Link::new();
        let connection = self.connection();
        std::thread::spawn(move || {
            if let Err(error) = block_on(run(connection, events, commands)) {
                eprintln!("the backend stopped: {error}");
            }
        });
        link
    }
}

impl Drop for MockBus {
    fn drop(&mut self) {
        for mock in &mut self.mocks {
            let _ = mock.kill();
            let _ = mock.wait();
        }
        let _ = self.daemon.kill();
        let _ = self.daemon.wait();
    }
}

fn connect(address: &str) -> zbus::Connection {
    block_on(zbus::connection::Builder::address(address).build())
        .expect("connecting to the private bus")
}

/// Folds a backend's events into a state and waits for conditions on it.
pub struct Follower<M: Model> {
    pub link: Link<M>,
    pub state: M,
}

impl<M: Model + Default + std::fmt::Debug> Follower<M> {
    pub fn new(link: Link<M>) -> Self {
        Self {
            link,
            state: M::default(),
        }
    }

    /// Applies events until `done` holds for the state. Panics, showing the
    /// state, if that takes longer than [`TIMEOUT`].
    pub fn wait(&mut self, what: &str, done: impl Fn(&M) -> bool) {
        let deadline = Instant::now() + TIMEOUT;
        while !done(&self.state) {
            let left = deadline.saturating_duration_since(Instant::now());
            let event = block_on(futures_lite::future::or(
                async { self.link.events.recv().await.ok() },
                async {
                    async_io_timer(left).await;
                    None
                },
            ));
            match event {
                Some(event) => self.state.apply(event),
                None => panic!("timed out waiting for {what}; state: {:#?}", self.state),
            }
        }
    }

    pub fn send(&self, command: M::Command) {
        self.link
            .commands
            .send_blocking(command)
            .expect("the backend is running");
    }
}

/// A timer without an async runtime: a thread that sleeps.
async fn async_io_timer(duration: Duration) {
    let (sender, receiver) = async_channel::bounded::<()>(1);
    std::thread::spawn(move || {
        std::thread::sleep(duration);
        let _ = sender.send_blocking(());
    });
    let _ = receiver.recv().await;
}
