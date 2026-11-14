// Spike S5 probe (docs/DECISIONS.md): a wallpaper with a full-screen blurred
// surface over it, dimmed to 0.6 like blur-my-shell's overview background.
// tools/blur-fit/fit.py screenshots this under different niri blur settings.
//
// Run inside nested niri:
//   LD_PRELOAD=/usr/lib64/libgtk4-layer-shell.so.0 gjs -m spikes/gjs/s5_probe.js WALLPAPER.png

import Gdk from 'gi://Gdk?version=4.0';
import Gio from 'gi://Gio';
import Gtk from 'gi://Gtk?version=4.0';
import LayerShell from 'gi://Gtk4LayerShell?version=1.0';
import System from 'system';

const CSS = `
window.layer { background: transparent; }
.probe {
    backdrop-filter: blur(10px);
    background-color: rgba(0, 0, 0, 0.4);
}
`;

const [wallpaperPath] = System.programArgs;

function layerWindow(app, namespace, layer) {
    const win = new Gtk.ApplicationWindow({application: app, css_classes: ['layer']});
    LayerShell.init_for_window(win);
    LayerShell.set_namespace(win, namespace);
    LayerShell.set_layer(win, layer);
    for (const edge of [LayerShell.Edge.TOP, LayerShell.Edge.BOTTOM,
        LayerShell.Edge.LEFT, LayerShell.Edge.RIGHT])
        LayerShell.set_anchor(win, edge, true);
    LayerShell.set_exclusive_zone(win, -1);
    return win;
}

const app = new Gtk.Application({
    application_id: 'dev.gniri.spike.probe',
    flags: Gio.ApplicationFlags.NON_UNIQUE,
});

app.connect('activate', () => {
    const provider = new Gtk.CssProvider();
    provider.load_from_string(CSS);
    Gtk.StyleContext.add_provider_for_display(Gdk.Display.get_default(), provider,
        Gtk.STYLE_PROVIDER_PRIORITY_APPLICATION);

    const wallpaper = layerWindow(app, 'gniri-wallpaper', LayerShell.Layer.BACKGROUND);
    wallpaper.set_child(new Gtk.Picture({
        paintable: Gdk.Texture.new_from_filename(wallpaperPath),
        content_fit: Gtk.ContentFit.FILL,
    }));
    wallpaper.present();

    // Namespace gniri-panel: no layer rule, so niri's default xray blur, the
    // same path the real panel uses.
    const probe = layerWindow(app, 'gniri-panel', LayerShell.Layer.TOP);
    probe.set_child(new Gtk.Box({css_classes: ['probe'], hexpand: true, vexpand: true}));
    probe.present();
});

app.run([]);
