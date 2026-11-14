//! Scenes: a fixture for gniri plus the steps that bring the nested session
//! to the state a screenshot should show.
//!
//! ```toml
//! # tests/visual/scenes/<name>.toml
//! [fixture]                      # passed to `gniri --fixture`
//! time = "2026-11-14T08:28:00"
//!
//! [variants.flat-808080]         # per-wallpaper replacements for fixture keys
//! time = "2026-11-13T22:10:00"
//!
//! [[steps]]
//! window = "#f6f5f4"             # open a debug window of this colour
//!
//! [[steps]]
//! action = { FocusWorkspaceDown = {} }   # any niri action
//! ```

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::Duration;

use anyhow::{Context, Result};
use serde::Deserialize;

use crate::session::{self, Session, SessionOptions};

#[derive(Deserialize)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
pub enum Step {
    /// Open a `gniri debug-window` of this colour and wait for it.
    Window(String),
    /// Run a niri action, written as niri's IPC writes it.
    Action(niri_ipc::Action),
}

#[derive(Deserialize, Default)]
#[serde(default, deny_unknown_fields)]
pub struct Scene {
    fixture: toml::Table,
    variants: BTreeMap<String, toml::Table>,
    steps: Vec<Step>,
}

/// What one run of a scene produced.
pub struct Capture {
    /// gniri's widget dump.
    pub dump: serde_json::Value,
    /// The screenshot, in physical pixels.
    pub shot: PathBuf,
}

pub fn scenes_dir() -> PathBuf {
    session::workspace_root().join("tests/visual/scenes")
}

impl Scene {
    pub fn load(name: &str) -> Result<Self> {
        let path = scenes_dir().join(format!("{name}.toml"));
        let text =
            fs::read_to_string(&path).with_context(|| format!("reading {}", path.display()))?;
        toml::from_str(&text).with_context(|| format!("parsing {}", path.display()))
    }

    /// The fixture for `variant`: the scene's, with the variant's keys
    /// replacing those of the same name.
    fn fixture(&self, variant: Option<&str>) -> toml::Table {
        let mut fixture = self.fixture.clone();
        if let Some(overrides) = variant.and_then(|variant| self.variants.get(variant)) {
            for (key, value) in overrides {
                fixture.insert(key.clone(), value.clone());
            }
        }
        fixture
    }

    /// Writes the fixture for `variant` where gniri can read it.
    pub fn write_fixture(&self, run: &str, variant: Option<&str>) -> Result<PathBuf> {
        let path = session::visual_dir()?
            .join("logs")
            .join(format!("{run}.fixture.toml"));
        fs::write(&path, toml::to_string(&self.fixture(variant))?)?;
        Ok(path)
    }

    /// Runs the scene in a fresh nested session and captures the result.
    ///
    /// `run` names the log files and the screenshot; `wallpaper` is an image
    /// path or a colour.
    pub fn capture(&self, run: &str, variant: Option<&str>, wallpaper: &str) -> Result<Capture> {
        let visual = session::visual_dir()?;
        let fixture_path = self.write_fixture(run, variant)?;

        let session = Session::start(&SessionOptions {
            name: run.to_owned(),
            niri_config: None,
            renderer: "vulkan".to_owned(),
            gniri_args: gniri_args(Some(&fixture_path), Some(wallpaper)),
        })?;
        self.play(&session)?;

        // The steps are done when niri has acted; give gniri a moment to
        // hear about it and niri a frame to composite the result. Fixtures
        // turn animations off, so nothing is still moving after that.
        std::thread::sleep(Duration::from_millis(300));
        let dump = session.dump_tree()?;
        let shot = visual.join(format!("{run}.png"));
        session.screenshot(&shot).context("taking the screenshot")?;
        session.quit()?;
        Ok(Capture { dump, shot })
    }

    pub fn play(&self, session: &Session) -> Result<()> {
        for step in &self.steps {
            match step {
                Step::Window(colour) => session.spawn_window(colour)?,
                Step::Action(action) => session.action(action.clone())?,
            }
            std::thread::sleep(Duration::from_millis(100));
        }
        Ok(())
    }
}

/// gniri's arguments for a test run: always the test config, never the
/// user's.
pub fn gniri_args(fixture: Option<&Path>, wallpaper: Option<&str>) -> Vec<String> {
    let mut args = vec![
        "--config".to_owned(),
        scenes_dir().join("config.toml").display().to_string(),
    ];
    if let Some(fixture) = fixture {
        args.extend(["--fixture".to_owned(), fixture.display().to_string()]);
    }
    if let Some(wallpaper) = wallpaper {
        args.extend(["--wallpaper".to_owned(), wallpaper.to_owned()]);
    }
    args
}
