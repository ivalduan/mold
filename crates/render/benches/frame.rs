//! Frame-time budget: 500 poses × ~2,400 atoms (the EGFR fixture, replicated
//! and perturbed in memory) with spheres + sticks, offscreen at 1920×1080.
//! Measures GPU frame time as submit + wait. Budget: 16.6 ms per frame.
//! Cartoon ribbons are not in mold-render yet, so the number excludes them.
//!
//! ```text
//! cargo bench -p mold-render --bench frame
//! ```
//! Prints the adapter first. With no adapter it prints why and does nothing.

#[path = "../tests/common/mod.rs"]
mod common;

use std::time::Duration;

use criterion::{Criterion, criterion_group, criterion_main};
use mold_mol::{Arena, PoseId};
use mold_render::{
    FrameParams, OFFSCREEN_FORMAT, OffscreenTarget, OrbitCamera, Renderer, Representation, Scene,
    guess_bonds,
};

const POSES: usize = 500;
const WIDTH: u32 = 1920;
const HEIGHT: u32 = 1080;

/// Deterministic jitter in [-1, 1].
fn jitter(seed: u64) -> f32 {
    let mut z = seed.wrapping_add(0x9e37_79b9_7f4a_7c15);
    z = (z ^ (z >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
    ((z ^ (z >> 31)) >> 40) as f32 / (1u64 << 23) as f32 - 1.0
}

/// 500 copies of the fixture. `spread`: laid out on a 25 × 20 grid, all
/// visible side by side; otherwise superimposed with ±0.5 Å noise, like
/// docking poses in one receptor frame (the overdraw worst case).
fn poses(spread: bool) -> Arena {
    let source = common::reference_arena();
    let mut arena = Arena::new();
    arena.reserve_atoms(source.atom_count() * POSES);
    for i in 0..POSES {
        let offset = if spread {
            [(i % 25) as f32 * 70.0, (i / 25) as f32 * 70.0, 0.0]
        } else {
            [0.0; 3]
        };
        let seed = (i as u64) << 32;
        common::copy_pose(&source, PoseId(0), &mut arena, "pose", |p| {
            let k = seed ^ (p[0].to_bits() as u64);
            [
                p[0] + offset[0] + 0.5 * jitter(k),
                p[1] + offset[1] + 0.5 * jitter(k + 1),
                p[2] + offset[2] + 0.5 * jitter(k + 2),
            ]
        });
    }
    arena
}

fn frame(c: &mut Criterion) {
    let gpu = match mold_render::Gpu::headless() {
        Ok(gpu) => gpu,
        Err(error) => {
            eprintln!("SKIPPED frame bench: {error}");
            return;
        }
    };
    eprintln!("adapter: {}", gpu.adapter_name());
    let renderer = Renderer::new(&gpu.device, OFFSCREEN_FORMAT);
    let target = OffscreenTarget::new(&gpu.device, WIDTH, HEIGHT);
    let mut group = c.benchmark_group("frame_500_poses_1920x1080");
    group
        .sample_size(30)
        .measurement_time(Duration::from_secs(8));
    for (name, spread) in [("grid", true), ("superimposed", false)] {
        let arena = poses(spread);
        let bonds = guess_bonds(&arena);
        let scene = Scene::new(&gpu.device, &renderer, &arena, &bonds).expect("scene");
        eprintln!(
            "{name}: {} poses, {} atoms, {} bonds",
            scene.pose_count(),
            scene.atom_count(),
            scene.bond_count()
        );
        let camera = OrbitCamera::framing(
            scene.visible_bounds(&arena).unwrap(),
            WIDTH as f32 / HEIGHT as f32,
        );
        for representation in [Representation::BallAndStick, Representation::SpaceFilling] {
            let params = FrameParams::new(camera, representation);
            group.bench_function(format!("{name}/{representation:?}"), |b| {
                b.iter(|| {
                    target.render(&gpu.device, &gpu.queue, &renderer, &scene, &params);
                    gpu.wait();
                })
            });
        }
    }
    group.finish();
}

criterion_group!(benches, frame);
criterion_main!(benches);
