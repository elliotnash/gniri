// Spike S2 (docs/DECISIONS.md): does CSS `backdrop-filter` on a child widget of
// a layer-shell surface make GTK 4.24 send a per-widget, rounded blur region
// to niri?
//
// Run inside nested niri:
//   LD_PRELOAD=/usr/lib64/libgtk4-layer-shell.so.0 gjs -m spikes/gjs/s2_blur.js
//
// Layout: a striped "wallpaper" on the background layer, a panel strip on the
// top layer, and a full-screen transparent stage on the overlay layer holding
// one rounded bubble.

import Gtk from 'gi://Gtk?version=4.0';
import Gdk from 'gi://Gdk?version=4.0';
import LayerShell from 'gi://Gtk4LayerShell?version=1.0';

const CSS = `
window { background: transparent; }

window.wallpaper {
    background: repeating-linear-gradient(45deg,
        #e01b24 0 16px, #f6d32d 16px 32px, #3584e4 32px 48px, #26a269 48px 64px);
}

.panel {
    min-height: 32px;
    backdrop-filter: blur(10px);
    background-color: rgba(30, 30, 33, 0.5);
    color: white;
    font-weight: bold;
}

.bubble {
    min-width: 400px;
    min-height: 200px;
    border-radius: 36px;
    backdrop-filter: blur(10px);
    background-color: rgba(30, 30, 33, 0.5);
    border: 1px solid rgba(255, 255, 255, 0.06);
    color: white;
}

/* Below GTK's threshold: should NOT produce a blur region. */
.bubble.weak { backdrop-filter: blur(4px); }
`;

function layerWindow(app, namespace, layer, anchors) {
    const win = new Gtk.ApplicationWindow({application: app});
    LayerShell.init_for_window(win);
    LayerShell.set_namespace(win, namespace);
    LayerShell.set_layer(win, layer);
    for (const edge of anchors)
        LayerShell.set_anchor(win, edge, true);
    return win;
}

const app = new Gtk.Application({application_id: 'dev.gniri.spike.s2'});

app.connect('activate', () => {
    print(`layer-shell supported: ${LayerShell.is_supported()}`);

    const provider = new Gtk.CssProvider();
    provider.load_from_string(CSS);
    Gtk.StyleContext.add_provider_for_display(Gdk.Display.get_default(), provider,
        Gtk.STYLE_PROVIDER_PRIORITY_APPLICATION);

    const {TOP, BOTTOM, LEFT, RIGHT} = LayerShell.Edge;

    const wallpaper = layerWindow(app, 'gniri-wallpaper', LayerShell.Layer.BACKGROUND,
        [TOP, BOTTOM, LEFT, RIGHT]);
    LayerShell.set_exclusive_zone(wallpaper, -1);
    wallpaper.add_css_class('wallpaper');
    wallpaper.present();

    const panel = layerWindow(app, 'gniri-panel', LayerShell.Layer.TOP, [TOP, LEFT, RIGHT]);
    LayerShell.auto_exclusive_zone_enable(panel);
    const panelBox = new Gtk.CenterBox({css_classes: ['panel']});
    panelBox.set_center_widget(new Gtk.Label({label: 'Jan 15  9:41 AM'}));
    panel.set_child(panelBox);
    panel.present();

    const stage = layerWindow(app, 'gniri-stage', LayerShell.Layer.OVERLAY,
        [TOP, BOTTOM, LEFT, RIGHT]);
    const row = new Gtk.Box({
        spacing: 24, halign: Gtk.Align.CENTER, valign: Gtk.Align.CENTER,
    });
    const strong = new Gtk.Label({label: 'blur(10px)', css_classes: ['bubble']});
    const weak = new Gtk.Label({label: 'blur(4px)', css_classes: ['bubble', 'weak']});
    row.append(strong);
    row.append(weak);
    stage.set_child(row);
    stage.present();

    const renderer = stage.get_native().get_renderer();
    print(`renderer: ${renderer.constructor.name}`);
});

app.run([]);
