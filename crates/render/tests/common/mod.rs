//! Shared by the GPU tests and the bench: fixture loading, the skip rule, and
//! a BMP writer for humans (written under `target/` only, never committed).
#![allow(dead_code)]

use std::path::{Path, PathBuf};

use mold_mol::{Arena, AtomRecord, PoseId};
use mold_render::Gpu;

pub fn fixture(relative: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../fixtures")
        .join(relative)
}

/// EGFR kinase domain with gefitinib: 2369 atoms, one pose.
pub fn reference_arena() -> Arena {
    let mut arena = Arena::new();
    mold_mol::io::read_file(
        &mut arena,
        fixture("egfr/reference.cif"),
        &Default::default(),
    )
    .expect("fixtures/egfr/reference.cif reads");
    arena
}

/// A headless GPU, or `None` with the reason printed: the GPU tests then pass
/// as skipped (CI runners have no adapter). `MOLD_RENDER_REQUIRE_GPU=1`
/// turns the skip into a failure, for a GPU runner.
pub fn gpu_or_skip(test: &str) -> Option<Gpu> {
    match Gpu::headless() {
        Ok(gpu) => {
            eprintln!("{test}: adapter {}", gpu.adapter_name());
            Some(gpu)
        }
        Err(error) => {
            if std::env::var_os("MOLD_RENDER_REQUIRE_GPU").is_some_and(|v| v == "1") {
                panic!("{test}: MOLD_RENDER_REQUIRE_GPU=1 but {error}");
            }
            eprintln!("SKIPPED {test}: {error}");
            None
        }
    }
}

/// Appends a copy of pose `source` of `from` to `to`, every position moved
/// by `transform`.
pub fn copy_pose(
    from: &Arena,
    source: PoseId,
    to: &mut Arena,
    name: &str,
    transform: impl Fn([f32; 3]) -> [f32; 3],
) -> PoseId {
    let pose = from.pose(source).expect("pose exists").clone();
    let mut builder = to.begin_pose(name);
    for chain in &from.chains()[pose.chains.clone()] {
        builder.add_chain(chain.id.clone());
        for residue in &from.residues()[chain.residues.clone()] {
            builder.add_residue(
                residue.name.clone(),
                residue.seq_num,
                residue.insertion_code,
                residue.hetero,
            );
            for atom in residue.atoms.clone() {
                builder.add_atom(AtomRecord {
                    name: from.atom_names()[atom],
                    element: from.elements()[atom],
                    position: transform(from.positions()[atom]),
                    b_factor: from.b_factors()[atom],
                    occupancy: from.occupancies()[atom],
                    formal_charge: from.formal_charges()[atom],
                });
            }
        }
    }
    builder.finish()
}

/// Writes `image` as an uncompressed 32-bit BMP under the cargo target
/// directory and returns the path.
pub fn write_bmp(image: &mold_render::Image, name: &str) -> PathBuf {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target/render-tests");
    std::fs::create_dir_all(&dir).ok();
    let path = dir.join(format!("{name}.bmp"));
    let (w, h) = (image.width, image.height);
    let data_len = w * h * 4;
    let mut bmp = Vec::with_capacity(54 + data_len as usize);
    bmp.extend_from_slice(b"BM");
    bmp.extend_from_slice(&(54 + data_len).to_le_bytes());
    bmp.extend_from_slice(&[0; 4]);
    bmp.extend_from_slice(&54u32.to_le_bytes());
    bmp.extend_from_slice(&40u32.to_le_bytes());
    bmp.extend_from_slice(&(w as i32).to_le_bytes());
    // Negative height: rows top to bottom.
    bmp.extend_from_slice(&(-(h as i32)).to_le_bytes());
    bmp.extend_from_slice(&1u16.to_le_bytes());
    bmp.extend_from_slice(&32u16.to_le_bytes());
    bmp.extend_from_slice(&[0; 24]);
    for [r, g, b, _] in image.rgba.as_chunks::<4>().0 {
        bmp.extend_from_slice(&[*b, *g, *r, 255]);
    }
    std::fs::write(&path, bmp).ok();
    path
}
