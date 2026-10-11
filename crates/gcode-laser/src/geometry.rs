//! Copper geometry: rasterise, dilate, trace contours, simplify, order.
//!
//! See `design.md` D2. Rasterising every primitive type is far more robust than
//! writing our own polygon union, and the resolution error is absorbed on the
//! safe side: the dilation radius includes half a pixel **and** the
//! simplification tolerance, so a simplified path can never creep closer to the
//! copper than requested. That property is asserted in tests by measuring the
//! real minimum distance rather than trusting the construction.

use crate::gerber::{Aperture, Bounds, Flash, GerberLayer, MacroPrimitive, Region};
use serde::{Deserialize, Serialize};

/// A toolpath in millimetres.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Path {
    pub points: Vec<(f64, f64)>,
    pub closed: bool,
}

impl Path {
    pub fn length(&self) -> f64 {
        let mut total = 0.0;
        for w in self.points.windows(2) {
            total += dist(w[0], w[1]);
        }
        if self.closed && self.points.len() > 2 {
            total += dist(*self.points.last().unwrap(), self.points[0]);
        }
        total
    }

    pub fn start(&self) -> Option<(f64, f64)> {
        self.points.first().copied()
    }

    pub fn end(&self) -> Option<(f64, f64)> {
        self.points.last().copied()
    }
}

fn dist(a: (f64, f64), b: (f64, f64)) -> f64 {
    ((a.0 - b.0).powi(2) + (a.1 - b.1).powi(2)).sqrt()
}

/// A binary occupancy grid over a board area.
#[derive(Debug, Clone, PartialEq)]
pub struct Raster {
    pub cols: usize,
    pub rows: usize,
    /// Millimetres per pixel.
    pub resolution: f64,
    /// World coordinate of the centre of pixel (0, 0).
    pub origin: (f64, f64),
    cells: Vec<bool>,
    occupied: usize,
}

impl Raster {
    /// Build an empty raster covering `bounds` plus `margin` on every side.
    pub fn for_bounds(bounds: Bounds, resolution: f64, margin: f64) -> Self {
        assert!(resolution > 0.0, "resolution must be positive");
        let min_x = bounds.min_x - margin;
        let min_y = bounds.min_y - margin;
        let cols = ((bounds.width() + 2.0 * margin) / resolution).ceil() as usize + 1;
        let rows = ((bounds.height() + 2.0 * margin) / resolution).ceil() as usize + 1;
        Self {
            cols,
            rows,
            resolution,
            origin: (min_x + resolution / 2.0, min_y + resolution / 2.0),
            cells: vec![false; cols * rows],
            occupied: 0,
        }
    }

    pub fn cell_count(&self) -> usize {
        self.cells.len()
    }

    pub fn occupied_count(&self) -> usize {
        self.occupied
    }

    pub fn is_occupied(&self, col: i64, row: i64) -> bool {
        if col < 0 || row < 0 {
            return false;
        }
        let (c, r) = (col as usize, row as usize);
        if c >= self.cols || r >= self.rows {
            return false;
        }
        self.cells[r * self.cols + c]
    }

    /// Mark one cell.
    pub fn set(&mut self, col: usize, row: usize) {
        if col < self.cols && row < self.rows {
            let i = row * self.cols + col;
            if !self.cells[i] {
                self.cells[i] = true;
                self.occupied += 1;
            }
        }
    }

    /// World position of a pixel centre.
    pub fn world(&self, col: usize, row: usize) -> (f64, f64) {
        (
            self.origin.0 + col as f64 * self.resolution,
            self.origin.1 + row as f64 * self.resolution,
        )
    }

    /// Pixel index containing a world point (may be out of range).
    pub fn index_of(&self, x: f64, y: f64) -> (i64, i64) {
        (
            ((x - self.origin.0) / self.resolution).round() as i64,
            ((y - self.origin.1) / self.resolution).round() as i64,
        )
    }

    /// Fill cells whose centre satisfies `inside`.
    fn fill_where(&mut self, b: Bounds, mut inside: impl FnMut(f64, f64) -> bool) {
        let (c0, r0) = self.index_of(b.min_x, b.min_y);
        let (c1, r1) = self.index_of(b.max_x, b.max_y);
        for r in r0.max(0)..=r1.min(self.rows as i64 - 1) {
            for c in c0.max(0)..=c1.min(self.cols as i64 - 1) {
                let (x, y) = self.world(c as usize, r as usize);
                if inside(x, y) {
                    self.set(c as usize, r as usize);
                }
            }
        }
    }

