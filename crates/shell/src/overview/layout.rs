// SPDX-License-Identifier: GPL-3.0-or-later

//! The overview's arithmetic: where its parts go for a given screen, how
//! many icons fit a page of the app grid and how large the dash's icons
//! are. Ports of `ControlsManagerLayout` (overviewControls.js),
//! `IconGridLayout` and `IconGrid` (iconGrid.js), `BaseAppViewGridLayout`
//! (appDisplay.js) and `Dash._adjustIconSize` (dash.js).
//!
//! Everything here depends on the size of the screen, and the nested
//! session the visual tests run in is smaller than the screen GNOME was
//! captured on. So these are plain functions, checked against the boxes
//! GNOME Shell 51.1 gave its own actors on that screen.

/// overviewControls.js `VERTICAL_SPACING_RATIO`.
const VERTICAL_SPACING_RATIO: f64 = 0.02;
/// overviewControls.js `DASH_MAX_HEIGHT_RATIO`.
const DASH_MAX_HEIGHT_RATIO: f64 = 0.16;
/// overviewControls.js `SMALL_WORKSPACE_RATIO`: the height GNOME keeps for
/// its strip of workspaces above the app grid. gniri has no such strip
/// (docs/FIDELITY.md) and leaves the room empty, so the grid is where
/// GNOME's is.
const SMALL_WORKSPACE_RATIO: f64 = 0.15;

/// `Math.round`: halves go up.
fn round(value: f64) -> i32 {
    (value + 0.5).floor() as i32
}

/// A vertical span: where it starts and how tall it is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Span {
    pub y: i32,
    pub height: i32,
}

/// Where the overview's parts go, in a surface that starts under the panel.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Controls {
    /// The tallest the dash may be; its icons shrink to fit.
    pub max_dash_height: i32,
    pub dash: Span,
    pub app_display: Span,
    pub search_results: Span,
}

/// `ControlsManagerLayout.vfunc_allocate` for a surface `height` tall, a
/// search entry asking for `search_height` and a dash asking for
/// `dash_height`.
pub fn controls(height: i32, search_height: i32, dash_height: i32) -> Controls {
    let spacing = round(f64::from(height) * VERTICAL_SPACING_RATIO);
    let max_dash_height = max_dash_height(height);
    let dash_height = dash_height.min(max_dash_height);
    let workspaces = round(f64::from(height) * SMALL_WORKSPACE_RATIO);
    Controls {
        max_dash_height,
        dash: Span {
            y: height - dash_height,
            height: dash_height,
        },
        app_display: Span {
            y: search_height + spacing + workspaces + spacing,
            height: (height
                - search_height
                - spacing
                - workspaces
                - spacing
                - dash_height
                - spacing)
                .max(0),
        },
        search_results: Span {
            y: search_height + spacing,
            height: (height - search_height - spacing - dash_height - spacing).max(0),
        },
    }
}

/// The tallest the dash may be on a surface `height` tall.
pub fn max_dash_height(height: i32) -> i32 {
    round(f64::from(height) * DASH_MAX_HEIGHT_RATIO)
}

/// dash.js `baseIconSizes`.
const DASH_ICON_SIZES: [i32; 6] = [16, 22, 24, 32, 48, 64];

/// What surrounds the icons of the dash, from the theme.
#[derive(Clone, Copy, Debug)]
pub struct DashMetrics {
    /// `#dash`'s horizontal padding, both sides together.
    pub horizontal_padding: i32,
    /// `.dash-background`'s vertical padding, both sides together.
    pub background_vertical_padding: i32,
    /// How much wider an item's button is than its icon.
    pub button_extra_width: i32,
    /// How much taller.
    pub button_extra_height: i32,
}

