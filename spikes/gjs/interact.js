// Interactive spikes S3, S9, S12 and S17 (docs/DECISIONS.md).
//
// Reads one command per line from stdin and prints every input event it sees,
// so a driver script can inject input with wtype/wlrctl and read the result.
//
//   stage on|off    map/unmap the full-screen overlay stage. Its input region
//                   is everything except the panel strip (S12).
//   search on|off   map/unmap a top-layer entry with exclusive keyboard (S9).
//   anim            slide the stage bubble down in steps, then hide it (S3).
//   fade|shift|scale|reset
//                   give the bubble opacity 0.5, a CSS translate or a CSS
//                   scale, or undo all three (S3).
//   spin            put a spinner in the app window, so it repaints every
//                   frame under the bubble (S4 cost).
//   quit
//
// Run inside nested niri:
//   LD_PRELOAD=/usr/lib64/libgtk4-layer-shell.so.0 gjs -m spikes/gjs/interact.js < fifo

import Cairo from 'cairo';
import Gdk from 'gi://Gdk?version=4.0';
import Gio from 'gi://Gio';
import GioUnix from 'gi://GioUnix';
import GLib from 'gi://GLib';
import Gtk from 'gi://Gtk?version=4.0';
import LayerShell from 'gi://Gtk4LayerShell?version=1.0';

const PANEL_HEIGHT = 32;

const CSS = `
window.layer { background: transparent; }
window.wallpaper { background: #3584e4; }
window.app { background: #f6f5f4; }
.panel {
    min-height: ${PANEL_HEIGHT}px;
    backdrop-filter: blur(10px);
    background-color: rgba(30, 30, 33, 0.5);
    color: white;
}
.bubble {
    min-width: 300px;
    min-height: 150px;
    border-radius: 36px;
    backdrop-filter: blur(10px);
    background-color: rgba(30, 30, 33, 0.5);
    color: white;
}
.bubble.shift { transform: translateY(60px); }
.bubble.scaled { transform: scale(0.8); }
.search { min-width: 300px; }
`;

function log(...parts) {
    print(parts.join(' '));
}

function layerWindow(app, namespace, layer, anchors) {
    const win = new Gtk.ApplicationWindow({application: app, css_classes: ['layer']});
    LayerShell.init_for_window(win);
    LayerShell.set_namespace(win, namespace);
    LayerShell.set_layer(win, layer);
    for (const edge of anchors)
        LayerShell.set_anchor(win, edge, true);
    return win;
}

function reportClicks(widget, label) {
    const click = new Gtk.GestureClick();
    click.connect('pressed', (_gesture, _n, x, y) =>
        log('CLICK', label, Math.round(x), Math.round(y)));
    widget.add_controller(click);
}

const app = new Gtk.Application({application_id: 'dev.gniri.spike.interact'});

