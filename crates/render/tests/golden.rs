//! Headless render test: the EGFR fixture drawn with each representation to
//! an offscreen target, compared by perceptual hash against committed golden
//! hashes. Skips (passes, with the reason printed) when there is no adapter.

mod common;

use mold_render::hash::{DHash, dhash};
use mold_render::{
    FrameParams, Image, OFFSCREEN_FORMAT, OffscreenTarget, OrbitCamera, Renderer, Representation,
    Scene, guess_bonds,
};

const WIDTH: u32 = 320;
const HEIGHT: u32 = 240;
/// Bits of 256 that may differ from the golden hash: room for driver and
/// rasterisation differences at the silhouette's edges, not for a moved,
/// rotated or rescaled structure.
const TOLERANCE: u32 = 12;
/// Percentage points the covered fraction may differ from the golden one.
/// Coverage is what tells the representations apart (the dHash mostly sees
/// the silhouette, which they share): about 23% / 12% / 7%.
const COVERAGE_TOLERANCE: f32 = 1.5;

/// Golden dHash and coverage (% of pixels not background) per
/// representation, recorded on an NVIDIA RTX 5060 Laptop GPU (Vulkan, the
/// first primary backend wgpu picks on Windows). After a deliberate change
/// to the picture, re-record from the test's output and look at the BMP it
/// writes under `target/render-tests/` first.
const GOLDEN: [(Representation, DHash, f32); 3] = [
    (
        Representation::SpaceFilling,
        DHash([
            0x0000000000000038,
            0x00580218030c0498,
            0x04d8055802b8029c,
            0x06b0000000000000,
        ]),
        22.80,
    ),
    (
        Representation::BallAndStick,
        DHash([
            0x0000000000000020,
            0x0038023807580178,
            0x017801b800580298,
            0x0690000000000000,
        ]),
        11.60,
    ),
    (
        Representation::Sticks,
        DHash([
            0x0000000000000020,
            0x0030023802d80138,
            0x017800b800580298,
            0x0290000000000000,
        ]),
        7.40,
    ),
];

fn render(gpu: &mold_render::Gpu, representation: Representation) -> Image {
    render_turned(gpu, representation, 0.0)
}

/// The golden view with the camera turned by `yaw` radians.
fn render_turned(gpu: &mold_render::Gpu, representation: Representation, yaw: f32) -> Image {
    let arena = common::reference_arena();
    let bonds = guess_bonds(&arena);
    let renderer = Renderer::new(&gpu.device, OFFSCREEN_FORMAT);
    let scene = Scene::new(&gpu.device, &renderer, &arena, &bonds).expect("scene");
    let bounds = scene.visible_bounds(&arena).expect("atoms");
    let mut camera = OrbitCamera::framing(bounds, WIDTH as f32 / HEIGHT as f32);
    camera.yaw += yaw;
    let target = OffscreenTarget::new(&gpu.device, WIDTH, HEIGHT);
    target.render(
        &gpu.device,
        &gpu.queue,
        &renderer,
        &scene,
        &FrameParams::new(camera, representation),
    );
    target.read_rgba(&gpu.device, &gpu.queue)
}

/// Percentage of pixels that differ from the corner (background) pixel.
fn coverage(image: &Image) -> f32 {
    let background = image.pixel(0, 0);
    let drawn = image
        .rgba
        .as_chunks::<4>()
        .0
        .iter()
        .filter(|px| **px != background)
        .count();
    100.0 * drawn as f32 / (image.width * image.height) as f32
}

#[test]
fn representations_match_their_golden_hashes() {
    let Some(gpu) = common::gpu_or_skip("representations_match_their_golden_hashes") else {
        return;
    };
    let mut failures = Vec::new();
    for (representation, golden, golden_coverage) in GOLDEN {
        let image = render(&gpu, representation);
        let path = common::write_bmp(&image, &format!("egfr-{representation:?}"));
        let hash = dhash(&image);
        let covered = coverage(&image);
        let distance = hash.distance(&golden);
        eprintln!(
            "{representation:?}: dhash {hash}, {distance} bits from golden; \
             {covered:.2}% covered (golden {golden_coverage:.2}%); image {}",
            path.display()
        );
        if distance > TOLERANCE {
            failures.push(format!(
                "{representation:?}: dhash {hash} is {distance} bits from golden \
                 {golden} (tolerance {TOLERANCE})"
            ));
        }
        if (covered - golden_coverage).abs() > COVERAGE_TOLERANCE {
            failures.push(format!(
                "{representation:?}: {covered:.2}% covered, golden {golden_coverage:.2}% \
                 (tolerance {COVERAGE_TOLERANCE} points)"
            ));
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

/// The goldens themselves tell the representations apart.
#[test]
fn goldens_are_distinct() {
    for (i, a) in GOLDEN.iter().enumerate() {
        for b in &GOLDEN[i + 1..] {
            assert!(
                (a.2 - b.2).abs() > 2.0 * COVERAGE_TOLERANCE,
                "{:?} and {:?} cover alike",
                a.0,
                b.0
            );
        }
    }
}

/// Rendering is deterministic on one device: the same frame twice is the
/// same bytes.
#[test]
fn rendering_is_deterministic() {
    let Some(gpu) = common::gpu_or_skip("rendering_is_deterministic") else {
        return;
    };
    let a = render(&gpu, Representation::BallAndStick);
    let b = render(&gpu, Representation::BallAndStick);
    assert!(a == b, "two renders of the same frame differ");
}

/// The tolerance is tight enough to catch a changed picture: the same
/// structure seen from 15° further round is rejected.
#[test]
fn a_turned_camera_fails_the_golden() {
    let Some(gpu) = common::gpu_or_skip("a_turned_camera_fails_the_golden") else {
        return;
    };
    let (representation, golden, _) = GOLDEN[0];
    let turned = dhash(&render_turned(&gpu, representation, 15f32.to_radians()));
    let distance = turned.distance(&golden);
    eprintln!("turned 15°: {distance} bits from golden");
    assert!(distance > 2 * TOLERANCE, "{distance} bits");
}
