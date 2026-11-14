// Spikes S4 and S10 (docs/DECISIONS.md).
//
// S4:  with `background-effect { xray false }` on the stage namespace, does a
//      bubble blur the window underneath it, while the panel (default xray)
//      still shows only blurred wallpaper?
// S10: does a background-layer surface matched by `place-within-backdrop` show
//      behind the zoomed-out workspaces in niri's overview, while the normal
//      wallpaper zooms with them and the top-layer panel stays put?
//
// Run inside nested niri started with spikes/gjs/s4_s10.kdl:
//   LD_PRELOAD=/usr/lib64/libgtk4-layer-shell.so.0 gjs -m spikes/gjs/s4_s10.js

import Gtk from 'gi://Gtk?version=4.0';
import Gdk from 'gi://Gdk?version=4.0';
import LayerShell from 'gi://Gtk4LayerShell?version=1.0';

const CSS = `
window.layer { background: transparent; }

window.wallpaper {
    background: repeating-linear-gradient(45deg,
        #e01b24 0 16px, #f6d32d 16px 32px, #3584e4 32px 48px, #26a269 48px 64px);
}

/* Stands in for the pre-blurred, dimmed wallpaper copy. */
window.backdrop {
    background: linear-gradient(135deg, #613583, #1a5fb4);
}

/* An ordinary app window with high-contrast content. */
window.app {
    background: repeating-linear-gradient(0deg, #ffffff 0 12px, #000000 12px 24px);
}

.panel {
    min-height: 32px;
    backdrop-filter: blur(10px);
    background-color: rgba(30, 30, 33, 0.5);
    color: white;
    font-weight: bold;
}

.bubble {
    min-width: 500px;
    min-height: 240px;
    border-radius: 36px;
    backdrop-filter: blur(10px);
    background-color: rgba(30, 30, 33, 0.5);
    border: 1px solid rgba(255, 255, 255, 0.06);
    color: white;
}
`;

function layerWindow(app, namespace, layer, anchors) {
    const win = new Gtk.ApplicationWindow({application: app, css_classes: ['layer']});
    LayerShell.init_for_window(win);
    LayerShell.set_namespace(win, namespace);
    LayerShell.set_layer(win, layer);
    for (const edge of anchors)
        LayerShell.set_anchor(win, edge, true);
    return win;
}

const app = new Gtk.Application({application_id: 'dev.gniri.spike.s4s10'});

app.connect('activate', () => {
    const provider = new Gtk.CssProvider();
    provider.load_from_string(CSS);
    Gtk.StyleContext.add_provider_for_display(Gdk.Display.get_default(), provider,
        Gtk.STYLE_PROVIDER_PRIORITY_APPLICATION);

    const {TOP, BOTTOM, LEFT, RIGHT} = LayerShell.Edge;
    const ALL = [TOP, BOTTOM, LEFT, RIGHT];

    for (const [namespace, cls] of [['gniri-backdrop', 'backdrop'], ['gniri-wallpaper', 'wallpaper']]) {
        const win = layerWindow(app, namespace, LayerShell.Layer.BACKGROUND, ALL);
        LayerShell.set_exclusive_zone(win, -1);
        win.add_css_class(cls);
        win.present();
    }

    const panel = layerWindow(app, 'gniri-panel', LayerShell.Layer.TOP, [TOP, LEFT, RIGHT]);
    LayerShell.auto_exclusive_zone_enable(panel);
    const panelBox = new Gtk.CenterBox({css_classes: ['panel']});
    panelBox.set_center_widget(new Gtk.Label({label: 'Jan 15  9:41 AM'}));
    panel.set_child(panelBox);
    panel.present();

    const appWindow = new Gtk.ApplicationWindow({
        application: app, title: 'app', css_classes: ['app'], decorated: false,
    });
    appWindow.present();

    const stage = layerWindow(app, 'gniri-stage', LayerShell.Layer.OVERLAY, ALL);
    stage.set_child(new Gtk.Label({
        label: 'stage bubble', css_classes: ['bubble'],
        halign: Gtk.Align.CENTER, valign: Gtk.Align.CENTER,
    }));
    stage.present();
});

app.run([]);
