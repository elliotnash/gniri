//! The `/dev/rfkill` backend.
//!
//! The kernel lists every kill switch to whoever opens the device (one
//! "add" record each), then reports changes as they happen; writing a record
//! changes switches. gnome-settings-daemon's rfkill plugin does the same and
//! `status/rfkill.js` only sees its D-Bus properties, so the bookkeeping
//! here is a port of the plugin (`gsd-rfkill-manager.c`).
//!
//! Two threads: reads block until the next record, so one thread does
//! nothing else, and the other keeps the table and writes the commands.
//!
//! Left out: the plugin also counts NetworkManager's WWAN switch (a USB
//! modem has no kill switch), keeps the kernel from handling the airplane
//! mode key itself, and waits for the device to appear when the rfkill
//! module is loaded late.

use std::collections::BTreeMap;
use std::fs::{File, OpenOptions};
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};
use std::time::Duration;

use async_channel::{Receiver, Sender};
use futures_lite::StreamExt;

use super::{Command, Event, State};
use crate::model::Link;
use crate::system_bus::merge;

pub const DEVICE: &str = "/dev/rfkill";

// `enum rfkill_type`; only the two that are ever written.
pub const TYPE_ALL: u8 = 0;
pub const TYPE_BLUETOOTH: u8 = 2;

// `enum rfkill_operation`.
const OP_ADD: u8 = 0;
const OP_DEL: u8 = 1;
const OP_CHANGE: u8 = 2;
const OP_CHANGE_ALL: u8 = 3;

/// The size of `struct rfkill_event`. Kernels 5.11 and later can append a
/// ninth byte (`hard_block_reasons`), which nothing here needs.
pub const RECORD_SIZE: usize = 8;

/// How long a burst of records gets to arrive before the state is reported:
/// the kernel sends one record per switch, and the states in between (the
/// first switch of several blocked, say) would flash by as airplane mode.
const SETTLE: Duration = Duration::from_millis(20);

/// `struct rfkill_event`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Record {
    pub index: u32,
    pub kind: u8,
    pub operation: u8,
    pub soft: bool,
    pub hard: bool,
}

impl Record {
    /// Reads one record from what one `read` returned. Longer records are
    /// accepted for their first eight bytes, shorter ones are not records.
    pub fn parse(bytes: &[u8]) -> Option<Self> {
        let [i0, i1, i2, i3, kind, operation, soft, hard, ..] = *bytes else {
            return None;
        };
        Some(Self {
            // The kernel writes the struct as it has it in memory.
            index: u32::from_ne_bytes([i0, i1, i2, i3]),
            kind,
            operation,
            soft: soft != 0,
            hard: hard != 0,
        })
    }

    /// The request that blocks or unblocks every switch of `kind`, as
    /// `cc_rfkill_glib_send_change_all_event` builds it.
    pub fn change_all(kind: u8, blocked: bool) -> Self {
        Self {
            index: 0,
            kind,
            operation: OP_CHANGE_ALL,
            soft: blocked,
            hard: false,
        }
    }

    pub fn bytes(&self) -> [u8; RECORD_SIZE] {
        let [i0, i1, i2, i3] = self.index.to_ne_bytes();
        [
            i0,
            i1,
            i2,
            i3,
            self.kind,
            self.operation,
            u8::from(self.soft),
            u8::from(self.hard),
        ]
    }
}

/// `RFKILL_STATE_*` in the plugin: a hardware block hides a software one.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Block {
    None,
    Soft,
    Hard,
}

/// Every kill switch the kernel has listed, by index.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Switches {
    all: BTreeMap<u32, (u8, Block)>,
}

impl Switches {
    /// Port of `rfkill_changed`.
    pub fn apply(&mut self, record: &Record) {
        match record.operation {
            OP_ADD | OP_CHANGE => {
                let block = if record.hard {
                    Block::Hard
                } else if record.soft {
                    Block::Soft
                } else {
                    Block::None
                };
                self.all.insert(record.index, (record.kind, block));
            }
            OP_DEL => drop(self.all.remove(&record.index)),
            // The kernel does not send anything else.
            _ => {}
        }
    }

    /// `shown` is [`shows_airplane_mode`], which does not depend on the
    /// switches.
    pub fn state(&self, shown: bool) -> State {
        let all = || self.all.values().map(|(_, block)| *block);
        let bluetooth = || {
            self.all
                .values()
                .filter(|(kind, _)| *kind == TYPE_BLUETOOTH)
                .map(|(_, block)| *block)
        };
        State {
            // `engine_get_has_airplane_mode`.
            available: shown && all().next().is_some(),
            airplane_mode: none_unblocked(all()),
            hardware_blocked: all_hard(all()),
            // `engine_get_has_bluetooth_airplane_mode`.
            has_bluetooth: bluetooth().next().is_some(),
            bluetooth_hardware_blocked: all_hard(bluetooth()),
        }
    }
}

