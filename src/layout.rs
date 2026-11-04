//! Geometry for placing folders: wrapping titles, stacks of folders, snapping,
//! arranging and finding a free spot. Plain functions on rectangles, with no
//! window calls, so all of it is tested here.

/// A rectangle in screen pixels.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub struct Rect {
    pub x: i32,
    pub y: i32,
    pub w: i32,
    pub h: i32,
}

impl Rect {
    pub fn right(&self) -> i32 {
        self.x + self.w
    }

    pub fn bottom(&self) -> i32 {
        self.y + self.h
    }

    fn overlaps(&self, o: &Rect, margin: i32) -> bool {
        self.x < o.right() + margin && o.x < self.right() + margin && self.y < o.bottom() + margin && o.y < self.bottom() + margin
    }
}

/// Splits `text` into at most `max_lines` lines no wider than `width`, breaking
/// between words, or inside a word too long for a line. The last line keeps
/// the rest of the text: it is drawn with "…" when it is still too long.
pub fn wrap(text: &str, width: i32, max_lines: usize, measure: impl Fn(&str) -> i32) -> Vec<String> {
    let mut rest = text.trim();
    let mut lines = Vec::new();
    while lines.len() + 1 < max_lines && !rest.is_empty() && measure(rest) > width {
        // The longest start of the text that ends between two words and fits.
        let mut cut = None;
        for (i, c) in rest.char_indices() {
            if c == ' ' {
                if measure(rest[..i].trim_end()) > width {
                    break;
                }
                cut = Some(i);
            }
        }
        let end = match cut {
            Some(i) if !rest[..i].trim().is_empty() => i,
            // The first word alone is too long: break inside it, keeping at least one character.
            _ => {
                let mut end = rest.char_indices().nth(1).map_or(rest.len(), |(i, _)| i);
                for (i, _) in rest.char_indices().skip(2) {
                    if measure(&rest[..i]) > width {
                        break;
                    }
                    end = i;
                }
                end
            }
        };
        lines.push(rest[..end].trim_end().to_string());
        rest = rest[end..].trim_start();
    }
    if !rest.is_empty() || lines.is_empty() {
        lines.push(rest.to_string());
    }
    lines
}

/// The folders stacked under `labels[from]`: touching its bottom within `gap`
/// and overlapping its column, then the ones stacked under those, and so on.
pub fn stacked_below(labels: &[Rect], from: usize, gap: i32) -> Vec<usize> {
    let mut chain = vec![from];
    let mut k = 0;
    while k < chain.len() {
        let a = labels[chain[k]];
        for (j, b) in labels.iter().enumerate() {
            if chain.contains(&j) {
                continue;
            }
            let overlap = a.right().min(b.right()) - a.x.max(b.x);
            let dy = b.y - a.bottom();
            if overlap > 10 && dy >= -2 && dy <= gap + 4 {
                chain.push(j);
            }
        }
        k += 1;
    }
    chain.remove(0);
    chain
}

/// Whether a folder whose label is at `top` opens upwards: when it does not fit
/// below but fits above, or, fitting nowhere, when there is more room above.
pub fn opens_up(top: i32, closed_h: i32, open_h: i32, area: Rect) -> bool {
    let fits_below = top + open_h <= area.bottom();
    let fits_above = top + closed_h - open_h >= area.y;
    !fits_below && (fits_above || top + closed_h - area.y > area.bottom() - top)
}

/// Where a moved folder snaps, and the folders it snaps to.
#[derive(PartialEq, Debug)]
pub struct Snap {
    pub x: i32,
    pub y: i32,
    pub with: Vec<usize>,
}

