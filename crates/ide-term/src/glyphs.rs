//! Box-drawing lines and block elements drawn as rectangles: font glyphs leave seams between
//! cells when the row is taller than the glyph.

/// Built-in geometry for box-drawing lines and block elements (fonts leave gaps between cells
/// when the row height is larger than the glyph). Returns rects in cell fractions + alpha.
/// (x, y, width, height, alpha) in fractions of the cell.
pub(crate) type GlyphRect = (f32, f32, f32, f32, f32);

pub(crate) fn builtin_glyph(c: char, cw: f32, ch: f32) -> Option<Vec<GlyphRect>> {
    let cu = c as u32;
    let mut v = Vec::new();
    // (left, right, up, down) weights: 1 light, 2 heavy
    let lines: Option<(u8, u8, u8, u8)> = match c {
        '─' => Some((1, 1, 0, 0)),
        '━' => Some((2, 2, 0, 0)),
        '│' => Some((0, 0, 1, 1)),
        '┃' => Some((0, 0, 2, 2)),
        '┌' | '╭' => Some((0, 1, 0, 1)),
        '┐' | '╮' => Some((1, 0, 0, 1)),
        '└' | '╰' => Some((0, 1, 1, 0)),
        '┘' | '╯' => Some((1, 0, 1, 0)),
        '├' => Some((0, 1, 1, 1)),
        '┤' => Some((1, 0, 1, 1)),
        '┬' => Some((1, 1, 0, 1)),
        '┴' => Some((1, 1, 1, 0)),
        '┼' => Some((1, 1, 1, 1)),
        '┏' => Some((0, 2, 0, 2)),
        '┓' => Some((2, 0, 0, 2)),
        '┗' => Some((0, 2, 2, 0)),
        '┛' => Some((2, 0, 2, 0)),
        '┣' => Some((0, 2, 2, 2)),
        '┫' => Some((2, 0, 2, 2)),
        '┳' => Some((2, 2, 0, 2)),
        '┻' => Some((2, 2, 2, 0)),
        '╋' => Some((2, 2, 2, 2)),
        _ => None,
    };
    if let Some((l, r, u, d)) = lines {
        // fractions of the cell; thickness in logical px converted to fractions
        let t = |w: u8| if w == 2 { 2.0 } else { 1.0 };
        let (cx, cy) = (0.5, 0.5);
        let maxw = t(l.max(r).max(u).max(d));
        let hx = maxw / 2.0 / cw;
        let hy = maxw / 2.0 / ch;
        if l > 0 { let th = t(l) / ch; v.push((0.0, cy - th / 2.0, cx + hx, th, 1.0)); }
        if r > 0 { let th = t(r) / ch; v.push((cx - hx, cy - th / 2.0, 1.0 - (cx - hx), th, 1.0)); }
        if u > 0 { let tw = t(u) / cw; v.push((cx - tw / 2.0, 0.0, tw, cy + hy, 1.0)); }
        if d > 0 { let tw = t(d) / cw; v.push((cx - tw / 2.0, cy - hy, tw, 1.0 - (cy - hy), 1.0)); }
        return Some(v);
    }
    match cu {
        0x2588 => v.push((0.0, 0.0, 1.0, 1.0, 1.0)),
        0x2580 => v.push((0.0, 0.0, 1.0, 0.5, 1.0)),
        0x2581..=0x2587 => {
            let f = (cu - 0x2580) as f32 / 8.0;
            v.push((0.0, 1.0 - f, 1.0, f, 1.0));
        }
        0x2589..=0x258F => {
            let f = (0x2590 - cu) as f32 / 8.0;
            v.push((0.0, 0.0, f, 1.0, 1.0));
        }
        0x2590 => v.push((0.5, 0.0, 0.5, 1.0, 1.0)),
        0x2591 => v.push((0.0, 0.0, 1.0, 1.0, 0.25)),
        0x2592 => v.push((0.0, 0.0, 1.0, 1.0, 0.5)),
        0x2593 => v.push((0.0, 0.0, 1.0, 1.0, 0.75)),
        0x2594 => v.push((0.0, 0.0, 1.0, 0.125, 1.0)),
        0x2595 => v.push((0.875, 0.0, 0.125, 1.0, 1.0)),
        0x2596..=0x259F => {
            // quadrants: UL, UR, LL, LR
            let q: [bool; 4] = match cu {
                0x2596 => [false, false, true, false],
                0x2597 => [false, false, false, true],
                0x2598 => [true, false, false, false],
                0x2599 => [true, false, true, true],
                0x259A => [true, false, false, true],
                0x259B => [true, true, true, false],
                0x259C => [true, true, false, true],
                0x259D => [false, true, false, false],
                0x259E => [false, true, true, false],
                _ => [false, true, true, true],
            };
            let pos = [(0.0, 0.0), (0.5, 0.0), (0.0, 0.5), (0.5, 0.5)];
            for i in 0..4 {
                if q[i] {
                    v.push((pos[i].0, pos[i].1, 0.5, 0.5, 1.0));
                }
            }
        }
        _ => return None,
    }
    Some(v)
}
