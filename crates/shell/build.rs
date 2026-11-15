// SPDX-License-Identifier: GPL-3.0-or-later

//! Compiles the bundled icons into a GResource (needs glib-compile-resources,
//! from glib2-devel).

fn main() {
    glib_build_tools::compile_resources(
        &["resources"],
        "resources/gniri.gresource.xml",
        "gniri.gresource",
    );
}