/// `Dash._adjustIconSize`: the largest of the dash's icon sizes at which
/// `items` icons (the show-apps button included) fit `max_width` by
/// `max_height`.
pub fn dash_icon_size(max_width: i32, max_height: i32, items: usize, metrics: DashMetrics) -> i32 {
    let items = items.max(1) as i32;
    let available_width =
        max_width - metrics.horizontal_padding - items * metrics.button_extra_width;
    let available_height =
        max_height - metrics.background_vertical_padding - metrics.button_extra_height;
    let largest = f64::from(available_width) / f64::from(items);
    let largest = largest.min(f64::from(available_height));
    DASH_ICON_SIZES
        .into_iter()
        .rfind(|&size| f64::from(size) <= largest)
        .unwrap_or(DASH_ICON_SIZES[0])
}

/// iconGrid.js `IconSize`, largest first.
pub const GRID_ICON_SIZES: [i32; 6] = [96, 64, 48, 32, 24, 16];

/// iconGrid.js `defaultGridModes`: (columns, rows).
const GRID_MODES: [(usize, usize); 4] = [(3, 8), (4, 6), (6, 4), (8, 3)];
/// `FolderGrid`'s one mode.
const FOLDER_MODES: [(usize, usize); 1] = [(3, 3)];

/// Which of GNOME's two icon grids: the app grid, which spreads its icons
/// over the page (`page_halign` and `page_valign` FILL), or a folder's,
/// three by three in the middle of the page (CENTER).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum GridKind {
    #[default]
    Apps,
    Folder,
}

impl GridKind {
    fn modes(self) -> &'static [(usize, usize)] {
        match self {
            Self::Apps => &GRID_MODES,
            Self::Folder => &FOLDER_MODES,
        }
    }
}

/// appDisplay.js `PAGE_PREVIEW_RATIO`.
const PAGE_PREVIEW_RATIO: f64 = 0.20;

/// `.icon-grid`'s lengths, from the theme.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct GridStyle {
    pub row_spacing: f64,
    pub column_spacing: f64,
    pub max_row_spacing: f64,
    pub max_column_spacing: f64,
    pub page_padding_top: f64,
    pub page_padding_bottom: f64,
    pub page_padding_left: f64,
    pub page_padding_right: f64,
}

/// How one page of the app grid is laid out.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Grid {
    pub columns: usize,
    pub rows: usize,
    /// The side of an item's square.
    pub child_size: f64,
    /// The room at each side for the arrows that turn the page.
    pub indicators_width: f64,
    left: f64,
    top: f64,
    column_step: f64,
    row_step: f64,
}

/// iconGrid.js `LEFT_DIVIDER_LEEWAY` and `RIGHT_DIVIDER_LEEWAY`: how far
/// into an icon its edge reaches, for a drop.
const DIVIDER_LEEWAY: f64 = 20.0;

/// Where on an icon something dragged is (iconGrid.js `DragLocation`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DragLocation {
    /// Its left edge: the place before it.
    StartEdge,
    /// The icon itself: something to drop onto it.
    OnIcon,
    /// Its right edge: the place after it.
    EndEdge,
}

/// What is under a point of a page of the grid.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DropTarget {
    /// Above or below the icons: nowhere to drop.
    Invalid,
    /// Among the icons, but on none: after the last.
    Empty,
    /// On or beside the icon at this place of the page.
    Item(usize, DragLocation),
}

impl Grid {
    pub fn per_page(&self) -> usize {
        self.columns * self.rows
    }

    /// How many pages `items` icons take; an empty grid still has one.
    pub fn pages(&self, items: usize) -> usize {
        items.div_ceil(self.per_page()).max(1)
    }

    /// Where the item at `position` of a page goes, relative to the page
    /// (`IconGridLayout.vfunc_allocate`).
    pub fn origin(&self, position: usize) -> (i32, i32) {
        let (row, column) = (position / self.columns, position % self.columns);
        (
            (self.left + column as f64 * self.column_step).floor() as i32,
            (self.top + row as f64 * self.row_step).floor() as i32,
        )
    }