    /// Fill a disc.
    pub fn fill_disc(&mut self, cx: f64, cy: f64, radius: f64) {
        let b = Bounds {
            min_x: cx - radius,
            min_y: cy - radius,
            max_x: cx + radius,
            max_y: cy + radius,
        };
        let r2 = radius * radius;
        self.fill_where(b, |x, y| (x - cx).powi(2) + (y - cy).powi(2) <= r2);
    }

    /// Fill an axis-aligned rectangle.
    pub fn fill_rect(&mut self, cx: f64, cy: f64, w: f64, h: f64) {
        let b = Bounds {
            min_x: cx - w / 2.0,
            min_y: cy - h / 2.0,
            max_x: cx + w / 2.0,
            max_y: cy + h / 2.0,
        };
        self.fill_where(b, |x, y| {
            (x - cx).abs() <= w / 2.0 && (y - cy).abs() <= h / 2.0
        });
    }

    /// Fill an obround (stadium) shape.
    pub fn fill_obround(&mut self, cx: f64, cy: f64, w: f64, h: f64) {
        let b = Bounds {
            min_x: cx - w / 2.0,
            min_y: cy - h / 2.0,
            max_x: cx + w / 2.0,
            max_y: cy + h / 2.0,
        };
        let r = w.min(h) / 2.0;
        let (half_w, half_h) = if w >= h {
            ((w - h) / 2.0, 0.0)
        } else {
            (0.0, (h - w) / 2.0)
        };
        self.fill_where(b, |x, y| {
            // Rounded rectangle: distance to the inner segment <= r
            let dx = (x - cx).abs() - half_w;
            let dy = (y - cy).abs() - half_h;
            let (dx, dy) = (dx.max(0.0), dy.max(0.0));
            (dx * dx + dy * dy).sqrt() <= r + 1e-12
        });
    }

    /// Fill a capsule: a stroked segment with a round aperture.
    pub fn fill_capsule(&mut self, from: (f64, f64), to: (f64, f64), radius: f64) {
        let b = Bounds {
            min_x: from.0.min(to.0) - radius,
            min_y: from.1.min(to.1) - radius,
            max_x: from.0.max(to.0) + radius,
            max_y: from.1.max(to.1) + radius,
        };
        self.fill_where(b, |x, y| dist_to_segment((x, y), from, to) <= radius);
    }

    /// Fill the union of axis-aligned squares of side `side` centred on every
    /// point of the segment (used for rect/obround apertures, conservatively).
    pub fn fill_square_band(&mut self, from: (f64, f64), to: (f64, f64), side: f64) {
        let half = side / 2.0;
        let b = Bounds {
            min_x: from.0.min(to.0) - half,
            min_y: from.1.min(to.1) - half,
            max_x: from.0.max(to.0) + half,
            max_y: from.1.max(to.1) + half,
        };
        self.fill_where(b, |x, y| chebyshev_to_segment((x, y), from, to) <= half);
    }

    /// Fill a polygon (even-odd rule).
    pub fn fill_polygon(&mut self, points: &[(f64, f64)]) {
        if points.len() < 3 {
            return;
        }
        let mut b = Bounds {
            min_x: f64::INFINITY,
            min_y: f64::INFINITY,
            max_x: f64::NEG_INFINITY,
            max_y: f64::NEG_INFINITY,
        };
        for (x, y) in points {
            b.min_x = b.min_x.min(*x);
            b.min_y = b.min_y.min(*y);
            b.max_x = b.max_x.max(*x);
            b.max_y = b.max_y.max(*y);
        }
        self.fill_where(b, |x, y| point_in_polygon((x, y), points));
    }

    /// Rasterise a whole Gerber copper layer.
    pub fn rasterize_layer(
        layer: &GerberLayer,
        resolution: f64,
        margin: f64,
    ) -> Result<Raster, GeometryError> {
        let bounds = layer.bounds().ok_or(GeometryError::EmptyLayer)?;
        // Pad by the largest aperture so strokes near the edge are not clipped.
        let max_extent = layer
            .apertures
            .values()
            .map(|a| a.max_extent())
            .fold(0.0f64, f64::max);
        let mut r = Raster::for_bounds(bounds, resolution, margin + max_extent);
        r.rasterize_into(layer, resolution)?;
        Ok(r)
    }

