// SPDX-License-Identifier: GPL-3.0-or-later

// Spike: ways to close a blurred menu without the blur staying at full
// strength until the surface unmaps.
//
// niri draws a layer surface's blur at full strength wherever the surface's
// blur region is: neither the surface's own pixels nor a rule can fade it,
// and niri has no closing animation for layer surfaces. GTK leaves opacity
// out of the region it reports. So a menu that fades out keeps its blur
// until the last frame, and then the blur is gone at once: `current` below,
// which is what gniri's menus do today.
//
// Each other variant is one way around it:
//
//   snapshot  capture the screen where the menu is (wlr-screencopy), draw
//             that picture in place of the menu and its blur, and fade the
//             picture. The blur is in the picture, so it fades too.
//   dither    fade the blur by blurring fewer and fewer rows of pixels: the
//             region is drawn here, not by CSS, so it can be any shape.
//   roll      no fade: the menu rolls up into the panel and the region
//             shrinks with it.
//   zoom      no fade: the menu shrinks into its corner, the region with it.
//   opaque    the tint turns opaque first, which hides the blur; then the
//             blur is dropped unseen and the opaque menu fades.
//
// Build the capture helper once (the `snapshot` variant needs it):
//   cargo build --release --manifest-path spikes/shot/Cargo.toml
//
// Run in a niri session whose config includes data/niri/gniri.kdl (for
// `xray false` on the `gniri-stage` namespace):
//   LD_PRELOAD=/usr/lib64/libgtk4-layer-shell.so.0 gjs -m spikes/gjs/close_blur.js
//
// The bar at the bottom opens the menu and closes it each way, at full
// speed or slowed down; the menu itself takes no clicks. The same as lines
// on stdin: `open`, `close VARIANT`, `play VARIANT`, `speed N`, `quit`.

import Cairo from 'cairo';
import Gdk from 'gi://Gdk?version=4.0';
import Gio from 'gi://Gio';
import GioUnix from 'gi://GioUnix';
import GLib from 'gi://GLib';
import GObject from 'gi://GObject';
import Graphene from 'gi://Graphene';
import Gsk from 'gi://Gsk?version=4.0';
import Gtk from 'gi://Gtk?version=4.0';
import LayerShell from 'gi://Gtk4LayerShell?version=1.0';

// boxpointer.js POPUP_ANIMATION_TIME, and what gniri's stage uses.
const POPUP_MS = 150;
// The roll and the zoom move further than a 6 px slide, so they get longer.
const MOTION_MS = 200;
const PANEL_HEIGHT = 32;
const ARROW_RISE = 6;
const RADIUS = 36;
// GTK reports a blur region for a blur of this radius or more
// (gskblurutils.c BACKGROUND_BLUR_THRESHOLD); CSS `blur(10px)` is 20 here.
const BLUR_RADIUS = 20;
// Which rows of every eight are blurred first as the coverage grows.
const ROW_ORDER = [0, 4, 2, 6, 1, 5, 3, 7];

const SHOT = GLib.build_filenamev([
    GLib.path_get_dirname(GLib.path_get_dirname(
        GLib.filename_from_uri(import.meta.url)[0])),
    'shot', 'target', 'release', 'gniri-spike-shot',
]);

const VARIANTS = ['current', 'snapshot', 'dither', 'roll', 'zoom', 'opaque'];

let speed = 1;

function log(...parts) {
    print(parts.join(' '));
}

function rect(x, y, width, height) {
    return new Graphene.Rect().init(x, y, width, height);
}

const easeOutQuad = t => 1 - (1 - t) * (1 - t);
const easeInOutQuad = t => t < 0.5 ? 2 * t * t : 1 - 2 * (1 - t) * (1 - t);

// Runs `step(progress)` each frame of `widget` for `ms` (times the speed),
// then `done()`.
function animate(widget, ms, ease, step, done) {
    const length = ms * speed * 1000;
    let start = null;
    widget.add_tick_callback((_, clock) => {
        start ??= clock.get_frame_time();
        const t = Math.min((clock.get_frame_time() - start) / length, 1);
        step(ease(t));
        if (t < 1)
            return GLib.SOURCE_CONTINUE;
        done?.();
        return GLib.SOURCE_REMOVE;
    });
}

