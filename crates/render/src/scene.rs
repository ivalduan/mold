//! The arena on the GPU: positions uploaded as-is, one byte of element per
//! atom, bonds as atom index pairs, and per-pose draw ranges.

use std::fmt;
use std::ops::Range;

use mold_mol::{Arena, PoseId};
use wgpu::util::DeviceExt as _;

use crate::camera::BoundingSphere;
use crate::renderer::Renderer;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SceneError {
    /// A bond names an atom past the end of the arena.
    BondOutOfRange { bond: [u32; 2], atoms: usize },
    /// A bond joins atoms of two poses; poses draw independently.
    BondCrossesPoses { bond: [u32; 2] },
    /// The arena is larger than one storage binding of this device.
    TooLarge { bytes: u64, limit: u64 },
}

impl fmt::Display for SceneError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::BondOutOfRange { bond, atoms } => {
                write!(
                    f,
                    "bond {bond:?} names an atom past the {atoms} in the arena"
                )
            }
            Self::BondCrossesPoses { bond } => write!(f, "bond {bond:?} joins two poses"),
            Self::TooLarge { bytes, limit } => write!(
                f,
                "the scene needs a {bytes}-byte storage buffer; this GPU allows {limit}"
            ),
        }
    }
}

impl std::error::Error for SceneError {}

/// One arena's atoms and bonds on the GPU. Holds no CPU copy of coordinates:
/// they live in the arena only.
#[derive(Debug)]
pub struct Scene {
    positions: wgpu::Buffer,
    pub(crate) bind_group: wgpu::BindGroup,
    atom_count: u32,
    bond_count: u32,
    /// Per pose: its atoms, and its bonds (indices into the sorted bonds).
    pose_atoms: Vec<Range<u32>>,
    pose_bonds: Vec<Range<u32>>,
    visible: Vec<bool>,
}

impl Scene {
    /// Uploads `arena` and `bonds` (global atom index pairs, each within one
    /// pose, e.g. from [`guess_bonds`](crate::guess_bonds)).
    pub fn new(
        device: &wgpu::Device,
        renderer: &Renderer,
        arena: &Arena,
        bonds: &[[u32; 2]],
    ) -> Result<Self, SceneError> {
        let atoms = arena.atom_count();
        let pose_atoms: Vec<Range<u32>> = arena
            .poses()
            .iter()
            .map(|p| p.atoms.start as u32..p.atoms.end as u32)
            .collect();

        // Sorted by first atom, so each pose's bonds are one range.
        let mut sorted: Vec<[u32; 2]> = bonds
            .iter()
            .map(|&[a, b]| if a <= b { [a, b] } else { [b, a] })
            .collect();
        sorted.sort_unstable();
        for &bond in &sorted {
            if bond[1] as usize >= atoms {
                return Err(SceneError::BondOutOfRange { bond, atoms });
            }
            if pose_index(&pose_atoms, bond[0]) != pose_index(&pose_atoms, bond[1]) {
                return Err(SceneError::BondCrossesPoses { bond });
            }
        }
        let pose_bonds = pose_atoms
            .iter()
            .map(|r| {
                let start = sorted.partition_point(|b| b[0] < r.start) as u32;
                let end = sorted.partition_point(|b| b[0] < r.end) as u32;
                start..end
            })
            .collect();

        let limit = device.limits().max_storage_buffer_binding_size;
        let position_bytes = (atoms * size_of::<[f32; 3]>()) as u64;
        if position_bytes > limit {
            return Err(SceneError::TooLarge {
                bytes: position_bytes,
                limit,
            });
        }
        // Storage buffers cannot be empty; the padding is never read.
        let mut flat: &[f32] = arena.positions_flat();
        let empty = [0f32; 4];
        if flat.is_empty() {
            flat = &empty;
        }
        let positions = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("positions"),
            contents: bytemuck::cast_slice(flat),
            usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
        });
        // One byte per atom, four to a u32.
        let mut elements: Vec<u8> = arena.elements().iter().map(|e| e.0).collect();
        elements.resize(elements.len().next_multiple_of(4).max(4), 0);
        let elements = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("elements"),
            contents: &elements,
            usage: wgpu::BufferUsages::STORAGE,
        });
        let dummy = [[0u32; 2]];
        let bond_data: &[[u32; 2]] = if sorted.is_empty() { &dummy } else { &sorted };
        let bonds_buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("bonds"),
            contents: bytemuck::cast_slice(bond_data),
            usage: wgpu::BufferUsages::STORAGE,
        });
        let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("scene"),
            layout: &renderer.scene_layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: positions.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: elements.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: bonds_buffer.as_entire_binding(),
                },
            ],
        });
        Ok(Self {
            positions,
            bind_group,
            atom_count: atoms as u32,
            bond_count: sorted.len() as u32,
            visible: vec![true; pose_atoms.len()],
            pose_atoms,
            pose_bonds,
        })
    }

    pub fn atom_count(&self) -> u32 {
        self.atom_count
    }

    pub fn bond_count(&self) -> u32 {
        self.bond_count
    }

    pub fn pose_count(&self) -> usize {
        self.pose_atoms.len()
    }

    /// Re-uploads the coordinates after an in-place change in the arena
    /// (e.g. superposition). The topology must be the one this scene was
    /// built from.
    pub fn update_positions(&self, queue: &wgpu::Queue, arena: &Arena) {
        assert_eq!(arena.atom_count(), self.atom_count as usize, "same arena");
        if self.atom_count > 0 {
            queue.write_buffer(
                &self.positions,
                0,
                bytemuck::cast_slice(arena.positions_flat()),
            );
        }
    }

    pub fn is_visible(&self, pose: PoseId) -> bool {
        self.visible.get(pose.0 as usize).copied().unwrap_or(false)
    }

    /// Shows or hides one pose. Unknown poses are ignored.
    pub fn set_visible(&mut self, pose: PoseId, visible: bool) {
        if let Some(v) = self.visible.get_mut(pose.0 as usize) {
            *v = visible;
        }
    }

    /// Shows every pose (`None`) or exactly one.
    pub fn show_only(&mut self, pose: Option<PoseId>) {
        for (i, v) in self.visible.iter_mut().enumerate() {
            *v = pose.is_none_or(|p| p.0 as usize == i);
        }
    }

    /// The pose a global atom index (e.g. a pick) belongs to.
    pub fn pose_of_atom(&self, atom: u32) -> Option<PoseId> {
        pose_index(&self.pose_atoms, atom).map(|i| PoseId(i as u32))
    }

    /// What to draw: maximal runs of consecutive visible poses, as an atom
    /// range and a bond range each. All poses visible is one run.
    pub fn draw_runs(&self) -> Vec<DrawRun> {
        draw_runs(&self.visible, &self.pose_atoms, &self.pose_bonds)
    }

    /// Bounds of the visible poses' atoms, padded by the largest van der
    /// Waals radius. `None` if nothing is visible.
    pub fn visible_bounds(&self, arena: &Arena) -> Option<BoundingSphere> {
        let positions = arena.positions();
        let mut sphere: Option<BoundingSphere> = None;
        for run in self.draw_runs() {
            let run_positions = &positions[run.atoms.start as usize..run.atoms.end as usize];
            let Some(next) = BoundingSphere::from_points(run_positions, 2.0) else {
                continue;
            };
            sphere = Some(match sphere {
                None => next,
                Some(s) => merge(s, next),
            });
        }
        sphere
    }
}