    /// Draw a layer into this raster.
    pub fn rasterize_into(
        &mut self,
        layer: &GerberLayer,
        _resolution: f64,
    ) -> Result<(), GeometryError> {
        for f in &layer.flashes {
            self.flash(f, &layer.apertures)?;
        }
        for d in &layer.draws {
            let ap = layer
                .apertures
                .get(&d.aperture)
                .ok_or(GeometryError::MissingAperture(d.aperture))?;
            self.stroke(d.from, d.to, ap);
        }
        for r in &layer.regions {
            self.region(r);
        }
        Ok(())
    }

    fn flash(
        &mut self,
        f: &Flash,
        apertures: &std::collections::BTreeMap<u32, Aperture>,
    ) -> Result<(), GeometryError> {
        let ap = apertures
            .get(&f.aperture)
            .ok_or(GeometryError::MissingAperture(f.aperture))?;
        match ap {
            Aperture::Circle { diameter } => self.fill_disc(f.x, f.y, diameter / 2.0),
            Aperture::Rect { width, height } => self.fill_rect(f.x, f.y, *width, *height),
            Aperture::Obround { width, height } => self.fill_obround(f.x, f.y, *width, *height),
            Aperture::Macro { primitives, .. } => {
                self.fill_macro(f.x, f.y, primitives);
            }
        }
        Ok(())
    }

    fn stroke(&mut self, from: (f64, f64), to: (f64, f64), aperture: &Aperture) {
        match aperture {
            Aperture::Circle { diameter } => self.fill_capsule(from, to, diameter / 2.0),
            // Rect/obround strokes are conservatively treated as a square band of
            // the aperture's larger side: it never under-covers the real shape.
            Aperture::Rect { width, height } | Aperture::Obround { width, height } => {
                self.fill_square_band(from, to, width.max(*height))
            }
            // A macro aperture swept along a line: place it at both ends and at
            // intermediate steps so the sweep is covered.
            Aperture::Macro { primitives, .. } => {
                let length = dist(from, to);
                let reach = crate::gerber::macro_reach(primitives).max(self.resolution);
                let steps = ((length / reach).ceil() as usize).clamp(1, 4096);
                for k in 0..=steps {
                    let t = k as f64 / steps as f64;
                    let p = (from.0 + (to.0 - from.0) * t, from.1 + (to.1 - from.1) * t);
                    self.fill_macro(p.0, p.1, primitives);
                }
            }
        }
    }

    /// Place a macro aperture's primitives at a position.
    pub fn fill_macro(&mut self, cx: f64, cy: f64, primitives: &[MacroPrimitive]) {
        for p in primitives {
            match p {
                MacroPrimitive::Circle {
                    diameter,
                    center,
                    rotation,
                } => {
                    let (x, y) = rotate_about(*center, (0.0, 0.0), *rotation);
                    self.fill_disc(cx + x, cy + y, diameter / 2.0);
                }
                MacroPrimitive::Outline { points, rotation } => {
                    if points.len() < 3 {
                        continue;
                    }
                    let pivot = points[0];
                    let rotated: Vec<(f64, f64)> = points
                        .iter()
                        .map(|p| {
                            let (x, y) = rotate_about(*p, pivot, *rotation);
                            (cx + x, cy + y)
                        })
                        .collect();
                    self.fill_polygon(&rotated);
                }
                MacroPrimitive::VectorLine {
                    width,
                    from,
                    to,
                    rotation,
                } => {
                    let pivot = (0.0, 0.0);
                    let a = rotate_about(*from, pivot, *rotation);
                    let b = rotate_about(*to, pivot, *rotation);
                    self.fill_capsule((cx + a.0, cy + a.1), (cx + b.0, cy + b.1), width / 2.0);
                }
                MacroPrimitive::CenterLine {
                    width,
                    height,
                    center,
                    rotation,
                } => {
                    let (x, y) = rotate_about(*center, (0.0, 0.0), *rotation);
                    if rotation.abs() < 1e-9 {
                        self.fill_rect(cx + x, cy + y, *width, *height);
                    } else {
                        let (hw, hh) = (width / 2.0, height / 2.0);
                        let corners = [
                            (x - hw, y - hh),
                            (x + hw, y - hh),
                            (x + hw, y + hh),
                            (x - hw, y + hh),
                        ];
                        let rotated: Vec<(f64, f64)> = corners
                            .iter()
                            .map(|p| {
                                let (rx, ry) = rotate_about(*p, (x, y), *rotation);
                                (cx + rx, cy + ry)
                            })
                            .collect();
                        self.fill_polygon(&rotated);
                    }
                }
            }
        }
    }