// What the menu is drawn on. Either it asks the compositor for blur behind
// its rounded box (as CSS `backdrop-filter` does, but with a say in which
// rows), or it draws a picture there instead.
const Backdrop = GObject.registerClass({
    CssName: 'backdrop',
}, class Backdrop extends Gtk.Widget {
    constructor(child) {
        super();
        this.set_layout_manager(new Gtk.BinLayout());
        this._child = child;
        child.set_parent(this);
        this.blur = true;
        this.coverage = 1;
        this.radius = RADIUS;
        // How much of the menu shows, and how far up it has slid. The blur
        // is not faded with the rest: GtkWidget too keeps a
        // `backdrop-filter` out of the widget's opacity as far as the
        // region goes, and a blur inside the opacity would have nothing
        // behind it to blur, so no region at all.
        this.fade = 1;
        this.shift = 0;
        // {texture, bounds}: a capture of the screen, placed in this
        // widget's coordinates.
        this.picture = null;
    }

    update(change) {
        Object.assign(this, change);
        this.queue_draw();
    }

    vfunc_snapshot(snapshot) {
        const [width, height] = [this.get_width(), this.get_height()];
        if (width <= 0 || height <= 0)
            return;
        snapshot.save();
        snapshot.translate(new Graphene.Point().init(0, this.shift));
        {
            const bounds = rect(0, 0, width, height);
            const box = new Gsk.RoundedRect().init_from_rect(bounds, this.radius);
            if (this.picture) {
                snapshot.push_opacity(this.fade);
                snapshot.push_rounded_clip(box);
                snapshot.append_texture(this.picture.texture, this.picture.bounds);
                snapshot.pop();
                snapshot.pop();
            } else if (this.blur && this.coverage > 0) {
                // What GtkWidget makes of `backdrop-filter: blur()`: the
                // blurred copy of what is under the widget. Of the surface
                // that is nothing; GDK reads the region off these nodes and
                // gives it to the compositor.
                snapshot.push_copy();
                snapshot.push_rounded_clip(box);
                const rows = this.coverage < 1;
                if (rows) {
                    const path = new Gsk.PathBuilder();
                    const level = this.coverage * ROW_ORDER.length;
                    for (let y = 0; y < height; y++) {
                        if (ROW_ORDER[y % ROW_ORDER.length] < level)
                            path.add_rect(rect(0, y, width, 1));
                    }
                    snapshot.push_fill(path.to_path(), Gsk.FillRule.WINDING);
                }
                snapshot.push_blur(BLUR_RADIUS);
                snapshot.append_paste(bounds, 0);
                snapshot.pop();
                if (rows)
                    snapshot.pop();
                snapshot.pop();
                snapshot.pop();
            }
        }
        if (this.fade > 0) {
            snapshot.push_opacity(this.fade);
            this.snapshot_child(this._child, snapshot);
            snapshot.pop();
        }
        snapshot.restore();
    }
});

// Holds the menu's content and takes up less than it: by `progress` of its
// height with the content sliding up out of sight (`roll`), or by
// `progress` of both with the content scaled to fit (`zoom`).
const Shrink = GObject.registerClass({
    CssName: 'shrink',
}, class Shrink extends Gtk.Widget {
    constructor(child) {
        super({overflow: Gtk.Overflow.HIDDEN});
        this._child = child;
        child.set_parent(this);
        this.mode = 'none';
        this.progress = 1;
    }

    update(mode, progress) {
        this.mode = mode;
        this.progress = progress;
        this.queue_resize();
    }

    _natural() {
        const [, width] = this._child.measure(Gtk.Orientation.HORIZONTAL, -1);
        const [, height] = this._child.measure(Gtk.Orientation.VERTICAL, width);
        return [width, height];
    }

    vfunc_measure(orientation, _forSize) {
        const [width, height] = this._natural();
        let size = orientation === Gtk.Orientation.HORIZONTAL ? width : height;
        if (this.mode === 'zoom' || (this.mode === 'roll' && orientation === Gtk.Orientation.VERTICAL))
            size = Math.round(size * this.progress);
        return [size, size, -1, -1];
    }

    vfunc_size_allocate(width, height, _baseline) {
        const [naturalWidth, naturalHeight] = this._natural();
        let transform = null;
        if (this.mode === 'roll') {
            transform = new Gsk.Transform().translate(
                new Graphene.Point().init(0, height - naturalHeight));
        } else if (this.mode === 'zoom') {
            const scale = Math.max(width / naturalWidth, 0.001);
            transform = new Gsk.Transform().scale(scale, scale);
        }
        this._child.allocate(naturalWidth, naturalHeight, -1, transform);
    }

    vfunc_snapshot(snapshot) {
        this.snapshot_child(this._child, snapshot);
    }
});