/// Snaps `r` to the nearest edge within `reach`: beside another folder with
/// `gap` between them and tops aligned, under or above one with left edges
/// aligned, or `gap` away from the screen edges. X and Y snap separately.
pub fn snap(r: Rect, others: &[Rect], area: Rect, gap: i32, reach: i32) -> Snap {
    let mut bx: (i32, i32, Option<usize>) = (reach + 1, r.x, None);
    let mut by: (i32, i32, Option<usize>) = (reach + 1, r.y, None);
    let try_x = |b: &mut (i32, i32, Option<usize>), cand: i32, who: Option<usize>| {
        let d = (cand - r.x).abs();
        if d < b.0 {
            *b = (d, cand, who);
        }
    };
    let try_y = |b: &mut (i32, i32, Option<usize>), cand: i32, who: Option<usize>| {
        let d = (cand - r.y).abs();
        if d < b.0 {
            *b = (d, cand, who);
        }
    };
    try_x(&mut bx, area.x + gap, None);
    try_x(&mut bx, area.right() - r.w - gap, None);
    try_y(&mut by, area.y + gap, None);
    try_y(&mut by, area.bottom() - r.h - gap, None);
    for (i, o) in others.iter().enumerate() {
        let same_row = r.bottom().min(o.bottom()) - r.y.max(o.y) > -reach;
        let same_column = r.right().min(o.right()) - r.x.max(o.x) > 0;
        let close_x = r.x < o.right() + gap + reach && r.right() > o.x - gap - reach;
        let close_y = r.y < o.bottom() + gap + reach && r.bottom() > o.y - gap - reach;
        if same_row && close_x {
            try_x(&mut bx, o.right() + gap, Some(i));
            try_x(&mut bx, o.x - r.w - gap, Some(i));
            try_y(&mut by, o.y, Some(i));
        }
        if same_column && close_y {
            try_y(&mut by, o.bottom() + gap, Some(i));
            try_y(&mut by, o.y - r.h - gap, Some(i));
            try_x(&mut bx, o.x, Some(i));
        }
    }
    let mut with = Vec::new();
    for (d, _, who) in [bx, by] {
        if d <= reach
            && let Some(i) = who
            && !with.contains(&i)
        {
            with.push(i);
        }
    }
    Snap { x: if bx.0 <= reach { bx.1 } else { r.x }, y: if by.0 <= reach { by.1 } else { r.y }, with }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Shape {
    /// One row, onto a new row when the screen is full.
    Row,
    /// One column, into a new column when the screen is full.
    Column,
    /// A grid of `count` columns, or of `count` rows.
    Grid,
}

/// An arrangement: its shape, and where it sits on the screen.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Spec {
    pub shape: Shape,
    /// For a grid: the number of columns, or of rows with `by_rows`.
    pub count: usize,
    /// A grid of rows is filled top to bottom, then the next column.
    pub by_rows: bool,
    /// (horizontal, vertical): 0 = left or top, 1 = center or middle, 2 = right or bottom.
    pub anchor: (usize, usize),
    /// Moved by this much from the anchor, in pixels.
    pub offset: (i32, i32),
}

/// Where each folder goes inside an arrangement, and its size.
#[derive(PartialEq, Debug)]
pub struct Slots {
    pub pos: Vec<(i32, i32)>,
    /// The height of each folder's row: a folder lower in the same column is
    /// stacked under it, even when its own label is shorter than the row.
    pub heights: Vec<i32>,
    pub w: i32,
    pub h: i32,
}

fn fraction(a: usize) -> f32 {
    [0.0, 0.5, 1.0][a.min(2)]
}

/// How many folders of these lengths fit one after the other in `room`.
fn fitting(lengths: impl Iterator<Item = i32>, room: i32, gap: i32) -> usize {
    let (mut used, mut n) = (0, 0);
    for len in lengths {
        if n > 0 && used + gap + len > room {
            break;
        }
        used += if n > 0 { gap } else { 0 } + len;
        n += 1;
    }
    n.max(1)
}

