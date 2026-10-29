//! Pure geometry for drawing packets on edges.

/// Point at fraction `t` (0..=1) of a polyline's length; `None` if there are no points.
pub fn point_along(points: &[(f64, f64)], t: f64) -> Option<(f64, f64)> {
    let first = *points.first()?;
    let len = |a: (f64, f64), b: (f64, f64)| ((b.0 - a.0).powi(2) + (b.1 - a.1).powi(2)).sqrt();
    let total: f64 = points.windows(2).map(|w| len(w[0], w[1])).sum();
    if total == 0.0 {
        return Some(first);
    }
    let mut left = t.clamp(0.0, 1.0) * total;
    for w in points.windows(2) {
        let l = len(w[0], w[1]);
        if l > 0.0 && left <= l {
            let k = left / l;
            return Some((w[0].0 + (w[1].0 - w[0].0) * k, w[0].1 + (w[1].1 - w[0].1) * k));
        }
        left -= l;
    }
    points.last().copied()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn interpolates_by_arc_length() {
        assert_eq!(point_along(&[(0.0, 0.0), (10.0, 0.0)], 0.5), Some((5.0, 0.0)));
        assert_eq!(point_along(&[(0.0, 0.0), (10.0, 0.0), (10.0, 10.0)], 0.75), Some((10.0, 5.0)));
        assert_eq!(point_along(&[(0.0, 0.0), (10.0, 0.0)], 2.0), Some((10.0, 0.0)));
        assert_eq!(point_along(&[(0.0, 0.0), (10.0, 0.0)], -1.0), Some((0.0, 0.0)));
    }

    #[test]
    fn degenerate_paths() {
        assert_eq!(point_along(&[], 0.5), None);
        assert_eq!(point_along(&[(3.0, 4.0)], 0.5), Some((3.0, 4.0)));
        assert_eq!(point_along(&[(3.0, 4.0), (3.0, 4.0)], 0.5), Some((3.0, 4.0)));
    }
}