function css() {
    const popup = POPUP_MS * speed;
    const quad = 'cubic-bezier(0.25, 0.46, 0.45, 0.94)';
    return `
window.layer { background: transparent; }

.tint {
    color: rgba(255, 255, 255, 0.9);
    background-color: rgba(30, 30, 33, 0.5);
    border: 1px solid rgba(255, 255, 255, 0.06);
    border-radius: ${RADIUS}px;
    box-shadow: 0 4px 20px rgba(0, 0, 0, 0.2);
}
/* snapshot: the picture has the tint, the border and the content in it;
   only the shadow is still drawn. */
.tint.ghost { background-color: transparent; border-color: transparent; }
.tint.ghost .content { opacity: 0; }
/* opaque: about what the tint over a blurred, dimmed backdrop comes to. */
.tint.solid {
    background-color: rgb(46, 46, 48);
    transition: background-color ${popup * 0.4}ms ${quad};
}

.content { margin: 18px; }
.content .title { font-weight: bold; font-size: 1.1em; }
.content .dim { color: rgba(255, 255, 255, 0.68); }
.content button {
    min-height: 44px;
    min-width: 150px;
    border-radius: 99px;
    color: inherit;
    background: rgba(255, 255, 255, 0.1);
    box-shadow: none;
    border: none;
    font-weight: bold;
}
.content button.on { background: #3584e4; color: white; }
.content scale trough { background: rgba(255, 255, 255, 0.2); min-height: 6px; }
.content scale highlight { background: white; }

.controls {
    padding: 8px;
    border-radius: 16px;
    background: #1d1d20;
    color: white;
}
.controls button { min-height: 30px; padding: 0 12px; }
`;
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

function buildContent() {
    const content = new Gtk.Box({
        orientation: Gtk.Orientation.VERTICAL, spacing: 12, css_classes: ['content'],
    });
    const top = new Gtk.Box({spacing: 12});
    top.append(new Gtk.Image({icon_name: 'battery-level-80-symbolic'}));
    top.append(new Gtk.Label({label: '86 %', hexpand: true, xalign: 0, css_classes: ['title']}));
    for (const icon of ['camera-photo-symbolic', 'emblem-system-symbolic', 'system-shutdown-symbolic'])
        top.append(new Gtk.Image({icon_name: icon}));
    content.append(top);

    for (const [icon, value] of [['audio-volume-high-symbolic', 0.6], ['display-brightness-symbolic', 0.8]]) {
        const row = new Gtk.Box({spacing: 12});
        row.append(new Gtk.Image({icon_name: icon}));
        const scale = Gtk.Scale.new_with_range(Gtk.Orientation.HORIZONTAL, 0, 1, 0.01);
        scale.set_value(value);
        scale.set_hexpand(true);
        row.append(scale);
        content.append(row);
    }

    const grid = new Gtk.Grid({row_spacing: 12, column_spacing: 12, column_homogeneous: true});
    const toggles = [
        ['Wi-Fi', true], ['Bluetooth', true], ['Power Mode', false],
        ['Night Light', false], ['Dark Style', true], ['Airplane Mode', false],
    ];
    toggles.forEach(([label, on], index) => {
        grid.attach(new Gtk.Button({label, css_classes: on ? ['on'] : []}),
            index % 2, Math.floor(index / 2), 1, 1);
    });
    content.append(grid);
    content.append(new Gtk.Label({
        label: 'Close animation spike', xalign: 0, css_classes: ['dim'],
    }));
    return content;
}

const app = new Gtk.Application({application_id: 'dev.gniri.spike.closeblur'});

app.connect('activate', () => {
    const display = Gdk.Display.get_default();
    const provider = new Gtk.CssProvider();
    provider.load_from_string(css());
    Gtk.StyleContext.add_provider_for_display(display, provider,
        Gtk.STYLE_PROVIDER_PRIORITY_APPLICATION);
    // The zoom changes the tint's corner radius each frame.
    const radiusProvider = new Gtk.CssProvider();
    Gtk.StyleContext.add_provider_for_display(display, radiusProvider,
        Gtk.STYLE_PROVIDER_PRIORITY_APPLICATION + 1);

    const {TOP, BOTTOM, LEFT, RIGHT} = LayerShell.Edge;

    // The stage, as gniri's: screen-sized, on the overlay layer, mapped
    // only while the menu shows.
    const stage = layerWindow(app, 'gniri-stage', LayerShell.Layer.OVERLAY,
        [TOP, BOTTOM, LEFT, RIGHT]);
    LayerShell.set_exclusive_zone(stage, -1);
    const root = new Gtk.Box({hexpand: true, vexpand: true});
    stage.set_child(root);

    const content = buildContent();
    const shrink = new Shrink(content);
    const tint = new Gtk.Box({css_classes: ['tint']});
    tint.append(shrink);
    const menu = new Backdrop(tint);
    menu.add_css_class('menu');
    menu.set_halign(Gtk.Align.END);
    menu.set_valign(Gtk.Align.START);
    menu.set_hexpand(true);
    menu.set_margin_top(PANEL_HEIGHT + ARROW_RISE);
    menu.set_margin_end(ARROW_RISE);
    root.append(menu);

    let generation = 0;
    let shown = false;

    function setRadius(radius) {
        menu.update({radius});
        radiusProvider.load_from_string(
            radius === RADIUS ? '' : `.tint { border-radius: ${radius}px; }`);
    }

    function open() {
        generation += 1;
        const mine = generation;
        shown = true;
        // Everything a close may have left behind.
        menu.update({blur: true, coverage: 1, picture: null, fade: 0, shift: -ARROW_RISE});
        setRadius(RADIUS);
        tint.remove_css_class('ghost');
        tint.remove_css_class('solid');
        content.set_opacity(1);
        shrink.update('none', 1);

        stage.present();
        // As gniri's menus open: a fade and a slide down by the arrow's
        // rise, the blur there in full from the first frame.
        animate(menu, POPUP_MS, easeOutQuad, progress => {
            if (generation === mine)
                menu.update({fade: progress, shift: -ARROW_RISE * (1 - progress)});
        });
    }

    // The fade and the slide up that every fading variant ends with.
    function fadeOut(mine, ms, also) {
        animate(menu, ms, easeOutQuad, progress => {
            if (generation !== mine)
                return;
            menu.update({fade: 1 - progress, shift: -ARROW_RISE * progress});
            also?.(progress);
        }, () => unmapNow(mine));
    }

    // Unmaps the stage unless the menu was opened again meanwhile.
    function unmapNow(mine) {
        if (generation === mine)
            stage.set_visible(false);
    }

    const closers = {
        // gniri today: fade and slide; the blur stays until the unmap.
        current(mine) {
            fadeOut(mine, POPUP_MS);
        },

        snapshot(mine) {
            const [, box] = menu.compute_bounds(root);
            // A rectangle whose corners fall on whole device pixels at any
            // scale that is a multiple of a quarter.
            const x = Math.floor(box.get_x() / 4) * 4;
            const y = Math.floor(box.get_y() / 4) * 4;
            const width = Math.ceil((box.get_x() + box.get_width() - x) / 4) * 4;
            const height = Math.ceil((box.get_y() + box.get_height() - y) / 4) * 4;
            const file = GLib.build_filenamev([
                GLib.get_user_runtime_dir(), 'gniri-spike-closeblur.raw']);

            const asked = GLib.get_monotonic_time();
            const shot = Gio.Subprocess.new(
                [SHOT, `${x}`, `${y}`, `${width}`, `${height}`, file],
                Gio.SubprocessFlags.STDOUT_PIPE);
            shot.communicate_utf8_async(null, null, (_, result) => {
                const [, out] = shot.communicate_utf8_finish(result);
                if (generation !== mine)
                    return;
                if (!shot.get_successful()) {
                    log('SNAPSHOT failed; closing as today');
                    closers.current(mine);
                    return;
                }
                const [, pixelWidth, pixelHeight, stride, format] = out.trim().split(' ');
                const [, bytes] = GLib.file_get_contents(file);
                const texture = Gdk.MemoryTexture.new(
                    Number(pixelWidth), Number(pixelHeight),
                    format === 'Xbgr8888' || format === 'Abgr8888'
                        ? Gdk.MemoryFormat.R8G8B8X8 : Gdk.MemoryFormat.B8G8R8X8,
                    new GLib.Bytes(bytes), Number(stride));
                log('SNAPSHOT', `${pixelWidth}x${pixelHeight}`, format, 'after',
                    ((GLib.get_monotonic_time() - asked) / 1000).toFixed(1), 'ms');

                // One frame swaps the menu and its blur for their picture,
                // and starts the fade from the same look.
                menu.update({
                    picture: {
                        texture,
                        bounds: rect(x - box.get_x(), y - box.get_y(), width, height),
                    },
                });
                tint.add_css_class('ghost');
                fadeOut(mine, POPUP_MS);
            });
        },

        dither(mine) {
            fadeOut(mine, POPUP_MS, progress => menu.update({coverage: 1 - progress}));
        },

        roll(mine) {
            animate(menu, MOTION_MS, easeInOutQuad, progress => {
                if (generation !== mine)
                    return;
                shrink.update('roll', 1 - progress);
                content.set_opacity(1 - progress);
            }, () => unmapNow(mine));
        },

        zoom(mine) {
            animate(menu, MOTION_MS, easeInOutQuad, progress => {
                if (generation !== mine)
                    return;
                shrink.update('zoom', 1 - progress);
                setRadius(RADIUS * (1 - progress));
                content.set_opacity(1 - progress);
            }, () => unmapNow(mine));
        },

        opaque(mine) {
            tint.add_css_class('solid');
            GLib.timeout_add(GLib.PRIORITY_DEFAULT, POPUP_MS * 0.4 * speed, () => {
                if (generation === mine) {
                    // Nothing shows through the tint any more.
                    menu.update({blur: false});
                    fadeOut(mine, POPUP_MS * 0.6);
                }
                return GLib.SOURCE_REMOVE;
            });
        },
    };

    function close(variant) {
        if (!shown || !closers[variant])
            return;
        shown = false;
        generation += 1;
        log('CLOSE', variant);
        closers[variant](generation);
    }

    // Opens the menu if it is not open, lets it be seen, and closes it.
    function play(variant) {
        if (shown) {
            close(variant);
            return;
        }
        open();
        GLib.timeout_add(GLib.PRIORITY_DEFAULT, POPUP_MS * speed + 700, () => {
            close(variant);
            return GLib.SOURCE_REMOVE;
        });
    }

    function setSpeed(factor) {
        speed = factor;
        provider.load_from_string(css());
        log('SPEED', factor);
    }

    // The stage takes no input here. It is as large as the screen and lies
    // over the controls, so a click it took would never reach them; and
    // gniri's click outside the menu, which closes it, would close it as
    // `current` whichever button was meant.
    stage.connect('map', () => {
        const surface = stage.get_surface();
        const ignore = () => surface.set_input_region(new Cairo.Region());
        ignore();
        // GTK may put the region back when the surface is laid out.
        surface.connect('layout', ignore);
    });

    // The controls, on their own surface so they stay while the stage goes.
    const controls = layerWindow(app, 'gniri-spike-controls', LayerShell.Layer.OVERLAY, [BOTTOM]);
    LayerShell.set_margin(controls, BOTTOM, 24);
    const bar = new Gtk.Box({spacing: 6, css_classes: ['controls']});
    const openButton = new Gtk.Button({label: 'Open'});
    openButton.connect('clicked', () => open());
    bar.append(openButton);
    bar.append(new Gtk.Label({label: 'Close:', margin_start: 12}));
    for (const variant of VARIANTS) {
        const button = new Gtk.Button({label: variant});
        button.connect('clicked', () => play(variant));
        bar.append(button);
    }
    bar.append(new Gtk.Label({label: 'Speed:', margin_start: 12}));
    let group = null;
    for (const factor of [1, 4, 16]) {
        const button = new Gtk.ToggleButton({label: factor === 1 ? 'real' : `${factor}× slower`});
        if (group)
            button.set_group(group);
        else
            group = button;
        button.set_active(factor === speed);
        button.connect('toggled', () => {
            if (button.get_active())
                setSpeed(factor);
        });
        bar.append(button);
    }
    const quit = new Gtk.Button({label: 'Quit', margin_start: 12});
    quit.connect('clicked', () => app.quit());
    bar.append(quit);
    controls.set_child(bar);
    controls.present();

    const stdin = new Gio.DataInputStream({
        base_stream: new GioUnix.InputStream({fd: 0, close_fd: false}),
    });
    const readNext = () => {
        stdin.read_line_async(GLib.PRIORITY_DEFAULT, null, (stream, result) => {
            const [line] = stream.read_line_finish_utf8(result);
            if (line === null)
                return;
            const [command, argument] = line.trim().split(/\s+/);
            if (command === 'open')
                open();
            else if (command === 'close')
                close(argument);
            else if (command === 'play')
                play(argument);
            else if (command === 'speed')
                setSpeed(Number(argument));
            else if (command === 'quit')
                app.quit();
            readNext();
        });
    };
    readNext();
});

app.run([]);