    /// What is at `(x, y)` of a page that shows `count` items
    /// (`IconGridLayout.getDropTarget`).
    pub fn drop_target(&self, (x, y): (f64, f64), count: usize) -> DropTarget {
        let child = self.child_size;
        let (column_spacing, row_spacing) = (self.column_step - child, self.row_step - child);
        let width = child * self.columns as f64 + column_spacing * (self.columns as f64 - 1.0);
        let height = child * self.rows as f64 + row_spacing * (self.rows as f64 - 1.0);
        if y < self.top || y > self.top + height {
            return DropTarget::Invalid;
        }
        let in_left_space = x < self.left;
        let in_right_space = x > self.left + width;
        let (half_column, half_row) = (column_spacing / 2.0, row_spacing / 2.0);
        for position in 0..count.min(self.per_page()) {
            let (left, top) = self.origin(position);
            let (left, top) = (f64::from(left), f64::from(top));
            let (right, bottom) = (left + child, top + child);
            let first_in_row = position % self.columns == 0;
            let last_in_row = position % self.columns == self.columns - 1;
            let outside_row = y < top - half_row || y > bottom + half_row;
            let outside = if (in_left_space && first_in_row) || (in_right_space && last_in_row) {
                outside_row
            } else {
                outside_row || x < left - half_column || x > right + half_column
            };
            if outside {
                continue;
            }
            let location = if x < left + DIVIDER_LEEWAY {
                DragLocation::StartEdge
            } else if x > right - DIVIDER_LEEWAY {
                DragLocation::EndEdge
            } else {
                DragLocation::OnIcon
            };
            return DropTarget::Item(position, location);
        }
        DropTarget::Empty
    }
}

/// Where in the grid's order an item dragged from `source` ends up when it
/// is let go over the item at `target`, both counted through every page
/// (`AppDisplay._getDropTarget` and `_moveItem`, with pages that are always
/// full). `source` is `None` for an item from elsewhere, which comes in
/// after everything. The item is taken out first and put in at the place
/// this returns.
pub fn reorder_target(
    source: Option<usize>,
    target: usize,
    location: DragLocation,
    columns: usize,
) -> usize {
    let column = target % columns;
    match source {
        // Taking it out moves everything after it back by one: dropped at
        // the start of the icon it is the place before it, unless that
        // icon starts a row.
        Some(source) if source < target => match location {
            DragLocation::StartEdge if column > 0 => target - 1,
            _ => target,
        },
        Some(source) if source == target => target,
        // Before the item, or from elsewhere: its end edge is the place
        // after it, unless it ends a row.
        _ => match location {
            DragLocation::EndEdge if column < columns - 1 => target + 1,
            _ => target,
        },
    }
}

/// `BaseAppViewGridLayout._getIndicatorsWidth`: a tenth of the width, or
/// what an arrow needs if that is more.
fn indicators_width(width: f64, arrow_width: f64) -> f64 {
    (width * PAGE_PREVIEW_RATIO / 2.0).max(arrow_width)
}

/// `IconGrid._findBestModeForSize`: the mode whose shape is nearest the
/// page's, the page's padding left out.
fn grid_mode(
    width: f64,
    height: f64,
    style: &GridStyle,
    indicators: f64,
    kind: GridKind,
) -> (usize, usize) {
    let width = width - style.page_padding_left - style.page_padding_right - 2.0 * indicators;
    let height = height - style.page_padding_top - style.page_padding_bottom;
    let ratio = width / height;
    let mut closest = f64::INFINITY;
    let modes = kind.modes();
    let mut best = modes[0];
    for &mode in modes {
        let mode_ratio = mode.0 as f64 / mode.1 as f64;
        if (ratio - mode_ratio).abs() < (ratio - closest).abs() {
            closest = mode_ratio;
            best = mode;
        }
    }
    best
}

