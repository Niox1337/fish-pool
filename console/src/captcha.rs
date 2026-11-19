//! Image captcha rendered as SVG. Glyphs are stroke paths built from a vector stroke font,
//! then rotated, skewed, warped and jittered, mixed with crossing noise curves of the same
//! stroke style and shuffled in document order. No text elements are emitted.

use std::fmt::Write;

pub const WIDTH: f64 = 160.0;
pub const HEIGHT: f64 = 56.0;
/// Characters that remain distinguishable under distortion.
pub const CHARSET: &str = "ACDEFGHJKLMNPRTUVWXY3456789";

/// SplitMix64 pseudo random generator seeded from the CSPRNG.
pub struct Rng(u64);

impl Rng {
    pub fn new(seed: u64) -> Self {
        Self(seed)
    }
    pub fn next_u64(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }
    /// Uniform float in [0, 1).
    pub fn unit(&mut self) -> f64 {
        (self.next_u64() >> 11) as f64 / (1u64 << 53) as f64
    }
    pub fn range(&mut self, lo: f64, hi: f64) -> f64 {
        lo + (hi - lo) * self.unit()
    }
    pub fn below(&mut self, n: usize) -> usize {
        (self.next_u64() % n as u64) as usize
    }
}

type Stroke = &'static [(f64, f64)];

