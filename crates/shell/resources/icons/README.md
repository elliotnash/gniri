# Bundled icons

Icons GNOME Shell ships in its own resources rather than in the Adwaita icon
theme, copied unmodified from gnome-shell 51.1 (`data/icons/`, GPL-2.0-or-later).
Everything else gniri shows comes from the system icon theme.

Add a file here, list it in `../gniri.gresource.xml`, and it is available by
name to `gtk::Image`.
