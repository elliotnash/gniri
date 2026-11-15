// SPDX-License-Identifier: GPL-3.0-or-later

//! `cargo xtask report`: every component's diff as one HTML page.

use std::fmt::Write as _;
use std::fs;
use std::path::PathBuf;

use anyhow::Result;

use crate::diff::{self, Check};
use crate::session;

fn escape(text: &str) -> String {
    text.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

/// Writes `target/visual/report/index.html` and returns its path.
pub fn write(results: &[(String, Vec<Check>)]) -> Result<PathBuf> {
    let directory = session::visual_dir()?.join("report");
    fs::create_dir_all(&directory)?;

    let mut html = String::from(
        "<!doctype html>\n<meta charset=\"utf-8\">\n<title>gniri visual report</title>\n\
         <style>\n\
         body { font: 14px sans-serif; margin: 2em; background: #222; color: #eee; }\n\
         table { border-collapse: collapse; margin-bottom: 2em; }\n\
         td, th { padding: 4px 12px; text-align: left; border-bottom: 1px solid #444; }\n\
         .fail { color: #ff7b63; font-weight: bold; }\n\
         img { display: block; margin: 4px 0 20px; image-rendering: pixelated; max-width: 100%; }\n\
         </style>\n\
         <h1>gniri against GNOME Shell 51.1</h1>\n\
         <p>Each picture: gniri on top, GNOME in the middle, differing pixels marked at the \
         bottom, enlarged three times.</p>\n",
    );
    for (component, checks) in results {
        let _ = writeln!(html, "<h2>{}</h2>\n<table>", escape(component));
        html.push_str("<tr><th></th><th>Check</th><th>Subject</th><th>Result</th></tr>\n");
        for check in checks {
            let _ = writeln!(
                html,
                "<tr><td class=\"{}\">{}</td><td>{}</td><td>{}</td><td>{}</td></tr>",
                if check.pass { "ok" } else { "fail" },
                if check.pass { "ok" } else { "FAIL" },
                check.tier,
                escape(&check.subject),
                escape(&check.detail),
            );
        }
        html.push_str("</table>\n");
        for check in checks {
            let Some(montage) = &check.montage else {
                continue;
            };
            let relative = montage.strip_prefix(&directory).unwrap_or(montage);
            let _ = writeln!(
                html,
                "<div>{} ({}): {}</div>\n<img src=\"{}\">",
                escape(&check.subject),
                check.tier,
                escape(&check.detail),
                escape(&relative.display().to_string()),
            );
        }
    }

    let path = directory.join("index.html");
    fs::write(&path, html)?;
    Ok(path)
}

/// Diffs every component, prints the tables, writes the page. Returns
/// whether everything passed.
pub fn run() -> Result<bool> {
    let mut results = Vec::new();
    let mut all_passed = true;
    for component in diff::components()? {
        let checks = diff::run(&component)?;
        all_passed &= diff::print(&component, &checks);
        results.push((component, checks));
    }
    println!("{}", write(&results)?.display());
    Ok(all_passed)
}