/// The room left over on a page whose items are `child_size` wide and
/// tall, horizontally and vertically.
fn empty_space(
    (width, height): (f64, f64),
    (columns, rows): (usize, usize),
    child_size: f64,
    style: &GridStyle,
    indicators: f64,
) -> (f64, f64) {
    let (columns, rows) = (columns as f64, rows as f64);
    (
        width
            - child_size * columns
            - style.column_spacing * (columns - 1.0)
            - style.page_padding_left
            - style.page_padding_right
            - 2.0 * indicators,
        height
            - child_size * rows
            - style.row_spacing * (rows - 1.0)
            - style.page_padding_top
            - style.page_padding_bottom,
    )
}

/// `IconGridLayout._findBestIconSize`: the largest icon size whose items
/// fit the page. `item_size` says how large an item with an icon of a
/// given size is.
pub fn grid_icon_size(
    size: (f64, f64),
    style: &GridStyle,
    arrow_width: f64,
    kind: GridKind,
    item_size: impl Fn(i32) -> f64,
) -> i32 {
    let indicators = indicators_width(size.0, arrow_width);
    let mode = grid_mode(size.0, size.1, style, indicators, kind);
    GRID_ICON_SIZES
        .into_iter()
        .find(|&icon| {
            let (horizontal, vertical) =
                empty_space(size, mode, item_size(icon), style, indicators);
            horizontal >= 0.0 && vertical > 0.0
        })
        .unwrap_or(GRID_ICON_SIZES[GRID_ICON_SIZES.len() - 1])
}

/// Lays a page of `size` out for items `child_size` wide and tall
/// (`IconGridLayout._calculateSpacing`, with the page filled both ways as
/// the app grid has it). `arrow_width` is what a page-turning arrow needs.
pub fn grid(
    size: (f64, f64),
    style: &GridStyle,
    arrow_width: f64,
    child_size: f64,
    kind: GridKind,
) -> Grid {
    let indicators = indicators_width(size.0, arrow_width);
    let mode = grid_mode(size.0, size.1, style, indicators, kind);
    let (empty_horizontal, empty_vertical) = empty_space(size, mode, child_size, style, indicators);
    if kind == GridKind::Folder {
        // CENTER both ways: the spacing stays, the room goes around.
        return Grid {
            columns: mode.0,
            rows: mode.1,
            child_size,
            indicators_width: indicators,
            left: style.page_padding_left + indicators + (empty_horizontal / 2.0).floor(),
            top: style.page_padding_top + (empty_vertical / 2.0).floor(),
            column_step: child_size + style.column_spacing,
            row_step: child_size + style.row_spacing,
        };
    }

    // The spare room goes between the items, up to the theme's limit; what
    // is left after that goes around them.
    let spread = |spacing: f64, max_spacing: f64, empty: f64, count: usize| {
        let gaps = (count as f64 - 1.0).max(1.0);
        let spaced = spacing + empty / gaps;
        if max_spacing >= 0.0 && spaced > max_spacing {
            let extra = (max_spacing - spacing) * gaps;
            (max_spacing, ((empty - extra) / 2.0).max(0.0))
        } else {
            (spaced, 0.0)
        }
    };
    let (column_spacing, left) = spread(
        style.column_spacing,
        style.max_column_spacing,
        empty_horizontal,
        mode.0,
    );
    let (row_spacing, top) = spread(
        style.row_spacing,
        style.max_row_spacing,
        empty_vertical,
        mode.1,
    );
    Grid {
        columns: mode.0,
        rows: mode.1,
        child_size,
        indicators_width: indicators,
        left: style.page_padding_left + indicators + left,
        top: style.page_padding_top + top,
        column_step: child_size + column_spacing,
        row_step: child_size + row_spacing,
    }
}