/// Atoms and bonds drawn by one draw call each.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DrawRun {
    pub atoms: Range<u32>,
    pub bonds: Range<u32>,
}

fn draw_runs(visible: &[bool], atoms: &[Range<u32>], bonds: &[Range<u32>]) -> Vec<DrawRun> {
    let mut runs: Vec<DrawRun> = Vec::new();
    let mut previous_visible = false;
    for (i, &shown) in visible.iter().enumerate() {
        if shown {
            let (atoms, bonds) = (atoms[i].clone(), bonds[i].clone());
            match runs.last_mut() {
                Some(run) if previous_visible => {
                    run.atoms.end = atoms.end;
                    run.bonds.end = bonds.end;
                }
                _ => runs.push(DrawRun { atoms, bonds }),
            }
        }
        previous_visible = shown;
    }
    runs
}

fn pose_index(poses: &[Range<u32>], atom: u32) -> Option<usize> {
    let i = poses.partition_point(|r| r.end <= atom);
    (i < poses.len() && poses[i].contains(&atom)).then_some(i)
}

fn merge(a: BoundingSphere, b: BoundingSphere) -> BoundingSphere {
    let offset = b.center - a.center;
    let d = offset.length();
    if d + b.radius <= a.radius {
        return a;
    }
    if d + a.radius <= b.radius {
        return b;
    }
    let radius = (d + a.radius + b.radius) * 0.5;
    let center = a.center + offset * ((radius - a.radius) / d.max(1e-6));
    BoundingSphere { center, radius }
}

#[cfg(test)]
mod tests {
    use super::*;
    use glam::Vec3;

    #[test]
    fn pose_lookup() {
        let poses = [0..3, 3..3, 3..10];
        assert_eq!(pose_index(&poses, 0), Some(0));
        assert_eq!(pose_index(&poses, 2), Some(0));
        assert_eq!(pose_index(&poses, 3), Some(2));
        assert_eq!(pose_index(&poses, 9), Some(2));
        assert_eq!(pose_index(&poses, 10), None);
    }

    #[test]
    fn visible_poses_merge_into_runs() {
        let atoms = [0..10, 10..25, 25..30, 30..42];
        let bonds = [0..9, 9..23, 23..27, 27..38];
        let all = draw_runs(&[true; 4], &atoms, &bonds);
        assert_eq!(
            all,
            vec![DrawRun {
                atoms: 0..42,
                bonds: 0..38
            }]
        );
        let some = draw_runs(&[true, false, true, true], &atoms, &bonds);
        assert_eq!(
            some,
            vec![
                DrawRun {
                    atoms: 0..10,
                    bonds: 0..9
                },
                DrawRun {
                    atoms: 25..42,
                    bonds: 23..38
                },
            ]
        );
        assert!(draw_runs(&[false; 4], &atoms, &bonds).is_empty());
    }

    #[test]
    fn merged_sphere_contains_both() {
        let a = BoundingSphere {
            center: Vec3::ZERO,
            radius: 2.0,
        };
        let b = BoundingSphere {
            center: Vec3::new(10.0, 0.0, 0.0),
            radius: 3.0,
        };
        let m = merge(a, b);
        assert!((m.center - Vec3::new(5.5, 0.0, 0.0)).length() < 1e-5);
        assert!((m.radius - 7.5).abs() < 1e-5);
        assert_eq!(merge(m, a), m);
    }
}