    fn region(&mut self, r: &Region) {
        self.fill_polygon(&r.points);
    }

    /// Grow the occupied set by `radius` (a true disc dilation).
    ///
    /// The disc offsets are computed once, so this is O(occupied x area of disc).
    pub fn dilate(&self, radius: f64) -> Raster {
        let mut out = Raster {
            cols: self.cols,
            rows: self.rows,
            resolution: self.resolution,
            origin: self.origin,
            cells: vec![false; self.cells.len()],
            occupied: 0,
        };
        let r_px = (radius / self.resolution).ceil() as i64;
        let mut offsets: Vec<(i64, i64)> = Vec::new();
        for dy in -r_px..=r_px {
            for dx in -r_px..=r_px {
                if (dx as f64 * self.resolution).hypot(dy as f64 * self.resolution) <= radius {
                    offsets.push((dx, dy));
                }
            }
        }
        for row in 0..self.rows {
            for col in 0..self.cols {
                if !self.is_occupied(col as i64, row as i64) {
                    continue;
                }
                for (dx, dy) in &offsets {
                    let (c, r) = (col as i64 + dx, row as i64 + dy);
                    if c >= 0 && r >= 0 && (c as usize) < out.cols && (r as usize) < out.rows {
                        out.set(c as usize, r as usize);
                    }
                }
            }
        }
        out
    }

    /// Distance from a world point to the nearest occupied cell, searching up to
    /// `max_radius`. Returns `None` if nothing is found within the radius.
    pub fn distance_to_occupied(&self, x: f64, y: f64, max_radius: f64) -> Option<f64> {
        let mut best: Option<f64> = None;
        let r_px = (max_radius / self.resolution).ceil() as i64;
        let (c0, r0) = self.index_of(x, y);
        for dr in -r_px..=r_px {
            for dc in -r_px..=r_px {
                if !self.is_occupied(c0 + dc, r0 + dr) {
                    continue;
                }
                let p = self.world((c0 + dc) as usize, (r0 + dr) as usize);
                let d = dist((x, y), p);
                if best.map(|b| d < b).unwrap_or(true) {
                    best = Some(d);
                }
            }
        }
        best
    }
}

/// Rotate `p` counter-clockwise by `degrees` about `pivot`.
fn rotate_about(p: (f64, f64), pivot: (f64, f64), degrees: f64) -> (f64, f64) {
    if degrees.abs() < 1e-9 {
        return p;
    }
    let a = degrees.to_radians();
    let (dx, dy) = (p.0 - pivot.0, p.1 - pivot.1);
    (
        pivot.0 + dx * a.cos() - dy * a.sin(),
        pivot.1 + dx * a.sin() + dy * a.cos(),
    )
}

fn dist_to_segment(p: (f64, f64), a: (f64, f64), b: (f64, f64)) -> f64 {
    let (abx, aby) = (b.0 - a.0, b.1 - a.1);
    let len2 = abx * abx + aby * aby;
    if len2 <= f64::EPSILON {
        return dist(p, a);
    }
    let t = (((p.0 - a.0) * abx + (p.1 - a.1) * aby) / len2).clamp(0.0, 1.0);
    dist(p, (a.0 + t * abx, a.1 + t * aby))
}

fn chebyshev_to_segment(p: (f64, f64), a: (f64, f64), b: (f64, f64)) -> f64 {
    let (abx, aby) = (b.0 - a.0, b.1 - a.1);
    let len2 = abx * abx + aby * aby;
    let t = if len2 <= f64::EPSILON {
        0.0
    } else {
        (((p.0 - a.0) * abx + (p.1 - a.1) * aby) / len2).clamp(0.0, 1.0)
    };
    let q = (a.0 + t * abx, a.1 + t * aby);
    (p.0 - q.0).abs().max((p.1 - q.1).abs())
}

