//! Pick round-trip: render offscreen, read the atom id back at a pixel, and
//! check it against an exact CPU ray cast. Skips with no adapter.

mod common;

use glam::Vec3;
use mold_mol::{Arena, PoseId};
use mold_render::ray::{cast_sticks, ray_sphere};
use mold_render::{
    FrameParams, Gpu, OFFSCREEN_FORMAT, OffscreenTarget, OrbitCamera, Pick, Renderer,
    Representation, Scene, guess_bonds, pick,
};

const WIDTH: u32 = 480;
const HEIGHT: u32 = 360;

struct Setup {
    gpu: Gpu,
    renderer: Renderer,
    target: OffscreenTarget,
}

impl Setup {
    fn new(gpu: Gpu) -> Self {
        let renderer = Renderer::new(&gpu.device, OFFSCREEN_FORMAT);
        let target = OffscreenTarget::new(&gpu.device, WIDTH, HEIGHT);
        Self {
            gpu,
            renderer,
            target,
        }
    }

    fn scene(&self, arena: &Arena) -> Scene {
        Scene::new(&self.gpu.device, &self.renderer, arena, &guess_bonds(arena)).expect("scene")
    }

    fn render(&self, scene: &Scene, params: &FrameParams) {
        self.target.render(
            &self.gpu.device,
            &self.gpu.queue,
            &self.renderer,
            scene,
            params,
        );
    }

    fn pick(&self, scene: &Scene, x: f32, y: f32) -> Option<Pick> {
        pick(
            &self.gpu.device,
            &self.gpu.queue,
            &self.target.frame,
            scene,
            x,
            y,
        )
    }
}

/// The atom a ray through the centre of pixel `(x, y)` hits first (atom
/// spheres of `atoms`, plus `bonds` as sticks when the representation draws
/// them), if that is unambiguous: `Err` when a hit on another atom is within
/// 0.02 Å of the nearest one.
fn oracle(
    arena: &Arena,
    camera: &OrbitCamera,
    representation: Representation,
    atoms: std::ops::Range<usize>,
    bonds: &[[u32; 2]],
    x: u32,
    y: u32,
) -> Result<Option<u32>, ()> {
    let ray = camera.ray_through(x as f32 + 0.5, y as f32 + 0.5, WIDTH as f32, HEIGHT as f32);
    let positions = arena.positions();
    let elements = arena.elements();
    let mut hits: Vec<(u32, f32)> = atoms
        .filter_map(|i| {
            ray_sphere(
                &ray,
                Vec3::from(positions[i]),
                representation.sphere_radius(elements[i]),
            )
            .map(|t| (i as u32, t))
        })
        .collect();
    if representation.draws_sticks() {
        let radius = representation.radii().stick_radius;
        hits.extend(
            bonds
                .iter()
                .filter_map(|&bond| cast_sticks(&ray, positions, &[bond], radius)),
        );
    }
    hits.sort_by(|a, b| a.1.total_cmp(&b.1));
    let Some(&(best, t)) = hits.first() else {
        return Ok(None);
    };
    match hits.iter().find(|h| h.0 != best) {
        Some(&(_, runner_up)) if runner_up - t < 0.02 => Err(()),
        _ => Ok(Some(best)),
    }
}

/// Over a grid of pixels, the GPU id buffer agrees with an exact CPU ray cast
/// wherever the CPU answer is unambiguous: background where it misses, the
/// same atom where it hits.
#[test]
fn gpu_pick_agrees_with_cpu_ray_cast() {
    let Some(gpu) = common::gpu_or_skip("gpu_pick_agrees_with_cpu_ray_cast") else {
        return;
    };
    let setup = Setup::new(gpu);
    let arena = common::reference_arena();
    let scene = setup.scene(&arena);
    let camera = OrbitCamera::framing(
        scene.visible_bounds(&arena).unwrap(),
        WIDTH as f32 / HEIGHT as f32,
    );
    let representation = Representation::SpaceFilling;
    setup.render(&scene, &FrameParams::new(camera, representation));

    let (mut checked, mut hits, mut mismatches) = (0, 0, Vec::new());
    for y in (3..HEIGHT).step_by(12) {
        for x in (5..WIDTH).step_by(12) {
            let Ok(expected) = oracle(
                &arena,
                &camera,
                representation,
                0..arena.atom_count(),
                &[],
                x,
                y,
            ) else {
                continue;
            };
            let got = setup.pick(&scene, x as f32 + 0.5, y as f32 + 0.5);
            checked += 1;
            if expected.is_some() {
                hits += 1;
            }
            if got.map(|p| p.atom) != expected {
                mismatches.push(format!("({x}, {y}): gpu {got:?}, cpu {expected:?}"));
            }
        }
    }
    eprintln!(
        "{checked} pixels checked, {hits} on atoms, {} mismatches",
        mismatches.len()
    );
    assert!(hits > 200, "only {hits} pixels on atoms");
    // Silhouette pixels can round either way; anything more is a bug.
    assert!(
        mismatches.len() * 100 <= checked,
        "{} of {checked} picks disagree:\n{}",
        mismatches.len(),
        mismatches.join("\n")
    );
}

