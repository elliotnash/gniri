// SPDX-License-Identifier: GPL-3.0-or-later

// Evaluated inside GNOME Shell through org.gnome.Shell.Eval (unsafe mode).
// capture.py replaces __ROOT__ with a JS expression for the root actor.
//
// Returns a tree of every mapped actor under the root: type, name, style
// classes, stage-space box, paint opacity, text/icon, and the computed St
// theme-node values that the gniri theme has to reproduce.
(async () => {
    const St = (await import('gi://St')).default;

    const SIDES = [St.Side.TOP, St.Side.RIGHT, St.Side.BOTTOM, St.Side.LEFT];
    const CORNERS = [St.Corner.TOPLEFT, St.Corner.TOPRIGHT,
        St.Corner.BOTTOMRIGHT, St.Corner.BOTTOMLEFT];
    const round = v => Math.round(v * 100) / 100;
    const color = c => c ? [c.red, c.green, c.blue, c.alpha] : null;
    const attempt = fn => {
        try {
            return fn();
        } catch {
            return undefined;
        }
    };
    const nonZero = list => list && list.some(v => v) ? list : undefined;
    const visible = c => c && c[3] > 0 ? c : undefined;

    function style(widget) {
        const node = attempt(() => widget.get_theme_node());
        if (!node)
            return undefined;

        const out = {
            bg: visible(attempt(() => color(node.get_background_color()))),
            fg: attempt(() => color(node.get_foreground_color())),
            radius: nonZero(attempt(() => CORNERS.map(c => node.get_border_radius(c)))),
            padding: nonZero(attempt(() => SIDES.map(s => round(node.get_padding(s))))),
            margin: nonZero(attempt(() => SIDES.map(s => round(node.get_margin(s))))),
            border_width: nonZero(attempt(() => SIDES.map(s => node.get_border_width(s)))),
            font: attempt(() => node.get_font().to_string()),
            transition_ms: attempt(() => node.get_transition_duration()) || undefined,
        };
        if (out.border_width)
            out.border_color = attempt(() => SIDES.map(s => color(node.get_border_color(s))));

        const spacing = attempt(() => node.lookup_length('spacing', false));
        if (spacing && spacing[0])
            out.spacing = round(spacing[1]);

        const size = attempt(() => ({
            width: node.get_width(), height: node.get_height(),
            min_width: node.get_min_width(), min_height: node.get_min_height(),
            max_width: node.get_max_width(), max_height: node.get_max_height(),
        }));
        for (const [key, value] of Object.entries(size ?? {})) {
            if (value >= 0)
                out[key] = round(value);
        }

        const shadow = attempt(() => node.get_box_shadow());
        if (shadow) {
            out.box_shadow = {
                color: color(shadow.color), blur: shadow.blur, spread: shadow.spread,
                x: shadow.xoffset, y: shadow.yoffset, inset: shadow.inset,
            };
        }
        return out;
    }

    const skipped = new Set([global.window_group, global.top_window_group]);

    function walk(actor, depth) {
        if (!actor.mapped || skipped.has(actor) || depth > 60)
            return null;

        const extents = actor.get_transformed_extents();
        const rec = {
            type: actor.constructor.$gtype.name,
            name: actor.name || undefined,
            box: [round(extents.origin.x), round(extents.origin.y),
                round(extents.size.width), round(extents.size.height)],
            opacity: actor.get_paint_opacity(),
        };

        if (actor instanceof St.Widget) {
            rec.classes = actor.style_class || undefined;
            rec.pseudo = actor.pseudo_class || undefined;
            rec.style = style(actor);
            if (actor.style)
                rec.inline_style = actor.style;
        }
        if (actor instanceof St.Label)
            rec.text = actor.text;
        if (actor instanceof St.Icon) {
            rec.icon = actor.icon_name ?? attempt(() => actor.gicon?.to_string());
            rec.icon_size = actor.icon_size;
        }
        const effects = actor.get_effects().map(e => e.constructor.$gtype.name);
        if (effects.length)
            rec.effects = effects;

        const children = actor.get_children().map(c => walk(c, depth + 1)).filter(c => c);
        if (children.length)
            rec.children = children;
        return rec;
    }

    const themeContext = St.ThemeContext.get_for_stage(global.stage);
    return {
        stage: [global.stage.width, global.stage.height],
        scale_factor: themeContext.scale_factor,
        resource_scale: attempt(() => global.stage.get_resource_scale()),
        tree: walk(__ROOT__, 0),
    };
})()