fn point_in_polygon(p: (f64, f64), poly: &[(f64, f64)]) -> bool {
    let mut inside = false;
    let n = poly.len();
    let mut j = n - 1;
    for i in 0..n {
        let (xi, yi) = poly[i];
        let (xj, yj) = poly[j];
        let intersects = ((yi > p.1) != (yj > p.1))
            && (p.0 < (xj - xi) * (p.1 - yi) / (yj - yi + f64::EPSILON) + xi);
        if intersects {
            inside = !inside;
        }
        j = i;
    }
    inside
}

/// Trace the boundary of the occupied region as closed paths.
///
/// Walks the edges of the occupied cells' squares, linking shared corner points
/// into loops. This is exact for the rasterised region, deterministic, and stays
/// on the safe side: the resulting polygon runs on the cell boundary, i.e.
/// outside the pixel centres that represent copper.
pub fn trace_contours(raster: &Raster) -> Vec<Path> {
    // Corner (r, c) is the lower-left corner of cell (r, c), in half-cell units.
    let mut edges: Vec<((i64, i64), (i64, i64))> = Vec::new();
    for row in 0..raster.rows as i64 {
        for col in 0..raster.cols as i64 {
            if !raster.is_occupied(col, row) {
                continue;
            }
            // Cell (row, col) spans corners (row, col) .. (row+1, col+1).
            if !raster.is_occupied(col, row - 1) {
                // top edge: left "" right
                edges.push(((row, col), (row, col + 1)));
            }
            if !raster.is_occupied(col, row + 1) {
                // bottom edge
                edges.push(((row + 1, col + 1), (row + 1, col)));
            }
            if !raster.is_occupied(col - 1, row) {
                // left edge
                edges.push(((row + 1, col), (row, col)));
            }
            if !raster.is_occupied(col + 1, row) {
                // right edge
                edges.push(((row, col + 1), (row + 1, col + 1)));
            }
        }
    }
    if edges.is_empty() {
        return Vec::new();
    }

    use std::collections::HashMap;
    let mut outgoing: HashMap<(i64, i64), Vec<usize>> = HashMap::new();
    for (i, (a, _)) in edges.iter().enumerate() {
        outgoing.entry(*a).or_default().push(i);
    }
    for v in outgoing.values_mut() {
        v.sort_unstable();
    }

    let corner_world = |(r, c): (i64, i64)| -> (f64, f64) {
        (
            raster.origin.0 + (c as f64 - 0.5) * raster.resolution,
            raster.origin.1 + (r as f64 - 0.5) * raster.resolution,
        )
    };

    let mut used = vec![false; edges.len()];
    let mut paths = Vec::new();
    for start_edge in 0..edges.len() {
        if used[start_edge] {
            continue;
        }
        let start_corner = edges[start_edge].0;
        let mut points = vec![corner_world(start_corner)];
        let mut current_edge = start_edge;
        let mut guard = 0usize;
        loop {
            guard += 1;
            if guard > edges.len() + 8 {
                break;
            }
            used[current_edge] = true;
            let next_corner = edges[current_edge].1;
            points.push(corner_world(next_corner));
            if next_corner == start_corner {
                break;
            }
            let Some(candidates) = outgoing.get(&next_corner) else {
                break;
            };
            match candidates.iter().copied().find(|i| !used[*i]) {
                Some(next) => current_edge = next,
                None => break,
            }
        }
        if points.len() >= 4 {
            points.pop(); // the closing point repeats the first
            paths.push(Path {
                points,
                closed: true,
            });
        }
    }
    // Deterministic order: by position, then size.
    paths.sort_by(|a, b| {
        a.start()
            .partial_cmp(&b.start())
            .unwrap_or(std::cmp::Ordering::Equal)
            .then_with(|| a.points.len().cmp(&b.points.len()))
    });
    paths
}

/// Douglas-Peucker simplification.
///
/// Returns a subset of the input points, preserving the first and last.
pub fn simplify(points: &[(f64, f64)], tolerance: f64) -> Vec<(f64, f64)> {
    if points.len() < 3 || tolerance <= 0.0 {
        return points.to_vec();
    }
    let a = points[0];
    let b = *points.last().unwrap();
    let mut worst = 0.0f64;
    let mut worst_i = 0usize;
    for (i, p) in points.iter().enumerate().take(points.len() - 1).skip(1) {
        let d = dist_to_segment(*p, a, b);
        if d > worst {
            worst = d;
            worst_i = i;
        }
    }
    if worst <= tolerance {
        vec![a, b]
    } else {
        let mut left = simplify(&points[..=worst_i], tolerance);
        let right = simplify(&points[worst_i..], tolerance);
        left.pop(); // drop the duplicated split point
        left.extend(right);
        left
    }
}