/// The slots for folders of these closed sizes, in this order. Each column is
/// as wide as its widest folder and each row as tall as its tallest one. An
/// incomplete last row (or column) follows the anchor: at the end it keeps to
/// the columns (rows) above, in the middle it is centered.
pub fn slots(sizes: &[(i32, i32)], spec: &Spec, area: Rect, gap: i32) -> Slots {
    let n = sizes.len();
    if n == 0 {
        return Slots { pos: Vec::new(), heights: Vec::new(), w: 0, h: 0 };
    }
    let (cols, rows, by_column) = match spec.shape {
        Shape::Row => {
            let cols = fitting(sizes.iter().map(|s| s.0), area.w - 2 * gap, gap);
            (cols, n.div_ceil(cols), false)
        }
        Shape::Column => {
            let rows = fitting(sizes.iter().map(|s| s.1), area.h - 2 * gap, gap);
            (n.div_ceil(rows), rows, true)
        }
        Shape::Grid if spec.by_rows => {
            let rows = spec.count.clamp(1, n);
            (n.div_ceil(rows), rows, true)
        }
        Shape::Grid => {
            let cols = spec.count.clamp(1, n);
            (cols, n.div_ceil(cols), false)
        }
    };
    let cell = |i: usize| if by_column { (i / rows, i % rows) } else { (i % cols, i / cols) };
    let mut col_w = vec![0; cols];
    let mut row_h = vec![0; rows];
    for (i, &(w, h)) in sizes.iter().enumerate() {
        let (c, r) = cell(i);
        col_w[c] = col_w[c].max(w);
        row_h[r] = row_h[r].max(h);
    }
    let starts = |lengths: &[i32]| {
        let mut acc = 0;
        lengths
            .iter()
            .map(|&l| {
                let start = acc;
                acc += l + gap;
                start
            })
            .collect::<Vec<i32>>()
    };
    let (xs, ys) = (starts(&col_w), starts(&row_h));
    let w = xs[cols - 1] + col_w[cols - 1];
    let h = ys[rows - 1] + row_h[rows - 1];
    let mut pos: Vec<(i32, i32)> = (0..n).map(|i| (xs[cell(i).0], ys[cell(i).1])).collect();
    let (fx, fy) = (fraction(spec.anchor.0), fraction(spec.anchor.1));
    if !by_column && !n.is_multiple_of(cols) && fx > 0.0 {
        let used = n % cols;
        let shift = ((w - (xs[used - 1] + col_w[used - 1])) as f32 * fx).round() as i32;
        for (k, p) in pos[n - used..].iter_mut().enumerate() {
            p.0 = if fx >= 1.0 { xs[cols - used + k] } else { p.0 + shift };
        }
    }
    if by_column && !n.is_multiple_of(rows) && fy > 0.0 {
        let used = n % rows;
        let shift = ((h - (ys[used - 1] + row_h[used - 1])) as f32 * fy).round() as i32;
        for (k, p) in pos[n - used..].iter_mut().enumerate() {
            p.1 = if fy >= 1.0 { ys[rows - used + k] } else { p.1 + shift };
        }
    }
    let heights = (0..n).map(|i| row_h[cell(i).1]).collect();
    Slots { pos, heights, w, h }
}

/// Where the anchor puts a w × h arrangement in `area`, `gap` from the edges.
fn anchored(w: i32, h: i32, anchor: (usize, usize), area: Rect, gap: i32) -> (i32, i32) {
    let along = |start: i32, len: i32, size: i32, f: f32| (start as f32 + gap as f32 + f * (len - 2 * gap - size) as f32).round() as i32;
    (along(area.x, area.w, w, fraction(anchor.0)), along(area.y, area.h, h, fraction(anchor.1)))
}

/// The top left corner of a w × h arrangement: at its anchor, plus its offset.
pub fn origin(w: i32, h: i32, spec: &Spec, area: Rect, gap: i32) -> (i32, i32) {
    let (x, y) = anchored(w, h, spec.anchor, area, gap);
    (x + spec.offset.0, y + spec.offset.1)
}

/// The nearest anchor for a w × h arrangement put at (x, y), and the offset
/// from it, so that `origin` puts it back exactly there.
pub fn anchor_of(x: i32, y: i32, w: i32, h: i32, area: Rect, gap: i32) -> ((usize, usize), (i32, i32)) {
    let nearest = |p: i32, base: &dyn Fn(usize) -> i32| {
        let k = (0..3).min_by_key(|&k| (p - base(k)).abs()).unwrap_or(0);
        (k, p - base(k))
    };
    let (kh, ox) = nearest(x, &|k| anchored(w, h, (k, 0), area, gap).0);
    let (kv, oy) = nearest(y, &|k| anchored(w, h, (0, k), area, gap).1);
    ((kh, kv), (ox, oy))
}

/// Snaps a w × h arrangement moved to (x, y) onto the nearest anchor within
/// `reach`, separately for X and Y. Also says which axes snapped.
pub fn snap_block(x: i32, y: i32, w: i32, h: i32, area: Rect, gap: i32, reach: i32) -> (i32, i32, bool, bool) {
    let ((kh, kv), (ox, oy)) = anchor_of(x, y, w, h, area, gap);
    let (bx, by) = anchored(w, h, (kh, kv), area, gap);
    let (sx, sy) = (ox.abs() <= reach, oy.abs() <= reach);
    (if sx { bx } else { x }, if sy { by } else { y }, sx, sy)
}

/// The slot whose top left corner is nearest to `at`.
pub fn nearest_slot(slots: &[(i32, i32)], at: (i32, i32)) -> usize {
    let d = |p: &(i32, i32)| (p.0 - at.0) as i64 * (p.0 - at.0) as i64 + (p.1 - at.1) as i64 * (p.1 - at.1) as i64;
    (0..slots.len()).min_by_key(|&i| d(&slots[i])).unwrap_or(0)
}