/// Port of `engine_get_airplane_mode_helper`: "A single rfkill switch that's
/// unblocked? Airplane mode is off". No switches at all is off too.
fn none_unblocked(mut blocks: impl Iterator<Item = Block>) -> bool {
    let mut any = false;
    blocks.all(|block| {
        any = true;
        block != Block::None
    }) && any
}

/// Port of `engine_get_hardware_airplane_mode`.
fn all_hard(mut blocks: impl Iterator<Item = Block>) -> bool {
    let mut any = false;
    blocks.all(|block| {
        any = true;
        block == Block::Hard
    }) && any
}

/// Port of `engine_get_should_show_airplane_mode`: GNOME hides the toggle on
/// machines that stay where they are. `root` is `/`.
pub fn shows_airplane_mode(root: &Path) -> bool {
    !matches!(
        chassis(root).as_deref(),
        Some("desktop" | "server" | "vm" | "container")
    )
}

/// The chassis type systemd-hostnamed would report, which is where the
/// plugin gets it. Port of `fallback_chassis` in hostnamed, after the
/// administrator's own word in `/etc/machine-info`. hostnamed first checks
/// for a virtual machine or a container, which is left out: one with a
/// radio passed through to it is rare.
fn chassis(root: &Path) -> Option<String> {
    let read = |path: &str| std::fs::read_to_string(root.join(path)).ok();

    let configured = read("etc/machine-info").and_then(|text| {
        text.lines()
            .find_map(|line| line.trim().strip_prefix("CHASSIS="))
            .map(|value| value.trim_matches(['"', '\'']).to_owned())
    });
    if let Some(chassis) = configured.filter(|chassis| !chassis.is_empty()) {
        return Some(chassis);
    }

    // SMBIOS 3.5.0, section 7.4.1; hostnamed lists "only the really obvious
    // cases".
    let dmi = read("sys/class/dmi/id/chassis_type")
        .and_then(|text| text.trim().parse::<u32>().ok())
        .and_then(|kind| match kind {
            0x03 | 0x04 | 0x06 | 0x07 | 0x0D | 0x23 | 0x24 => Some("desktop"),
            0x08 | 0x09 | 0x0A | 0x0E => Some("laptop"),
            0x0B => Some("handset"),
            0x11 | 0x1C | 0x1D => Some("server"),
            0x1E => Some("tablet"),
            0x1F | 0x20 => Some("convertible"),
            0x21 | 0x22 => Some("embedded"),
            _ => None,
        });
    // The ACPI "Preferred PM Profile".
    let acpi = || {
        read("sys/firmware/acpi/pm_profile")
            .and_then(|text| text.trim().parse::<u32>().ok())
            .and_then(|profile| match profile {
                1 | 3 | 6 => Some("desktop"),
                2 => Some("laptop"),
                4 | 5 | 7 => Some("server"),
                8 => Some("tablet"),
                _ => None,
            })
    };
    if let Some(chassis) = dmi.or_else(acpi) {
        return Some(chassis.to_owned());
    }

    // The devicetree specification uses hostnamed's vocabulary.
    read("proc/device-tree/chassis-type")
        .map(|text| text.trim_matches(['\0', '\n', ' ']).to_owned())
        .filter(|chassis| !chassis.is_empty())
}

/// Blocks or unblocks every switch of `kind` with one change-all request.
/// Port of `cc_rfkill_glib_send_change_all_event`, without its second
/// request for Bluetooth: the plugin sends the unblock again when a switch
/// changes within half a second, for machines whose platform switch brings
/// back an adapter that is itself still blocked.
pub fn set_blocked(device: &Path, kind: u8, blocked: bool) -> io::Result<()> {
    let mut file = OpenOptions::new().write(true).open(device)?;
    file.write_all(&Record::change_all(kind, blocked).bytes())
}

/// Starts the backend on its own threads.
pub fn spawn() -> Link<State> {
    spawn_on(PathBuf::from(DEVICE), shows_airplane_mode(Path::new("/")))
}

