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

#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Arrange {
    /// Side by side from the top left, onto a new row when the screen is full.
    Row,
    /// One under the other from the top left, into a new column when full.
    Column,
    /// Rows of equal columns, as wide as the widest folder.
    Grid,
}

/// Positions for folders of these closed sizes, in this order, inside `area`.
pub fn arrange(sizes: &[(i32, i32)], area: Rect, gap: i32, how: Arrange) -> Vec<(i32, i32)> {
    let (left, top) = (area.x + gap, area.y + gap);
    let mut out = Vec::with_capacity(sizes.len());
    match how {
        Arrange::Row => {
            let (mut x, mut y, mut row_h) = (left, top, 0);
            for &(w, h) in sizes {
                if x > left && x + w > area.right() - gap {
                    x = left;
                    y += row_h + gap;
                    row_h = 0;
                }
                out.push((x, y));
                x += w + gap;
                row_h = row_h.max(h);
            }
        }
        Arrange::Column => {
            let (mut x, mut y, mut col_w) = (left, top, 0);
            for &(w, h) in sizes {
                if y > top && y + h > area.bottom() - gap {
                    y = top;
                    x += col_w + gap;
                    col_w = 0;
                }
                out.push((x, y));
                y += h + gap;
                col_w = col_w.max(w);
            }
        }
        Arrange::Grid => {
            let cell_w = sizes.iter().map(|s| s.0).max().unwrap_or(0);
            let per_row = ((area.w - gap) / (cell_w + gap).max(1)).max(1) as usize;
            let (mut y, mut row_h) = (top, 0);
            for (i, &(_, h)) in sizes.iter().enumerate() {
                if i > 0 && i % per_row == 0 {
                    y += row_h + gap;
                    row_h = 0;
                }
                out.push((left + (i % per_row) as i32 * (cell_w + gap), y));
                row_h = row_h.max(h);
            }
        }
    }
    out
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

    #[test]
    fn arranging_in_a_row_wraps_at_the_screen_edge() {
        let sizes = [(800, 36), (800, 50), (800, 36)];
        assert_eq!(arrange(&sizes, SCREEN, 8, Arrange::Row), [(8, 8), (816, 8), (8, 66)]);
    }

    #[test]
    fn arranging_in_a_column_stacks_with_the_gap() {
        let sizes = [(240, 36), (200, 52), (240, 36)];
        assert_eq!(arrange(&sizes, SCREEN, 8, Arrange::Column), [(8, 8), (8, 52), (8, 112)]);
        let small = Rect { x: 0, y: 0, w: 1920, h: 120 };
        assert_eq!(arrange(&sizes, small, 8, Arrange::Column), [(8, 8), (8, 52), (256, 8)]);
    }

    #[test]
    fn arranging_as_a_grid_uses_equal_columns() {
        let sizes = [(240, 36), (200, 52), (240, 36)];
        let narrow = Rect { x: 0, y: 0, w: 600, h: 1040 };
        assert_eq!(arrange(&sizes, narrow, 8, Arrange::Grid), [(8, 8), (256, 8), (8, 68)]);
    }

    #[test]
    fn a_free_spot_keeps_the_gap_from_other_folders() {
        let taken = [Rect { x: 8, y: 8, w: 240, h: 36 }];
        let (x, y) = free_spot(&taken, 240, 36, SCREEN, 8);
        assert!(!Rect { x, y, w: 240, h: 36 }.overlaps(&taken[0], 8), "{x},{y}");
        assert_eq!(y, 8);
    }
}