/// Stroke font on a 4 by 6 grid, y pointing down.
fn glyph(c: char) -> &'static [Stroke] {
    match c {
        'A' => &[&[(0.0, 6.0), (2.0, 0.0), (4.0, 6.0)], &[(0.9, 3.6), (3.1, 3.6)]],
        'C' => &[&[(4.0, 1.0), (3.0, 0.0), (1.0, 0.0), (0.0, 1.5), (0.0, 4.5), (1.0, 6.0), (3.0, 6.0), (4.0, 5.0)]],
        'D' => &[&[(0.0, 0.0), (0.0, 6.0), (2.5, 6.0), (4.0, 4.5), (4.0, 1.5), (2.5, 0.0), (0.0, 0.0)]],
        'E' => &[&[(4.0, 0.0), (0.0, 0.0), (0.0, 6.0), (4.0, 6.0)], &[(0.0, 3.0), (3.0, 3.0)]],
        'F' => &[&[(4.0, 0.0), (0.0, 0.0), (0.0, 6.0)], &[(0.0, 3.0), (3.0, 3.0)]],
        'G' => &[&[(4.0, 1.0), (3.0, 0.0), (1.0, 0.0), (0.0, 1.5), (0.0, 4.5), (1.0, 6.0), (3.0, 6.0), (4.0, 5.0), (4.0, 3.5), (2.2, 3.5)]],
        'H' => &[&[(0.0, 0.0), (0.0, 6.0)], &[(4.0, 0.0), (4.0, 6.0)], &[(0.0, 3.0), (4.0, 3.0)]],
        'J' => &[&[(4.0, 0.0), (4.0, 4.5), (3.0, 6.0), (1.0, 6.0), (0.0, 4.5)]],
        'K' => &[&[(0.0, 0.0), (0.0, 6.0)], &[(4.0, 0.0), (0.0, 3.6)], &[(1.4, 2.6), (4.0, 6.0)]],
        'L' => &[&[(0.0, 0.0), (0.0, 6.0), (4.0, 6.0)]],
        'M' => &[&[(0.0, 6.0), (0.0, 0.0), (2.0, 3.5), (4.0, 0.0), (4.0, 6.0)]],
        'N' => &[&[(0.0, 6.0), (0.0, 0.0), (4.0, 6.0), (4.0, 0.0)]],
        'P' => &[&[(0.0, 6.0), (0.0, 0.0), (3.0, 0.0), (4.0, 1.0), (4.0, 2.5), (3.0, 3.5), (0.0, 3.5)]],
        'R' => &[&[(0.0, 6.0), (0.0, 0.0), (3.0, 0.0), (4.0, 1.0), (4.0, 2.5), (3.0, 3.5), (0.0, 3.5)], &[(2.0, 3.5), (4.0, 6.0)]],
        'T' => &[&[(0.0, 0.0), (4.0, 0.0)], &[(2.0, 0.0), (2.0, 6.0)]],
        'U' => &[&[(0.0, 0.0), (0.0, 4.5), (1.0, 6.0), (3.0, 6.0), (4.0, 4.5), (4.0, 0.0)]],
        'V' => &[&[(0.0, 0.0), (2.0, 6.0), (4.0, 0.0)]],
        'W' => &[&[(0.0, 0.0), (1.0, 6.0), (2.0, 2.5), (3.0, 6.0), (4.0, 0.0)]],
        'X' => &[&[(0.0, 0.0), (4.0, 6.0)], &[(4.0, 0.0), (0.0, 6.0)]],
        'Y' => &[&[(0.0, 0.0), (2.0, 3.0)], &[(4.0, 0.0), (2.0, 3.0), (2.0, 6.0)]],
        '3' => &[
            &[(0.0, 0.5), (1.0, 0.0), (3.0, 0.0), (4.0, 1.0), (4.0, 2.0), (3.0, 3.0), (1.5, 3.0)],
            &[(3.0, 3.0), (4.0, 4.0), (4.0, 5.0), (3.0, 6.0), (1.0, 6.0), (0.0, 5.5)],
        ],
        '4' => &[&[(3.0, 6.0), (3.0, 0.0), (0.0, 4.0), (4.0, 4.0)]],
        '5' => &[&[(4.0, 0.0), (0.0, 0.0), (0.0, 3.0), (3.0, 2.6), (4.0, 3.6), (4.0, 5.0), (3.0, 6.0), (0.0, 6.0)]],
        '6' => &[&[(3.5, 0.0), (1.0, 0.0), (0.0, 1.5), (0.0, 5.0), (1.0, 6.0), (3.0, 6.0), (4.0, 5.0), (4.0, 4.0), (3.0, 3.0), (0.0, 3.2)]],
        '7' => &[&[(0.0, 0.0), (4.0, 0.0), (1.5, 6.0)]],
        '8' => &[&[
            (1.0, 3.0), (0.0, 2.0), (0.0, 1.0), (1.0, 0.0), (3.0, 0.0), (4.0, 1.0), (4.0, 2.0), (3.0, 3.0), (1.0, 3.0),
            (0.0, 4.0), (0.0, 5.0), (1.0, 6.0), (3.0, 6.0), (4.0, 5.0), (4.0, 4.0), (3.0, 3.0),
        ]],
        '9' => &[&[(4.0, 2.8), (1.0, 3.0), (0.0, 2.0), (0.0, 1.0), (1.0, 0.0), (3.0, 0.0), (4.0, 1.0), (4.0, 4.5), (3.0, 6.0), (0.5, 6.0)]],
        _ => &[],
    }
}

fn charset() -> Vec<char> {
    CHARSET.chars().filter(|c| !c.is_whitespace()).collect()
}

/// Picks 4 or 5 characters from the charset.
pub fn random_answer(rng: &mut Rng) -> String {
    let set = charset();
    let len = 4 + rng.below(2);
    (0..len).map(|_| set[rng.below(set.len())]).collect()
}

/// Normalizes user input for comparison: trimmed, uppercase, spaces removed.
pub fn normalize_answer(s: &str) -> String {
    s.chars().filter(|c| !c.is_whitespace()).collect::<String>().to_uppercase()
}

struct Warp {
    amp: f64,
    freq: f64,
    phase: f64,
    amp_x: f64,
    freq_x: f64,
    phase_x: f64,
}

impl Warp {
    fn apply(&self, x: f64, y: f64) -> (f64, f64) {
        (
            x + self.amp_x * (y * self.freq_x + self.phase_x).sin(),
            y + self.amp * (x * self.freq + self.phase).sin(),
        )
    }
}

