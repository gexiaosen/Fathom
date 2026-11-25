//! Procedural furniture built from real solids (boxes, cylinders, lathes,
//! ellipsoids, tori) with per-part surface finish, mirroring the web build.

use std::f32::consts::{PI, TAU};

use fp_model::Furniture;
use fp_model::catalog::{CATALOG, hex_color};

use crate::mesh::{FURNITURE_FALLBACK, MeshParts, MeshVertex};

pub fn rgb(hex: &str) -> [f32; 3] {
    // Hex colours are authored in sRGB; the shader works in linear light.
    hex_color(hex).map(|c| {
        if c <= 0.04045 {
            c / 12.92
        } else {
            ((c + 0.055) / 1.055).powf(2.4)
        }
    })
}

pub fn dark(c: [f32; 3], k: f32) -> [f32; 3] {
    [c[0] * k, c[1] * k, c[2] * k]
}

pub fn light(c: [f32; 3], k: f32) -> [f32; 3] {
    [
        c[0] + (1.0 - c[0]) * k,
        c[1] + (1.0 - c[1]) * k,
        c[2] + (1.0 - c[2]) * k,
    ]
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Finish {
    pub rough: f32,
    pub metal: f32,
    pub glow: f32,
    /// Surface family carried to the shader through `MeshVertex::pattern`:
    /// 0 plain, 10 fabric, 11 wood, 12 brushed metal, 13 glazed ceramic,
    /// 14 mirror glass.
    pub pattern: f32,
}

impl Finish {
    pub const fn new(rough: f32, metal: f32, glow: f32) -> Self {
        Self {
            rough,
            metal,
            glow,
            pattern: 0.0,
        }
    }
    pub const fn patterned(mut self, pattern: f32) -> Self {
        self.pattern = pattern;
        self
    }
    pub const fn fabric() -> Self {
        Self::new(0.96, 0.0, 0.0).patterned(10.0)
    }
    pub const fn wood() -> Self {
        Self::new(0.55, 0.0, 0.0).patterned(11.0)
    }
    pub const fn matte() -> Self {
        Self::new(0.55, 0.0, 0.0)
    }
    pub const fn ceramic() -> Self {
        Self::new(0.12, 0.0, 0.0).patterned(13.0)
    }
    pub const fn metal() -> Self {
        Self::new(0.25, 0.9, 0.0).patterned(12.0)
    }
    pub const fn chrome() -> Self {
        Self::new(0.08, 1.0, 0.0).patterned(12.0)
    }
    pub const fn black_metal() -> Self {
        Self::new(0.4, 0.6, 0.0).patterned(12.0)
    }
    pub const fn mirror() -> Self {
        Self::new(0.06, 0.55, 0.0).patterned(14.0)
    }
    pub const fn screen(glow: f32) -> Self {
        Self::new(0.1, 0.3, glow)
    }
    pub const fn lamp() -> Self {
        Self::new(0.9, 0.0, 0.55)
    }
    pub const fn glow(k: f32) -> Self {
        Self::new(0.9, 0.0, k)
    }
    pub fn to_array(self) -> [f32; 3] {
        [self.rough, self.metal, self.glow]
    }
}

#[derive(Clone, Copy)]
pub struct Style {
    pub color: [f32; 3],
    pub fin: Finish,
    pub alpha: f32,
    /// Non-zero marks an optical surface: value is physical thickness in
    /// metres, carried to the shader through `MeshVertex::pattern`.
    pub glass: f32,
}

pub fn st(color: [f32; 3], fin: Finish) -> Style {
    Style {
        color,
        fin,
        alpha: 1.0,
        glass: 0.0,
    }
}

/// Glass, water and other see-through surfaces.
pub fn sg(color: [f32; 3], fin: Finish, alpha: f32) -> Style {
    Style {
        color,
        fin,
        alpha,
        glass: 0.012,
    }
}

pub struct Frame {
    sin: f32,
    cos: f32,
    ox: f32,
    oz: f32,
}

impl Frame {
    pub fn new(rotation_deg: f64, center: [f32; 2]) -> Self {
        let (sin, cos) = (rotation_deg as f32).to_radians().sin_cos();
        Self {
            sin,
            cos,
            ox: center[0],
            oz: center[1],
        }
    }

    pub fn map(&self, p: [f32; 3]) -> [f32; 3] {
        [
            self.ox + p[0] * self.cos - p[2] * self.sin,
            p[1],
            self.oz + p[0] * self.sin + p[2] * self.cos,
        ]
    }

    pub fn rot(&self, v: [f32; 3]) -> [f32; 3] {
        [
            v[0] * self.cos - v[2] * self.sin,
            v[1],
            v[0] * self.sin + v[2] * self.cos,
        ]
    }

    fn vert(&self, p: [f32; 3], n: [f32; 3], s: Style) -> MeshVertex {
        let mut v =
            MeshVertex::with_alpha(self.map(p), self.rot(n), s.color, s.fin.to_array(), s.alpha);
        if s.glass > 0.0 {
            v.pattern = 8.0 + s.glass;
        } else {
            v.pattern = s.fin.pattern;
        }
        v
    }

    #[allow(clippy::too_many_arguments)]
    pub fn quad(
        &self,
        out: &mut Vec<MeshVertex>,
        a: [f32; 3],
        b: [f32; 3],
        c: [f32; 3],
        d: [f32; 3],
        n: [f32; 3],
        s: Style,
    ) {
        for p in [a, b, c, a, c, d] {
            out.push(self.vert(p, n, s));
        }
    }

    #[allow(clippy::too_many_arguments)]
    pub fn quad_n(
        &self,
        out: &mut Vec<MeshVertex>,
        a: [f32; 3],
        b: [f32; 3],
        c: [f32; 3],
        d: [f32; 3],
        na: [f32; 3],
        nb: [f32; 3],
        nc: [f32; 3],
        nd: [f32; 3],
        s: Style,
    ) {
        for (p, n) in [(a, na), (b, nb), (c, nc), (a, na), (c, nc), (d, nd)] {
            out.push(self.vert(p, n, s));
        }
    }
}

// ------------------------------------------------------------------- solids

pub fn bx(out: &mut Vec<MeshVertex>, f: &Frame, lo: [f32; 3], hi: [f32; 3], s: Style) {
    let size = [hi[0] - lo[0], hi[1] - lo[1], hi[2] - lo[2]];
    let r = 0.022f32
        .min(size[0] * 0.28)
        .min(size[1] * 0.28)
        .min(size[2] * 0.28);
    // Tiny parts (handles, slats, paper-thin fronts) stay sharp: a bevel there
    // would only add vertices without reading as a rounded edge.
    if r < 0.008 || size.iter().any(|d| *d < 0.025) {
        sharp_bx(out, f, lo, hi, s);
        return;
    }
    let [x0, y0, z0] = lo;
    let [x1, y1, z1] = hi;
    let v = |x: f32, y: f32, z: f32| [x, y, z];
    let inv = 1.0 / 2.0f32.sqrt();
    let inv3 = 1.0 / 3.0f32.sqrt();
    // Six inset faces.
    let faces = [
        (
            [
                v(x1, y0 + r, z0 + r),
                v(x1, y0 + r, z1 - r),
                v(x1, y1 - r, z1 - r),
                v(x1, y1 - r, z0 + r),
            ],
            [1.0, 0.0, 0.0],
        ),
        (
            [
                v(x0, y0 + r, z1 - r),
                v(x0, y0 + r, z0 + r),
                v(x0, y1 - r, z0 + r),
                v(x0, y1 - r, z1 - r),
            ],
            [-1.0, 0.0, 0.0],
        ),
        (
            [
                v(x0 + r, y1, z0 + r),
                v(x1 - r, y1, z0 + r),
                v(x1 - r, y1, z1 - r),
                v(x0 + r, y1, z1 - r),
            ],
            [0.0, 1.0, 0.0],
        ),
        (
            [
                v(x0 + r, y0, z0 + r),
                v(x0 + r, y0, z1 - r),
                v(x1 - r, y0, z1 - r),
                v(x1 - r, y0, z0 + r),
            ],
            [0.0, -1.0, 0.0],
        ),
        (
            [
                v(x1 - r, y0 + r, z1),
                v(x0 + r, y0 + r, z1),
                v(x0 + r, y1 - r, z1),
                v(x1 - r, y1 - r, z1),
            ],
            [0.0, 0.0, 1.0],
        ),
        (
            [
                v(x0 + r, y0 + r, z0),
                v(x1 - r, y0 + r, z0),
                v(x1 - r, y1 - r, z0),
                v(x0 + r, y1 - r, z0),
            ],
            [0.0, 0.0, -1.0],
        ),
    ];
    for (q, n) in faces {
        f.quad(out, q[0], q[1], q[2], q[3], n, s);
    }
    // Twelve edge bevels.
    let edges = [
        (
            [
                v(x0 + r, y1, z1 - r),
                v(x1 - r, y1, z1 - r),
                v(x1 - r, y1 - r, z1),
                v(x0 + r, y1 - r, z1),
            ],
            [0.0, inv, inv],
        ),
        (
            [
                v(x0 + r, y1, z0 + r),
                v(x1 - r, y1, z0 + r),
                v(x1 - r, y1 - r, z0),
                v(x0 + r, y1 - r, z0),
            ],
            [0.0, inv, -inv],
        ),
        (
            [
                v(x0 + r, y0, z1 - r),
                v(x1 - r, y0, z1 - r),
                v(x1 - r, y0 + r, z1),
                v(x0 + r, y0 + r, z1),
            ],
            [0.0, -inv, inv],
        ),
        (
            [
                v(x0 + r, y0, z0 + r),
                v(x1 - r, y0, z0 + r),
                v(x1 - r, y0 + r, z0),
                v(x0 + r, y0 + r, z0),
            ],
            [0.0, -inv, -inv],
        ),
        (
            [
                v(x1 - r, y0 + r, z1),
                v(x1 - r, y1 - r, z1),
                v(x1, y1 - r, z1 - r),
                v(x1, y0 + r, z1 - r),
            ],
            [inv, 0.0, inv],
        ),
        (
            [
                v(x1 - r, y0 + r, z0),
                v(x1 - r, y1 - r, z0),
                v(x1, y1 - r, z0 + r),
                v(x1, y0 + r, z0 + r),
            ],
            [inv, 0.0, -inv],
        ),
        (
            [
                v(x0 + r, y0 + r, z1),
                v(x0 + r, y1 - r, z1),
                v(x0, y1 - r, z1 - r),
                v(x0, y0 + r, z1 - r),
            ],
            [-inv, 0.0, inv],
        ),
        (
            [
                v(x0 + r, y0 + r, z0),
                v(x0 + r, y1 - r, z0),
                v(x0, y1 - r, z0 + r),
                v(x0, y0 + r, z0 + r),
            ],
            [-inv, 0.0, -inv],
        ),
        (
            [
                v(x1 - r, y1, z0 + r),
                v(x1 - r, y1, z1 - r),
                v(x1, y1 - r, z1 - r),
                v(x1, y1 - r, z0 + r),
            ],
            [inv, inv, 0.0],
        ),
        (
            [
                v(x1 - r, y0, z0 + r),
                v(x1 - r, y0, z1 - r),
                v(x1, y0 + r, z1 - r),
                v(x1, y0 + r, z0 + r),
            ],
            [inv, -inv, 0.0],
        ),
        (
            [
                v(x0 + r, y1, z0 + r),
                v(x0 + r, y1, z1 - r),
                v(x0, y1 - r, z1 - r),
                v(x0, y1 - r, z0 + r),
            ],
            [-inv, inv, 0.0],
        ),
        (
            [
                v(x0 + r, y0, z0 + r),
                v(x0 + r, y0, z1 - r),
                v(x0, y0 + r, z1 - r),
                v(x0, y0 + r, z0 + r),
            ],
            [-inv, -inv, 0.0],
        ),
    ];
    for (q, n) in edges {
        f.quad(out, q[0], q[1], q[2], q[3], n, s);
    }
    // Eight corner triangles.
    for sx in [0.0f32, 1.0] {
        for sy in [0.0f32, 1.0] {
            for sz in [0.0f32, 1.0] {
                let (cx, cy, cz) = if sx == 0.0 {
                    (
                        x0,
                        if sy == 0.0 { y0 } else { y1 },
                        if sz == 0.0 { z0 } else { z1 },
                    )
                } else {
                    (
                        x1,
                        if sy == 0.0 { y0 } else { y1 },
                        if sz == 0.0 { z0 } else { z1 },
                    )
                };
                let ix = if sx == 0.0 { r } else { -r };
                let iy = if sy == 0.0 { r } else { -r };
                let iz = if sz == 0.0 { r } else { -r };
                let nx = if sx == 0.0 { -inv3 } else { inv3 };
                let ny = if sy == 0.0 { -inv3 } else { inv3 };
                let nz = if sz == 0.0 { -inv3 } else { inv3 };
                let a = [cx, cy - iy, cz - iz];
                let b = [cx - ix, cy, cz - iz];
                let c = [cx - ix, cy - iy, cz];
                for p in [a, b, c] {
                    out.push(f.vert(p, [nx, ny, nz], s));
                }
            }
        }
    }
}

fn sharp_bx(out: &mut Vec<MeshVertex>, f: &Frame, lo: [f32; 3], hi: [f32; 3], s: Style) {
    let n = [
        [1.0, 0.0, 0.0],
        [-1.0, 0.0, 0.0],
        [0.0, 1.0, 0.0],
        [0.0, -1.0, 0.0],
        [0.0, 0.0, 1.0],
        [0.0, 0.0, -1.0],
    ];
    let v = |x: f32, y: f32, z: f32| [x, y, z];
    f.quad(
        out,
        v(hi[0], lo[1], lo[2]),
        v(hi[0], lo[1], hi[2]),
        v(hi[0], hi[1], hi[2]),
        v(hi[0], hi[1], lo[2]),
        n[0],
        s,
    );
    f.quad(
        out,
        v(lo[0], lo[1], hi[2]),
        v(lo[0], lo[1], lo[2]),
        v(lo[0], hi[1], lo[2]),
        v(lo[0], hi[1], hi[2]),
        n[1],
        s,
    );
    f.quad(
        out,
        v(lo[0], hi[1], lo[2]),
        v(hi[0], hi[1], lo[2]),
        v(hi[0], hi[1], hi[2]),
        v(lo[0], hi[1], hi[2]),
        n[2],
        s,
    );
    f.quad(
        out,
        v(lo[0], lo[1], hi[2]),
        v(hi[0], lo[1], hi[2]),
        v(hi[0], lo[1], lo[2]),
        v(lo[0], lo[1], lo[2]),
        n[3],
        s,
    );
    f.quad(
        out,
        v(hi[0], lo[1], hi[2]),
        v(lo[0], lo[1], hi[2]),
        v(lo[0], hi[1], hi[2]),
        v(hi[0], hi[1], hi[2]),
        n[4],
        s,
    );
    f.quad(
        out,
        v(lo[0], lo[1], lo[2]),
        v(hi[0], lo[1], lo[2]),
        v(hi[0], hi[1], lo[2]),
        v(lo[0], hi[1], lo[2]),
        n[5],
        s,
    );
}

/// Axis-aligned block: centre x/z, base y.
pub fn block(out: &mut Vec<MeshVertex>, f: &Frame, c: [f32; 3], size: [f32; 3], s: Style) {
    let lo = [c[0] - size[0] * 0.5, c[1], c[2] - size[2] * 0.5];
    let hi = [c[0] + size[0] * 0.5, c[1] + size[1], c[2] + size[2] * 0.5];
    bx(out, f, lo, hi, s);
}

#[allow(clippy::too_many_arguments)]
pub fn cyl(
    out: &mut Vec<MeshVertex>,
    f: &Frame,
    c: [f32; 3],
    r0: f32,
    r1: f32,
    h: f32,
    s: Style,
    seg: usize,
) {
    let seg = seg.max(6);
    let segf = seg as f32;
    for i in 0..seg {
        let a0 = i as f32 / segf * TAU;
        let a1 = (i + 1) as f32 / segf * TAU;
        let am = (a0 + a1) * 0.5;
        let n = [am.cos(), 0.0, am.sin()];
        let p = |r: f32, a: f32, y: f32| [c[0] + r * a.cos(), c[1] + y, c[2] + r * a.sin()];
        f.quad(
            out,
            p(r0, a0, 0.0),
            p(r0, a1, 0.0),
            p(r1, a1, h),
            p(r1, a0, h),
            n,
            s,
        );
    }
    // caps
    let top = [0.0, 1.0, 0.0];
    let bottom = [0.0, -1.0, 0.0];
    for i in 0..seg {
        let a0 = i as f32 / segf * TAU;
        let a1 = (i + 1) as f32 / segf * TAU;
        let t = |a: f32| [c[0] + r1 * a.cos(), c[1] + h, c[2] + r1 * a.sin()];
        f.quad(out, [c[0], c[1] + h, c[2]], t(a0), t(a1), t(a1), top, s);
        let b = |a: f32| [c[0] + r0 * a.cos(), c[1], c[2] + r0 * a.sin()];
        f.quad(out, [c[0], c[1], c[2]], b(a1), b(a0), b(a0), bottom, s);
    }
}

#[allow(clippy::too_many_arguments)]
pub fn blob(
    out: &mut Vec<MeshVertex>,
    f: &Frame,
    c: [f32; 3],
    radii: [f32; 3],
    s: Style,
    seg: usize,
) {
    let us = seg.max(8);
    let vs = (seg as f32 * 0.7).max(5.0) as usize;
    for iv in 0..vs {
        let v0 = PI * iv as f32 / vs as f32;
        let v1 = PI * (iv + 1) as f32 / vs as f32;
        for iu in 0..us {
            let u0 = TAU * iu as f32 / us as f32;
            let u1 = TAU * (iu + 1) as f32 / us as f32;
            let p = |v: f32, u: f32| {
                [
                    c[0] + radii[0] * v.sin() * u.cos(),
                    c[1] + radii[1] * v.cos(),
                    c[2] + radii[2] * v.sin() * u.sin(),
                ]
            };
            let nrm = |v: f32, u: f32| {
                let x = v.sin() * u.cos() / radii[0];
                let y = v.cos() / radii[1];
                let z = v.sin() * u.sin() / radii[2];
                let l = (x * x + y * y + z * z).sqrt().max(1e-6);
                [x / l, y / l, z / l]
            };
            let vm = (v0 + v1) * 0.5;
            let um = (u0 + u1) * 0.5;
            f.quad(
                out,
                p(v0, u0),
                p(v0, u1),
                p(v1, u1),
                p(v1, u0),
                nrm(vm, um),
                s,
            );
        }
    }
}

pub fn ring(
    out: &mut Vec<MeshVertex>,
    f: &Frame,
    c: [f32; 3],
    big: f32,
    small: f32,
    s: Style,
    seg: usize,
) {
    let us = seg.max(8);
    let vs = 10usize;
    for iu in 0..us {
        let u0 = TAU * iu as f32 / us as f32;
        let u1 = TAU * (iu + 1) as f32 / us as f32;
        for iv in 0..vs {
            let v0 = TAU * iv as f32 / vs as f32;
            let v1 = TAU * (iv + 1) as f32 / vs as f32;
            let p = |u: f32, v: f32| {
                [
                    c[0] + (big + small * v.cos()) * u.cos(),
                    c[1] + small * v.sin(),
                    c[2] + (big + small * v.cos()) * u.sin(),
                ]
            };
            let nrm = |u: f32, v: f32| [v.cos() * u.cos(), v.sin(), v.cos() * u.sin()];
            let um = (u0 + u1) * 0.5;
            let vm = (v0 + v1) * 0.5;
            f.quad(
                out,
                p(u0, v0),
                p(u1, v0),
                p(u1, v1),
                p(u0, v1),
                nrm(um, vm),
                s,
            );
        }
    }
}

/// Surface of revolution; `pts` are (radius, height) from bottom to top.
pub fn lathe(
    out: &mut Vec<MeshVertex>,
    f: &Frame,
    c: [f32; 3],
    pts: &[(f32, f32)],
    s: Style,
    seg: usize,
) {
    let us = seg.max(8);
    for pair in pts.windows(2) {
        let (r0, h0) = pair[0];
        let (r1, h1) = pair[1];
        if (r1 - r0).abs() < 1e-6 && (h1 - h0).abs() < 1e-6 {
            continue;
        }
        let dr = r1 - r0;
        let dh = h1 - h0;
        let l = (dr * dr + dh * dh).sqrt().max(1e-6);
        let nr = dh / l;
        let ny = -dr / l;
        for i in 0..us {
            let a0 = TAU * i as f32 / us as f32;
            let a1 = TAU * (i + 1) as f32 / us as f32;
            let am = (a0 + a1) * 0.5;
            let p = |r: f32, h: f32, a: f32| [c[0] + r * a.cos(), c[1] + h, c[2] + r * a.sin()];
            let n = [nr * am.cos(), ny, nr * am.sin()];
            f.quad(
                out,
                p(r0, h0, a0),
                p(r0, h0, a1),
                p(r1, h1, a1),
                p(r1, h1, a0),
                n,
                s,
            );
        }
    }
}

/// Tapered rod between two points (chair legs, lamp arms).
#[allow(clippy::too_many_arguments)]
pub fn rod(
    out: &mut Vec<MeshVertex>,
    f: &Frame,
    a: [f32; 3],
    b: [f32; 3],
    r0: f32,
    r1: f32,
    s: Style,
    seg: usize,
) {
    let d = [b[0] - a[0], b[1] - a[1], b[2] - a[2]];
    let len = (d[0] * d[0] + d[1] * d[1] + d[2] * d[2]).sqrt();
    if len < 1e-5 {
        return;
    }
    let w = [d[0] / len, d[1] / len, d[2] / len];
    let tmp = if w[1].abs() < 0.9 {
        [0.0, 1.0, 0.0]
    } else {
        [1.0, 0.0, 0.0]
    };
    let u = [
        w[1] * tmp[2] - w[2] * tmp[1],
        w[2] * tmp[0] - w[0] * tmp[2],
        w[0] * tmp[1] - w[1] * tmp[0],
    ];
    let ul = (u[0] * u[0] + u[1] * u[1] + u[2] * u[2]).sqrt().max(1e-6);
    let u = [u[0] / ul, u[1] / ul, u[2] / ul];
    let v = [
        w[1] * u[2] - w[2] * u[1],
        w[2] * u[0] - w[0] * u[2],
        w[0] * u[1] - w[1] * u[0],
    ];
    let seg = seg.max(6);
    let segf = seg as f32;
    for i in 0..seg {
        let a0 = i as f32 / segf * TAU;
        let a1 = (i + 1) as f32 / segf * TAU;
        let am = (a0 + a1) * 0.5;
        let n = [
            u[0] * am.cos() + v[0] * am.sin(),
            u[1] * am.cos() + v[1] * am.sin(),
            u[2] * am.cos() + v[2] * am.sin(),
        ];
        let pa = |r: f32, ang: f32, at: [f32; 3]| {
            [
                at[0] + u[0] * r * ang.cos() + v[0] * r * ang.sin(),
                at[1] + u[1] * r * ang.cos() + v[1] * r * ang.sin(),
                at[2] + u[2] * r * ang.cos() + v[2] * r * ang.sin(),
            ]
        };
        f.quad(
            out,
            pa(r0, a0, a),
            pa(r0, a1, a),
            pa(r1, a1, b),
            pa(r1, a0, b),
            n,
            s,
        );
    }
    // end caps as fans
    let cap = |out: &mut Vec<MeshVertex>, at: [f32; 3], r: f32, n: [f32; 3]| {
        for i in 0..seg {
            let a0 = i as f32 / segf * TAU;
            let a1 = (i + 1) as f32 / segf * TAU;
            let p0 = [
                at[0] + u[0] * r * a0.cos() + v[0] * r * a0.sin(),
                at[1] + u[1] * r * a0.cos() + v[1] * r * a0.sin(),
                at[2] + u[2] * r * a0.cos() + v[2] * r * a0.sin(),
            ];
            let p1 = [
                at[0] + u[0] * r * a1.cos() + v[0] * r * a1.sin(),
                at[1] + u[1] * r * a1.cos() + v[1] * r * a1.sin(),
                at[2] + u[2] * r * a1.cos() + v[2] * r * a1.sin(),
            ];
            out.push(f.vert(at, n, s));
            out.push(f.vert(p0, n, s));
            out.push(f.vert(p1, n, s));
        }
    };
    let flip = [-w[0], -w[1], -w[2]];
    cap(out, a, r0, flip);
    cap(out, b, r1, w);
}

// ------------------------------------------------------------------ details

#[allow(clippy::too_many_arguments)]
pub fn legs(
    out: &mut Vec<MeshVertex>,
    f: &Frame,
    w: f32,
    d: f32,
    h: f32,
    inset: f32,
    r: f32,
    s: Style,
) {
    for (a, b) in [(-1.0f32, -1.0f32), (1.0, -1.0), (-1.0, 1.0), (1.0, 1.0)] {
        let x = a * (w * 0.5 - inset);
        let z = b * (d * 0.5 - inset);
        rod(out, f, [x, 0.0, z], [x, h, z], r * 0.7, r, s, 10);
    }
}

#[allow(clippy::too_many_arguments)]
pub fn pull(
    out: &mut Vec<MeshVertex>,
    f: &Frame,
    len: f32,
    vert: bool,
    x: f32,
    y: f32,
    z: f32,
    s: Style,
) {
    let bar = if vert {
        [0.012, len, 0.012]
    } else {
        [len, 0.012, 0.012]
    };
    block(out, f, [x, y - len * 0.5, z + 0.026], bar, s);
    for side in [-1.0f32, 1.0] {
        let post = [0.01, 0.01, 0.026];
        let px = if vert {
            x
        } else {
            x + side * (len * 0.5 - 0.014)
        };
        let py = if vert {
            y + side * (len * 0.5 - 0.014)
        } else {
            y - 0.006
        };
        block(out, f, [px, py, z + 0.013], post, s);
    }
}

pub fn knob(out: &mut Vec<MeshVertex>, f: &Frame, x: f32, y: f32, z: f32, s: Style) {
    cyl(out, f, [x, y, z], 0.014, 0.011, 0.022, s, 16);
    blob(out, f, [x, y + 0.022, z], [0.014, 0.008, 0.014], s, 12);
}

#[derive(Clone, Copy, PartialEq)]
pub enum Hardware {
    Bar,
    Knob,
    Edge,
    None,
}

#[allow(clippy::too_many_arguments)]
pub fn fronts(
    out: &mut Vec<MeshVertex>,
    f: &Frame,
    x0: f32,
    y0: f32,
    w: f32,
    h: f32,
    z: f32,
    nx: usize,
    ny: usize,
    s: Style,
    hw: Hardware,
) {
    let gap = 0.005;
    block(
        out,
        f,
        [x0 + w * 0.5, y0, z + 0.002],
        [w, h, 0.004],
        st([0.09, 0.08, 0.07], Finish::new(0.8, 0.0, 0.0)),
    );
    let pw = w / nx as f32;
    let ph = h / ny as f32;
    let hw_style = st([0.72, 0.70, 0.66], Finish::metal());
    for i in 0..nx {
        for j in 0..ny {
            let cx = x0 + pw * (i as f32 + 0.5);
            let yb = y0 + ph * j as f32;
            block(
                out,
                f,
                [cx, yb + gap * 0.5, z + 0.012],
                [pw - gap, ph - gap, 0.016],
                s,
            );
            if hw == Hardware::None {
                continue;
            }
            if hw == Hardware::Edge {
                block(
                    out,
                    f,
                    [cx, yb + ph - gap - 0.014, z + 0.021],
                    [pw - 0.05, 0.008, 0.006],
                    st([0.20, 0.19, 0.18], Finish::matte()),
                );
                continue;
            }
            let drawer = ph < 0.4 && pw >= ph;
            if drawer {
                if hw == Hardware::Knob {
                    knob(out, f, cx, yb + ph * 0.5, z + 0.022, hw_style);
                } else {
                    pull(
                        out,
                        f,
                        (pw * 0.45).min(0.3),
                        false,
                        cx,
                        yb + ph * 0.5,
                        z + 0.022,
                        hw_style,
                    );
                }
                continue;
            }
            let len = (ph * 0.35).min(0.45);
            let hx = if nx == 1 {
                cx + pw * 0.5 - 0.045
            } else if i % 2 == 1 {
                cx - pw * 0.5 + 0.04
            } else {
                cx + pw * 0.5 - 0.04
            };
            let hy = (yb + ph * 0.5)
                .min(yb + ph - 0.05 - len * 0.5)
                .max(yb + 0.05 + len * 0.5);
            if hw == Hardware::Knob {
                knob(out, f, hx, hy, z + 0.022, hw_style);
            } else {
                pull(out, f, len, true, hx, hy, z + 0.022, hw_style);
            }
        }
    }
}

#[allow(clippy::too_many_arguments)]
pub fn vase(
    out: &mut Vec<MeshVertex>,
    f: &Frame,
    x: f32,
    y: f32,
    z: f32,
    r: f32,
    h: f32,
    color: [f32; 3],
    rnd: &mut u32,
) {
    let pts = [
        (0.0, 0.0),
        (r * 0.65, 0.0),
        (r, h * 0.32),
        (r * 0.92, h * 0.62),
        (r * 0.42, h * 0.86),
        (r * 0.5, h),
    ];
    lathe(
        out,
        f,
        [x, y, z],
        &pts,
        st(color, Finish::new(0.35, 0.0, 0.0)),
        24,
    );
    if (*rnd).is_multiple_of(3) {
        return;
    }
    let stems = st([0.42, 0.54, 0.28], Finish::matte());
    for k in 0..5 {
        let a = k as f32 * 1.26 + rand01(rnd) * 0.4;
        let s = 0.03 + rand01(rnd) * 0.05;
        let top = [
            x + a.cos() * s,
            y + h + 0.12 + rand01(rnd) * 0.14,
            z + a.sin() * s,
        ];
        rod(out, f, [x, y + h * 0.6, z], top, 0.003, 0.003, stems, 6);
        let petals = [
            [0.95, 0.88, 0.80],
            [0.90, 0.66, 0.58],
            [0.96, 0.80, 0.44],
            [1.0, 1.0, 1.0],
        ][k % 4];
        blob(
            out,
            f,
            top,
            [0.022, 0.018, 0.022],
            st(petals, Finish::fabric()),
            12,
        );
    }
}

pub fn rand01(state: &mut u32) -> f32 {
    *state = state.wrapping_mul(1664525).wrapping_add(1013904223);
    ((*state >> 8) & 0xffff) as f32 / 65535.0
}

pub fn table_lamp(out: &mut Vec<MeshVertex>, f: &Frame, x: f32, y: f32, z: f32, s: f32) {
    let base = [
        (0.0, 0.0),
        (0.06 * s, 0.0),
        (0.075 * s, 0.05 * s),
        (0.08 * s, 0.12 * s),
        (0.055 * s, 0.2 * s),
        (0.018 * s, 0.25 * s),
        (0.012 * s, 0.3 * s),
    ];
    lathe(
        out,
        f,
        [x, y, z],
        &base,
        st([0.92, 0.90, 0.86], Finish::ceramic()),
        20,
    );
    let shade = [(0.13 * s, 0.0), (0.10 * s, 0.18 * s)];
    lathe(
        out,
        f,
        [x, y + 0.26 * s, z],
        &shade,
        st([0.97, 0.92, 0.83], Finish::lamp()),
        24,
    );
    blob(
        out,
        f,
        [x, y + 0.31 * s, z],
        [0.03 * s, 0.03 * s, 0.03 * s],
        st([1.0, 1.0, 1.0], Finish::glow(1.2)),
        12,
    );
}

pub fn plate(out: &mut Vec<MeshVertex>, f: &Frame, r: f32, x: f32, y: f32, z: f32) {
    let pts = [(0.0, 0.0), (r * 0.6, 0.0), (r * 0.72, 0.008), (r, 0.02)];
    lathe(
        out,
        f,
        [x, y, z],
        &pts,
        st([0.99, 0.98, 0.96], Finish::ceramic()),
        20,
    );
}

// ------------------------------------------------------------------ builders

/// A squashed superellipsoid with a slight backward tilt: a stuffed pillow
/// reads as a soft rectangle instead of a polished egg.
#[allow(clippy::too_many_arguments)]
fn pillow(
    out: &mut Vec<MeshVertex>,
    f: &Frame,
    c: [f32; 3],
    half: [f32; 3],
    tilt_deg: f32,
    s: Style,
    seg: usize,
) {
    let us = seg.max(12);
    let vs = (seg as f32 * 0.55).max(8.0) as usize;
    let e = 0.55f32;
    let (tilt_sin, tilt_cos) = (tilt_deg.to_radians().sin(), tilt_deg.to_radians().cos());
    let p = |v: f32, u: f32| {
        let cv = v.cos();
        let sv = v.sin();
        let pv = cv.signum() * cv.abs().powf(e);
        let yv = sv.signum() * sv.abs().powf(e);
        let cu = u.cos();
        let su = u.sin();
        let pu = cu.signum() * cu.abs().powf(e);
        let zu = su.signum() * su.abs().powf(e);
        let x = half[0] * pv * pu;
        let raw_y = half[1] * yv;
        let raw_z = half[2] * pv * zu;
        [
            c[0] + x,
            c[1] + raw_y * tilt_cos - raw_z * tilt_sin,
            c[2] + raw_y * tilt_sin + raw_z * tilt_cos,
        ]
    };
    let nrm = |v: f32, u: f32| {
        let p0 = p(v, u);
        let mut n = [
            (p0[0] - c[0]) / (half[0] * half[0]),
            (p0[1] - c[1]) / (half[1] * half[1]),
            (p0[2] - c[2]) / (half[2] * half[2]),
        ];
        let ny = n[1];
        n[1] = ny * tilt_cos - n[2] * tilt_sin;
        n[2] = ny * tilt_sin + n[2] * tilt_cos;
        let l = (n[0] * n[0] + n[1] * n[1] + n[2] * n[2]).sqrt().max(1e-6);
        [n[0] / l, n[1] / l, n[2] / l]
    };
    for iv in 0..vs {
        let v0 = -PI * 0.5 + PI * iv as f32 / vs as f32;
        let v1 = -PI * 0.5 + PI * (iv + 1) as f32 / vs as f32;
        for iu in 0..us {
            let u0 = TAU * iu as f32 / us as f32;
            let u1 = TAU * (iu + 1) as f32 / us as f32;
            f.quad_n(
                out,
                p(v0, u0),
                p(v0, u1),
                p(v1, u1),
                p(v1, u0),
                nrm(v0, u0),
                nrm(v0, u1),
                nrm(v1, u1),
                nrm(v1, u0),
                s,
            );
        }
    }
}

fn b_seat(out: &mut Vec<MeshVertex>, f: &Frame, w: f32, d: f32, c: [f32; 3], single: bool) {
    let a = 0.18f32.min(w * 0.14);
    let n = if single {
        1
    } else if w > 2.2 {
        3
    } else {
        2
    };
    let cw = (w - 2.0 * a) / n as f32;
    let bd = 0.2f32.min(d * 0.24);
    let lh = 0.12f32;
    let sd = d - bd - 0.02;
    let dk = dark(c, 0.88);
    legs(
        out,
        f,
        w,
        d,
        lh,
        0.07,
        0.018,
        st(rgb("#3a3027"), Finish::wood()),
    );
    block(
        out,
        f,
        [0.0, lh, 0.0],
        [w, 0.16, d],
        st(dk, Finish::fabric()),
    );
    block(
        out,
        f,
        [0.0, lh, -d * 0.5 + bd * 0.5],
        [w, 0.73, bd],
        st(dk, Finish::fabric()),
    );
    for side in [-1.0f32, 1.0] {
        pillow(
            out,
            f,
            [side * (w * 0.5 - a * 0.5), lh + 0.25, 0.0],
            [a * 0.5, 0.25, d * 0.5 - 0.01],
            0.0,
            st(dk, Finish::fabric()),
            14,
        );
    }
    // A soft roll along the top of the backrest catches a highlight and
    // keeps the silhouette from reading as a hard box.
    pillow(
        out,
        f,
        [0.0, lh + 0.72, -d * 0.5 + bd * 0.55],
        [w * 0.5 - 0.055, 0.055, bd * 0.55],
        0.0,
        st(dark(c, 0.9), Finish::fabric()),
        14,
    );
    for i in 0..n {
        let x = -w * 0.5 + a + cw * (i as f32 + 0.5);
        // Soft seat and back cushions: superellipsoids keep the corners from
        // reading as wooden slabs while still filling the frame.
        pillow(
            out,
            f,
            [x, lh + 0.36, -d * 0.5 + bd + sd * 0.5],
            [cw * 0.5 - 0.022, 0.085, sd * 0.5 - 0.018],
            0.0,
            st(c, Finish::fabric()),
            14,
        );
        pillow(
            out,
            f,
            [x, lh + 0.45, -d * 0.5 + bd + 0.085],
            [cw * 0.5 - 0.022, 0.235, 0.085],
            0.0,
            st(c, Finish::fabric()),
            14,
        );
        // Piping along the seat front and two tufting buttons on the back
        // cushion break up the otherwise flat upholstery panels.
        let seam = st(dark(c, 0.62), Finish::fabric());
        block(
            out,
            f,
            [x, lh + 0.43, -d * 0.5 + bd + sd - 0.008],
            [cw - 0.05, 0.012, 0.012],
            seam,
        );
        let button = st(dark(c, 0.5), Finish::fabric());
        for bx in [-cw * 0.18, cw * 0.18] {
            cyl(
                out,
                f,
                [x + bx, lh + 0.47, -d * 0.5 + bd + 0.085],
                0.012,
                0.007,
                0.012,
                button,
                10,
            );
        }
    }
    let px = if single { 0.0 } else { w * 0.22 };
    let pillow_a = [0.90, 0.86, 0.78];
    // One rust accent pillow against the neutral sofa reads as styling
    // rather than a second upholstery panel.
    let pillow_b = [0.60, 0.36, 0.24];
    for (k, x) in [(0usize, -px), (1, px)] {
        if single && k == 1 {
            break;
        }
        if x == 0.0 {
            continue;
        }
        let pc = if k == 0 { pillow_a } else { pillow_b };
        pillow(
            out,
            f,
            [x, lh + 0.54, -d * 0.5 + bd + 0.16],
            [0.205, 0.18, 0.10],
            -11.0,
            st(pc, Finish::fabric()),
            18,
        );
    }
    if single {
        pillow(
            out,
            f,
            [0.0, lh + 0.50, -d * 0.5 + bd + 0.16],
            [0.19, 0.16, 0.09],
            -9.0,
            st(pillow_a, Finish::fabric()),
            14,
        );
    }
}

fn b_bed(out: &mut Vec<MeshVertex>, f: &Frame, w: f32, d: f32, h: f32, c: [f32; 3], single: bool) {
    legs(
        out,
        f,
        w,
        d,
        0.1,
        0.06,
        0.02,
        st(rgb("#3a3027"), Finish::wood()),
    );
    block(
        out,
        f,
        [0.0, 0.1, 0.0],
        [w - 0.08, 0.28, d - 0.06],
        st(c, Finish::wood()),
    );
    block(
        out,
        f,
        [0.0, 0.0, -d * 0.5 + 0.04],
        [w, h.min(1.1), 0.07],
        st(dark(c, 0.82), Finish::fabric()),
    );
    let tufts = (w / 0.28).round().max(1.0) as usize;
    for i in 0..tufts {
        let x = -w * 0.5 + w / tufts as f32 * (i as f32 + 0.5);
        block(
            out,
            f,
            [x, 0.1, -d * 0.5 + 0.09],
            [w / tufts as f32 - 0.02, h.min(1.1) * 0.7, 0.03],
            st(dark(c, 0.9), Finish::fabric()),
        );
    }
    block(
        out,
        f,
        [0.0, 0.38, 0.0],
        [w - 0.1, 0.2, d - 0.1],
        st(rgb("#f6f3ec"), Finish::fabric()),
    );
    block(
        out,
        f,
        [0.0, 0.58, d * 0.12],
        [w - 0.04, 0.09, d * 0.62],
        st(light(c, 0.22), Finish::fabric()),
    );
    block(
        out,
        f,
        [0.0, 0.67, d * 0.12],
        [w - 0.06, 0.03, 0.2],
        st(dark(c, 0.75), Finish::fabric()),
    );
    let pw = if single { 0.0 } else { w * 0.22 };
    for k in 0..2 {
        let x = if single {
            0.0
        } else {
            if k == 0 { -pw } else { pw }
        };
        if single && k == 1 {
            break;
        }
        pillow(
            out,
            f,
            [x, 0.67, -d * 0.5 + 0.35],
            [0.34, 0.15, 0.11],
            -7.0,
            st(rgb("#f8f4ea"), Finish::fabric()),
            16,
        );
    }
    // A folded throw across the foot of the bed adds a second fabric tone and
    // a visible edge, which reads much more like a made bed than one smooth
    // slab.
    let throw = st([0.46, 0.52, 0.58], Finish::fabric());
    block(
        out,
        f,
        [0.0, 0.675, d * 0.26],
        [w - 0.18, 0.02, d * 0.42],
        throw,
    );
    block(
        out,
        f,
        [0.0, 0.655, d * 0.47],
        [w - 0.16, 0.06, 0.03],
        st(dark([0.46, 0.52, 0.58], 0.82), Finish::fabric()),
    );
}

fn b_table(out: &mut Vec<MeshVertex>, f: &Frame, w: f32, d: f32, h: f32, c: [f32; 3]) {
    legs(
        out,
        f,
        w,
        d,
        h - 0.03,
        0.09,
        0.022,
        st(dark(c, 0.55), Finish::wood()),
    );
    block(
        out,
        f,
        [0.0, h - 0.035, 0.0],
        [w, 0.035, d],
        st(c, Finish::wood()),
    );
    block(
        out,
        f,
        [0.0, h, -0.05],
        [w * 0.7, 0.004, 0.32],
        st(rgb("#b9a58c"), Finish::fabric()),
    );
    let n = (w / 0.62).round().max(1.0) as usize;
    for i in 0..n {
        for side in [-1.0f32, 1.0] {
            let x = -w * 0.5 + w / n as f32 * (i as f32 + 0.5);
            let z = side * (d * 0.5 - 0.17);
            block(
                out,
                f,
                [x, h, z],
                [0.38, 0.004, 0.28],
                st(rgb("#efe8db"), Finish::fabric()),
            );
            plate(out, f, 0.12, x, h + 0.004, z);
            cyl(
                out,
                f,
                [x + 0.15, h + 0.004, z - side * 0.1],
                0.03,
                0.026,
                0.1,
                sg([0.88, 0.95, 0.97], Finish::new(0.04, 0.0, 0.0), 0.38),
                16,
            );
        }
    }
    let mut rnd = 7u32;
    vase(
        out,
        f,
        0.0,
        h + 0.004,
        0.0,
        0.06,
        0.2,
        rgb("#e9e2d6"),
        &mut rnd,
    );
}

fn b_round_table(out: &mut Vec<MeshVertex>, f: &Frame, w: f32, d: f32, h: f32, c: [f32; 3]) {
    let r = w.min(d) * 0.5;
    let b = 0.25f32.min(w * 0.3);
    let pts = [
        (b, 0.0),
        (b, 0.015),
        (b * 0.8, 0.035),
        (0.06, 0.12),
        (0.045, 0.35),
        (0.05, h * 0.72),
        (0.12, h - 0.06),
        (0.18, h - 0.035),
    ];
    lathe(
        out,
        f,
        [0.0, 0.0, 0.0],
        &pts,
        st(rgb("#4a3e33"), Finish::wood()),
        24,
    );
    cyl(
        out,
        f,
        [0.0, h - 0.035, 0.0],
        r,
        r,
        0.035,
        st(c, Finish::new(0.4, 0.0, 0.0)),
        32,
    );
    for k in 0..4 {
        let a = k as f32 * PI * 0.5 + 0.4;
        plate(out, f, 0.11, a.cos() * (r - 0.17), h, a.sin() * (r - 0.17));
    }
    let mut rnd = 11u32;
    vase(out, f, 0.0, h, 0.0, 0.05, 0.18, rgb("#e9e2d6"), &mut rnd);
}

fn b_chair(out: &mut Vec<MeshVertex>, f: &Frame, w: f32, d: f32, h: f32, c: [f32; 3]) {
    let lw = st(rgb("#6b543f"), Finish::wood());
    let sy = h * 0.53;
    for side in [-1.0f32, 1.0] {
        rod(
            out,
            f,
            [side * (w * 0.5 - 0.04), 0.0, d * 0.5 - 0.05],
            [side * (w * 0.5 - 0.05), sy, d * 0.5 - 0.07],
            0.012,
            0.018,
            lw,
            8,
        );
        rod(
            out,
            f,
            [side * (w * 0.5 - 0.04), 0.0, -d * 0.5 + 0.04],
            [side * (w * 0.5 - 0.05), h * 0.95, -d * 0.5 + 0.07],
            0.012,
            0.018,
            lw,
            8,
        );
        block(
            out,
            f,
            [side * (w * 0.5 - 0.045), 0.18, 0.0],
            [0.02, 0.025, d - 0.12],
            lw,
        );
    }
    block(
        out,
        f,
        [0.0, 0.15, d * 0.5 - 0.06],
        [w - 0.08, 0.025, 0.02],
        lw,
    );
    block(
        out,
        f,
        [0.0, 0.15, -d * 0.5 + 0.05],
        [w - 0.08, 0.025, 0.02],
        lw,
    );
    block(out, f, [0.0, sy - 0.03, 0.0], [w - 0.06, 0.04, d - 0.1], lw);
    block(
        out,
        f,
        [0.0, sy, 0.005],
        [w - 0.04, 0.055, d - 0.08],
        st(c, Finish::fabric()),
    );
    block(
        out,
        f,
        [0.0, h * 0.72, -d * 0.5 + 0.065],
        [w - 0.08, 0.15, 0.03],
        lw,
    );
    block(
        out,
        f,
        [0.0, h * 0.56, -d * 0.5 + 0.055],
        [w - 0.08, 0.025, 0.02],
        lw,
    );
}

fn b_bookshelf(out: &mut Vec<MeshVertex>, f: &Frame, w: f32, d: f32, h: f32, c: [f32; 3]) {
    let wm = st(c, Finish::wood());
    let t = 0.022f32;
    let ns = 5usize;
    let sh = (h - 0.06 - t) / ns as f32;
    let book_cols = [
        rgb("#b88a6a"),
        rgb("#6f8f8a"),
        rgb("#d9c08c"),
        rgb("#9aa58c"),
        rgb("#a8675e"),
        rgb("#e6dccd"),
        rgb("#7d8ea3"),
    ];
    block(
        out,
        f,
        [0.0, 0.0, -d * 0.5 + 0.006],
        [w, h, 0.012],
        st(dark(c, 0.85), Finish::wood()),
    );
    for side in [-1.0f32, 1.0] {
        block(
            out,
            f,
            [side * (w * 0.5 - t * 0.5), 0.0, 0.0],
            [t, h, d],
            wm,
        );
    }
    block(out, f, [0.0, 0.0, 0.0], [w - 2.0 * t, 0.06, d - 0.03], wm);
    let mut rnd = 21u32;
    for s in 0..=ns {
        let y = if s < ns { 0.06 + s as f32 * sh } else { h - t };
        block(out, f, [0.0, y, 0.006], [w - 2.0 * t, t, d - 0.012], wm);
        if s == ns {
            break;
        }
        let mut x = -w * 0.5 + t + 0.01;
        while x < w * 0.5 - t - 0.04 {
            if rand01(&mut rnd) < 0.08 {
                x += 0.05 + rand01(&mut rnd) * 0.04;
                continue;
            }
            let bw = 0.018 + rand01(&mut rnd) * 0.03;
            let bh = (sh - t - 0.03).min(0.17 + rand01(&mut rnd) * 0.13);
            let bd = d * (0.62 + rand01(&mut rnd) * 0.22);
            let col =
                book_cols[(rand01(&mut rnd) * book_cols.len() as f32) as usize % book_cols.len()];
            block(
                out,
                f,
                [x + bw * 0.5, y + t, d * 0.5 - 0.015 - bd * 0.5],
                [bw, bh, bd],
                st(col, Finish::new(0.75, 0.0, 0.0)),
            );
            x += bw + 0.002;
        }
    }
}

fn b_plant(out: &mut Vec<MeshVertex>, f: &Frame, w: f32, d: f32, _h: f32, _c: [f32; 3]) {
    let r = w.min(d) * 0.5;
    let hh = 0.9 + r * 1.6;
    let pot = [0.9, 0.86, 0.79];
    let pts = [
        (0.0, 0.0),
        (r * 0.4, 0.0),
        (r * 0.44, 0.02),
        (r * 0.54, 0.36),
        (r * 0.57, 0.4),
        (r * 0.52, 0.4),
        (r * 0.5, 0.37),
        (0.0, 0.37),
    ];
    lathe(
        out,
        f,
        [0.0, 0.0, 0.0],
        &pts,
        st(pot, Finish::new(0.6, 0.0, 0.0)),
        28,
    );
    cyl(
        out,
        f,
        [0.0, 0.368, 0.0],
        r * 0.49,
        r * 0.49,
        0.006,
        st([0.29, 0.23, 0.17], Finish::matte()),
        20,
    );
    rod(
        out,
        f,
        [0.0, 0.37, 0.0],
        [0.02, hh * 0.62, -0.01],
        0.014,
        0.009,
        st([0.42, 0.33, 0.25], Finish::wood()),
        8,
    );
    let leaf_a = [0.37, 0.56, 0.31];
    let leaf_b = [0.47, 0.66, 0.38];
    let mut rnd = 31u32;
    let n = 16usize;
    for k in 0..n {
        let t = k as f32 / n as f32;
        let a = k as f32 * 2.399 + rand01(&mut rnd) * 0.3;
        let len = (0.14 + rand01(&mut rnd) * 0.08) * (1.1 - t * 0.4);
        let leaf = if k % 2 == 0 { leaf_a } else { leaf_b };
        let cx = a.cos() * (0.1 + t * 0.16);
        let cz = a.sin() * (0.1 + t * 0.16);
        let cy = hh * (0.42 + 0.58 * t);
        blob(
            out,
            f,
            [cx, cy, cz],
            [len * 0.36, 0.012, len * 0.2],
            st(leaf, Finish::new(0.6, 0.0, 0.0)),
            10,
        );
    }
}

fn b_floor_lamp(out: &mut Vec<MeshVertex>, f: &Frame, w: f32, d: f32, _h: f32, c: [f32; 3]) {
    let r = w.min(d) * 0.5;
    let lm = st(c, Finish::new(0.35, 0.5, 0.0));
    lathe(
        out,
        f,
        [0.0, 0.0, 0.0],
        &[
            (0.0, 0.0),
            (r * 0.55, 0.0),
            (r * 0.58, 0.012),
            (r * 0.5, 0.03),
            (0.02, 0.035),
        ],
        lm,
        24,
    );
    rod(
        out,
        f,
        [0.0, 0.03, 0.0],
        [0.0, 1.36, 0.0],
        0.011,
        0.011,
        lm,
        8,
    );
    lathe(
        out,
        f,
        [0.0, 1.22, 0.0],
        &[(r * 0.85, 0.0), (r * 0.55, 0.36)],
        st([0.96, 0.92, 0.85], Finish::lamp()),
        28,
    );
    blob(
        out,
        f,
        [0.0, 1.36, 0.0],
        [0.035, 0.035, 0.035],
        st([1.0, 1.0, 1.0], Finish::glow(1.3)),
        12,
    );
    ring(out, f, [0.0, 1.22, 0.0], r * 0.85, 0.004, lm, 24);
    ring(out, f, [0.0, 1.58, 0.0], r * 0.55, 0.004, lm, 24);
}

fn b_rug(out: &mut Vec<MeshVertex>, f: &Frame, w: f32, d: f32, _h: f32, c: [f32; 3]) {
    for (a, b, y, col) in [
        (w, d, 0.002, c),
        (w - 0.16, d - 0.16, 0.012, dark(c, 0.82)),
        (w - 0.26, d - 0.26, 0.0135, light(c, 0.12)),
    ] {
        block(
            out,
            f,
            [0.0, y, 0.0],
            [a, 0.006, b],
            st(col, Finish::fabric()),
        );
    }
}

fn b_tv(out: &mut Vec<MeshVertex>, f: &Frame, w: f32, d: f32, _h: f32, _c: [f32; 3]) {
    let th = w * 0.5625;
    let yb = 1.2 - th * 0.5;
    block(
        out,
        f,
        [0.0, yb, 0.01],
        [w, th, 0.025],
        st(rgb("#18181a"), Finish::new(0.4, 0.3, 0.0)),
    );
    block(
        out,
        f,
        [0.0, yb + 0.014, 0.0235],
        [w - 0.016, th - 0.022, 0.002],
        st(rgb("#1f2b3a"), Finish::screen(0.25)),
    );
    block(
        out,
        f,
        [0.0, yb + 0.004, 0.0235],
        [0.04, 0.006, 0.002],
        st([0.55, 0.55, 0.56], Finish::matte()),
    );
    block(
        out,
        f,
        [0.0, 0.0, -d * 0.5 + 0.02],
        [0.5, 0.06, 0.3],
        st(rgb("#222"), Finish::matte()),
    );
    rod(
        out,
        f,
        [0.0, 0.06, -d * 0.5 + 0.1],
        [0.0, yb, 0.0],
        0.02,
        0.02,
        st(rgb("#2a2a2c"), Finish::black_metal()),
        8,
    );
}

fn b_tv_stand(out: &mut Vec<MeshVertex>, f: &Frame, w: f32, d: f32, h: f32, c: [f32; 3]) {
    legs(
        out,
        f,
        w,
        d,
        0.1,
        0.08,
        0.014,
        st(rgb("#2b2b2d"), Finish::black_metal()),
    );
    block(
        out,
        f,
        [0.0, 0.1, -0.01],
        [w, h * 0.55, d - 0.02],
        st(c, Finish::new(0.5, 0.0, 0.0)),
    );
    fronts(
        out,
        f,
        -w * 0.5 + 0.005,
        0.105,
        w - 0.01,
        h * 0.55 - 0.02,
        d * 0.5 - 0.02,
        (w / 0.6).round().max(2.0) as usize,
        1,
        st(c, Finish::new(0.5, 0.0, 0.0)),
        Hardware::Edge,
    );
    let th = 1.45 * (w / 1.8).clamp(0.6, 1.0);
    let _ = th;
    let tvw = (w * 0.8).min(1.45);
    let tvh = tvw * 0.5625;
    block(
        out,
        f,
        [0.0, 0.5 + tvh * 0.5, -d * 0.5 + 0.04],
        [tvw, tvh, 0.025],
        st(rgb("#18181a"), Finish::new(0.4, 0.3, 0.0)),
    );
    block(
        out,
        f,
        [0.0, 0.5 + tvh * 0.5 + 0.014, -d * 0.5 + 0.054],
        [tvw - 0.02, tvh - 0.022, 0.002],
        st(rgb("#1f2b3a"), Finish::screen(0.22)),
    );
    let mut rnd = 41u32;
    vase(
        out,
        f,
        [w * 0.5 - 0.25][0],
        h * 0.55 + 0.1,
        0.0,
        0.06,
        0.26,
        rgb("#d8cfc2"),
        &mut rnd,
    );
    block(
        out,
        f,
        [-w * 0.5 + 0.3, h * 0.55 + 0.1, 0.0],
        [0.25, 0.035, 0.18],
        st(rgb("#a9433b"), Finish::matte()),
    );
    block(
        out,
        f,
        [-w * 0.5 + 0.3, h * 0.55 + 0.135, 0.0],
        [0.22, 0.03, 0.16],
        st(rgb("#e6dccd"), Finish::matte()),
    );
}

fn b_nightstand(out: &mut Vec<MeshVertex>, f: &Frame, w: f32, d: f32, h: f32, c: [f32; 3]) {
    legs(
        out,
        f,
        w,
        d,
        0.16,
        0.05,
        0.016,
        st(rgb("#4a3a2c"), Finish::wood()),
    );
    block(
        out,
        f,
        [0.0, 0.16, 0.0],
        [w, h - 0.2, d],
        st(c, Finish::wood()),
    );
    fronts(
        out,
        f,
        -w * 0.5 + 0.02,
        0.2,
        w - 0.04,
        h - 0.26,
        d * 0.5 - 0.005,
        1,
        2,
        st(c, Finish::wood()),
        Hardware::Bar,
    );
    block(
        out,
        f,
        [0.0, h - 0.04, 0.0],
        [w + 0.02, 0.04, d + 0.02],
        st(dark(c, 0.85), Finish::wood()),
    );
    table_lamp(out, f, -w * 0.5 + 0.2, h, -d * 0.5 + 0.2, 0.9);
    block(
        out,
        f,
        [w * 0.5 - 0.24, h, 0.0],
        [0.22, 0.032, 0.16],
        st(rgb("#8d5a4a"), Finish::matte()),
    );
    block(
        out,
        f,
        [w * 0.5 - 0.23, h + 0.032, 0.0],
        [0.2, 0.028, 0.15],
        st(rgb("#e6dccd"), Finish::matte()),
    );
}

fn b_wardrobe(out: &mut Vec<MeshVertex>, f: &Frame, w: f32, d: f32, h: f32, c: [f32; 3]) {
    let hh = h.max(2.0);
    block(
        out,
        f,
        [0.0, 0.0, -0.02],
        [w - 0.02, 0.08, d - 0.04],
        st(dark(c, 0.7), Finish::wood()),
    );
    block(
        out,
        f,
        [0.0, 0.08, -0.01],
        [w, hh - 0.1, d - 0.02],
        st(c, Finish::wood()),
    );
    let n = (w / 0.5).round().max(1.0) as usize;
    fronts(
        out,
        f,
        -w * 0.5 + 0.006,
        0.09,
        w - 0.012,
        hh - 0.12,
        d * 0.5 - 0.02,
        n,
        1,
        st(c, Finish::wood()),
        Hardware::Bar,
    );
    block(
        out,
        f,
        [0.0, hh - 0.02, 0.0],
        [w + 0.01, 0.02, d],
        st(dark(c, 0.8), Finish::wood()),
    );
}

fn b_dresser(out: &mut Vec<MeshVertex>, f: &Frame, w: f32, d: f32, h: f32, c: [f32; 3]) {
    legs(
        out,
        f,
        w,
        d,
        0.14,
        0.05,
        0.016,
        st(rgb("#4a3a2c"), Finish::wood()),
    );
    block(
        out,
        f,
        [0.0, 0.14, 0.0],
        [w, h - 0.18, d],
        st(c, Finish::wood()),
    );
    fronts(
        out,
        f,
        -w * 0.5 + 0.02,
        0.16,
        w - 0.04,
        h - 0.2,
        d * 0.5 - 0.005,
        1,
        3,
        st(c, Finish::wood()),
        Hardware::Bar,
    );
    block(
        out,
        f,
        [0.0, h - 0.04, 0.0],
        [w + 0.02, 0.04, d + 0.02],
        st(dark(c, 0.85), Finish::wood()),
    );
    // round mirror
    let my = h + 0.42;
    cyl(
        out,
        f,
        [0.0, my, -d * 0.5 + 0.02],
        0.36,
        0.36,
        0.02,
        st(rgb("#dfeaee"), Finish::mirror()),
        40,
    );
    ring(
        out,
        f,
        [0.0, my, -d * 0.5 + 0.033],
        0.36,
        0.018,
        st([0.42, 0.32, 0.24], Finish::wood()),
        40,
    );
    rod(
        out,
        f,
        [0.0, h, -d * 0.5 + 0.02],
        [0.0, my - 0.2, -d * 0.5 + 0.02],
        0.01,
        0.01,
        st([0.42, 0.32, 0.24], Finish::wood()),
        8,
    );
    for (dx, colr, hh) in [
        (-0.24f32, [0.95, 0.75, 0.55], 0.2f32),
        (0.0, [0.9, 0.9, 0.86], 0.14),
        (0.22, [0.7, 0.85, 0.8], 0.18),
    ] {
        cyl(
            out,
            f,
            [dx, h, d * 0.16],
            0.03,
            0.026,
            hh,
            st(colr, Finish::new(0.1, 0.0, 0.0)),
            16,
        );
        cyl(
            out,
            f,
            [dx, h + hh, d * 0.16],
            0.012,
            0.012,
            0.02,
            st([0.3, 0.3, 0.3], Finish::matte()),
            10,
        );
    }
}

fn b_bench(out: &mut Vec<MeshVertex>, f: &Frame, w: f32, d: f32, h: f32, c: [f32; 3]) {
    legs(
        out,
        f,
        w,
        d,
        h - 0.12,
        0.07,
        0.02,
        st(rgb("#3a3027"), Finish::wood()),
    );
    block(
        out,
        f,
        [0.0, h - 0.12, 0.0],
        [w, 0.1, d],
        st(dark(c, 0.85), Finish::wood()),
    );
    block(
        out,
        f,
        [0.0, h - 0.03, 0.0],
        [w - 0.01, 0.06, d - 0.01],
        st(c, Finish::fabric()),
    );
}

fn b_crib(out: &mut Vec<MeshVertex>, f: &Frame, w: f32, d: f32, h: f32, c: [f32; 3]) {
    let wm = st(rgb("#c9b28f"), Finish::wood());
    let y0 = h * 0.45;
    block(out, f, [0.0, y0 - 0.12, 0.0], [w - 0.06, 0.1, d - 0.06], wm);
    block(
        out,
        f,
        [0.0, y0 - 0.03, 0.0],
        [w - 0.1, 0.08, d - 0.1],
        st([0.97, 0.95, 0.9], Finish::fabric()),
    );
    for (sx, sz) in [(-1.0f32, -1.0f32), (1.0, -1.0), (-1.0, 1.0), (1.0, 1.0)] {
        block(
            out,
            f,
            [sx * (w * 0.5 - 0.035), 0.0, sz * (d * 0.5 - 0.035)],
            [0.07, h, 0.07],
            wm,
        );
        blob(
            out,
            f,
            [sx * (w * 0.5 - 0.035), h, sz * (d * 0.5 - 0.035)],
            [0.045, 0.045, 0.045],
            st(rgb("#e8dcc8"), Finish::wood()),
            10,
        );
    }
    let nbar = (w / 0.078).round() as usize;
    for i in 0..=nbar {
        let x = -w * 0.5 + 0.04 + (w - 0.08) * i as f32 / nbar as f32;
        for side in [-1.0f32, 1.0] {
            rod(
                out,
                f,
                [x, y0, side * (d * 0.5 - 0.035)],
                [x, h - 0.05, side * (d * 0.5 - 0.035)],
                0.008,
                0.008,
                wm,
                6,
            );
        }
    }
    for side in [-1.0f32, 1.0] {
        block(
            out,
            f,
            [0.0, y0, side * (d * 0.5 - 0.035)],
            [w - 0.07, 0.035, 0.03],
            wm,
        );
        block(
            out,
            f,
            [0.0, h - 0.08, side * (d * 0.5 - 0.035)],
            [w - 0.07, 0.03, 0.03],
            wm,
        );
    }
    block(
        out,
        f,
        [0.0, y0 + 0.05, d * 0.14],
        [w * 0.55, 0.05, d * 0.42],
        st(light(c, 0.1), Finish::fabric()),
    );
    blob(
        out,
        f,
        [w * 0.2, y0 + 0.2, -d * 0.12],
        [0.1, 0.12, 0.08],
        st([0.72, 0.56, 0.38], Finish::fabric()),
        12,
    );
    blob(
        out,
        f,
        [w * 0.2, y0 + 0.32, -d * 0.12],
        [0.07, 0.07, 0.07],
        st([0.72, 0.56, 0.38], Finish::fabric()),
        10,
    );
}

fn b_kids_desk(out: &mut Vec<MeshVertex>, f: &Frame, w: f32, d: f32, h: f32, c: [f32; 3]) {
    legs(
        out,
        f,
        w,
        d,
        h - 0.03,
        0.05,
        0.016,
        st(rgb("#7d8ea3"), Finish::metal()),
    );
    block(
        out,
        f,
        [0.0, h - 0.03, 0.0],
        [w, 0.03, d],
        st(c, Finish::wood()),
    );
    block(
        out,
        f,
        [0.0, h - 0.06, -d * 0.5 + 0.12],
        [w - 0.06, 0.03, 0.2],
        st(dark(c, 0.86), Finish::wood()),
    );
    block(
        out,
        f,
        [0.0, h - 0.09, -d * 0.5 + 0.12],
        [w - 0.06, 0.03, 0.2],
        st(dark(c, 0.86), Finish::wood()),
    );
}

fn b_mirror_stand(out: &mut Vec<MeshVertex>, f: &Frame, w: f32, d: f32, h: f32, c: [f32; 3]) {
    let _ = c;
    block(
        out,
        f,
        [0.0, 0.0, 0.0],
        [w * 0.7, 0.04, d * 0.6],
        st([0.25, 0.23, 0.21], Finish::black_metal()),
    );
    for side in [-1.0f32, 1.0] {
        rod(
            out,
            f,
            [side * w * 0.3, 0.02, 0.0],
            [side * w * 0.36, h, 0.02],
            0.014,
            0.01,
            st([0.25, 0.23, 0.21], Finish::black_metal()),
            8,
        );
    }
    block(
        out,
        f,
        [0.0, h * 0.25, 0.02],
        [w * 0.78, h * 0.72, 0.03],
        st(rgb("#3a3632"), Finish::wood()),
    );
    block(
        out,
        f,
        [0.0, h * 0.27, 0.04],
        [w * 0.72, h * 0.66, 0.004],
        st(rgb("#dfeaee"), Finish::mirror()),
    );
}

fn b_sideboard(out: &mut Vec<MeshVertex>, f: &Frame, w: f32, d: f32, h: f32, c: [f32; 3]) {
    legs(
        out,
        f,
        w,
        d,
        0.16,
        0.06,
        0.02,
        st(rgb("#4a3a2c"), Finish::wood()),
    );
    block(
        out,
        f,
        [0.0, 0.16, -0.01],
        [w, h - 0.2, d - 0.02],
        st(c, Finish::wood()),
    );
    let n = (w / 0.55).round().max(2.0) as usize;
    fronts(
        out,
        f,
        -w * 0.5 + 0.01,
        0.17,
        w - 0.02,
        h - 0.24,
        d * 0.5 - 0.02,
        n,
        1,
        st(c, Finish::wood()),
        Hardware::Bar,
    );
    block(
        out,
        f,
        [0.0, h - 0.04, 0.0],
        [w + 0.02, 0.04, d + 0.02],
        st(dark(c, 0.85), Finish::wood()),
    );
    let mut rnd = 51u32;
    vase(
        out,
        f,
        -w * 0.3,
        h,
        0.0,
        0.07,
        0.3,
        rgb("#d8cfc2"),
        &mut rnd,
    );
    block(
        out,
        f,
        [w * 0.22, h, -d * 0.5 + 0.04],
        [0.3, 0.36, 0.025],
        st(rgb("#6d6152"), Finish::wood()),
    );
    block(
        out,
        f,
        [w * 0.22, h + 0.02, -d * 0.5 + 0.055],
        [0.26, 0.32, 0.004],
        st(rgb("#8d9a86"), Finish::matte()),
    );
}

fn b_bar_chair(out: &mut Vec<MeshVertex>, f: &Frame, w: f32, d: f32, h: f32, _c: [f32; 3]) {
    let lm = st(rgb("#2b2b2d"), Finish::black_metal());
    let r = w.min(d) * 0.5;
    for (a, b) in [(-1.0f32, -1.0f32), (1.0, -1.0), (-1.0, 1.0), (1.0, 1.0)] {
        rod(
            out,
            f,
            [a * (r + 0.1), 0.0, b * (r + 0.1)],
            [a * r * 0.6, h * 0.66, b * r * 0.6],
            0.012,
            0.012,
            lm,
            8,
        );
    }
    ring(out, f, [0.0, h * 0.3, 0.0], r * 0.85, 0.008, lm, 20);
    cyl(
        out,
        f,
        [0.0, h * 0.66, 0.0],
        r * 0.75,
        r * 0.75,
        0.06,
        lm,
        24,
    );
    lathe(
        out,
        f,
        [0.0, h * 0.72, 0.0],
        &[(r * 0.7, 0.0), (r * 0.72, 0.04), (r * 0.66, 0.09)],
        st(rgb("#8d6a4f"), Finish::fabric()),
        24,
    );
}

fn b_island(out: &mut Vec<MeshVertex>, f: &Frame, w: f32, d: f32, h: f32, c: [f32; 3]) {
    block(
        out,
        f,
        [0.0, 0.0, 0.0],
        [w - 0.06, 0.1, d - 0.06],
        st(dark(c, 0.6), Finish::wood()),
    );
    block(
        out,
        f,
        [0.0, 0.1, 0.0],
        [w, h - 0.14, d],
        st(c, Finish::new(0.45, 0.0, 0.0)),
    );
    let n = (w / 0.5).round().max(1.0) as usize;
    fronts(
        out,
        f,
        -w * 0.5 + 0.01,
        0.12,
        w - 0.02,
        h - 0.2,
        -d * 0.5 + 0.02,
        n,
        1,
        st(c, Finish::new(0.45, 0.0, 0.0)),
        Hardware::Edge,
    );
    block(
        out,
        f,
        [0.0, h - 0.04, 0.0],
        [w + 0.03, 0.04, d + 0.03],
        st([0.88, 0.86, 0.82], Finish::new(0.2, 0.0, 0.0)),
    );
    for side in [-1.0f32, 1.0] {
        block(
            out,
            f,
            [side * (w * 0.5 + 0.005), 0.0, 0.0],
            [0.03, h - 0.04, d + 0.03],
            st([0.88, 0.86, 0.82], Finish::new(0.2, 0.0, 0.0)),
        );
    }
    let mut rnd = 61u32;
    plate(out, f, 0.16, 0.0, h, 0.0);
    for k in 0..4 {
        let a = k as f32 * 1.57;
        blob(
            out,
            f,
            [a.cos() * 0.08, h + 0.06, a.sin() * 0.08],
            [0.045, 0.04, 0.045],
            st([0.85, 0.55, 0.25], Finish::new(0.7, 0.0, 0.0)),
            10,
        );
    }
    vase(
        out,
        f,
        [w * 0.5 - 0.2][0],
        h,
        0.0,
        0.07,
        0.24,
        rgb("#cfd6cc"),
        &mut rnd,
    );
}

fn b_counter(out: &mut Vec<MeshVertex>, f: &Frame, w: f32, d: f32, h: f32, c: [f32; 3]) {
    let _ = h;
    block(
        out,
        f,
        [0.0, 0.0, 0.0],
        [w - 0.04, 0.1, d - 0.06],
        st(rgb("#2c2620"), Finish::matte()),
    );
    block(
        out,
        f,
        [0.0, 0.1, 0.0],
        [w, 0.78, d],
        st(c, Finish::new(0.5, 0.0, 0.0)),
    );
    let n = (w / 0.5).round().max(1.0) as usize;
    fronts(
        out,
        f,
        -w * 0.5 + 0.01,
        0.11,
        w - 0.02,
        0.76,
        d * 0.5 - 0.02,
        n,
        1,
        st(c, Finish::new(0.5, 0.0, 0.0)),
        Hardware::Bar,
    );
    block(
        out,
        f,
        [0.0, 0.88, 0.0],
        [w + 0.03, 0.04, d + 0.04],
        st([0.86, 0.85, 0.82], Finish::new(0.25, 0.0, 0.0)),
    );
    block(
        out,
        f,
        [0.0, 0.92, -d * 0.5 - 0.02],
        [w + 0.03, 0.1, 0.03],
        st([0.86, 0.85, 0.82], Finish::new(0.25, 0.0, 0.0)),
    );
    block(
        out,
        f,
        [0.0, 1.48, -d * 0.5],
        [w - 0.04, 0.7, d * 0.45],
        st(c, Finish::new(0.5, 0.0, 0.0)),
    );
    fronts(
        out,
        f,
        -w * 0.5 + 0.03,
        1.49,
        w - 0.06,
        0.66,
        -d * 0.5 + d * 0.225,
        n,
        1,
        st(c, Finish::new(0.5, 0.0, 0.0)),
        Hardware::Edge,
    );
    block(
        out,
        f,
        [0.0, 1.46, -d * 0.5],
        [w - 0.1, 0.012, 0.04],
        st([1.0, 0.95, 0.82], Finish::glow(0.5)),
    );
    block(
        out,
        f,
        [0.0, 0.92, d * 0.16],
        [0.4, 0.015, 0.26],
        st(rgb("#c9a06a"), Finish::wood()),
    );
    for k in 0..3 {
        cyl(
            out,
            f,
            [-w * 0.2 + k as f32 * 0.16, 0.935, d * 0.34],
            0.055,
            0.05,
            0.16,
            st([0.9, 0.88, 0.82], Finish::new(0.34, 0.0, 0.0)),
            16,
        );
        cyl(
            out,
            f,
            [-w * 0.2 + k as f32 * 0.16, 1.095, d * 0.34],
            0.052,
            0.052,
            0.015,
            st([0.55, 0.42, 0.3], Finish::wood()),
            16,
        );
    }
    cyl(
        out,
        f,
        [w * 0.3, 0.935, d * 0.3],
        0.09,
        0.075,
        0.12,
        st([0.93, 0.92, 0.9], Finish::ceramic()),
        20,
    );
}

fn b_sink_cab(out: &mut Vec<MeshVertex>, f: &Frame, w: f32, d: f32, _h: f32, c: [f32; 3]) {
    block(
        out,
        f,
        [0.0, 0.0, 0.0],
        [w - 0.04, 0.1, d - 0.06],
        st(rgb("#2c2620"), Finish::matte()),
    );
    block(
        out,
        f,
        [0.0, 0.1, 0.0],
        [w, 0.78, d],
        st(c, Finish::new(0.5, 0.0, 0.0)),
    );
    fronts(
        out,
        f,
        -w * 0.5 + 0.01,
        0.11,
        w - 0.02,
        0.76,
        d * 0.5 - 0.02,
        2,
        1,
        st(c, Finish::new(0.5, 0.0, 0.0)),
        Hardware::Edge,
    );
    block(
        out,
        f,
        [0.0, 0.88, 0.0],
        [w + 0.03, 0.04, d + 0.04],
        st([0.88, 0.87, 0.84], Finish::new(0.25, 0.0, 0.0)),
    );
    let sw = (w - 0.3).min(w * 0.7);
    block(
        out,
        f,
        [0.0, 0.85, 0.0],
        [sw, 0.03, d * 0.6],
        st([0.7, 0.72, 0.73], Finish::metal()),
    );
    block(
        out,
        f,
        [0.0, 0.85, 0.0],
        [sw - 0.06, 0.032, d * 0.6 - 0.06],
        st([0.62, 0.64, 0.65], Finish::metal()),
    );
    for (dx, dz, sx, sz) in [
        (sw * 0.5, 0.0, 0.03, d * 0.6),
        (-sw * 0.5, 0.0, 0.03, d * 0.6),
        (0.0, d * 0.3, sw, 0.03),
        (0.0, -d * 0.3, sw, 0.03),
    ] {
        block(
            out,
            f,
            [dx, 0.85, dz],
            [sx, 0.035, sz],
            st([0.75, 0.77, 0.78], Finish::metal()),
        );
    }
    let fy = 0.92;
    cyl(
        out,
        f,
        [0.0, fy, -d * 0.22],
        0.035,
        0.03,
        0.03,
        st([0.85, 0.86, 0.87], Finish::chrome()),
        16,
    );
    rod(
        out,
        f,
        [0.0, fy, -d * 0.22],
        [0.0, fy + 0.34, -d * 0.22],
        0.018,
        0.018,
        st([0.85, 0.86, 0.87], Finish::chrome()),
        10,
    );
    rod(
        out,
        f,
        [0.0, fy + 0.34, -d * 0.22],
        [0.0, fy + 0.34, -d * 0.22 + 0.2],
        0.018,
        0.015,
        st([0.85, 0.86, 0.87], Finish::chrome()),
        10,
    );
}

fn b_stove(out: &mut Vec<MeshVertex>, f: &Frame, w: f32, d: f32, _h: f32, c: [f32; 3]) {
    block(
        out,
        f,
        [0.0, 0.0, 0.0],
        [w, 0.86, d],
        st(c, Finish::new(0.4, 0.3, 0.0)),
    );
    block(
        out,
        f,
        [0.0, 0.86, d * 0.16],
        [w, 0.03, d * 0.66],
        st(rgb("#0e0f11"), Finish::new(0.06, 0.2, 0.0)),
    );
    block(
        out,
        f,
        [0.0, 0.89, -d * 0.5 + 0.06],
        [w, 0.5, 0.08],
        st([0.62, 0.62, 0.6], Finish::metal()),
    );
    let br = (w / 4.0).min(0.16);
    for i in 0..4 {
        let gx = -w * 0.5 + w / 4.0 * (i as f32 + 0.5);
        let gz = d * 0.16;
        ring(
            out,
            f,
            [gx, 0.895, gz],
            br * 0.6,
            0.008,
            st([0.22, 0.22, 0.2], Finish::black_metal()),
            24,
        );
        cyl(
            out,
            f,
            [gx, 0.892, gz],
            br * 0.4,
            br * 0.4,
            0.006,
            st([0.35, 0.25, 0.15], Finish::metal()),
            20,
        );
        cyl(
            out,
            f,
            [gx, 0.892, gz],
            br * 0.14,
            br * 0.14,
            0.008,
            st([0.1, 0.1, 0.1], Finish::black_metal()),
            12,
        );
    }
    for i in 0..4 {
        knob(
            out,
            f,
            -w * 0.5 + 0.1 + i as f32 * 0.1,
            0.9,
            d * 0.5 - 0.06,
            st([0.2, 0.2, 0.2], Finish::black_metal()),
        );
    }
}

fn b_toilet(out: &mut Vec<MeshVertex>, f: &Frame, w: f32, d: f32, h: f32, _c: [f32; 3]) {
    let pm = st([0.97, 0.97, 0.95], Finish::ceramic());
    let tank_h = (h - 0.45).clamp(0.45, 0.95);
    block(out, f, [0.0, 0.0, -d * 0.5 + 0.1], [w, tank_h, 0.18], pm);
    block(
        out,
        f,
        [0.0, tank_h, -d * 0.5 + 0.1],
        [w * 0.9, 0.02, 0.16],
        pm,
    );
    block(
        out,
        f,
        [0.0, 0.78, -d * 0.5 + 0.1],
        [w * 0.9, 0.02, 0.16],
        pm,
    );
    let pts = [
        (0.0, 0.0),
        (w * 0.28, 0.0),
        (w * 0.34, 0.18),
        (w * 0.32, 0.34),
        (w * 0.24, 0.4),
    ];
    lathe(out, f, [0.0, 0.0, d * 0.04], &pts, pm, 28);
    cyl(
        out,
        f,
        [0.0, 0.4, d * 0.06],
        w * 0.34,
        w * 0.3,
        0.05,
        pm,
        28,
    );
    cyl(
        out,
        f,
        [0.0, 0.45, d * 0.06],
        w * 0.3,
        w * 0.28,
        0.03,
        pm,
        28,
    );
    knob(
        out,
        f,
        0.0,
        0.8,
        -d * 0.5 + 0.08,
        st([0.7, 0.7, 0.7], Finish::metal()),
    );
}

fn b_basin(out: &mut Vec<MeshVertex>, f: &Frame, w: f32, d: f32, h: f32, c: [f32; 3]) {
    block(
        out,
        f,
        [0.0, 0.0, 0.0],
        [w, 0.42, d],
        st(c, Finish::new(0.5, 0.0, 0.0)),
    );
    fronts(
        out,
        f,
        -w * 0.5 + 0.01,
        0.02,
        w - 0.02,
        0.38,
        d * 0.5 - 0.02,
        2,
        1,
        st(c, Finish::new(0.5, 0.0, 0.0)),
        Hardware::Edge,
    );
    block(
        out,
        f,
        [0.0, 0.42, 0.0],
        [w + 0.02, 0.04, d + 0.02],
        st([0.9, 0.89, 0.86], Finish::new(0.25, 0.0, 0.0)),
    );
    let r = w * 0.3;
    let pts = [
        (0.0, 0.0),
        (r * 0.9, 0.0),
        (r, 0.02),
        (r * 0.85, 0.12),
        (r * 0.55, 0.12),
        (r * 0.5, 0.0),
    ];
    lathe(
        out,
        f,
        [0.0, 0.46, 0.0],
        &pts,
        st([0.99, 0.99, 0.98], Finish::ceramic()),
        28,
    );
    let fy = 0.52;
    cyl(
        out,
        f,
        [0.0, fy, -d * 0.3],
        0.03,
        0.028,
        0.03,
        st([0.85, 0.86, 0.87], Finish::chrome()),
        16,
    );
    rod(
        out,
        f,
        [0.0, fy, -d * 0.3],
        [0.0, fy + 0.24, -d * 0.3],
        0.016,
        0.016,
        st([0.85, 0.86, 0.87], Finish::chrome()),
        10,
    );
    rod(
        out,
        f,
        [0.0, fy + 0.24, -d * 0.3],
        [0.0, fy + 0.24, -d * 0.3 + 0.14],
        0.016,
        0.012,
        st([0.85, 0.86, 0.87], Finish::chrome()),
        10,
    );
    block(
        out,
        f,
        [0.0, h - 0.72, -d * 0.5 + 0.03],
        [w * 0.86, 0.7, 0.03],
        st(rgb("#3a3632"), Finish::wood()),
    );
    block(
        out,
        f,
        [0.0, h - 0.7, -d * 0.5 + 0.045],
        [w * 0.8, 0.66, 0.004],
        st(rgb("#dfeaee"), Finish::mirror()),
    );
    block(
        out,
        f,
        [0.0, h - 0.74, -d * 0.5 + 0.03],
        [w * 0.9, 0.02, 0.03],
        st([1.0, 0.95, 0.82], Finish::glow(0.4)),
    );
}

fn b_bathtub(out: &mut Vec<MeshVertex>, f: &Frame, w: f32, d: f32, h: f32, _c: [f32; 3]) {
    let shell = st([0.97, 0.97, 0.95], Finish::ceramic());
    block(out, f, [0.0, 0.0, 0.0], [w, 0.1, d], shell);
    block(
        out,
        f,
        [0.0, 0.1, d * 0.5 - 0.04],
        [w, h - 0.1, 0.08],
        shell,
    );
    block(
        out,
        f,
        [0.0, 0.1, -d * 0.5 + 0.04],
        [w, h - 0.1, 0.08],
        shell,
    );
    for side in [-1.0f32, 1.0] {
        block(
            out,
            f,
            [side * (w * 0.5 - 0.04), 0.1, 0.0],
            [0.08, h - 0.1, d - 0.1],
            shell,
        );
    }
    block(
        out,
        f,
        [0.0, h - 0.03, 0.0],
        [w - 0.14, 0.03, d - 0.14],
        sg([0.84, 0.94, 0.96], Finish::new(0.04, 0.0, 0.1), 0.55),
    );
    cyl(
        out,
        f,
        [-w * 0.5 + 0.12, h, -d * 0.5 + 0.08],
        0.03,
        0.026,
        0.16,
        st([0.85, 0.86, 0.87], Finish::chrome()),
        14,
    );
    rod(
        out,
        f,
        [-w * 0.5 + 0.12, h + 0.16, -d * 0.5 + 0.08],
        [-w * 0.5 + 0.3, h + 0.16, -d * 0.5 + 0.08],
        0.016,
        0.014,
        st([0.85, 0.86, 0.87], Finish::chrome()),
        10,
    );
    for side in [-1.0f32, 1.0] {
        knob(
            out,
            f,
            side * 0.12,
            h - 0.02,
            -d * 0.5 + 0.1,
            st([0.8, 0.8, 0.8], Finish::chrome()),
        );
    }
    blob(
        out,
        f,
        [0.0, h + 0.04, -d * 0.5 + 0.2],
        [0.22, 0.05, 0.12],
        st([0.92, 0.9, 0.84], Finish::fabric()),
        12,
    );
}

fn b_shower(out: &mut Vec<MeshVertex>, f: &Frame, w: f32, d: f32, h: f32, _c: [f32; 3]) {
    block(
        out,
        f,
        [0.0, 0.0, 0.0],
        [w, 0.06, d],
        st([0.9, 0.9, 0.88], Finish::ceramic()),
    );
    block(
        out,
        f,
        [0.0, 0.06, d * 0.3],
        [w * 0.6, 0.008, 0.12],
        st([0.4, 0.4, 0.4], Finish::metal()),
    );
    let gm = sg([0.80, 0.90, 0.93], Finish::new(0.04, 0.0, 0.0), 0.22);
    block(out, f, [w * 0.5 - 0.02, 0.06, 0.0], [0.012, h - 0.1, d], gm);
    block(
        out,
        f,
        [0.0, 0.06, d * 0.5 - 0.02],
        [w * 0.55, h - 0.1, 0.012],
        gm,
    );
    for side in [-1.0f32, 1.0] {
        block(
            out,
            f,
            [side * (w * 0.5 - 0.02), 0.06, 0.0],
            [0.012, 0.012, d],
            st([0.7, 0.72, 0.73], Finish::metal()),
        );
    }
    rod(
        out,
        f,
        [-w * 0.4, 0.06, -d * 0.3],
        [-w * 0.4, h - 0.02, -d * 0.3],
        0.018,
        0.016,
        st([0.85, 0.86, 0.87], Finish::chrome()),
        10,
    );
    rod(
        out,
        f,
        [-w * 0.4, h - 0.02, -d * 0.3],
        [0.0, h - 0.02, -d * 0.1],
        0.016,
        0.014,
        st([0.85, 0.86, 0.87], Finish::chrome()),
        10,
    );
    block(
        out,
        f,
        [0.0, h - 0.035, -d * 0.1],
        [0.24, 0.03, 0.2],
        st([0.8, 0.81, 0.82], Finish::chrome()),
    );
    cyl(
        out,
        f,
        [-w * 0.4, h * 0.45, -d * 0.36],
        0.04,
        0.035,
        0.18,
        st([0.85, 0.86, 0.87], Finish::chrome()),
        12,
    );
}

fn b_towel_rack(out: &mut Vec<MeshVertex>, f: &Frame, w: f32, d: f32, h: f32, c: [f32; 3]) {
    let _ = d;
    block(
        out,
        f,
        [0.0, h - 0.02, 0.0],
        [w, 0.02, 0.06],
        st([0.3, 0.3, 0.3], Finish::black_metal()),
    );
    for side in [-1.0f32, 1.0] {
        rod(
            out,
            f,
            [side * (w * 0.5 - 0.1), h - 0.02, 0.0],
            [side * (w * 0.5 - 0.1), 0.5, 0.0],
            0.012,
            0.012,
            st([0.3, 0.3, 0.3], Finish::black_metal()),
            8,
        );
    }
    block(
        out,
        f,
        [0.0, 0.55, 0.0],
        [w - 0.22, 0.5, 0.02],
        st(c, Finish::fabric()),
    );
    block(
        out,
        f,
        [0.0, 0.95, 0.03],
        [w - 0.26, 0.12, 0.02],
        st(light(c, 0.1), Finish::fabric()),
    );
}

fn b_mirror_cab(out: &mut Vec<MeshVertex>, f: &Frame, w: f32, d: f32, h: f32, c: [f32; 3]) {
    block(
        out,
        f,
        [0.0, h * 0.35, 0.0],
        [w, h * 0.65, d],
        st(c, Finish::wood()),
    );
    fronts(
        out,
        f,
        -w * 0.5 + 0.01,
        h * 0.36,
        w - 0.02,
        h * 0.6,
        d * 0.5 - 0.02,
        2,
        1,
        st(c, Finish::wood()),
        Hardware::Edge,
    );
    block(
        out,
        f,
        [0.0, h * 0.4, d * 0.5 + 0.008],
        [w - 0.06, h * 0.5, 0.006],
        st(rgb("#dfeaee"), Finish::mirror()),
    );
    block(
        out,
        f,
        [0.0, h * 0.36, d * 0.5 + 0.004],
        [w - 0.02, 0.02, 0.02],
        st([1.0, 0.95, 0.82], Finish::glow(0.4)),
    );
}

fn b_bath_mat(out: &mut Vec<MeshVertex>, f: &Frame, w: f32, d: f32, _h: f32, c: [f32; 3]) {
    block(
        out,
        f,
        [0.0, 0.002, 0.0],
        [w, 0.012, d],
        st(c, Finish::fabric()),
    );
    block(
        out,
        f,
        [0.0, 0.003, 0.0],
        [w - 0.08, 0.012, d - 0.06],
        st(light(c, 0.15), Finish::fabric()),
    );
}

fn b_urinal(out: &mut Vec<MeshVertex>, f: &Frame, w: f32, d: f32, h: f32, _c: [f32; 3]) {
    let pm = st([0.97, 0.97, 0.95], Finish::ceramic());
    block(out, f, [0.0, 0.0, -d * 0.5 + 0.05], [w * 0.8, h, 0.08], pm);
    let r = w * 0.45;
    lathe(
        out,
        f,
        [0.0, h * 0.35, 0.02],
        &[
            (0.0, 0.0),
            (r * 0.5, 0.02),
            (r, h * 0.3),
            (r * 0.8, h * 0.6),
            (r * 0.4, h * 0.62),
        ],
        pm,
        20,
    );
    cyl(
        out,
        f,
        [0.0, h * 0.95, -d * 0.5 + 0.1],
        0.03,
        0.028,
        0.1,
        st([0.85, 0.86, 0.87], Finish::chrome()),
        12,
    );
}

fn b_laundry(out: &mut Vec<MeshVertex>, f: &Frame, w: f32, d: f32, h: f32, c: [f32; 3]) {
    let r = w.min(d) * 0.5;
    lathe(
        out,
        f,
        [0.0, 0.0, 0.0],
        &[(0.0, 0.0), (r * 0.8, 0.0), (r, h * 0.6), (r, h)],
        st(c, Finish::fabric()),
        24,
    );
    for k in 0..3 {
        blob(
            out,
            f,
            [(k as f32 - 1.0) * 0.12, h + 0.03, 0.0],
            [0.14, 0.06, 0.12],
            st([0.85, 0.88, 0.9], Finish::fabric()),
            10,
        );
    }
}

fn b_fridge(
    out: &mut Vec<MeshVertex>,
    f: &Frame,
    w: f32,
    d: f32,
    h: f32,
    c: [f32; 3],
    double: bool,
) {
    block(
        out,
        f,
        [0.0, 0.0, 0.0],
        [w - 0.04, 0.06, d - 0.03],
        st(rgb("#222"), Finish::matte()),
    );
    block(
        out,
        f,
        [0.0, 0.06, 0.0],
        [w, h - 0.08, d],
        st(c, Finish::new(0.22, 0.35, 0.0)),
    );
    let split = h * 0.62;
    let n = if double { 2 } else { 1 };
    for i in 0..n {
        let pw = w / n as f32;
        let cx = -w * 0.5 + pw * (i as f32 + 0.5);
        block(
            out,
            f,
            [cx - pw * 0.5 + 0.008, 0.07, d * 0.5 + 0.005],
            [pw - 0.016, split - 0.1, 0.02],
            st(light(c, 0.06), Finish::new(0.18, 0.4, 0.0)),
        );
        block(
            out,
            f,
            [cx - pw * 0.5 + 0.008, split + 0.02, d * 0.5 + 0.005],
            [pw - 0.016, h - split - 0.1, 0.02],
            st(light(c, 0.06), Finish::new(0.18, 0.4, 0.0)),
        );
        pull(
            out,
            f,
            (split * 0.3).min(0.4),
            true,
            cx + (if double {
                if i == 0 {
                    pw * 0.5 - 0.06
                } else {
                    -pw * 0.5 + 0.06
                }
            } else {
                pw * 0.5 - 0.06
            }),
            split * 0.6,
            d * 0.5 + 0.02,
            st([0.72, 0.72, 0.72], Finish::chrome()),
        );
    }
    block(
        out,
        f,
        [0.0, split - 0.03, d * 0.5 + 0.005],
        [w, 0.03, 0.02],
        st(dark(c, 0.7), Finish::new(0.4, 0.2, 0.0)),
    );
    if !double {
        block(
            out,
            f,
            [w * 0.22, split + 0.18, d * 0.5 + 0.02],
            [w * 0.28, 0.16, 0.004],
            st(rgb("#25384c"), Finish::screen(0.12)),
        );
    }
}

fn b_washer(out: &mut Vec<MeshVertex>, f: &Frame, w: f32, d: f32, h: f32, c: [f32; 3]) {
    block(
        out,
        f,
        [0.0, 0.0, 0.0],
        [w - 0.02, 0.05, d - 0.02],
        st(rgb("#3a3a3c"), Finish::matte()),
    );
    block(
        out,
        f,
        [0.0, 0.05, 0.0],
        [w, h - 0.07, d],
        st(c, Finish::new(0.3, 0.2, 0.0)),
    );
    block(
        out,
        f,
        [0.0, h - 0.24, d * 0.5 + 0.005],
        [w - 0.04, 0.16, 0.02],
        st(light(c, 0.4), Finish::new(0.12, 0.0, 0.0)),
    );
    block(
        out,
        f,
        [w * 0.16, h - 0.21, d * 0.5 + 0.02],
        [w * 0.24, 0.08, 0.004],
        st(rgb("#1f2b3a"), Finish::screen(0.12)),
    );
    knob(
        out,
        f,
        w * 0.4,
        h - 0.28,
        d * 0.5 + 0.02,
        st([0.85, 0.86, 0.87], Finish::chrome()),
    );
    let r = (w * 0.32).min(h * 0.26);
    let cy = 0.05 + (h - 0.3) * 0.5;
    cyl(
        out,
        f,
        [0.0, cy, d * 0.5 + 0.002],
        r + 0.02,
        r + 0.02,
        0.008,
        st(light(c, 0.5), Finish::metal()),
        32,
    );
    cyl(
        out,
        f,
        [0.0, cy, d * 0.5 + 0.008],
        r,
        r,
        0.004,
        st([0.12, 0.14, 0.16], Finish::new(0.05, 0.6, 0.0)),
        32,
    );
    cyl(
        out,
        f,
        [0.0, cy, d * 0.5 + 0.011],
        r * 0.75,
        r * 0.75,
        0.003,
        st([0.4, 0.52, 0.6], Finish::new(0.05, 0.2, 0.0)),
        28,
    );
}

fn b_dishwasher(out: &mut Vec<MeshVertex>, f: &Frame, w: f32, d: f32, h: f32, c: [f32; 3]) {
    block(
        out,
        f,
        [0.0, 0.0, 0.0],
        [w - 0.06, 0.06, d - 0.04],
        st(rgb("#2a2a2c"), Finish::matte()),
    );
    block(
        out,
        f,
        [0.0, 0.06, 0.0],
        [w, h - 0.1, d],
        st(c, Finish::new(0.25, 0.4, 0.0)),
    );
    fronts(
        out,
        f,
        -w * 0.5 + 0.008,
        0.08,
        w - 0.016,
        h - 0.18,
        d * 0.5 - 0.01,
        1,
        1,
        st(light(c, 0.08), Finish::new(0.3, 0.2, 0.0)),
        Hardware::Bar,
    );
    block(
        out,
        f,
        [0.0, h - 0.16, d * 0.5 - 0.02],
        [w - 0.04, 0.05, 0.004],
        st([0.1, 0.1, 0.11], Finish::matte()),
    );
    for i in 0..4 {
        blob(
            out,
            f,
            [w * 0.3 - i as f32 * 0.06, h - 0.135, d * 0.5 - 0.018],
            [0.006, 0.006, 0.002],
            st([0.4, 0.85, 0.9], Finish::glow(0.7)),
            8,
        );
    }
}

fn b_microwave(out: &mut Vec<MeshVertex>, f: &Frame, w: f32, d: f32, h: f32, c: [f32; 3]) {
    block(
        out,
        f,
        [0.0, 0.0, 0.0],
        [w, h, d],
        st(c, Finish::new(0.3, 0.35, 0.0)),
    );
    block(
        out,
        f,
        [-w * 0.12, 0.03, d * 0.5 + 0.002],
        [w * 0.62, h - 0.06, 0.004],
        st(rgb("#17191c"), Finish::new(0.05, 0.1, 0.0)),
    );
    block(
        out,
        f,
        [-w * 0.12, 0.03, d * 0.5 + 0.006],
        [w * 0.56, h - 0.09, 0.004],
        st(rgb("#252b31"), Finish::new(0.08, 0.2, 0.0)),
    );
    for i in 0..4 {
        block(
            out,
            f,
            [w * 0.28, 0.06 + i as f32 * 0.05, d * 0.5 + 0.006],
            [w * 0.2, 0.028, 0.008],
            st([0.6, 0.6, 0.6], Finish::new(0.3, 0.2, 0.0)),
        );
    }
    block(
        out,
        f,
        [w * 0.28, 0.12, d * 0.5 + 0.004],
        [w * 0.24, 0.06, 0.004],
        st(rgb("#1f2b3a"), Finish::screen(0.1)),
    );
}

fn b_dispenser(out: &mut Vec<MeshVertex>, f: &Frame, w: f32, d: f32, h: f32, c: [f32; 3]) {
    block(
        out,
        f,
        [0.0, 0.0, 0.0],
        [w, h * 0.55, d],
        st(c, Finish::new(0.35, 0.2, 0.0)),
    );
    block(
        out,
        f,
        [0.0, h * 0.55, d * 0.3],
        [w * 0.8, 0.06, d * 0.4],
        st(dark(c, 0.85), Finish::new(0.35, 0.2, 0.0)),
    );
    let r = w * 0.4;
    cyl(
        out,
        f,
        [0.0, h * 0.55 + 0.06, 0.0],
        r,
        r,
        h * 0.32,
        st([0.62, 0.78, 0.88], Finish::new(0.1, 0.0, 0.0)),
        24,
    );
    cyl(
        out,
        f,
        [0.0, h * 0.55 + 0.06 + h * 0.32, 0.0],
        r,
        r * 0.7,
        0.05,
        st([0.62, 0.78, 0.88], Finish::new(0.1, 0.0, 0.0)),
        24,
    );
    knob(
        out,
        f,
        0.0,
        h * 0.42,
        d * 0.5 + 0.02,
        st([0.85, 0.86, 0.87], Finish::chrome()),
    );
}

fn b_purifier(out: &mut Vec<MeshVertex>, f: &Frame, w: f32, d: f32, h: f32, c: [f32; 3]) {
    let r = w.min(d) * 0.45;
    cyl(
        out,
        f,
        [0.0, 0.0, 0.0],
        r,
        r,
        h - 0.06,
        st(c, Finish::new(0.4, 0.0, 0.0)),
        28,
    );
    cyl(
        out,
        f,
        [0.0, h - 0.06, 0.0],
        r * 1.02,
        r * 0.85,
        0.06,
        st(light(c, 0.1), Finish::new(0.4, 0.0, 0.0)),
        28,
    );
    for i in 0..7 {
        let a = i as f32 / 7.0 * TAU;
        rod(
            out,
            f,
            [a.cos() * r, 0.02, a.sin() * r],
            [a.cos() * r, h - 0.1, a.sin() * r],
            0.004,
            0.004,
            st(dark(c, 0.7), Finish::matte()),
            4,
        );
    }
    ring(
        out,
        f,
        [0.0, h - 0.1, 0.0],
        r * 0.9,
        0.012,
        st([0.55, 0.75, 0.95], Finish::glow(0.6)),
        24,
    );
    block(
        out,
        f,
        [0.0, h * 0.2, r * 0.9],
        [w * 0.3, 0.1, 0.004],
        st([0.2, 0.2, 0.2], Finish::screen(0.1)),
    );
}

fn b_heater(out: &mut Vec<MeshVertex>, f: &Frame, w: f32, d: f32, h: f32, c: [f32; 3]) {
    let _ = h;
    block(
        out,
        f,
        [0.0, 0.0, 0.0],
        [w, 0.08, d],
        st(dark(c, 0.8), Finish::matte()),
    );
    let n = (w / 0.09).round().max(3.0) as usize;
    for i in 0..n {
        let x = -w * 0.5 + w / n as f32 * (i as f32 + 0.5);
        block(
            out,
            f,
            [x, 0.08, 0.0],
            [w / n as f32 - 0.012, 0.6, d * 0.7],
            st(c, Finish::new(0.5, 0.2, 0.0)),
        );
    }
    block(
        out,
        f,
        [0.0, 0.68, 0.0],
        [w, 0.03, d * 0.8],
        st(light(c, 0.2), Finish::matte()),
    );
}

fn b_hood(out: &mut Vec<MeshVertex>, f: &Frame, w: f32, d: f32, h: f32, _c: [f32; 3]) {
    let lm = st([0.78, 0.78, 0.76], Finish::new(0.3, 0.6, 0.0));
    block(out, f, [0.0, h - 0.18, 0.0], [w, 0.18, d], lm);
    block(
        out,
        f,
        [0.0, h, 0.0],
        [w * 0.28, 0.7, d * 0.5],
        st([0.72, 0.72, 0.7], Finish::metal()),
    );
    block(
        out,
        f,
        [0.0, h - 0.2, d * 0.5 - 0.02],
        [w - 0.06, 0.02, 0.02],
        st([1.0, 0.94, 0.8], Finish::glow(0.5)),
    );
    block(
        out,
        f,
        [0.0, h - 0.22, 0.0],
        [w * 0.8, 0.04, d * 0.6],
        st([0.3, 0.3, 0.3], Finish::black_metal()),
    );
}

fn b_ac(out: &mut Vec<MeshVertex>, f: &Frame, w: f32, d: f32, h: f32, c: [f32; 3]) {
    block(
        out,
        f,
        [0.0, 0.0, 0.0],
        [w - 0.03, 0.08, d - 0.03],
        st([0.2, 0.2, 0.2], Finish::matte()),
    );
    block(
        out,
        f,
        [0.0, 0.08, 0.0],
        [w, h - 0.1, d],
        st(c, Finish::new(0.3, 0.1, 0.0)),
    );
    block(
        out,
        f,
        [0.0, h * 0.3, d * 0.5 + 0.004],
        [w - 0.06, h * 0.55, 0.008],
        st(light(c, 0.3), Finish::new(0.15, 0.0, 0.0)),
    );
    block(
        out,
        f,
        [0.0, h * 0.6, d * 0.5 + 0.01],
        [w - 0.1, h * 0.18, 0.006],
        st(dark(c, 0.75), Finish::matte()),
    );
    for i in 0..8 {
        block(
            out,
            f,
            [0.0, h * 0.61 + i as f32 * h * 0.017, d * 0.5 + 0.013],
            [w - 0.12, 0.008, 0.004],
            st(dark(c, 0.5), Finish::matte()),
        );
    }
    blob(
        out,
        f,
        [w * 0.3, h * 0.35, d * 0.5 + 0.012],
        [0.008, 0.008, 0.003],
        st([0.4, 0.85, 0.6], Finish::glow(0.6)),
        8,
    );
    block(
        out,
        f,
        [0.0, 0.1, -d * 0.5],
        [w - 0.1, 0.08, 0.02],
        st(dark(c, 0.6), Finish::matte()),
    );
}

fn b_desk(out: &mut Vec<MeshVertex>, f: &Frame, w: f32, d: f32, h: f32, c: [f32; 3]) {
    let lm = st(rgb("#2c2c2e"), Finish::black_metal());
    block(
        out,
        f,
        [0.0, h - 0.03, 0.0],
        [w, 0.03, d],
        st(c, Finish::wood()),
    );
    for side in [-1.0f32, 1.0] {
        let x = side * (w * 0.5 - 0.08);
        block(
            out,
            f,
            [x, 0.0, -d * 0.5 + 0.05],
            [0.04, h - 0.03, 0.04],
            lm,
        );
        block(out, f, [x, 0.0, d * 0.5 - 0.05], [0.04, h - 0.03, 0.04], lm);
        block(out, f, [x, h - 0.05, 0.0], [0.04, 0.02, d - 0.06], lm);
    }
    block(
        out,
        f,
        [0.0, h - 0.14, -d * 0.5 + 0.1],
        [w * 0.5, 0.1, 0.02],
        st(dark(c, 0.8), Finish::wood()),
    );
    block(
        out,
        f,
        [0.0, h - 0.3, -d * 0.5 + 0.12],
        [w * 0.6, 0.14, d * 0.5],
        st(dark(c, 0.86), Finish::wood()),
    );
    block(
        out,
        f,
        [0.0, h - 0.16, -d * 0.5 + 0.12],
        [w * 0.6 - 0.02, 0.1, 0.015],
        st(dark(c, 0.92), Finish::wood()),
    );
    // monitor
    let mw = (w * 0.45).min(0.6);
    block(out, f, [0.0, h, -d * 0.5 + 0.16], [0.22, 0.02, 0.16], lm);
    rod(
        out,
        f,
        [0.0, h + 0.02, -d * 0.5 + 0.16],
        [0.0, h + 0.28, -d * 0.5 + 0.16],
        0.02,
        0.02,
        lm,
        8,
    );
    block(
        out,
        f,
        [0.0, h + 0.24, -d * 0.5 + 0.18],
        [mw, mw * 0.58, 0.025],
        st(rgb("#17171a"), Finish::new(0.3, 0.3, 0.0)),
    );
    block(
        out,
        f,
        [0.0, h + 0.25, -d * 0.5 + 0.196],
        [mw - 0.02, mw * 0.56, 0.002],
        st(rgb("#1d2a3a"), Finish::screen(0.2)),
    );
    block(
        out,
        f,
        [0.0, h, d * 0.12],
        [0.44, 0.015, 0.15],
        st(rgb("#26262a"), Finish::matte()),
    );
    block(
        out,
        f,
        [0.0, h + 0.015, d * 0.12],
        [0.4, 0.005, 0.13],
        st(rgb("#3a3a40"), Finish::matte()),
    );
    blob(
        out,
        f,
        [w * 0.28, h, d * 0.1],
        [0.04, 0.02, 0.06],
        st([0.2, 0.2, 0.2], Finish::matte()),
        10,
    );
    cyl(
        out,
        f,
        [w * 0.42, h, d * 0.28],
        0.035,
        0.032,
        0.1,
        st([0.92, 0.92, 0.9], Finish::ceramic()),
        16,
    );
    table_lamp(out, f, -w * 0.5 + 0.2, h, -d * 0.5 + 0.14, 0.85);
}

fn b_office_chair(out: &mut Vec<MeshVertex>, f: &Frame, w: f32, d: f32, h: f32, c: [f32; 3]) {
    let lm = st(rgb("#2b2b2d"), Finish::black_metal());
    let r = w.min(d) * 0.55;
    for k in 0..5 {
        let a = k as f32 / 5.0 * TAU;
        rod(
            out,
            f,
            [0.0, 0.06, 0.0],
            [a.cos() * r, 0.02, a.sin() * r * 0.9],
            0.02,
            0.012,
            lm,
            8,
        );
        blob(
            out,
            f,
            [a.cos() * r, 0.0, a.sin() * r * 0.9],
            [0.025, 0.02, 0.025],
            st([0.15, 0.15, 0.15], Finish::matte()),
            8,
        );
    }
    rod(
        out,
        f,
        [0.0, 0.05, 0.0],
        [0.0, h * 0.5, 0.0],
        0.03,
        0.026,
        lm,
        12,
    );
    block(out, f, [0.0, h * 0.5, 0.0], [w * 0.75, 0.06, d * 0.6], lm);
    block(
        out,
        f,
        [0.0, h * 0.56, 0.02],
        [w * 0.8, 0.09, d * 0.58],
        st(c, Finish::fabric()),
    );
    block(
        out,
        f,
        [0.0, h * 0.6, -d * 0.5 + 0.08],
        [w * 0.86, h * 0.42, 0.07],
        st(c, Finish::fabric()),
    );
    block(
        out,
        f,
        [0.0, h * 0.8, -d * 0.5 + 0.04],
        [w * 0.6, h * 0.15, 0.05],
        st(c, Finish::fabric()),
    );
    for side in [-1.0f32, 1.0] {
        block(
            out,
            f,
            [side * (w * 0.5 - 0.05), h * 0.6, -d * 0.1],
            [0.05, 0.16, 0.05],
            lm,
        );
        block(
            out,
            f,
            [side * (w * 0.5 - 0.05), h * 0.76, 0.02],
            [0.05, 0.03, d * 0.4],
            st([0.2, 0.2, 0.2], Finish::matte()),
        );
    }
}

fn b_file(out: &mut Vec<MeshVertex>, f: &Frame, w: f32, d: f32, h: f32, c: [f32; 3]) {
    block(
        out,
        f,
        [0.0, 0.0, 0.0],
        [w - 0.04, 0.06, d - 0.04],
        st(dark(c, 0.7), Finish::matte()),
    );
    block(
        out,
        f,
        [0.0, 0.06, 0.0],
        [w, h - 0.08, d],
        st(c, Finish::new(0.35, 0.3, 0.0)),
    );
    fronts(
        out,
        f,
        -w * 0.5 + 0.01,
        0.08,
        w - 0.02,
        h - 0.12,
        d * 0.5 - 0.01,
        1,
        3,
        st(light(c, 0.05), Finish::new(0.35, 0.3, 0.0)),
        Hardware::Bar,
    );
}

fn b_lounge(out: &mut Vec<MeshVertex>, f: &Frame, w: f32, d: f32, h: f32, c: [f32; 3]) {
    legs(
        out,
        f,
        w,
        d,
        0.14,
        0.06,
        0.02,
        st(rgb("#3a3027"), Finish::wood()),
    );
    block(
        out,
        f,
        [0.0, 0.14, d * 0.1],
        [w, 0.2, d * 0.7],
        st(c, Finish::fabric()),
    );
    block(
        out,
        f,
        [0.0, 0.3, d * 0.1],
        [w - 0.02, 0.12, d * 0.66],
        st(light(c, 0.08), Finish::fabric()),
    );
    block(
        out,
        f,
        [0.0, 0.3, -d * 0.5 + 0.12],
        [w, h * 0.55, 0.14],
        st(c, Finish::fabric()),
    );
    block(
        out,
        f,
        [0.0, h * 0.55, -d * 0.5 + 0.16],
        [w, h * 0.35, 0.12],
        st(light(c, 0.06), Finish::fabric()),
    );
    for side in [-1.0f32, 1.0] {
        block(
            out,
            f,
            [side * (w * 0.5 - 0.05), 0.14, 0.0],
            [0.08, 0.36, d * 0.8],
            st(dark(c, 0.86), Finish::fabric()),
        );
    }
    blob(
        out,
        f,
        [0.0, 0.5, -d * 0.1],
        [0.2, 0.12, 0.1],
        st([0.9, 0.86, 0.78], Finish::fabric()),
        12,
    );
}

fn b_piano(out: &mut Vec<MeshVertex>, f: &Frame, w: f32, d: f32, h: f32, c: [f32; 3]) {
    let key_n = 36usize;
    block(
        out,
        f,
        [0.0, 0.0, 0.0],
        [w, h * 0.75, d],
        st(c, Finish::wood()),
    );
    block(
        out,
        f,
        [0.0, h * 0.75, -d * 0.5 + 0.1],
        [w, 0.04, d * 0.6],
        st(dark(c, 0.8), Finish::wood()),
    );
    block(
        out,
        f,
        [0.0, h * 0.68, d * 0.5 - 0.02],
        [w - 0.06, 0.1, 0.24],
        st(dark(c, 0.85), Finish::wood()),
    );
    let kw = (w - 0.12) / key_n as f32;
    for i in 0..key_n {
        let x = -w * 0.5 + 0.06 + kw * (i as f32 + 0.5);
        block(
            out,
            f,
            [x, h * 0.68 + 0.1, d * 0.5 - 0.02],
            [kw - 0.003, 0.02, 0.16],
            st([0.98, 0.97, 0.93], Finish::new(0.15, 0.0, 0.0)),
        );
        if i % 7 != 2 && i % 7 != 5 && i + 1 < key_n {
            block(
                out,
                f,
                [x + kw * 0.5, h * 0.68 + 0.1, d * 0.5 - 0.09],
                [kw * 0.55, 0.03, 0.1],
                st([0.1, 0.1, 0.1], Finish::new(0.2, 0.0, 0.0)),
            );
        }
    }
    block(
        out,
        f,
        [0.0, h * 0.85, -d * 0.5 + 0.18],
        [w * 0.7, 0.03, 0.15],
        st(dark(c, 0.7), Finish::wood()),
    );
    for side in [-1.0f32, 1.0] {
        block(
            out,
            f,
            [side * (w * 0.5 - 0.08), 0.0, d * 0.5 - 0.1],
            [0.07, h * 0.75, 0.07],
            st(dark(c, 0.8), Finish::wood()),
        );
    }
}

fn b_treadmill(out: &mut Vec<MeshVertex>, f: &Frame, w: f32, d: f32, h: f32, c: [f32; 3]) {
    block(
        out,
        f,
        [0.0, 0.05, d * 0.06],
        [w * 0.7, 0.16, d * 0.78],
        st(rgb("#25252a"), Finish::new(0.4, 0.2, 0.0)),
    );
    block(
        out,
        f,
        [0.0, 0.21, d * 0.06],
        [w * 0.62, 0.012, d * 0.7],
        st(rgb("#141416"), Finish::new(0.7, 0.0, 0.0)),
    );
    for side in [-1.0f32, 1.0] {
        block(
            out,
            f,
            [side * (w * 0.33), 0.05, d * 0.06],
            [0.05, 0.02, d * 0.76],
            st([0.6, 0.6, 0.62], Finish::metal()),
        );
        rod(
            out,
            f,
            [side * (w * 0.32), 0.2, -d * 0.32],
            [side * (w * 0.32), h * 0.9, -d * 0.3],
            0.03,
            0.025,
            st(c, Finish::metal()),
            10,
        );
    }
    block(
        out,
        f,
        [0.0, h * 0.9, -d * 0.34],
        [w * 0.7, 0.08, 0.3],
        st(c, Finish::new(0.25, 0.3, 0.0)),
    );
    block(
        out,
        f,
        [0.0, h * 0.9 + 0.08, -d * 0.34],
        [w * 0.5, 0.02, 0.22],
        st(rgb("#1f2b3a"), Finish::screen(0.12)),
    );
    for side in [-1.0f32, 1.0] {
        rod(
            out,
            f,
            [side * (w * 0.32), h * 0.78, -d * 0.28],
            [side * (w * 0.28), h * 0.78, d * 0.2],
            0.018,
            0.014,
            st([0.2, 0.2, 0.2], Finish::matte()),
            8,
        );
    }
}

fn b_cat_tree(out: &mut Vec<MeshVertex>, f: &Frame, w: f32, d: f32, h: f32, c: [f32; 3]) {
    block(
        out,
        f,
        [0.0, 0.0, 0.0],
        [w * 0.8, 0.06, d * 0.8],
        st(rgb("#a99a86"), Finish::fabric()),
    );
    rod(
        out,
        f,
        [0.0, 0.06, 0.0],
        [0.0, h * 0.6, 0.0],
        0.05,
        0.05,
        st(rgb("#c9bcac"), Finish::fabric()),
        12,
    );
    block(
        out,
        f,
        [0.0, h * 0.6, 0.0],
        [w * 0.7, 0.05, d * 0.7],
        st(c, Finish::fabric()),
    );
    rod(
        out,
        f,
        [w * 0.2, h * 0.6, 0.0],
        [w * 0.2, h, 0.0],
        0.04,
        0.04,
        st(rgb("#c9bcac"), Finish::fabric()),
        10,
    );
    block(
        out,
        f,
        [w * 0.2, h, 0.0],
        [w * 0.4, 0.04, d * 0.4],
        st(light(c, 0.1), Finish::fabric()),
    );
    let r = w * 0.2;
    lathe(
        out,
        f,
        [-w * 0.2, 0.06, 0.0],
        &[(0.0, 0.0), (r * 0.8, 0.0), (r, 0.12), (r * 0.9, 0.24)],
        st(rgb("#b8a894"), Finish::fabric()),
        20,
    );
}

fn b_chess(out: &mut Vec<MeshVertex>, f: &Frame, w: f32, d: f32, h: f32, c: [f32; 3]) {
    legs(
        out,
        f,
        w,
        d,
        h - 0.03,
        0.06,
        0.02,
        st(dark(c, 0.7), Finish::wood()),
    );
    block(
        out,
        f,
        [0.0, h - 0.03, 0.0],
        [w, 0.03, d],
        st(c, Finish::wood()),
    );
    let bw = (w * 0.7).min(0.5);
    for i in 0..8 {
        for j in 0..8 {
            let col = if (i + j) % 2 == 0 {
                [0.9, 0.87, 0.8]
            } else {
                [0.28, 0.24, 0.2]
            };
            block(
                out,
                f,
                [
                    -bw * 0.5 + bw / 8.0 * (i as f32 + 0.5),
                    h,
                    -bw * 0.5 + bw / 8.0 * (j as f32 + 0.5),
                ],
                [bw / 8.0, 0.008, bw / 8.0],
                st(col, Finish::new(0.5, 0.0, 0.0)),
            );
        }
    }
    for i in 0..4 {
        cyl(
            out,
            f,
            [-bw * 0.3 + i as f32 * 0.08, h + 0.008, -bw * 0.3],
            0.014,
            0.012,
            0.06,
            st([0.95, 0.95, 0.93], Finish::new(0.2, 0.0, 0.0)),
            10,
        );
        cyl(
            out,
            f,
            [-bw * 0.3 + i as f32 * 0.08, h + 0.008, bw * 0.3],
            0.014,
            0.012,
            0.06,
            st([0.15, 0.13, 0.12], Finish::new(0.2, 0.0, 0.0)),
            10,
        );
    }
}

fn b_guitar(out: &mut Vec<MeshVertex>, f: &Frame, w: f32, d: f32, h: f32, _c: [f32; 3]) {
    let _ = d;
    block(
        out,
        f,
        [0.0, 0.0, 0.0],
        [w * 0.6, 0.04, 0.3],
        st([0.2, 0.18, 0.16], Finish::black_metal()),
    );
    rod(
        out,
        f,
        [0.0, 0.04, 0.0],
        [0.0, h * 0.5, 0.06],
        0.015,
        0.015,
        st([0.2, 0.18, 0.16], Finish::black_metal()),
        8,
    );
    blob(
        out,
        f,
        [0.0, h * 0.55, 0.06],
        [w * 0.3, h * 0.22, 0.05],
        st(rgb("#8a5a3a"), Finish::new(0.25, 0.1, 0.0)),
        16,
    );
    rod(
        out,
        f,
        [0.0, h * 0.72, 0.04],
        [0.0, h, 0.02],
        0.025,
        0.02,
        st(rgb("#4a3020"), Finish::wood()),
        8,
    );
    block(
        out,
        f,
        [0.0, h * 0.76, 0.06],
        [w * 0.24, 0.02, 0.03],
        st(rgb("#3a2a1a"), Finish::wood()),
    );
}

// ------------------------------------------------------------------- dispatch

fn b_corner_sofa(out: &mut Vec<MeshVertex>, f: &Frame, w: f32, d: f32, _h: f32, c: [f32; 3]) {
    let seat = 0.42;
    let back = 0.38;
    // Long section on the left, chaise extending forward on the right.
    block(
        out,
        f,
        [-w * 0.16, 0.0, -d * 0.18],
        [w * 0.68, seat, d * 0.58],
        st(c, Finish::fabric()),
    );
    block(
        out,
        f,
        [w * 0.34, 0.0, d * 0.04],
        [w * 0.3, seat, d * 0.88],
        st(c, Finish::fabric()),
    );
    block(
        out,
        f,
        [-w * 0.16, seat, -d * 0.42],
        [w * 0.68, back, d * 0.14],
        st(dark(c, 0.92), Finish::fabric()),
    );
    block(
        out,
        f,
        [w * 0.43, seat, -d * 0.06],
        [w * 0.14, back, d * 0.72],
        st(dark(c, 0.92), Finish::fabric()),
    );
    block(
        out,
        f,
        [-w * 0.5 + 0.06, 0.0, -d * 0.18],
        [0.12, seat + 0.16, d * 0.58],
        st(dark(c, 0.88), Finish::fabric()),
    );
    for i in 0..2 {
        let x = -w * 0.34 + i as f32 * w * 0.3;
        block(
            out,
            f,
            [x, seat, -d * 0.16],
            [w * 0.26, 0.12, d * 0.5],
            st(light(c, 1.08), Finish::fabric()),
        );
    }
    block(
        out,
        f,
        [w * 0.34, seat, d * 0.12],
        [w * 0.26, 0.12, d * 0.62],
        st(light(c, 1.08), Finish::fabric()),
    );
}

fn b_bay_cushion(out: &mut Vec<MeshVertex>, f: &Frame, w: f32, d: f32, h: f32, c: [f32; 3]) {
    let th = h.max(0.1);
    block(out, f, [0.0, 0.0, 0.0], [w, th, d], st(c, Finish::fabric()));
    block(
        out,
        f,
        [0.0, th, d * 0.38],
        [w * 0.9, 0.06, d * 0.22],
        st(light(c, 1.12), Finish::fabric()),
    );
    let pillows = 3usize;
    for i in 0..pillows {
        let x = -w * 0.32 + i as f32 * w * 0.32;
        blob(
            out,
            f,
            [x, th + 0.2, -d * 0.22],
            [w * 0.12, 0.18, 0.1],
            st(light(c, 1.16), Finish::fabric()),
            12,
        );
    }
}

fn b_bean_bag(out: &mut Vec<MeshVertex>, f: &Frame, w: f32, d: f32, h: f32, c: [f32; 3]) {
    let hh = h.max(0.3);
    blob(
        out,
        f,
        [0.0, hh * 0.46, 0.0],
        [w * 0.5, hh * 0.46, d * 0.5],
        st(c, Finish::fabric()),
        18,
    );
    blob(
        out,
        f,
        [0.0, hh * 0.72, -d * 0.12],
        [w * 0.32, hh * 0.26, d * 0.3],
        st(light(c, 1.1), Finish::fabric()),
        14,
    );
}

fn b_water_heater(out: &mut Vec<MeshVertex>, f: &Frame, w: f32, d: f32, h: f32, c: [f32; 3]) {
    let r = (w.min(d) * 0.5).max(0.16);
    let hh = h.max(0.6);
    cyl(
        out,
        f,
        [0.0, 0.0, 0.0],
        r,
        r,
        hh,
        st(c, Finish::metal()),
        18,
    );
    cyl(
        out,
        f,
        [0.0, hh, 0.0],
        r * 1.04,
        r * 1.04,
        0.05,
        st(light(c, 1.1), Finish::metal()),
        18,
    );
    block(
        out,
        f,
        [0.0, hh + 0.08, 0.0],
        [0.16, 0.08, 0.16],
        st([0.75, 0.77, 0.8], Finish::metal()),
    );
    for sx in [-1.0f32, 1.0] {
        rod(
            out,
            f,
            [sx * (r * 0.5), hh * 0.18, -r * 0.9],
            [sx * (r * 0.5), hh * 0.18, -r * 1.35],
            0.02,
            0.02,
            st([0.78, 0.80, 0.82], Finish::metal()),
            8,
        );
    }
}

fn b_ac_wall(out: &mut Vec<MeshVertex>, f: &Frame, w: f32, d: f32, h: f32, c: [f32; 3]) {
    let y = 2.18;
    let hh = h.clamp(0.22, 0.34);
    block(
        out,
        f,
        [0.0, y, 0.0],
        [w, hh, d.max(0.18)],
        st(c, Finish::matte()),
    );
    block(
        out,
        f,
        [0.0, y - 0.04, d * 0.5 - 0.02],
        [w * 0.92, 0.04, 0.05],
        st(dark(c, 0.8), Finish::matte()),
    );
    block(
        out,
        f,
        [0.0, y + hh * 0.5 + 0.02, 0.0],
        [w * 0.98, 0.03, d * 0.7],
        st(light(c, 1.06), Finish::matte()),
    );
}

fn b_oven_column(out: &mut Vec<MeshVertex>, f: &Frame, w: f32, d: f32, h: f32, c: [f32; 3]) {
    block(out, f, [0.0, 0.0, 0.0], [w, h, d], st(c, Finish::wood()));
    block(
        out,
        f,
        [0.0, h - 0.06, 0.0],
        [w + 0.02, 0.06, d + 0.02],
        st(dark(c, 0.8), Finish::matte()),
    );
    // Two appliance fronts with dark glass and steel bars.
    let glass = st([0.09, 0.10, 0.12], Finish::new(0.18, 0.1, 0.0));
    for (i, y) in [0.55f32, 1.45].iter().enumerate() {
        let fh = 0.55;
        block(
            out,
            f,
            [0.0, *y, d * 0.5 - 0.01],
            [w * 0.82, fh, 0.03],
            glass,
        );
        block(
            out,
            f,
            [0.0, *y + fh + 0.02, d * 0.5 - 0.01],
            [w * 0.86, 0.03, 0.04],
            st([0.72, 0.74, 0.76], Finish::metal()),
        );
        if i == 0 {
            block(
                out,
                f,
                [0.0, *y + fh * 0.5, d * 0.5 + 0.02],
                [0.28, 0.02, 0.02],
                st([0.72, 0.74, 0.76], Finish::metal()),
            );
        }
    }
}

pub fn build_asset(
    out: &mut Vec<MeshVertex>,
    f: &Frame,
    asset: &str,
    w: f32,
    d: f32,
    h: f32,
    c: [f32; 3],
) {
    match asset {
        "sofa" => b_seat(out, f, w, d, c, false),
        "corner_sofa" => b_corner_sofa(out, f, w, d, h, c),
        "bay_cushion" => b_bay_cushion(out, f, w, d, h, c),
        "bean_bag" => b_bean_bag(out, f, w, d, h, c),
        "water_heater" => b_water_heater(out, f, w, d, h, c),
        "ac_wall" => b_ac_wall(out, f, w, d, h, c),
        "oven_column" => b_oven_column(out, f, w, d, h, c),
        "sofa_two" => b_seat(out, f, w, d, c, false),
        "armchair" => b_seat(out, f, w, d, c, true),
        "coffee_table" => b_table(out, f, w, d, h, c),
        "side_table" => b_round_table(out, f, w, d, h, c),
        "tv" => b_tv(out, f, w, d, h, c),
        "tv_stand" => b_tv_stand(out, f, w, d, h, c),
        "rug" => b_rug(out, f, w, d, h, c),
        "floor_lamp" => b_floor_lamp(out, f, w, d, h, c),
        "bookshelf" => b_bookshelf(out, f, w, d, h, c),
        "shoe_cabinet" => b_sideboard(out, f, w, d, h, c),
        "plant" => b_plant(out, f, w, d, h, c),
        "bed" => b_bed(out, f, w, d, h, c, false),
        "bed_single" => b_bed(out, f, w, d, h, c, true),
        "nightstand" => b_nightstand(out, f, w, d, h, c),
        "wardrobe" | "wardrobe_wide" => b_wardrobe(out, f, w, d, h, c),
        "dresser" => b_dresser(out, f, w, d, h, c),
        "bench_bed" => b_bench(out, f, w, d, h, c),
        "crib" => b_crib(out, f, w, d, h, c),
        "kids_desk" => b_kids_desk(out, f, w, d, h, c),
        "mirror_stand" => b_mirror_stand(out, f, w, d, h, c),
        "dining_table" => b_table(out, f, w, d, h, c),
        "round_table" => b_round_table(out, f, w, d, h, c),
        "chair" => b_chair(out, f, w, d, h, c),
        "bar_chair" => b_bar_chair(out, f, w, d, h, c),
        "sideboard" | "wine_cabinet" => b_sideboard(out, f, w, d, h, c),
        "kitchen_island" => b_island(out, f, w, d, h, c),
        "counter" => b_counter(out, f, w, d, h, c),
        "sink_cabinet" => b_sink_cab(out, f, w, d, h, c),
        "stove" => b_stove(out, f, w, d, h, c),
        "dining_bench" => b_bench(out, f, w, d, h, c),
        "toilet" => b_toilet(out, f, w, d, h, c),
        "basin" => b_basin(out, f, w, d, h, c),
        "bathtub" => b_bathtub(out, f, w, d, h, c),
        "shower" => b_shower(out, f, w, d, h, c),
        "towel_rack" => b_towel_rack(out, f, w, d, h, c),
        "mirror_cab" => b_mirror_cab(out, f, w, d, h, c),
        "bath_mat" => b_bath_mat(out, f, w, d, h, c),
        "urinal" => b_urinal(out, f, w, d, h, c),
        "laundry_basket" => b_laundry(out, f, w, d, h, c),
        "fridge" => b_fridge(out, f, w, d, h, c, false),
        "fridge_double" => b_fridge(out, f, w, d, h, c, true),
        "washing_machine" | "dryer" => b_washer(out, f, w, d, h, c),
        "dishwasher" => b_dishwasher(out, f, w, d, h, c),
        "microwave" => b_microwave(out, f, w, d, h, c),
        "water_dispenser" => b_dispenser(out, f, w, d, h, c),
        "air_purifier" => b_purifier(out, f, w, d, h, c),
        "heater" => b_heater(out, f, w, d, h, c),
        "range_hood" => b_hood(out, f, w, d, h, c),
        "ac" => b_ac(out, f, w, d, h, c),
        "desk" => b_desk(out, f, w, d, h, c),
        "office_chair" => b_office_chair(out, f, w, d, h, c),
        "file_cabinet" => b_file(out, f, w, d, h, c),
        "lounge_chair" => b_lounge(out, f, w, d, h, c),
        "piano" => b_piano(out, f, w, d, h, c),
        "treadmill" => b_treadmill(out, f, w, d, h, c),
        "cat_tree" => b_cat_tree(out, f, w, d, h, c),
        "chess_table" => b_chess(out, f, w, d, h, c),
        "guitar_stand" => b_guitar(out, f, w, d, h, c),
        _ => block(
            out,
            f,
            [0.0, 0.0, 0.0],
            [w, h.max(0.6), d],
            st(FURNITURE_FALLBACK, Finish::matte()),
        ),
    }
}

/// Build one item and split its output into opaque and glass buckets.
#[allow(clippy::too_many_arguments)]
fn build_item(
    out_opaque: &mut Vec<MeshVertex>,
    out_glass: &mut Vec<MeshVertex>,
    item: &Furniture,
    w: f32,
    d: f32,
    h: f32,
    base: [f32; 3],
    frame: &Frame,
) {
    let mut all = Vec::new();
    build_asset(&mut all, frame, &item.asset_id, w, d, h, base);
    // Soft contact shadow: triangle fan with alpha fading to zero at the
    // rim, drawn in the blended pass so it grounds the piece without the
    // hard "puddle" edge a flat disc would give.
    let seg = 24usize;
    let cx = frame.map([0.0, 0.004, 0.0]);
    let color = [0.02, 0.025, 0.04];
    let mat = [1.0, 0.0, 0.0];
    for i in 0..seg {
        let a0 = i as f32 / seg as f32 * std::f32::consts::TAU;
        let a1 = (i + 1) as f32 / seg as f32 * std::f32::consts::TAU;
        let r0 = frame.map([w * 0.56 * a0.cos(), 0.004, d * 0.58 * a0.sin()]);
        let r1 = frame.map([w * 0.56 * a1.cos(), 0.004, d * 0.58 * a1.sin()]);
        all.push(MeshVertex::with_alpha(cx, [0.0, 1.0, 0.0], color, mat, 0.2));
        all.push(MeshVertex::with_alpha(r0, [0.0, 1.0, 0.0], color, mat, 0.0));
        all.push(MeshVertex::with_alpha(r1, [0.0, 1.0, 0.0], color, mat, 0.0));
    }
    for v in all {
        if v.alpha >= 1.0 {
            out_opaque.push(v);
        } else {
            out_glass.push(v);
        }
    }
}

pub fn furniture_mesh_parts(items: &[Furniture]) -> MeshParts {
    let mut opaque = Vec::new();
    let mut glass = Vec::new();
    for item in items {
        let w = item.width_mm as f32 / 1000.0;
        let d = item.depth_mm as f32 / 1000.0;
        let h = CATALOG
            .iter()
            .find(|s| s.asset_id == item.asset_id)
            .map(|s| s.height_mm as f32 / 1000.0)
            .unwrap_or(item.width_mm as f32 / 1000.0 * 0.75);
        let base = if item.color.is_empty() {
            FURNITURE_FALLBACK
        } else {
            rgb(&item.color)
        };
        let frame = Frame::new(
            item.rotation_deg,
            [item.center.x as f32 / 1000.0, item.center.y as f32 / 1000.0],
        );
        build_item(&mut opaque, &mut glass, item, w, d, h, base, &frame);
    }
    let opaque_len = opaque.len();
    opaque.extend(glass);
    MeshParts {
        vertices: opaque,
        opaque: opaque_len,
    }
}

pub fn furniture_mesh(items: &[Furniture]) -> Vec<MeshVertex> {
    furniture_mesh_parts(items).vertices
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn geometry_primitives_emit_triangles() {
        let mut v = Vec::new();
        let f = Frame::new(0.0, [0.0, 0.0]);
        let s = st([1.0, 1.0, 1.0], Finish::matte());
        cyl(&mut v, &f, [0.0, 0.0, 0.0], 0.1, 0.1, 0.2, s, 12);
        assert_eq!(v.len() % 3, 0);
        assert!(v.len() > 100);
        let mut v2 = Vec::new();
        blob(&mut v2, &f, [0.0, 0.1, 0.0], [0.1, 0.1, 0.1], s, 10);
        assert_eq!(v2.len() % 3, 0);
        let mut v3 = Vec::new();
        lathe(
            &mut v3,
            &f,
            [0.0, 0.0, 0.0],
            &[(0.0, 0.0), (0.1, 0.0), (0.08, 0.2)],
            s,
            12,
        );
        assert_eq!(v3.len() % 3, 0);
    }

    #[test]
    fn sofa_is_built_from_solids() {
        let mut v = Vec::new();
        let f = Frame::new(0.0, [1.0, 0.0]);
        build_asset(&mut v, &f, "sofa", 2.2, 0.95, 0.8, [0.55, 0.44, 0.33]);
        assert!(v.len() > 1800, "sofa vertices {}", v.len());
    }

    #[test]
    fn every_catalog_asset_builds() {
        let mut total = 0usize;
        for spec in CATALOG {
            let mut v = Vec::new();
            let f = Frame::new(45.0, [2.0, 3.0]);
            build_asset(
                &mut v,
                &f,
                spec.asset_id,
                spec.width_mm as f32 / 1000.0,
                spec.depth_mm as f32 / 1000.0,
                spec.height_mm as f32 / 1000.0,
                rgb(spec.color),
            );
            assert!(v.len() >= 36, "{} produced {}", spec.asset_id, v.len());
            total += v.len();
        }
        println!("total vertices for all {} assets: {}", CATALOG.len(), total);
        assert!(total < 200_000, "all assets fit the vertex buffer: {total}");
    }

    #[test]
    fn furniture_finish_patterns_stay_out_of_the_glass_range() {
        // The shader only treats 8 as glass; anything close to it would make
        // upholstery and wood refract like a window.
        for f in [
            Finish::matte(),
            Finish::fabric(),
            Finish::wood(),
            Finish::metal(),
            Finish::ceramic(),
            Finish::mirror(),
        ] {
            assert!(
                !(7.5..8.5).contains(&f.pattern),
                "pattern {} collides with glass",
                f.pattern
            );
        }
    }
}