fn spawn_on(device: PathBuf, shown: bool) -> Link<State> {
    let (link, events, commands) = Link::new();
    let (record_sender, records) = async_channel::unbounded();

    let reader = {
        let device = device.clone();
        std::thread::Builder::new()
            .name("rfkill-read".to_owned())
            .spawn(move || read(&device, &record_sender))
    };
    let keeper = std::thread::Builder::new()
        .name("rfkill".to_owned())
        .spawn(move || {
            futures_lite::future::block_on(keep(&device, shown, records, events, commands));
        });
    for spawned in [reader, keeper] {
        if let Err(error) = spawned {
            eprintln!("gniri: rfkill: cannot start a thread: {error}");
        }
    }
    link
}

/// Passes on every record the kernel sends, for as long as it sends them.
fn read(device: &Path, records: &Sender<Record>) {
    let mut file = match File::open(device) {
        Ok(file) => file,
        // A machine without radios has no rfkill device.
        Err(error) if error.kind() == io::ErrorKind::NotFound => return,
        Err(error) => {
            eprintln!("gniri: rfkill: cannot open {}: {error}", device.display());
            return;
        }
    };
    // One record per read, whatever the size of the buffer. Room for the
    // ninth byte: some kernels send it unasked.
    let mut buffer = [0; RECORD_SIZE + 1];
    loop {
        match file.read(&mut buffer) {
            // The device never ends; an ordinary file standing in for it does.
            Ok(0) => return,
            Ok(length) => {
                if let Some(record) = Record::parse(&buffer[..length])
                    && records.send_blocking(record).is_err()
                {
                    return;
                }
            }
            Err(error) if error.kind() == io::ErrorKind::Interrupted => {}
            Err(error) => {
                eprintln!("gniri: rfkill: cannot read {}: {error}", device.display());
                return;
            }
        }
    }
}

enum Wake {
    Record(Record),
    Command(Command),
}