/// Splits each segment into short pieces so the warp bends straight strokes.
fn densify(points: &[(f64, f64)], step: f64) -> Vec<(f64, f64)> {
    let mut out = Vec::new();
    for w in points.windows(2) {
        let (a, b) = (w[0], w[1]);
        let len = ((b.0 - a.0).powi(2) + (b.1 - a.1).powi(2)).sqrt();
        let n = (len / step).ceil().max(1.0) as usize;
        for i in 0..n {
            let t = i as f64 / n as f64;
            out.push((a.0 + (b.0 - a.0) * t, a.1 + (b.1 - a.1) * t));
        }
    }
    if let Some(last) = points.last() {
        out.push(*last);
    }
    out
}

fn path_d(points: &[(f64, f64)]) -> String {
    let mut d = String::new();
    for (i, (x, y)) in points.iter().enumerate() {
        let _ = write!(d, "{}{:.1} {:.1}", if i == 0 { "M" } else { "L" }, x, y);
    }
    d
}

fn ink(rng: &mut Rng) -> String {
    let h = rng.range(0.0, 360.0);
    format!("hsl({:.0},{:.0}%,{:.0}%)", h, rng.range(35.0, 65.0), rng.range(22.0, 38.0))
}

/// Renders `answer` as an SVG document.
pub fn render_svg(answer: &str, rng: &mut Rng) -> String {
    let warp = Warp {
        amp: rng.range(2.0, 4.0),
        freq: rng.range(0.04, 0.09),
        phase: rng.range(0.0, 6.3),
        amp_x: rng.range(1.0, 2.5),
        freq_x: rng.range(0.08, 0.16),
        phase_x: rng.range(0.0, 6.3),
    };
    let mut elems: Vec<String> = Vec::new();
    let chars: Vec<char> = answer.chars().collect();
    let slot = (WIDTH - 24.0) / chars.len().max(1) as f64;

    for (i, c) in chars.iter().enumerate() {
        let scale = rng.range(5.2, 6.6);
        let rot = rng.range(-0.38, 0.38);
        let skew = rng.range(-0.3, 0.3);
        let cx = 12.0 + slot * (i as f64 + 0.5) + rng.range(-3.0, 3.0);
        let cy = HEIGHT / 2.0 + rng.range(-4.0, 4.0);
        let (sin, cos) = rot.sin_cos();
        let color = ink(rng);
        let width = rng.range(2.0, 3.0);
        for stroke in glyph(*c) {
            let pts: Vec<(f64, f64)> = densify(stroke, 0.5)
                .into_iter()
                .map(|(gx, gy)| {
                    let x = (gx - 2.0 + skew * (gy - 3.0)) * scale;
                    let y = (gy - 3.0) * scale;
                    let (rx, ry) = (x * cos - y * sin + cx, x * sin + y * cos + cy);
                    let (wx, wy) = warp.apply(rx, ry);
                    (wx + rng.range(-0.6, 0.6), wy + rng.range(-0.6, 0.6))
                })
                .collect();
            elems.push(format!(
                r#"<path d="{}" fill="none" stroke="{}" stroke-width="{:.1}" stroke-linecap="round" stroke-linejoin="round"/>"#,
                path_d(&pts),
                color,
                width
            ));
        }
    }

    // Crossing curves with the same stroke style as glyphs.
    for _ in 0..(3 + rng.below(3)) {
        let pts: Vec<(f64, f64)> = (0..4)
            .map(|k| (k as f64 / 3.0 * (WIDTH + 20.0) - 10.0 + rng.range(-8.0, 8.0), rng.range(6.0, HEIGHT - 6.0)))
            .collect();
        elems.push(format!(
            r#"<path d="M{:.1} {:.1}C{:.1} {:.1} {:.1} {:.1} {:.1} {:.1}" fill="none" stroke="{}" stroke-width="{:.1}" stroke-linecap="round"/>"#,
            pts[0].0, pts[0].1, pts[1].0, pts[1].1, pts[2].0, pts[2].1, pts[3].0, pts[3].1,
            ink(rng),
            rng.range(1.2, 2.6)
        ));
    }

    // Decoy fragments taken from random glyphs.
    let set = charset();
    for _ in 0..(3 + rng.below(3)) {
        let g = glyph(set[rng.below(set.len())]);
        if g.is_empty() {
            continue;
        }
        let stroke = g[rng.below(g.len())];
        let scale = rng.range(2.0, 4.0);
        let (ox, oy) = (rng.range(0.0, WIDTH), rng.range(0.0, HEIGHT));
        let rot = rng.range(0.0, 6.3);
        let (sin, cos) = rot.sin_cos();
        let pts: Vec<(f64, f64)> = densify(stroke, 1.0)
            .into_iter()
            .map(|(gx, gy)| {
                let (x, y) = ((gx - 2.0) * scale, (gy - 3.0) * scale);
                warp.apply(x * cos - y * sin + ox, x * sin + y * cos + oy)
            })
            .collect();
        elems.push(format!(
            r#"<path d="{}" fill="none" stroke="{}" stroke-width="{:.1}" stroke-linecap="round" opacity="{:.2}"/>"#,
            path_d(&pts),
            ink(rng),
            rng.range(1.0, 2.0),
            rng.range(0.5, 0.9)
        ));
    }

    for _ in 0..(40 + rng.below(30)) {
        elems.push(format!(
            r#"<circle cx="{:.1}" cy="{:.1}" r="{:.1}" fill="{}" opacity="{:.2}"/>"#,
            rng.range(0.0, WIDTH),
            rng.range(0.0, HEIGHT),
            rng.range(0.4, 1.6),
            ink(rng),
            rng.range(0.3, 0.8)
        ));
    }

    // Fisher-Yates shuffle so document order says nothing about glyph order.
    for i in (1..elems.len()).rev() {
        let j = rng.below(i + 1);
        elems.swap(i, j);
    }

    let bg = format!("hsl({:.0},40%,{:.0}%)", rng.range(0.0, 360.0), rng.range(92.0, 97.0));
    let mut svg = format!(
        r#"<svg xmlns="http://www.w3.org/2000/svg" width="{w}" height="{h}" viewBox="0 0 {w} {h}" role="img" aria-label="captcha"><rect width="100%" height="100%" fill="{bg}"/>"#,
        w = WIDTH,
        h = HEIGHT
    );
    for e in elems {
        svg.push_str(&e);
    }
    svg.push_str("</svg>");
    svg
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_charset_char_has_a_glyph() {
        for c in charset() {
            assert!(!glyph(c).is_empty(), "missing glyph {c}");
            for s in glyph(c) {
                assert!(s.len() >= 2);
                assert!(s.iter().all(|(x, y)| (0.0..=4.0).contains(x) && (0.0..=6.0).contains(y)));
            }
        }
    }

    #[test]
    fn answers_have_valid_length_and_chars() {
        let mut rng = Rng::new(42);
        let set = charset();
        for _ in 0..200 {
            let a = random_answer(&mut rng);
            assert!((4..=5).contains(&a.chars().count()));
            assert!(a.chars().all(|c| set.contains(&c)));
        }
    }

    #[test]
    fn svg_has_no_text_and_hides_answer() {
        let mut rng = Rng::new(7);
        let answer = "KX7AM";
        let svg = render_svg(answer, &mut rng);
        assert!(svg.starts_with("<svg"));
        assert!(svg.ends_with("</svg>"));
        assert!(!svg.contains("<text"));
        assert!(!svg.contains(answer));
        let paths = svg.matches("<path").count();
        let glyph_strokes: usize = answer.chars().map(|c| glyph(c).len()).sum();
        assert!(paths >= glyph_strokes + 6, "glyph and noise paths expected, got {paths}");
        assert!(svg.matches("<circle").count() >= 40);
    }

    #[test]
    fn rendering_is_randomized() {
        let a = render_svg("ACDE", &mut Rng::new(1));
        let b = render_svg("ACDE", &mut Rng::new(2));
        assert_ne!(a, b);
        assert_eq!(a, render_svg("ACDE", &mut Rng::new(1)));
    }

    #[test]
    fn normalizes_input() {
        assert_eq!(normalize_answer(" kx 7am "), "KX7AM");
    }
}
