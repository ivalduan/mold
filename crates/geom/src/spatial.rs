//! KD-tree over atom positions, backed by `kiddo`.
//!
//! Built once over a `&[[f32; 3]]` (what [`mold_mol::Arena::positions`] and
//! [`mold_mol::Arena::pose_positions`] hand out) and queried many times: a
//! receptor's tree serves every pose docked into it.

use kiddo::{ImmutableKdTree, SquaredEuclidean};

/// A static KD-tree over a slice of points. Items are indices into that slice.
pub struct SpatialIndex {
    // `None` for an empty point set (kiddo cannot build an empty tree).
    tree: Option<ImmutableKdTree<f32, 3>>,
    len: usize,
}

impl std::fmt::Debug for SpatialIndex {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SpatialIndex")
            .field("len", &self.len)
            .finish_non_exhaustive()
    }
}

/// One neighbour found by a query: its index in the indexed slice and its distance in Å.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Neighbour {
    pub index: u32,
    pub distance: f32,
}

impl SpatialIndex {
    /// Build the tree. O(n log n).
    ///
    /// # Panics
    /// If `points` has more than `u32::MAX` entries or contains non-finite coordinates.
    pub fn new(points: &[[f32; 3]]) -> Self {
        assert!(
            u32::try_from(points.len()).is_ok(),
            "SpatialIndex holds at most u32::MAX points"
        );
        let tree = if points.is_empty() {
            None
        } else {
            Some(
                ImmutableKdTree::new_from_slice(points)
                    .expect("KD-tree construction failed (non-finite coordinates?)"),
            )
        };
        SpatialIndex {
            tree,
            len: points.len(),
        }
    }

    pub fn len(&self) -> usize {
        self.len
    }

    pub fn is_empty(&self) -> bool {
        self.len == 0
    }

    /// The closest indexed point to `query`, or `None` if the index is empty.
    pub fn nearest(&self, query: [f32; 3]) -> Option<Neighbour> {
        let tree = self.tree.as_ref()?;
        let n = tree
            .query(&query)
            .nearest_one::<SquaredEuclidean<f32>>()
            .execute();
        Some(Neighbour {
            index: n.item,
            distance: n.distance.sqrt(),
        })
    }

    /// Call `visit` for every indexed point within `radius` Å of `query`
    /// (inclusive), in no particular order. No allocation.
    pub fn for_each_within(&self, query: [f32; 3], radius: f32, mut visit: impl FnMut(Neighbour)) {
        let Some(tree) = self.tree.as_ref() else {
            return;
        };
        tree.query(&query)
            .within::<SquaredEuclidean<f32>>(radius * radius)
            .unsorted()
            .visit(|r| {
                visit(Neighbour {
                    index: r.item,
                    distance: r.distance.sqrt(),
                })
            });
    }

    /// Every indexed point within `radius` Å of `query`, sorted by distance.
    pub fn within(&self, query: [f32; 3], radius: f32) -> Vec<Neighbour> {
        let mut out = Vec::new();
        self.for_each_within(query, radius, |n| out.push(n));
        out.sort_by(|a, b| {
            a.distance
                .total_cmp(&b.distance)
                .then(a.index.cmp(&b.index))
        });
        out
    }

    /// Number of indexed points within `radius` Å of `query`.
    pub fn count_within(&self, query: [f32; 3], radius: f32) -> usize {
        let mut n = 0;
        self.for_each_within(query, radius, |_| n += 1);
        n
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn brute_within(points: &[[f32; 3]], q: [f32; 3], r: f32) -> Vec<u32> {
        let mut v: Vec<u32> = points
            .iter()
            .enumerate()
            .filter(|(_, p)| crate::linalg::dist2_f32(**p, q) <= r * r)
            .map(|(i, _)| i as u32)
            .collect();
        v.sort_unstable();
        v
    }

    #[test]
    fn queries_match_brute_force() {
        // A deterministic jittered lattice, 1000 points.
        let mut points = Vec::new();
        let mut s = 12345u64;
        for i in 0..10 {
            for j in 0..10 {
                for k in 0..10 {
                    s = s
                        .wrapping_mul(6364136223846793005)
                        .wrapping_add(1442695040888963407);
                    let jit = ((s >> 40) as f32 / (1u64 << 24) as f32) - 0.5;
                    points.push([i as f32 * 1.5 + jit, j as f32 * 1.5 - jit, k as f32 * 1.5]);
                }
            }
        }
        let index = SpatialIndex::new(&points);
        assert_eq!(index.len(), 1000);
        for q in [[3.3, 4.1, 7.7], [0.0, 0.0, 0.0], [20.0, 20.0, 20.0]] {
            let mut got: Vec<u32> = index.within(q, 2.5).iter().map(|n| n.index).collect();
            got.sort_unstable();
            assert_eq!(got, brute_within(&points, q, 2.5));
            assert_eq!(index.count_within(q, 2.5), got.len());
            let nearest = index.nearest(q).unwrap();
            let best = points
                .iter()
                .map(|p| crate::linalg::dist2_f32(*p, q).sqrt())
                .fold(f32::INFINITY, f32::min);
            assert!((nearest.distance - best).abs() < 1e-5);
        }
    }

    #[test]
    fn empty_index_answers_nothing() {
        let index = SpatialIndex::new(&[]);
        assert!(index.is_empty());
        assert_eq!(index.nearest([0.0; 3]), None);
        assert!(index.within([0.0; 3], 100.0).is_empty());
    }
}