/// Keeps the table of switches, reports what it adds up to and carries out
/// the commands.
async fn keep(
    device: &Path,
    shown: bool,
    records: Receiver<Record>,
    events: Sender<Event>,
    commands: Receiver<Command>,
) {
    let mut switches = Switches::default();
    // What the shell has: it starts from the default too.
    let mut reported = State::default();
    let mut wakes = merge(vec![
        Box::pin(records.clone().map(Wake::Record)),
        Box::pin(commands.map(Wake::Command)),
    ]);

    while let Some(wake) = wakes.next().await {
        let commanded = match wake {
            Wake::Record(record) => {
                switches.apply(&record);
                false
            }
            // Port of `engine_set_airplane_mode`.
            Wake::Command(Command::SetAirplaneMode(on)) => {
                if let Err(error) = set_blocked(device, TYPE_ALL, on) {
                    eprintln!("gniri: rfkill: cannot set airplane mode: {error}");
                }
                true
            }
        };
        // The write returns once the kernel has queued the changes it made;
        // the rest of a burst is as close behind.
        std::thread::sleep(SETTLE);
        while let Ok(record) = records.try_recv() {
            switches.apply(&record);
        }
        let state = switches.state(shown);
        // After a command even when nothing changed (a hardware switch, a
        // failed write): the shell's prediction has to be undone.
        if commanded || state != reported {
            reported = state.clone();
            if events.send(Event::State(state)).await.is_err() {
                break; // The shell went away.
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::Model;

    const WLAN: u8 = 1;
    const WWAN: u8 = 5;

    fn record(index: u32, kind: u8, operation: u8, soft: bool, hard: bool) -> Record {
        Record {
            index,
            kind,
            operation,
            soft,
            hard,
        }
    }

    fn switches(records: &[Record]) -> Switches {
        let mut switches = Switches::default();
        for record in records {
            switches.apply(record);
        }
        switches
    }

    fn scratch(test: &str) -> PathBuf {
        let directory =
            std::env::temp_dir().join(format!("gniri-rfkill-{test}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&directory);
        std::fs::create_dir_all(&directory).unwrap();
        directory
    }

    /// The backend's next report, which must not take long.
    fn next(link: &Link<State>) -> Event {
        let deadline = std::time::Instant::now() + Duration::from_secs(10);
        loop {
            if let Ok(event) = link.events.try_recv() {
                return event;
            }
            assert!(
                std::time::Instant::now() < deadline,
                "the backend reported nothing"
            );
            std::thread::sleep(Duration::from_millis(5));
        }
    }

    #[test]
    fn records_parse_at_both_sizes() {
        let mut bytes = Vec::from(3u32.to_ne_bytes());
        bytes.extend([TYPE_BLUETOOTH, OP_CHANGE, 1, 0]);
        let expected = record(3, TYPE_BLUETOOTH, OP_CHANGE, true, false);
        assert_eq!(Record::parse(&bytes), Some(expected));
        // With `hard_block_reasons`.
        bytes.push(1);
        assert_eq!(Record::parse(&bytes), Some(expected));
        assert_eq!(Record::parse(&bytes[..7]), None);
        assert_eq!(Record::parse(&[]), None);

        // Anything but zero is "blocked".
        let hard = [0, 0, 0, 0, WLAN, OP_ADD, 0, 2];
        assert_eq!(
            Record::parse(&hard),
            Some(record(0, WLAN, OP_ADD, false, true))
        );
    }

    #[test]
    fn airplane_mode_is_one_change_all_request() {
        // idx 0, type 0 (all), op 3 (change all), soft 1, hard 0.
        assert_eq!(
            Record::change_all(TYPE_ALL, true).bytes(),
            [0, 0, 0, 0, 0, 3, 1, 0]
        );
        assert_eq!(
            Record::change_all(TYPE_ALL, false).bytes(),
            [0, 0, 0, 0, 0, 3, 0, 0]
        );
        assert_eq!(
            Record::change_all(TYPE_BLUETOOTH, true).bytes(),
            [0, 0, 0, 0, 2, 3, 1, 0]
        );
        let request = Record::change_all(TYPE_BLUETOOTH, false);
        assert_eq!(Record::parse(&request.bytes()), Some(request));
    }

    #[test]
    fn index_bytes_are_in_memory_order() {
        let request = record(0x0102_0304, WLAN, OP_ADD, false, false);
        assert_eq!(request.bytes()[..4], 0x0102_0304u32.to_ne_bytes());
        assert_eq!(Record::parse(&request.bytes()), Some(request));
    }

    #[test]
    fn no_switches_is_nothing() {
        assert_eq!(Switches::default().state(true), State::default());
    }

    #[test]
    fn airplane_mode_needs_every_switch_blocked() {
        let mut table = switches(&[
            record(0, WLAN, OP_ADD, false, false),
            record(1, TYPE_BLUETOOTH, OP_ADD, true, false),
        ]);
        let state = table.state(true);
        assert!(state.available);
        assert!(!state.airplane_mode);
        assert!(!state.hardware_blocked);
        assert!(state.has_bluetooth);
        assert!(state.bluetooth_available());

        table.apply(&record(0, WLAN, OP_CHANGE, true, false));
        let state = table.state(true);
        assert!(state.airplane_mode);
        assert!(!state.hardware_blocked);

        // One switch back on is enough to leave it.
        table.apply(&record(1, TYPE_BLUETOOTH, OP_CHANGE, false, false));
        assert!(!table.state(true).airplane_mode);
    }

    #[test]
    fn a_hardware_block_counts_only_when_it_holds_everything() {
        let mut table = switches(&[
            record(0, WLAN, OP_ADD, false, true),
            record(1, TYPE_BLUETOOTH, OP_ADD, true, false),
        ]);
        let state = table.state(true);
        assert!(state.airplane_mode);
        assert!(!state.hardware_blocked);
        assert!(!state.bluetooth_hardware_blocked);

        // Hard and soft at once is hard.
        table.apply(&record(1, TYPE_BLUETOOTH, OP_CHANGE, true, true));
        let state = table.state(true);
        assert!(state.airplane_mode);
        assert!(state.hardware_blocked);
        assert!(state.bluetooth_hardware_blocked);
        assert!(!state.bluetooth_available());
    }

    #[test]
    fn switches_come_and_go() {
        let mut table = switches(&[
            record(0, WLAN, OP_ADD, false, false),
            record(4, TYPE_BLUETOOTH, OP_ADD, true, false),
        ]);
        // Unplugging the only radio that was on.
        table.apply(&record(0, WLAN, OP_DEL, false, false));
        let state = table.state(true);
        assert!(state.available);
        assert!(state.airplane_mode);

        table.apply(&record(4, TYPE_BLUETOOTH, OP_DEL, true, false));
        assert_eq!(table.state(true), State::default());
        // Removing what was never listed.
        table.apply(&record(9, WWAN, OP_DEL, false, false));
        assert_eq!(table, Switches::default());

        // The kernel never sends this one; it must not become a switch.
        table.apply(&Record::change_all(TYPE_ALL, true));
        assert_eq!(table, Switches::default());
    }

    #[test]
    fn other_radios_count_for_airplane_mode_but_not_for_bluetooth() {
        let table = switches(&[record(2, WWAN, OP_ADD, true, true)]);
        let state = table.state(true);
        assert!(state.available);
        assert!(state.airplane_mode);
        assert!(state.hardware_blocked);
        assert!(!state.has_bluetooth);
        assert!(!state.bluetooth_hardware_blocked);
    }

    #[test]
    fn a_desktop_has_radios_but_no_toggle() {
        let table = switches(&[record(0, WLAN, OP_ADD, true, false)]);
        let state = table.state(false);
        assert!(!state.available);
        assert!(state.airplane_mode);
        assert!(!state.shows_indicator());
    }

    #[test]
    fn chassis_comes_from_machine_info_then_firmware() {
        let root = scratch("chassis");
        let write = |path: &str, text: &str| {
            let path = root.join(path);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(path, text).unwrap();
        };
        // Nothing known: GNOME shows the toggle.
        assert_eq!(chassis(&root), None);
        assert!(shows_airplane_mode(&root));

        write("proc/device-tree/chassis-type", "laptop\0");
        assert_eq!(chassis(&root).as_deref(), Some("laptop"));
        assert!(shows_airplane_mode(&root));

        write("sys/firmware/acpi/pm_profile", "4\n");
        assert_eq!(chassis(&root).as_deref(), Some("server"));
        assert!(!shows_airplane_mode(&root));

        write("sys/class/dmi/id/chassis_type", "3\n");
        assert_eq!(chassis(&root).as_deref(), Some("desktop"));
        assert!(!shows_airplane_mode(&root));

        // A notebook, in decimal as the kernel prints it.
        write("sys/class/dmi/id/chassis_type", "10\n");
        assert_eq!(chassis(&root).as_deref(), Some("laptop"));
        // A value hostnamed does not list falls through to ACPI.
        write("sys/class/dmi/id/chassis_type", "2\n");
        assert_eq!(chassis(&root).as_deref(), Some("server"));

        write("etc/machine-info", "PRETTY_HOSTNAME=box\nCHASSIS=\"vm\"\n");
        assert_eq!(chassis(&root).as_deref(), Some("vm"));
        assert!(!shows_airplane_mode(&root));
        write("etc/machine-info", "CHASSIS=tablet\n");
        assert!(shows_airplane_mode(&root));
        std::fs::remove_dir_all(root).unwrap();
    }

    /// The threads against an ordinary file: it yields its records and then
    /// ends, where the device would wait for more.
    #[test]
    fn the_backend_reports_what_the_device_lists_and_writes_commands() {
        let root = scratch("device");
        let device = root.join("rfkill");
        // Nine bytes each, as a kernel that sends `hard_block_reasons`.
        let mut listed = Vec::new();
        for record in [
            record(0, WLAN, OP_ADD, true, false),
            record(1, TYPE_BLUETOOTH, OP_ADD, true, false),
        ] {
            listed.extend(record.bytes());
            listed.push(0);
        }
        std::fs::write(&device, &listed).unwrap();

        let link = spawn_on(device.clone(), true);
        let listed_state = State {
            available: true,
            airplane_mode: true,
            hardware_blocked: false,
            has_bluetooth: true,
            bluetooth_hardware_blocked: false,
        };
        // Usually one report for the whole burst; a busy machine may split
        // it.
        let mut state = State::default();
        while state != listed_state {
            state.apply(next(&link));
        }

        link.commands
            .send_blocking(Command::SetAirplaneMode(false))
            .unwrap();
        // The file does not react as the kernel would: the report repeats
        // the old state, which is what undoes the shell's prediction.
        state.predict(&Command::SetAirplaneMode(false));
        assert!(!state.airplane_mode);
        state.apply(next(&link));
        assert_eq!(state, listed_state);
        let written = std::fs::read(&device).unwrap();
        assert_eq!(
            written[..RECORD_SIZE],
            Record::change_all(TYPE_ALL, false).bytes()
        );
        assert_eq!(written.len(), listed.len());
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn a_missing_device_reports_nothing_and_fails_writes() {
        let root = scratch("missing");
        let device = root.join("rfkill");
        assert!(set_blocked(&device, TYPE_BLUETOOTH, true).is_err());
        // The write must not create it either.
        assert!(!device.exists());

        let link = spawn_on(device, true);
        link.commands
            .send_blocking(Command::SetAirplaneMode(true))
            .unwrap();
        // The command is still answered, with nothing available.
        assert_eq!(next(&link), Event::State(State::default()));
        std::fs::remove_dir_all(root).unwrap();
    }
}
