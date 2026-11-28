//! The little linear algebra geom needs: 3-vectors and a Jacobi eigensolver for
//! small symmetric matrices (3×3 plane fits, 4×4 quaternion superposition).
//! Everything is `f64` internally; positions arrive as `f32`.

pub(crate) type V3 = [f64; 3];

#[inline]
pub(crate) fn v3(p: [f32; 3]) -> V3 {
    [f64::from(p[0]), f64::from(p[1]), f64::from(p[2])]
}

#[inline]
pub(crate) fn sub(a: V3, b: V3) -> V3 {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
}

#[inline]
pub(crate) fn dot(a: V3, b: V3) -> f64 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}

#[inline]
pub(crate) fn dist2_f32(a: [f32; 3], b: [f32; 3]) -> f32 {
    let d = [a[0] - b[0], a[1] - b[1], a[2] - b[2]];
    d[0] * d[0] + d[1] * d[1] + d[2] * d[2]
}

#[inline]
pub(crate) fn dist(a: [f32; 3], b: [f32; 3]) -> f64 {
    let d = sub(v3(a), v3(b));
    dot(d, d).sqrt()
}

/// Arithmetic mean of `points`, `[0; 3]` for an empty slice.
pub(crate) fn mean<I: IntoIterator<Item = [f32; 3]>>(points: I) -> V3 {
    let mut s = [0.0; 3];
    let mut n = 0usize;
    for p in points {
        let p = v3(p);
        s[0] += p[0];
        s[1] += p[1];
        s[2] += p[2];
        n += 1;
    }
    if n == 0 {
        return s;
    }
    let n = n as f64;
    [s[0] / n, s[1] / n, s[2] / n]
}

#[inline]
pub(crate) fn add(a: V3, b: V3) -> V3 {
    [a[0] + b[0], a[1] + b[1], a[2] + b[2]]
}

#[inline]
pub(crate) fn scale(a: V3, s: f64) -> V3 {
    [a[0] * s, a[1] * s, a[2] * s]
}

#[inline]
pub(crate) fn cross(a: V3, b: V3) -> V3 {
    [
        a[1] * b[2] - a[2] * b[1],
        a[2] * b[0] - a[0] * b[2],
        a[0] * b[1] - a[1] * b[0],
    ]
}

#[inline]
pub(crate) fn norm(a: V3) -> f64 {
    dot(a, a).sqrt()
}

/// `a / |a|`, or `None` for a (near-)zero vector.
#[inline]
pub(crate) fn unit(a: V3) -> Option<V3> {
    let n = norm(a);
    (n > 1e-9).then(|| scale(a, 1.0 / n))
}

/// Angle between two vectors in degrees (0 if either is zero).
pub(crate) fn angle_deg(a: V3, b: V3) -> f64 {
    let n = norm(a) * norm(b);
    if n < 1e-12 {
        return 0.0;
    }
    (dot(a, b) / n).clamp(-1.0, 1.0).acos().to_degrees()
}

/// Least-squares plane through `points`: `(centroid, unit normal)`. The
/// normal's sign is arbitrary. Needs at least three points.
pub(crate) fn plane_fit(points: &[[f32; 3]]) -> (V3, V3) {
    let c = mean(points.iter().copied());
    let mut cov = [[0.0f64; 3]; 3];
    for p in points {
        let d = sub(v3(*p), c);
        for (i, row) in cov.iter_mut().enumerate() {
            for (j, cell) in row.iter_mut().enumerate() {
                *cell += d[i] * d[j];
            }
        }
    }
    let (vals, vecs) = jacobi_eigen(cov);
    let k = (0..3)
        .min_by(|&a, &b| vals[a].total_cmp(&vals[b]))
        .unwrap_or(0);
    (c, [vecs[0][k], vecs[1][k], vecs[2][k]])
}

/// Eigen-decomposition of a symmetric `N×N` matrix by cyclic Jacobi rotations.
/// Returns `(eigenvalues, eigenvectors)` with eigenvector `k` in column `k`
/// (`vectors[i][k]`), unsorted.
// Index loops mirror the textbook rotation formulas; iterators would obscure them.
#[allow(clippy::needless_range_loop)]
pub(crate) fn jacobi_eigen<const N: usize>(mut a: [[f64; N]; N]) -> ([f64; N], [[f64; N]; N]) {
    let mut v = [[0.0; N]; N];
    for (i, row) in v.iter_mut().enumerate() {
        row[i] = 1.0;
    }
    for _sweep in 0..64 {
        let mut off = 0.0;
        for i in 0..N {
            for j in (i + 1)..N {
                off += a[i][j] * a[i][j];
            }
        }
        let scale: f64 = (0..N).map(|i| a[i][i] * a[i][i]).sum::<f64>() + off;
        if off <= 1e-30 * scale.max(1e-300) {
            break;
        }
        for p in 0..N {
            for q in (p + 1)..N {
                if a[p][q].abs() < 1e-300 {
                    continue;
                }
                let theta = (a[q][q] - a[p][p]) / (2.0 * a[p][q]);
                let t = theta.signum() / (theta.abs() + (theta * theta + 1.0).sqrt());
                let t = if theta == 0.0 { 1.0 } else { t };
                let c = 1.0 / (t * t + 1.0).sqrt();
                let s = t * c;
                for k in 0..N {
                    let akp = a[k][p];
                    let akq = a[k][q];
                    a[k][p] = c * akp - s * akq;
                    a[k][q] = s * akp + c * akq;
                }
                for k in 0..N {
                    let apk = a[p][k];
                    let aqk = a[q][k];
                    a[p][k] = c * apk - s * aqk;
                    a[q][k] = s * apk + c * aqk;
                }
                for row in v.iter_mut() {
                    let vkp = row[p];
                    let vkq = row[q];
                    row[p] = c * vkp - s * vkq;
                    row[q] = s * vkp + c * vkq;
                }
            }
        }
    }
    let mut values = [0.0; N];
    for (i, val) in values.iter_mut().enumerate() {
        *val = a[i][i];
    }
    (values, v)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn jacobi_recovers_a_known_spectrum() {
        let m = [[2.0, 1.0, 0.0], [1.0, 2.0, 0.0], [0.0, 0.0, 5.0]];
        let (vals, vecs) = jacobi_eigen(m);
        let mut sorted = vals;
        sorted.sort_by(f64::total_cmp);
        assert!((sorted[0] - 1.0).abs() < 1e-12);
        assert!((sorted[1] - 3.0).abs() < 1e-12);
        assert!((sorted[2] - 5.0).abs() < 1e-12);
        // A v = lambda v for every column.
        for k in 0..3 {
            for i in 0..3 {
                let av: f64 = (0..3).map(|j| m[i][j] * vecs[j][k]).sum();
                assert!((av - vals[k] * vecs[i][k]).abs() < 1e-10);
            }
        }
    }
}
