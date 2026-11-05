//! Small geometry helpers: points, projective (perspective) transforms.

use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, PartialEq, Default, Serialize, Deserialize)]
pub struct Pt {
    pub x: f32,
    pub y: f32,
}

impl Pt {
    pub const fn new(x: f32, y: f32) -> Self {
        Self { x, y }
    }
    pub fn len(self) -> f32 {
        (self.x * self.x + self.y * self.y).sqrt()
    }
    pub fn dot(self, o: Pt) -> f32 {
        self.x * o.x + self.y * o.y
    }
    pub fn normalized(self) -> Pt {
        let l = self.len();
        if l > 1e-6 {
            self * (1.0 / l)
        } else {
            Pt::new(1.0, 0.0)
        }
    }
    /// Rotate counter-clockwise in a y-down screen space (i.e. visually clockwise for positive angles).
    pub fn rotate(self, a: f32) -> Pt {
        let (s, c) = a.sin_cos();
        Pt::new(self.x * c - self.y * s, self.x * s + self.y * c)
    }
    pub fn lerp(self, o: Pt, t: f32) -> Pt {
        self + (o - self) * t
    }
    pub fn angle(self) -> f32 {
        self.y.atan2(self.x)
    }
    pub fn dist(self, o: Pt) -> f32 {
        (self - o).len()
    }
}

impl std::ops::Add for Pt {
    type Output = Pt;
    fn add(self, o: Pt) -> Pt {
        Pt::new(self.x + o.x, self.y + o.y)
    }
}
impl std::ops::Sub for Pt {
    type Output = Pt;
    fn sub(self, o: Pt) -> Pt {
        Pt::new(self.x - o.x, self.y - o.y)
    }
}
impl std::ops::Mul<f32> for Pt {
    type Output = Pt;
    fn mul(self, k: f32) -> Pt {
        Pt::new(self.x * k, self.y * k)
    }
}

/// 3x3 projective transform, row major: x' = (m0 x + m1 y + m2) / (m6 x + m7 y + m8).
#[derive(Clone, Copy, Debug)]
pub struct Homography {
    pub m: [f64; 9],
}

impl Homography {
    /// Maps the local rectangle (0,0)-(w,h) onto the quad (TL, TR, BR, BL).
    pub fn from_quad(q: &[Pt; 4], w: f32, h: f32) -> Self {
        let (x0, y0) = (q[0].x as f64, q[0].y as f64);
        let (x1, y1) = (q[1].x as f64, q[1].y as f64);
        let (x2, y2) = (q[2].x as f64, q[2].y as f64);
        let (x3, y3) = (q[3].x as f64, q[3].y as f64);
        let dx1 = x1 - x2;
        let dx2 = x3 - x2;
        let dx3 = x0 - x1 + x2 - x3;
        let dy1 = y1 - y2;
        let dy2 = y3 - y2;
        let dy3 = y0 - y1 + y2 - y3;
        let (g, hh) = if dx3.abs() < 1e-9 && dy3.abs() < 1e-9 {
            (0.0, 0.0)
        } else {
            let det = dx1 * dy2 - dx2 * dy1;
            if det.abs() < 1e-12 {
                (0.0, 0.0)
            } else {
                ((dx3 * dy2 - dx2 * dy3) / det, (dx1 * dy3 - dx3 * dy1) / det)
            }
        };
        let a = x1 - x0 + g * x1;
        let b = x3 - x0 + hh * x3;
        let c = x0;
        let d = y1 - y0 + g * y1;
        let e = y3 - y0 + hh * y3;
        let f = y0;
        let sw = 1.0 / (w.max(1e-3) as f64);
        let sh = 1.0 / (h.max(1e-3) as f64);
        Self {
            m: [a * sw, b * sh, c, d * sw, e * sh, f, g * sw, hh * sh, 1.0],
        }
    }

    pub fn apply(&self, x: f64, y: f64) -> Option<(f64, f64)> {
        let m = &self.m;
        let wz = m[6] * x + m[7] * y + m[8];
        if wz.abs() < 1e-12 {
            return None;
        }
        Some(((m[0] * x + m[1] * y + m[2]) / wz, (m[3] * x + m[4] * y + m[5]) / wz))
    }

    pub fn apply_pt(&self, p: Pt) -> Option<Pt> {
        self.apply(p.x as f64, p.y as f64)
            .map(|(x, y)| Pt::new(x as f32, y as f32))
    }

    pub fn inverse(&self) -> Option<Self> {
        let m = &self.m;
        let a = m[4] * m[8] - m[5] * m[7];
        let b = m[5] * m[6] - m[3] * m[8];
        let c = m[3] * m[7] - m[4] * m[6];
        let det = m[0] * a + m[1] * b + m[2] * c;
        if det.abs() < 1e-15 {
            return None;
        }
        let inv = 1.0 / det;
        Some(Self {
            m: [
                a * inv,
                (m[2] * m[7] - m[1] * m[8]) * inv,
                (m[1] * m[5] - m[2] * m[4]) * inv,
                b * inv,
                (m[0] * m[8] - m[2] * m[6]) * inv,
                (m[2] * m[3] - m[0] * m[5]) * inv,
                c * inv,
                (m[1] * m[6] - m[0] * m[7]) * inv,
                (m[0] * m[4] - m[1] * m[3]) * inv,
            ],
        })
    }
}

/// Even-odd point in polygon test.
pub fn point_in_poly(p: Pt, poly: &[Pt]) -> bool {
    let mut inside = false;
    let n = poly.len();
    let mut j = n - 1;
    for i in 0..n {
        let (a, b) = (poly[i], poly[j]);
        if (a.y > p.y) != (b.y > p.y) {
            let x = a.x + (p.y - a.y) / (b.y - a.y) * (b.x - a.x);
            if p.x < x {
                inside = !inside;
            }
        }
        j = i;
    }
    inside
}

/// Ramer-Douglas-Peucker polyline simplification.
pub fn simplify(points: &[Pt], eps: f32) -> Vec<Pt> {
    if points.len() < 3 {
        return points.to_vec();
    }
    let mut keep = vec![false; points.len()];
    keep[0] = true;
    keep[points.len() - 1] = true;
    let mut stack = vec![(0usize, points.len() - 1)];
    while let Some((s, e)) = stack.pop() {
        let (a, b) = (points[s], points[e]);
        let ab = b - a;
        let len = ab.len().max(1e-6);
        let mut best = 0.0;
        let mut idx = 0;
        for (i, &p) in points.iter().enumerate().take(e).skip(s + 1) {
            let d = ((p - a).x * ab.y - (p - a).y * ab.x).abs() / len;
            if d > best {
                best = d;
                idx = i;
            }
        }
        if best > eps {
            keep[idx] = true;
            stack.push((s, idx));
            stack.push((idx, e));
        }
    }
    points
        .iter()
        .zip(keep)
        .filter(|(_, k)| *k)
        .map(|(p, _)| *p)
        .collect()
}
