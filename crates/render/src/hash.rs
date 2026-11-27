//! A small perceptual hash (dHash) for golden-image tests: 32 bytes in the
//! repository instead of a reference image, tolerant of the last-bit
//! differences between drivers.

use crate::renderer::Image;

/// 256-bit difference hash: the image is box-filtered to 17×16 grey cells
/// and each bit says whether a cell is brighter than its right-hand
/// neighbour. 256 bits rather than the classic 64, so the hash sees the
/// texture of a structure (spheres vs sticks), not just its silhouette.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct DHash(pub [u64; 4]);

impl DHash {
    pub const BITS: u32 = 256;

    /// Number of differing bits.
    pub fn distance(&self, other: &DHash) -> u32 {
        self.0
            .iter()
            .zip(&other.0)
            .map(|(a, b)| (a ^ b).count_ones())
            .sum()
    }
}

impl std::fmt::Display for DHash {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let [a, b, c, d] = self.0;
        write!(f, "[0x{a:016x}, 0x{b:016x}, 0x{c:016x}, 0x{d:016x}]")
    }
}

const COLS: usize = 17;
const ROWS: usize = 16;
/// In 0–255 luma.
const MARGIN: f32 = 1.5;

pub fn dhash(image: &Image) -> DHash {
    let cells = grey_cells(image, COLS, ROWS);
    let mut words = [0u64; 4];
    for y in 0..ROWS {
        for x in 0..COLS - 1 {
            let bit = y * (COLS - 1) + x;
            // The margin keeps flat areas (background) at 0 instead of
            // following the last bit of the driver's rounding.
            if cells[y * COLS + x] > cells[y * COLS + x + 1] + MARGIN {
                words[bit / 64] |= 1 << (63 - bit % 64);
            }
        }
    }
    DHash(words)
}

/// Mean luma of a `cols × rows` grid of boxes over the image.
fn grey_cells(image: &Image, cols: usize, rows: usize) -> Vec<f32> {
    let (w, h) = (image.width as usize, image.height as usize);
    let mut sums = vec![0f32; cols * rows];
    let mut counts = vec![0u32; cols * rows];
    for y in 0..h {
        let cy = y * rows / h.max(1);
        for x in 0..w {
            let cx = x * cols / w.max(1);
            let i = (y * w + x) * 4;
            let [r, g, b] = [image.rgba[i], image.rgba[i + 1], image.rgba[i + 2]];
            let luma = 0.299 * f32::from(r) + 0.587 * f32::from(g) + 0.114 * f32::from(b);
            sums[cy * cols + cx] += luma;
            counts[cy * cols + cx] += 1;
        }
    }
    sums.iter()
        .zip(&counts)
        .map(|(s, &n)| if n == 0 { 0.0 } else { s / n as f32 })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn image(width: u32, height: u32, f: impl Fn(u32, u32) -> u8) -> Image {
        let mut rgba = Vec::new();
        for y in 0..height {
            for x in 0..width {
                let v = f(x, y);
                rgba.extend_from_slice(&[v, v, v, 255]);
            }
        }
        Image {
            width,
            height,
            rgba,
        }
    }

    #[test]
    fn gradients_hash_to_all_ones_or_zeros() {
        let falling = image(170, 160, |x, _| 255 - x as u8);
        let rising = image(170, 160, |x, _| x as u8);
        assert_eq!(dhash(&falling), DHash([u64::MAX; 4]));
        assert_eq!(dhash(&rising), DHash([0; 4]));
        assert_eq!(dhash(&falling).distance(&dhash(&rising)), 256);
    }

    #[test]
    fn small_noise_changes_few_bits_and_shapes_change_many() {
        let disc = |dx: f32| {
            move |x: u32, y: u32| {
                let (fx, fy) = (x as f32 - 100.0 - dx, y as f32 - 60.0);
                if fx * fx + fy * fy < 45.0 * 45.0 {
                    200
                } else {
                    20
                }
            }
        };
        let a = dhash(&image(200, 120, disc(0.0)));
        let noisy = dhash(&image(200, 120, |x, y| {
            disc(0.0)(x, y).saturating_add(((x * 7 + y * 13) % 3) as u8)
        }));
        assert!(a.distance(&noisy) <= 4, "{}", a.distance(&noisy));
        let moved = dhash(&image(200, 120, disc(50.0)));
        assert!(a.distance(&moved) >= 20, "{}", a.distance(&moved));
    }
}