/// The first spot from the top left of `area` where a w × h folder keeps `gap`
/// away from every taken rectangle.
pub fn free_spot(taken: &[Rect], w: i32, h: i32, area: Rect, gap: i32) -> (i32, i32) {
    let step = 8;
    let mut y = area.y + gap;
    while y + h <= area.bottom() - gap {
        let mut x = area.x + gap;
        while x + w <= area.right() - gap {
            let r = Rect { x, y, w, h };
            if !taken.iter().any(|t| r.overlaps(t, gap)) {
                return (x, y);
            }
            x += step;
        }
        y += step;
    }
    (area.x + gap, area.y + gap)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every character is 10 px wide.
    fn mono(t: &str) -> i32 {
        t.chars().count() as i32 * 10
    }

    const SCREEN: Rect = Rect { x: 0, y: 0, w: 1920, h: 1040 };

    #[test]
    fn short_titles_stay_on_one_line() {
        assert_eq!(wrap("Apps", 100, 2, mono), ["Apps"]);
        assert_eq!(wrap("", 100, 2, mono), [""]);
    }

    #[test]
    fn long_titles_wrap_between_words() {
        assert_eq!(wrap("Work documents and projects", 150, 3, mono), ["Work documents", "and projects"]);
        assert_eq!(wrap("Work documents and projects", 100, 3, mono), ["Work", "documents", "and projects"]);
        // Out of lines: the last one keeps the rest, to be cut with "…".
        assert_eq!(wrap("Work documents and projects", 100, 2, mono), ["Work", "documents and projects"]);
        assert_eq!(wrap("one two", 100, 1, mono), ["one two"]);
    }

    #[test]
    fn a_word_longer_than_a_line_is_broken() {
        assert_eq!(wrap("Abcdefghijkl xy", 50, 3, mono), ["Abcde", "fghij", "kl xy"]);
    }

    #[test]
    fn folders_stacked_below_are_found_step_by_step() {
        let apps = Rect { x: 24, y: 24, w: 240, h: 36 };
        let media = Rect { x: 24, y: 68, w: 240, h: 36 };
        let tools = Rect { x: 24, y: 112, w: 240, h: 36 };
        let games = Rect { x: 272, y: 24, w: 240, h: 36 };
        let far = Rect { x: 24, y: 400, w: 240, h: 36 };
        let labels = [apps, media, tools, games, far];
        assert_eq!(stacked_below(&labels, 0, 8), [1, 2]);
        assert_eq!(stacked_below(&labels, 1, 8), [2]);
        assert!(stacked_below(&labels, 3, 8).is_empty());
    }

    #[test]
    fn folders_near_the_bottom_open_upwards() {
        assert!(!opens_up(100, 36, 300, SCREEN));
        assert!(opens_up(900, 36, 300, SCREEN));
        // Too tall for both sides: the side with more room.
        assert!(!opens_up(100, 36, 2000, SCREEN));
        assert!(opens_up(900, 36, 2000, SCREEN));
    }

    #[test]
    fn a_folder_snaps_beside_another_with_the_gap() {
        let apps = Rect { x: 24, y: 24, w: 240, h: 36 };
        let moved = Rect { x: 24 + 240 + 8 + 5, y: 27, w: 240, h: 36 };
        assert_eq!(snap(moved, &[apps], SCREEN, 8, 10), Snap { x: 272, y: 24, with: vec![0] });
    }

    #[test]
    fn a_folder_snaps_under_another_aligned_left() {
        let tools = Rect { x: 24, y: 112, w: 240, h: 36 };
        let moved = Rect { x: 30, y: 155, w: 240, h: 36 };
        assert_eq!(snap(moved, &[tools], SCREEN, 8, 10), Snap { x: 24, y: 156, with: vec![0] });
    }

    #[test]
    fn far_away_nothing_snaps_but_screen_edges_do() {
        let apps = Rect { x: 24, y: 24, w: 240, h: 36 };
        let free = Rect { x: 400, y: 300, w: 240, h: 36 };
        assert_eq!(snap(free, &[apps], SCREEN, 8, 10), Snap { x: 400, y: 300, with: vec![] });
        let corner = Rect { x: 3, y: 1000, w: 240, h: 36 };
        assert_eq!(snap(corner, &[], SCREEN, 8, 10), Snap { x: 8, y: 1040 - 36 - 8, with: vec![] });
    }

    fn grid(count: usize, by_rows: bool, anchor: (usize, usize)) -> Spec {
        Spec { shape: Shape::Grid, count, by_rows, anchor, offset: (0, 0) }
    }

    const AREA: Rect = Rect { x: 0, y: 0, w: 1000, h: 600 };

    #[test]
    fn a_grid_of_columns_fills_rows_first() {
        let s = slots(&[(100, 40); 9], &grid(3, false, (0, 0)), AREA, 8);
        assert_eq!((s.w, s.h, s.pos[1], s.pos[3]), (316, 136, (108, 0), (0, 48)));
    }

    #[test]
    fn a_grid_of_rows_fills_columns_first() {
        let s = slots(&[(100, 40); 9], &grid(2, true, (0, 0)), AREA, 8);
        assert_eq!((s.h, s.pos[1], s.pos[2]), (88, (0, 48), (108, 0)));
    }

    #[test]
    fn an_incomplete_last_row_follows_the_anchor() {
        let centered = slots(&[(100, 40); 8], &grid(3, false, (1, 0)), AREA, 8);
        assert_eq!((centered.pos[6], centered.pos[7]), ((54, 96), (162, 96)));
        let right = slots(&[(100, 40); 8], &grid(3, false, (2, 0)), AREA, 8);
        assert_eq!(right.pos[6], (108, 96));
        // Filled by columns, at the bottom: the last column keeps to the rows.
        let mut sizes = vec![(100, 40); 5];
        sizes[0].1 = 60;
        let bottom = slots(&sizes, &grid(2, true, (0, 2)), AREA, 8);
        assert_eq!(bottom.pos[4], (216, bottom.pos[1].1));
    }

    #[test]
    fn each_column_is_as_wide_as_its_widest_folder() {
        let s = slots(&[(100, 40), (150, 60), (100, 40), (100, 40)], &grid(2, false, (0, 0)), AREA, 8);
        assert_eq!((s.pos[1].0, s.w), (108, 258));
        // The first row is as tall as its tallest folder, for each of its folders.
        assert_eq!(s.heights, [60, 60, 40, 40]);
    }

    #[test]
    fn rows_and_columns_wrap_at_the_screen_edge() {
        let row = Spec { shape: Shape::Row, ..grid(1, false, (0, 0)) };
        let s = slots(&[(250, 33); 12], &row, AREA, 8);
        // 4 × 250 + 3 gaps is wider than 1000 - 2 × 8.
        assert_eq!((s.pos[2], s.pos[3]), ((516, 0), (0, 41)));
        let column = Spec { shape: Shape::Column, ..row };
        let c = slots(&[(250, 100); 6], &column, AREA, 8);
        assert_eq!((c.pos[4], c.pos[5]), ((0, 432), (258, 0)));
    }

    #[test]
    fn the_anchor_and_offset_place_the_arrangement() {
        let top_right = Spec { anchor: (2, 0), ..grid(3, false, (0, 0)) };
        assert_eq!(origin(316, 136, &top_right, AREA, 8), (676, 8));
        let centered = Spec { anchor: (1, 1), offset: (10, -5), ..top_right };
        assert_eq!(origin(316, 136, &centered, AREA, 8), (352, 227));
    }

    #[test]
    fn a_moved_arrangement_gets_the_nearest_anchor_and_keeps_its_place() {
        assert_eq!(anchor_of(680, 10, 316, 136, AREA, 8), ((2, 0), (4, 2)));
        for (x, y) in [(3, 400), (345, 230), (700, 500), (120, 15)] {
            let (anchor, offset) = anchor_of(x, y, 316, 136, AREA, 8);
            let spec = Spec { anchor, offset, ..grid(3, false, (0, 0)) };
            assert_eq!(origin(316, 136, &spec, AREA, 8), (x, y));
        }
    }

    #[test]
    fn a_moved_arrangement_snaps_to_an_anchor() {
        // 7 px from the horizontal center, far from any vertical anchor.
        assert_eq!(snap_block(349, 150, 316, 136, AREA, 8, 10), (342, 150, true, false));
        assert_eq!(snap_block(5, 590 - 136 - 3, 316, 136, AREA, 8, 10), (8, 456, true, true));
    }

    #[test]
    fn the_nearest_slot_wins() {
        assert_eq!(nearest_slot(&[(0, 0), (108, 0), (0, 48)], (100, 10)), 1);
        assert_eq!(nearest_slot(&[(0, 0), (108, 0), (0, 48)], (5, 40)), 2);
    }

    #[test]
    fn a_free_spot_keeps_the_gap_from_other_folders() {
        let taken = [Rect { x: 8, y: 8, w: 240, h: 36 }];
        let (x, y) = free_spot(&taken, 240, 36, SCREEN, 8);
        assert!(!Rect { x, y, w: 240, h: 36 }.overlaps(&taken[0], 8), "{x},{y}");
        assert_eq!(y, 8);
    }
}