/// Order paths greedily by nearest endpoint (deterministic).
pub fn order_paths(mut paths: Vec<Path>) -> Vec<Path> {
    if paths.len() <= 1 {
        return paths;
    }
    let mut remaining = std::mem::take(&mut paths);
    let mut ordered = Vec::with_capacity(remaining.len());
    // Start from the path whose start is lowest-left, so the result is stable.
    remaining.sort_by(|a, b| {
        a.start()
            .partial_cmp(&b.start())
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    let mut current = remaining.remove(0);
    ordered.push(current.clone());
    while !remaining.is_empty() {
        let last = current.end().or(current.start()).unwrap_or((0.0, 0.0));
        let mut best_i = 0usize;
        let mut best_d = f64::INFINITY;
        for (i, p) in remaining.iter().enumerate() {
            let d = p.start().map(|s| dist(last, s)).unwrap_or(f64::INFINITY);
            if d < best_d {
                best_d = d;
                best_i = i;
            }
        }
        let next = remaining.remove(best_i);
        current = next.clone();
        ordered.push(next);
    }
    ordered
}

/// The minimum distance from any path vertex to the copper, and where.
#[derive(Debug, Clone, PartialEq)]
pub struct ClearanceCheck {
    pub min_distance: f64,
    pub required: f64,
    pub at: (f64, f64),
    pub passed: bool,
}

/// Verify that every toolpath point keeps at least `required` from the copper.
///
/// `copper` must be the *undilated* copper raster.
pub fn verify_clearance(paths: &[Path], copper: &Raster, required: f64) -> Option<ClearanceCheck> {
    let search = required.max(0.0) + copper.resolution * 4.0;
    let mut worst: Option<(f64, (f64, f64))> = None;
    for p in paths {
        for v in &p.points {
            match copper.distance_to_occupied(v.0, v.1, search) {
                Some(d) => {
                    if worst.map(|(w, _)| d < w).unwrap_or(true) {
                        worst = Some((d, *v));
                    }
                }
                None => {
                    // Nothing within the search radius: comfortably clear.
                    return Some(ClearanceCheck {
                        min_distance: search,
                        required,
                        at: *v,
                        passed: true,
                    });
                }
            }
        }
    }
    worst.map(|(min_distance, at)| ClearanceCheck {
        min_distance,
        required,
        at,
        passed: min_distance + copper.resolution / 2.0 >= required,
    })
}

#[derive(Debug, thiserror::Error, PartialEq)]
pub enum GeometryError {
    #[error("图层没有任何图形，无法生成走刀")]
    EmptyLayer,
    #[error("图元引用了未定义的孔径 D{0}")]
    MissingAperture(u32),
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::gerber;

    fn bounds(w: f64, h: f64) -> Bounds {
        Bounds {
            min_x: 0.0,
            min_y: 0.0,
            max_x: w,
            max_y: h,
        }
    }

    #[test]
    fn disc_flash_fills_a_round_region() {
        let mut r = Raster::for_bounds(bounds(10.0, 10.0), 0.1, 1.0);
        r.fill_disc(5.0, 5.0, 1.0);
        let occupied = r.occupied_count();
        // Area = pi r^2 = 3.14 mm^2 -> ~314 cells of 0.01 mm^2 (within 5%).
        assert!(
            (occupied as f64 - 314.0).abs() / 314.0 < 0.05,
            "occupied {occupied}"
        );
        assert!(r.is_occupied(r.index_of(5.0, 5.0).0, r.index_of(5.0, 5.0).1));
        assert!(!r.is_occupied(r.index_of(7.0, 5.0).0, r.index_of(7.0, 5.0).1));
    }

    #[test]
    fn rect_and_obround_flashes_fill_expected_areas() {
        let mut r = Raster::for_bounds(bounds(10.0, 10.0), 0.1, 1.0);
        r.fill_rect(2.0, 2.0, 2.0, 2.0); // 4 mm^2 -> 400 cells
        assert!((r.occupied_count() as f64 - 400.0).abs() / 400.0 < 0.05);

        let mut r2 = Raster::for_bounds(bounds(10.0, 10.0), 0.1, 1.0);
        r2.fill_obround(5.0, 5.0, 2.0, 1.0); // stadium ~ (1*1 + pi*0.25) = 1.785 mm^2
        let area = r2.occupied_count() as f64 * 0.01;
        assert!((area - 1.785).abs() / 1.785 < 0.1, "area {area}");
    }

    #[test]
    fn capsule_covers_the_segment_and_ends() {
        let mut r = Raster::for_bounds(bounds(10.0, 10.0), 0.1, 1.0);
        r.fill_capsule((2.0, 5.0), (8.0, 5.0), 0.5);
        let area = r.occupied_count() as f64 * 0.01;
        // rectangle 6x1 + two half discs = 6 + pi*0.25 = 6.785
        assert!((area - 6.785).abs() / 6.785 < 0.05, "area {area}");
        let (c, row) = r.index_of(5.0, 5.0);
        assert!(r.is_occupied(c, row), "the capsule must contain its middle");
    }

    #[test]
    fn polygon_fill_uses_even_odd_rule() {
        let mut r = Raster::for_bounds(bounds(10.0, 10.0), 0.1, 1.0);
        r.fill_polygon(&[(1.0, 1.0), (4.0, 1.0), (4.0, 4.0), (1.0, 4.0)]);
        let area = r.occupied_count() as f64 * 0.01;
        assert!((area - 9.0).abs() / 9.0 < 0.08, "area {area}");
    }

    #[test]
    fn rasterisation_is_deterministic() {
        let text = "%TF.FileFunction,Copper,L1,Top*%\n%FSLAX46Y46*%\n%MOMM*%\n%LPD*%\nG01*\n%ADD10C,1.000000*%\nD10*\nX5000000Y5000000D03*\nX5000000Y5000000D02*\nX8000000Y5000000D01*\nM02*\n";
        let layer = gerber::parse(text).unwrap();
        let a = Raster::rasterize_layer(&layer, 0.1, 1.0).unwrap();
        let b = Raster::rasterize_layer(&layer, 0.1, 1.0).unwrap();
        assert_eq!(a.occupied_count(), b.occupied_count());
        assert_eq!(a, b);
    }

    #[test]
    fn dilation_grows_by_the_requested_radius() {
        let mut r = Raster::for_bounds(bounds(20.0, 20.0), 0.1, 1.0);
        r.fill_disc(10.0, 10.0, 1.0);
        let grown = r.dilate(0.5);
        assert!(grown.occupied_count() > r.occupied_count());
        // A point 1.4 mm from the centre was empty, and is inside after growing by 0.5.
        let outside = r.index_of(11.3, 10.0);
        assert!(!r.is_occupied(outside.0, outside.1));
        assert!(grown.is_occupied(outside.0, outside.1));
        // A point 1.6 mm away stays empty.
        let far = grown.index_of(11.7, 10.0);
        assert!(!grown.is_occupied(far.0, far.1));
    }

    #[test]
    fn dilation_contains_the_original() {
        let mut r = Raster::for_bounds(bounds(10.0, 10.0), 0.2, 1.0);
        r.fill_rect(5.0, 5.0, 2.0, 2.0);
        let grown = r.dilate(0.4);
        for row in 0..r.rows {
            for col in 0..r.cols {
                if r.is_occupied(col as i64, row as i64) {
                    assert!(grown.is_occupied(col as i64, row as i64));
                }
            }
        }
    }

    #[test]
    fn contours_are_closed_and_around_the_shape() {
        let mut r = Raster::for_bounds(bounds(10.0, 10.0), 0.1, 1.0);
        r.fill_disc(5.0, 5.0, 2.0);
        let paths = trace_contours(&r);
        assert_eq!(paths.len(), 1, "one contour expected");
        let p = &paths[0];
        assert!(p.closed);
        assert!(p.points.len() > 20);
        // Every contour point is adjacent to occupied space; check the extremes
        // are near the disc radius.
        let cx = 5.0;
        let cy = 5.0;
        let max_r = p
            .points
            .iter()
            .map(|(x, y)| ((x - cx).powi(2) + (y - cy).powi(2)).sqrt())
            .fold(0.0f64, f64::max);
        assert!((max_r - 2.0).abs() < 0.3, "max radius {max_r}");
    }

    #[test]
    fn a_ring_produces_two_contours() {
        let mut r = Raster::for_bounds(bounds(10.0, 10.0), 0.1, 1.0);
        r.fill_disc(5.0, 5.0, 3.0);
        // Punch a hole by clearing the centre manually.
        let mut r2 = r.clone();
        for row in 0..r2.rows {
            for col in 0..r2.cols {
                let (x, y) = r2.world(col, row);
                if ((x - 5.0).powi(2) + (y - 5.0).powi(2)).sqrt() < 1.0 {
                    // simulate a clear hole
                    let i = row * r2.cols + col;
                    if r2.cells[i] {
                        r2.cells[i] = false;
                        r2.occupied -= 1;
                    }
                }
            }
        }
        let paths = trace_contours(&r2);
        assert_eq!(paths.len(), 2, "outer and inner contour, got {}", paths.len());
        // One loop hugs the hole (small), the other the outer edge (large).
        let mut lengths: Vec<f64> = paths.iter().map(|p| p.length()).collect();
        lengths.sort_by(|a, b| a.partial_cmp(b).unwrap());
        assert!(lengths[0] < 10.0, "{lengths:?} (hole loop)");
        assert!(lengths[1] > 15.0, "{lengths:?} (outer loop)");
    }

    #[test]
    fn simplification_reduces_points_within_tolerance() {
        // Points along a straight line: DP should collapse them to two.
        let pts: Vec<(f64, f64)> = (0..=20).map(|i| (i as f64 * 0.1, 1.0)).collect();
        let simple = simplify(&pts, 0.01);
        assert_eq!(simple.len(), 2);
        assert_eq!(simple[0], pts[0]);
        assert_eq!(*simple.last().unwrap(), *pts.last().unwrap());
    }

    #[test]
    fn simplification_keeps_a_real_corner() {
        let pts = vec![(0.0, 0.0), (1.0, 0.0), (2.0, 0.0), (2.0, 2.0)];
        let simple = simplify(&pts, 0.05);
        assert!(simple.contains(&(2.0, 0.0)), "corner must survive: {simple:?}");
    }

    #[test]
    fn ordering_follows_the_nearest_endpoint() {
        let a = Path {
            points: vec![(5.0, 5.0), (6.0, 5.0)],
            closed: false,
        };
        let near = Path {
            points: vec![(6.1, 5.0), (7.0, 5.0)],
            closed: false,
        };
        let far = Path {
            points: vec![(20.0, 20.0), (21.0, 20.0)],
            closed: false,
        };
        let ordered = order_paths(vec![a.clone(), far.clone(), near.clone()]);
        assert_eq!(ordered.len(), 3);
        // Start is the lowest-left path (a), then its nearest neighbour (near).
        assert_eq!(ordered[0].start(), Some((5.0, 5.0)));
        assert_eq!(ordered[1].start(), Some((6.1, 5.0)));
    }

    #[test]
    fn clearance_check_detects_a_too_close_path() {
        let mut copper = Raster::for_bounds(bounds(10.0, 10.0), 0.1, 1.0);
        copper.fill_disc(5.0, 5.0, 1.0);
        let good = Path {
            points: vec![(7.0, 5.0), (7.0, 6.0)],
            closed: false,
        };
        let check = verify_clearance(&[good.clone()], &copper, 1.0).unwrap();
        assert!(check.passed, "{check:?}");

        let bad = Path {
            points: vec![(6.2, 5.0)],
            closed: false,
        };
        let check = verify_clearance(&[bad], &copper, 1.0).unwrap();
        assert!(!check.passed, "{check:?}");
        assert!(check.min_distance < 1.0);
    }

    #[test]
    fn empty_layer_is_an_error() {
        let text = "%TF.FileFunction,Copper,L1,Top*%\n%FSLAX46Y46*%\n%MOMM*%\n%ADD10C,1.0*%\nM02*\n";
        let layer = gerber::parse(text).unwrap();
        assert_eq!(
            Raster::rasterize_layer(&layer, 0.1, 1.0).unwrap_err(),
            GeometryError::EmptyLayer
        );
    }
}