/// `GridSearchResultsLayout.columnsForWidth`: how many application results
/// `item_width` wide fit one row.
pub fn search_columns(width: i32, item_width: i32, spacing: i32) -> usize {
    if item_width <= 0 {
        return 0;
    }
    let mut width = width;
    let mut columns = 0;
    while width > item_width {
        width -= item_width;
        if columns > 0 {
            width -= spacing;
        }
        columns += 1;
    }
    columns
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `.icon-grid` in GNOME Shell 51.1's theme.
    const STYLE: GridStyle = GridStyle {
        row_spacing: 12.0,
        column_spacing: 12.0,
        max_row_spacing: 36.0,
        max_column_spacing: 36.0,
        page_padding_top: 24.0,
        page_padding_bottom: 24.0,
        page_padding_left: 18.0,
        page_padding_right: 18.0,
    };
    /// `.page-navigation-arrow`: 24 wide, 18 of padding and 6 of margin on
    /// each side.
    const ARROW: f64 = 74.0;
    /// The dash of GNOME Shell 51.1's theme.
    const DASH: DashMetrics = DashMetrics {
        horizontal_padding: 12,
        background_vertical_padding: 24,
        button_extra_width: 12,
        button_extra_height: 50,
    };

    /// The reference screen is 1792x1120 with a 32px panel
    /// (tests/visual/reference/gnome-51.1/raw/metrics/app-grid.json and
    /// search.json; boxes there include the panel's 32px).
    #[test]
    fn the_parts_go_where_gnome_puts_them() {
        let layout = controls(1088, 55, 114);
        assert_eq!(layout.max_dash_height, 174);
        assert_eq!(
            layout.dash,
            Span {
                y: 1006 - 32,
                height: 114
            }
        );
        assert_eq!(
            layout.app_display,
            Span {
                y: 294 - 32,
                height: 690
            }
        );
        assert_eq!(
            layout.search_results,
            Span {
                y: 109 - 32,
                height: 875
            }
        );
    }

    #[test]
    fn a_dash_that_asks_for_too_much_is_cut_to_its_share() {
        let layout = controls(500, 55, 114);
        assert_eq!(layout.dash, Span { y: 420, height: 80 });
    }

    #[test]
    fn the_dash_icons_shrink_on_a_small_screen() {
        // The reference screen, nine items: 64px, as captured.
        assert_eq!(dash_icon_size(1792, 174, 9, DASH), 64);
        // The nested test output, 1280x768 under the panel.
        assert_eq!(dash_icon_size(1280, max_dash_height(768), 9, DASH), 48);
        // Many items on a narrow screen: the width decides.
        assert_eq!(dash_icon_size(800, 174, 14, DASH), 32);
        assert_eq!(dash_icon_size(100, 20, 9, DASH), 16);
    }

    #[test]
    fn the_app_grid_is_laid_out_as_on_the_reference_screen() {
        // The scroll view: 674 of the app display's 690, the rest being
        // the page dots.
        let size = (1792.0, 674.0);
        assert_eq!(
            grid_icon_size(size, &STYLE, ARROW, GridKind::Apps, |icon| f64::from(icon)
                + 49.0),
            96
        );
        let grid = grid(size, &STYLE, ARROW, 145.0, GridKind::Apps);
        assert_eq!((grid.columns, grid.rows), (8, 3));
        // The first tile of each column and row, less the display's own
        // origin (0, 294).
        let columns: Vec<i32> = (0..8).map(|column| grid.origin(column).0).collect();
        assert_eq!(columns, [197, 376, 555, 734, 912, 1091, 1270, 1449]);
        let rows: Vec<i32> = (0..3).map(|row| grid.origin(row * 8).1 + 294).collect();
        assert_eq!(rows, [377, 558, 739]);
        assert_eq!(grid.pages(30), 2);
        assert_eq!(grid.pages(24), 1);
        assert_eq!(grid.pages(0), 1);
        // The arrow is centred in a band this wide at the page's edge:
        // 1792 - 179.2 + (179.2 - 62) / 2 = 1671.4, where GNOME has it.
        assert!((grid.indicators_width - 179.2).abs() < 1e-9);
    }

    /// Every icon of the first page, and the arrow, where the capture of
    /// GNOME's own app grid has them
    /// (tests/visual/reference/gnome-51.1/metrics/app-grid.json).
    #[test]
    fn every_icon_is_where_gnome_has_it() {
        let path = concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../tests/visual/reference/gnome-51.1/metrics/app-grid.json"
        );
        let reference: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap();
        fn find<'a>(
            node: &'a serde_json::Value,
            class: &str,
            found: &mut Vec<&'a serde_json::Value>,
        ) {
            let classes = node["classes"].as_str().unwrap_or_default();
            if classes.split(' ').any(|other| other == class) {
                found.push(node);
                return;
            }
            for child in node["children"].as_array().into_iter().flatten() {
                find(child, class, found);
            }
        }
        let rect = |node: &serde_json::Value| -> [f64; 4] {
            let numbers: Vec<f64> = node["box"]
                .as_array()
                .unwrap()
                .iter()
                .map(|number| number.as_f64().unwrap())
                .collect();
            [numbers[0], numbers[1], numbers[2], numbers[3]]
        };
        let one = |class: &str| {
            let mut found = Vec::new();
            find(&reference["tree"], class, &mut found);
            rect(found[0])
        };

        let view = one("apps-scroll-view");
        let arrow = one("page-navigation-arrow");
        // The arrow's box and the 6px of margin on each side of it.
        let arrow_room = arrow[2] + 12.0;
        let mut tiles = Vec::new();
        find(&reference["tree"], "overview-tile", &mut tiles);
        let tiles: Vec<[f64; 4]> = tiles.into_iter().map(rect).collect();
        assert!(tiles.len() > 24, "the capture has a second page");
        let child = tiles[0][2];

        let size = (view[2], view[3]);
        // An item is 49px larger than its icon: 96px icons give 145.
        assert_eq!(
            f64::from(grid_icon_size(
                size,
                &STYLE,
                arrow_room,
                GridKind::Apps,
                |icon| f64::from(icon) + 49.0
            )) + 49.0,
            child
        );
        let grid = grid(size, &STYLE, arrow_room, child, GridKind::Apps);
        assert_eq!(grid.per_page(), 24);
        assert_eq!(grid.pages(tiles.len()), 2);
        for (index, tile) in tiles.iter().enumerate() {
            let (page, position) = (index / grid.per_page(), index % grid.per_page());
            let (x, y) = grid.origin(position);
            // The pages are side by side, each as wide as the view.
            let expected = [
                f64::from(x) + view[0] + page as f64 * view[2],
                f64::from(y) + view[1],
            ];
            assert_eq!([tile[0], tile[1]], expected, "icon {index}");
            assert_eq!([tile[2], tile[3]], [child, child], "icon {index}");
        }

        // The arrow to the next page, in the middle of its band at the
        // right; app_grid.rs places it so.
        let band = grid.indicators_width;
        let left = (view[0] + view[2] - band + (band - arrow_room) / 2.0).floor() + 6.0;
        assert_eq!(arrow[0], left);
        assert_eq!(
            arrow[1],
            view[1] + ((view[3] - arrow_room) / 2.0).floor() + 6.0
        );
    }

    #[test]
    fn a_small_page_takes_smaller_icons() {
        // The nested test output: the scroll view is 1280x423.
        let size = (1280.0, 423.0);
        let item = |icon: i32| f64::from(icon) + 49.0;
        assert_eq!(
            grid_icon_size(size, &STYLE, ARROW, GridKind::Apps, item),
            64
        );
        let grid = grid(size, &STYLE, ARROW, item(64), GridKind::Apps);
        assert_eq!((grid.columns, grid.rows), (8, 3));
    }

    #[test]
    fn a_tall_page_takes_the_tall_mode() {
        let grid = grid((600.0, 1200.0), &STYLE, ARROW, 113.0, GridKind::Apps);
        assert_eq!((grid.columns, grid.rows), (3, 8));
    }

    #[test]
    fn application_results_fill_one_row() {
        // #searchResultsContent is 1044 wide on the reference screen;
        // tiles are 145 wide and 30 apart.
        assert_eq!(search_columns(1044, 145, 30), 6);
        assert_eq!(search_columns(145, 145, 30), 0);
        assert_eq!(search_columns(146, 145, 30), 1);
        assert_eq!(search_columns(0, 0, 30), 0);
    }

    #[test]
    fn a_folder_has_three_rows_of_three_in_the_middle() {
        // A 720px dialog's view: its own spacing, the rest around.
        let grid = grid((718.0, 600.0), &STYLE, ARROW, 120.0, GridKind::Folder);
        assert_eq!((grid.columns, grid.rows), (3, 3));
        let (first, second) = (grid.origin(0), grid.origin(1));
        assert_eq!(second.0 - first.0, 132, "120 and the 12 between them");
        let width = 3 * 120 + 2 * 12;
        let indicators = 71.8_f64.max(ARROW);
        let left = 18.0
            + indicators
            + ((718.0 - 36.0 - 2.0 * indicators - f64::from(width)) / 2.0).floor();
        assert_eq!(first.0, left as i32);
    }

    /// The page of the app grid on the reference screen: 8 by 3 items of
    /// 145, as `the_app_grid_is_laid_out_as_on_the_reference_screen` has it.
    fn reference_page() -> Grid {
        grid((1792.0, 674.0), &STYLE, ARROW, 145.0, GridKind::Apps)
    }

    #[test]
    fn a_drop_is_on_an_icon_or_beside_it() {
        let grid = reference_page();
        let (left, top) = grid.origin(9);
        let (left, top) = (f64::from(left), f64::from(top));
        let middle = top + 72.0;
        assert_eq!(
            grid.drop_target((left + 72.0, middle), 24),
            DropTarget::Item(9, DragLocation::OnIcon)
        );
        assert_eq!(
            grid.drop_target((left + 5.0, middle), 24),
            DropTarget::Item(9, DragLocation::StartEdge)
        );
        assert_eq!(
            grid.drop_target((left + 140.0, middle), 24),
            DropTarget::Item(9, DragLocation::EndEdge)
        );
        // Half the spacing beside it still counts as its edge.
        assert_eq!(
            grid.drop_target((left - 2.0, middle), 24),
            DropTarget::Item(9, DragLocation::StartEdge)
        );
        // Above the icons there is nothing; past the last one there is
        // room after it.
        assert_eq!(grid.drop_target((left, 2.0), 24), DropTarget::Invalid);
        assert_eq!(
            grid.drop_target((left + 72.0, middle), 5),
            DropTarget::Empty
        );
        // The room at the side of a row belongs to its first icon.
        let (_, row_top) = grid.origin(8);
        assert_eq!(
            grid.drop_target((1.0, f64::from(row_top) + 72.0), 24),
            DropTarget::Item(8, DragLocation::StartEdge)
        );
    }

    #[test]
    fn a_dragged_icon_lands_before_or_after_the_one_it_was_let_go_on() {
        use DragLocation::{EndEdge, StartEdge};
        // Eight columns. From before the target, its start edge is the
        // place before it, which after taking the item out is one less.
        assert_eq!(reorder_target(Some(2), 5, StartEdge, 8), 4);
        assert_eq!(reorder_target(Some(2), 5, EndEdge, 8), 5);
        // From after it, its end edge is the place after it.
        assert_eq!(reorder_target(Some(9), 5, StartEdge, 8), 5);
        assert_eq!(reorder_target(Some(9), 5, EndEdge, 8), 6);
        // From elsewhere, as from after it.
        assert_eq!(reorder_target(None, 5, EndEdge, 8), 6);
        // At the ends of a row GNOME does not reach into the next.
        assert_eq!(reorder_target(Some(2), 8, StartEdge, 8), 8);
        assert_eq!(reorder_target(Some(9), 7, EndEdge, 8), 7);
        assert_eq!(reorder_target(Some(5), 5, EndEdge, 8), 5);
    }
}