app.connect('activate', () => {
    const provider = new Gtk.CssProvider();
    provider.load_from_string(CSS);
    Gtk.StyleContext.add_provider_for_display(Gdk.Display.get_default(), provider,
        Gtk.STYLE_PROVIDER_PRIORITY_APPLICATION);

    const {TOP, BOTTOM, LEFT, RIGHT} = LayerShell.Edge;
    const ALL = [TOP, BOTTOM, LEFT, RIGHT];

    const wallpaper = layerWindow(app, 'gniri-wallpaper', LayerShell.Layer.BACKGROUND, ALL);
    LayerShell.set_exclusive_zone(wallpaper, -1);
    wallpaper.add_css_class('wallpaper');
    wallpaper.present();

    const panel = layerWindow(app, 'gniri-panel', LayerShell.Layer.TOP, [TOP, LEFT, RIGHT]);
    LayerShell.auto_exclusive_zone_enable(panel);
    const panelBox = new Gtk.CenterBox({css_classes: ['panel']});
    panelBox.set_center_widget(new Gtk.Label({label: 'panel'}));
    panel.set_child(panelBox);
    reportClicks(panelBox, 'panel');
    panel.present();

    const appWindow = new Gtk.ApplicationWindow({
        application: app, title: 'app', css_classes: ['app'], decorated: false,
    });
    const appArea = new Gtk.Box({hexpand: true, vexpand: true});
    appWindow.set_child(appArea);
    reportClicks(appArea, 'app');
    appWindow.present();

    // --- Stage (S12, S3) ---
    const stage = layerWindow(app, 'gniri-stage', LayerShell.Layer.OVERLAY, ALL);
    // Cover the panel too, as the real stage must to draw menus over it.
    LayerShell.set_exclusive_zone(stage, -1);
    const stageRoot = new Gtk.Box({hexpand: true, vexpand: true});
    const bubble = new Gtk.Label({
        label: 'bubble', css_classes: ['bubble'],
        halign: Gtk.Align.CENTER, valign: Gtk.Align.START, hexpand: true, margin_top: 100,
    });
    stageRoot.append(bubble);
    stage.set_child(stageRoot);
    reportClicks(stageRoot, 'stage');
    stage.connect('map', () => {
        const region = new Cairo.Region();
        region.unionRectangle({x: 0, y: PANEL_HEIGHT, width: 8192, height: 8192});
        stage.get_surface().set_input_region(region);
        log('STAGE mapped', stage.get_width(), stage.get_height());
    });

    // --- Search (S9) ---
    const search = layerWindow(app, 'gniri-search', LayerShell.Layer.TOP, [TOP]);
    LayerShell.set_margin(search, TOP, 12);
    LayerShell.set_keyboard_mode(search, LayerShell.KeyboardMode.EXCLUSIVE);
    const entry = new Gtk.Entry({css_classes: ['search']});
    entry.connect('changed', () => log('TEXT', JSON.stringify(entry.get_text())));
    entry.connect('activate', () => log('KEY entry-activate'));
    const keys = new Gtk.EventControllerKey();
    keys.set_propagation_phase(Gtk.PropagationPhase.CAPTURE);
    keys.connect('key-pressed', (_c, keyval) => {
        log('KEY', Gdk.keyval_name(keyval));
        return false;
    });
    search.add_controller(keys);
    search.set_child(entry);

    const commands = {
        'stage on': () => {
            bubble.set_visible(true);
            bubble.set_margin_top(100);
            stage.present();
        },
        'stage off': () => stage.set_visible(false),
        'search on': () => {
            entry.set_text('');
            search.present();
            entry.grab_focus();
        },
        'search off': () => search.set_visible(false),
        'anim': () => {
            let step = 0;
            GLib.timeout_add(GLib.PRIORITY_DEFAULT, 50, () => {
                step++;
                if (step <= 8) {
                    bubble.set_margin_top(100 + step * 20);
                    return GLib.SOURCE_CONTINUE;
                }
                bubble.set_visible(false);
                log('ANIM done');
                return GLib.SOURCE_REMOVE;
            });
        },
        'fade': () => bubble.set_opacity(0.5),
        'shift': () => bubble.add_css_class('shift'),
        'scale': () => bubble.add_css_class('scaled'),
        'reset': () => {
            bubble.set_opacity(1);
            bubble.remove_css_class('shift');
            bubble.remove_css_class('scaled');
        },
        'spin': () => appArea.append(new Gtk.Spinner({
            spinning: true, hexpand: true, vexpand: true,
        })),
        'quit': () => app.quit(),
    };

    const stdin = new Gio.DataInputStream({
        base_stream: new GioUnix.InputStream({fd: 0, close_fd: false}),
    });
    const readNext = () => {
        stdin.read_line_async(GLib.PRIORITY_DEFAULT, null, (stream, result) => {
            const [line] = stream.read_line_finish_utf8(result);
            if (line === null) {
                app.quit();
                return;
            }
            const command = commands[line.trim()];
            log('CMD', line.trim(), command ? 'ok' : 'unknown');
            command?.();
            readNext();
        });
    };
    readNext();
    log('READY');
});

app.run([]);
