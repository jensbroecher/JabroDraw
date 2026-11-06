//! Ready-made shapes (Insert menu): outlines generated on a 200 x 200 box and turned into
//! smooth, editable Bézier paths like any pen drawing.

use crate::vector::{self, BoolOp, Contours};
use std::f64::consts::{PI, TAU};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LibShape {
    Heart,
    Arrow,
    Speech,
    Cloud,
    Lightning,
    Drop,
    Moon,
    Plus,
    Gear,
    Badge,
}

impl LibShape {
    pub const ALL: [LibShape; 10] = [
        LibShape::Heart,
        LibShape::Arrow,
        LibShape::Speech,
        LibShape::Cloud,
        LibShape::Lightning,
        LibShape::Drop,
        LibShape::Moon,
        LibShape::Plus,
        LibShape::Gear,
        LibShape::Badge,
    ];

    pub fn label(self) -> &'static str {
        match self {
            LibShape::Heart => "Heart",
            LibShape::Arrow => "Arrow",
            LibShape::Speech => "Speech bubble",
            LibShape::Cloud => "Cloud",
            LibShape::Lightning => "Lightning",
            LibShape::Drop => "Drop",
            LibShape::Moon => "Moon",
            LibShape::Plus => "Plus",
            LibShape::Gear => "Gear",
            LibShape::Badge => "Badge",
        }
    }

    /// Outline(s) on a 200 x 200 box (y down). Holes run the other way round.
    pub fn outline(self) -> Contours {
        let circle = |cx: f64, cy: f64, r: f64, n: usize| -> Vec<[f64; 2]> {
            (0..n).map(|i| {
                let a = i as f64 * TAU / n as f64;
                [cx + r * a.cos(), cy + r * a.sin()]
            })
            .collect()
        };
        match self {
            LibShape::Heart => vec![(0..96)
                .map(|i| {
                    let t = i as f64 * TAU / 96.0;
                    let x = 16.0 * t.sin().powi(3);
                    let y = 13.0 * t.cos() - 5.0 * (2.0 * t).cos() - 2.0 * (3.0 * t).cos() - (4.0 * t).cos();
                    [100.0 + x * 5.8, 92.0 - y * 5.8]
                })
                .collect()],
            LibShape::Arrow => vec![vec![
                [0.0, 70.0],
                [120.0, 70.0],
                [120.0, 20.0],
                [200.0, 100.0],
                [120.0, 180.0],
                [120.0, 130.0],
                [0.0, 130.0],
            ]],
            LibShape::Speech => {
                // rounded box with a tail at the bottom left
                let mut pts = Vec::new();
                let (x0, y0, x1, y1, r) = (0.0, 0.0, 200.0, 140.0, 30.0);
                let corner = |pts: &mut Vec<[f64; 2]>, cx: f64, cy: f64, a0: f64| {
                    for i in 0..=8 {
                        let a = a0 + i as f64 * (PI / 2.0) / 8.0;
                        pts.push([cx + r * a.cos(), cy + r * a.sin()]);
                    }
                };
                corner(&mut pts, x0 + r, y0 + r, PI);
                corner(&mut pts, x1 - r, y0 + r, 1.5 * PI);
                corner(&mut pts, x1 - r, y1 - r, 0.0);
                pts.extend([[90.0, y1], [40.0, 200.0], [55.0, y1]]);
                corner(&mut pts, x0 + r, y1 - r, 0.5 * PI);
                vec![pts]
            }
            LibShape::Cloud => {
                let puffs = [(55.0, 120.0, 45.0), (95.0, 85.0, 55.0), (145.0, 95.0, 45.0), (160.0, 135.0, 38.0), (100.0, 140.0, 45.0)];
                let inputs: Vec<Contours> = puffs.iter().map(|&(x, y, r)| vec![circle(x, y, r, 48)]).collect();
                vector::boolean(&inputs, BoolOp::Union).into_iter().flatten().collect()
            }
            LibShape::Lightning => vec![vec![[120.0, 0.0], [40.0, 115.0], [95.0, 115.0], [70.0, 200.0], [165.0, 75.0], [108.0, 75.0], [140.0, 0.0]]],
            LibShape::Drop => vec![(0..72)
                .map(|i| {
                    let t = i as f64 * TAU / 72.0;
                    // a teardrop: round bottom, pointed top
                    let x = t.sin() * (t / 2.0).sin().powf(1.2);
                    let y = -t.cos();
                    [100.0 + x * 80.0, 110.0 + y * 85.0]
                })
                .collect()],
            LibShape::Moon => {
                let full = vec![circle(100.0, 100.0, 95.0, 72)];
                let bite = vec![circle(140.0, 75.0, 82.0, 72)];
                vector::boolean(&[full, bite], BoolOp::Subtract).into_iter().flatten().collect()
            }
            LibShape::Plus => vec![vec![
                [70.0, 0.0],
                [130.0, 0.0],
                [130.0, 70.0],
                [200.0, 70.0],
                [200.0, 130.0],
                [130.0, 130.0],
                [130.0, 200.0],
                [70.0, 200.0],
                [70.0, 130.0],
                [0.0, 130.0],
                [0.0, 70.0],
                [70.0, 70.0],
            ]],
            LibShape::Gear => {
                let teeth = 10;
                let mut pts = Vec::new();
                for i in 0..teeth {
                    let a = i as f64 * TAU / teeth as f64;
                    let w = TAU / teeth as f64;
                    for (da, r) in [(0.0, 78.0), (0.12, 78.0), (0.22, 100.0), (0.5, 100.0), (0.6, 78.0)] {
                        let b = a + da * w;
                        pts.push([100.0 + r * b.cos(), 100.0 + r * b.sin()]);
                    }
                }
                let mut hole = circle(100.0, 100.0, 32.0, 40);
                hole.reverse();
                vec![pts, hole]
            }
            LibShape::Badge => vec![(0..48)
                .map(|i| {
                    let a = i as f64 * TAU / 48.0 - PI / 2.0;
                    let r = if i % 2 == 0 { 100.0 } else { 86.0 };
                    [100.0 + r * a.cos(), 100.0 + r * a.sin()]
                })
                .collect()],
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_shape_becomes_an_editable_path() {
        for s in LibShape::ALL {
            let c = s.outline();
            assert!(!c.is_empty() && c.iter().all(|r| r.len() >= 3), "{s:?} has an outline");
            let (subpaths, _, size) = vector::polygons_to_object_parts(&vec![c]).unwrap_or_else(|| panic!("{s:?}"));
            assert!(!subpaths.is_empty() && size[0] > 50.0 && size[1] > 50.0, "{s:?} {size:?}");
            let nodes: usize = subpaths.iter().map(|p| p.nodes.len()).sum();
            assert!(nodes < 200, "{s:?} stays editable ({nodes} nodes)");
        }
        // the gear and the moon keep their hole / bite
        assert_eq!(LibShape::Gear.outline().len(), 2);
    }
}