/// Two poses side by side: clicking a known atom of the second returns
/// `PoseId(1)` and that atom; hiding the pose makes the same pixel miss it.
#[test]
fn pick_returns_pose_and_atom_and_respects_visibility() {
    let Some(gpu) = common::gpu_or_skip("pick_returns_pose_and_atom_and_respects_visibility")
    else {
        return;
    };
    let setup = Setup::new(gpu);
    let source = common::reference_arena();
    let mut arena = Arena::new();
    common::copy_pose(&source, PoseId(0), &mut arena, "left", |p| p);
    common::copy_pose(&source, PoseId(0), &mut arena, "right", |p| {
        [p[0] + 70.0, p[1], p[2]]
    });
    let mut scene = setup.scene(&arena);
    let camera = OrbitCamera::framing(
        scene.visible_bounds(&arena).unwrap(),
        WIDTH as f32 / HEIGHT as f32,
    );
    let representation = Representation::SpaceFilling;
    let params = FrameParams::new(camera, representation);
    setup.render(&scene, &params);

    // Known atom: the first atom of pose 1 (in pose order) whose projected
    // centre the CPU says is unambiguously itself.
    let second = arena.pose(PoseId(1)).unwrap().atoms.clone();
    let all = 0..arena.atom_count();
    let (atom, pixel) = second
        .clone()
        .filter_map(|atom| {
            let p = camera.project(
                Vec3::from(arena.positions()[atom]),
                WIDTH as f32,
                HEIGHT as f32,
            )?;
            let (x, y) = (p.x as u32, p.y as u32);
            (x < WIDTH && y < HEIGHT).then_some((atom as u32, (x, y)))
        })
        .find(|&(atom, (x, y))| {
            oracle(&arena, &camera, representation, all.clone(), &[], x, y) == Ok(Some(atom))
        })
        .expect("some atom of pose 1 is in front");
    let (x, y) = (pixel.0 as f32 + 0.5, pixel.1 as f32 + 0.5);
    assert_eq!(
        setup.pick(&scene, x, y),
        Some(Pick {
            pose: PoseId(1),
            atom
        })
    );

    scene.show_only(Some(PoseId(0)));
    setup.render(&scene, &params);
    let hidden = setup.pick(&scene, x, y);
    assert!(
        hidden.is_none_or(|p| p.pose == PoseId(0)),
        "hidden pose 1 was picked: {hidden:?}"
    );
    // Outside the target is a miss, not a panic.
    assert_eq!(setup.pick(&scene, -1.0, 5.0), None);
    assert_eq!(setup.pick(&scene, WIDTH as f32, 5.0), None);
}

/// Sticks: over a grid of pixels, the GPU id buffer agrees with an exact CPU
/// ray cast against the joints and the bond cylinders (each half carrying its
/// atom's id).
#[test]
fn sticks_pick_agrees_with_cpu_ray_cast() {
    let Some(gpu) = common::gpu_or_skip("sticks_pick_agrees_with_cpu_ray_cast") else {
        return;
    };
    let setup = Setup::new(gpu);
    let arena = common::reference_arena();
    let scene = setup.scene(&arena);
    let bonds = guess_bonds(&arena);
    let mut camera = OrbitCamera::framing(
        scene.visible_bounds(&arena).unwrap(),
        WIDTH as f32 / HEIGHT as f32,
    );
    // Close enough that sticks are several pixels wide, so most samples are
    // inside a stick rather than on its edge.
    camera.distance *= 0.3;
    let representation = Representation::Sticks;
    setup.render(&scene, &FrameParams::new(camera, representation));
    let (mut checked, mut hits, mut mismatches) = (0, 0, Vec::new());
    for y in (1..HEIGHT).step_by(5) {
        for x in (2..WIDTH).step_by(5) {
            let Ok(expected) = oracle(
                &arena,
                &camera,
                representation,
                0..arena.atom_count(),
                &bonds,
                x,
                y,
            ) else {
                continue;
            };
            let got = setup.pick(&scene, x as f32 + 0.5, y as f32 + 0.5);
            checked += 1;
            if expected.is_some() {
                hits += 1;
            }
            if got.map(|p| p.atom) != expected {
                mismatches.push(format!("({x}, {y}): gpu {got:?}, cpu {expected:?}"));
            }
        }
    }
    eprintln!(
        "sticks: {checked} pixels checked, {hits} on sticks, {} mismatches",
        mismatches.len()
    );
    assert!(hits > 300, "only {hits} pixels on sticks");
    // Edge pixels of thin cylinders can round either way (0.5% here).
    assert!(
        mismatches.len() * 100 <= hits,
        "{} of {hits} picks disagree:\n{}",
        mismatches.len(),
        mismatches.join("\n")
    );
}
