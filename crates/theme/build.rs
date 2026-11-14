//! Compiles the SCSS theme and extracts its design tokens.
//!
//! Outputs, in OUT_DIR:
//!  - `gniri-dark.css`: the stylesheet the shell loads.
//!  - `tokens.rs`: `(name, value)` pairs read back from the `:root` block.

use std::env;
use std::fmt::Write as _;
use std::fs;
use std::path::{Path, PathBuf};

fn main() {
    let scss_dir = Path::new("scss");
    println!("cargo::rerun-if-changed={}", scss_dir.display());

    let options = grass::Options::default()
        .style(grass::OutputStyle::Expanded)
        .load_path(scss_dir);
    let css = match grass::from_path(scss_dir.join("gniri-dark.scss"), &options) {
        Ok(css) => css,
        Err(error) => {
            // grass formats the error with file, line and a source excerpt.
            eprintln!("{error}");
            panic!("the SCSS theme failed to compile");
        }
    };

    let out_dir = PathBuf::from(env::var_os("OUT_DIR").expect("OUT_DIR is set by cargo"));
    fs::write(out_dir.join("gniri-dark.css"), &css).expect("write compiled CSS");
    fs::write(out_dir.join("tokens.rs"), tokens_source(&css)).expect("write tokens.rs");
}

/// Turns every `--gniri-name: value;` declaration into a pair.
fn tokens_source(css: &str) -> String {
    let mut source = String::from("&[\n");
    let mut found = 0;
    for line in css.lines() {
        let Some((name, value)) = line.trim().trim_end_matches(';').split_once(':') else {
            continue;
        };
        let Some(name) = name.trim().strip_prefix("--gniri-") else {
            continue;
        };
        writeln!(source, "    ({name:?}, {:?}),", value.trim()).expect("write to String");
        found += 1;
    }
    assert!(
        found > 0,
        "the compiled CSS defines no --gniri-* tokens (scss/_tokens.scss)"
    );
    source.push(']');
    source
}
