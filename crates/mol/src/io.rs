//! PDB and mmCIF I/O, wrapping `pdbtbx`.
//!
//! # What a read keeps
//! - **Models**: each model in the file becomes one pose (unless
//!   [`LoadOptions::first_model_only`]). A Boltz-2 sample file has one model.
//! - **Everything in the model**: protein, ligands, waters, ions, hydrogens. Filtering
//!   is a node's job (non-destructive by default); the reader drops nothing.
//! - **Covalent ligands** are ordinary HETATM residues in their chain and are kept
//!   whole. `pdbtbx` 0.12 does not parse `LINK` / `struct_conn`, so the
//!   ligand–protein bond itself is not recorded; bond perception is left to `chem`/`geom`.
//! - **Alternate locations**: one conformer per residue, chosen by [`AltLocPolicy`].
//!   Atoms without an altloc id are always kept, with their file occupancy.
//! - Atoms keep file order within a residue (sorted by serial number).
//!
//! # Write
//! One pose per file, as a single model. Atom serials are renumbered from 1.

use std::collections::HashSet;
use std::fmt;
use std::io::{BufReader, BufWriter, Read, Write};
use std::path::{Path, PathBuf};

use pdbtbx::StrictnessLevel;

use crate::arena::{Arena, AtomName, AtomRecord};
use crate::{Element, PoseId};

/// Structure file format.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Format {
    Pdb,
    Mmcif,
}

impl Format {
    /// From the extension: `.pdb`, `.ent`, `.cif`, `.mmcif`, each optionally `.gz`.
    /// Returns the format and whether the file is gzip-compressed.
    pub fn from_path(path: &Path) -> Option<(Format, bool)> {
        let name = path.file_name()?.to_str()?.to_ascii_lowercase();
        let (name, gz) = match name.strip_suffix(".gz") {
            Some(stem) => (stem.to_string(), true),
            None => (name, false),
        };
        let format = match Path::new(&name).extension()?.to_str()? {
            "pdb" | "ent" => Format::Pdb,
            "cif" | "mmcif" => Format::Mmcif,
            _ => return None,
        };
        Some((format, gz))
    }

    fn to_pdbtbx(self) -> pdbtbx::Format {
        match self {
            Format::Pdb => pdbtbx::Format::Pdb,
            Format::Mmcif => pdbtbx::Format::Mmcif,
        }
    }
}

/// Which alternate location to keep when a residue has several.
///
/// The choice is made **per residue**, never per atom, so a residue is always one
/// self-consistent conformer (atoms from altloc A and B are never mixed).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum AltLocPolicy {
    /// The altloc whose atoms have the highest mean occupancy; ties go to the one
    /// that appears first in the file. Default.
    #[default]
    HighestOccupancy,
    /// The altloc that appears first in the file (usually `A`).
    First,
}

/// Options for reading structures into the arena.
#[derive(Debug, Clone, Default)]
pub struct LoadOptions {
    pub altloc: AltLocPolicy,
    /// Read only the first model instead of one pose per model.
    pub first_model_only: bool,
}

/// Errors from reading or writing structures.
#[derive(Debug)]
pub enum IoError {
    Io(std::io::Error),
    /// Extension is not one of the [`Format::from_path`] ones, or the path is not UTF-8.
    UnknownFormat(PathBuf),
    /// `pdbtbx` refused the file. One message per error it reported.
    Parse(Vec<String>),
    /// An atom name longer than [`AtomName::MAX_LEN`] or not ASCII.
    InvalidAtomName {
        name: String,
        residue: String,
        seq_num: i32,
    },
    /// The pose could not be expressed as a `pdbtbx` structure (invalid id or name).
    Write(String),
    /// No pose with this id in the arena.
    NoSuchPose(PoseId),
}

impl fmt::Display for IoError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            IoError::Io(e) => write!(f, "I/O error: {e}"),
            IoError::UnknownFormat(p) => write!(f, "unknown structure format: {}", p.display()),
            IoError::Parse(msgs) => write!(f, "could not parse structure: {}", msgs.join("; ")),
            IoError::InvalidAtomName {
                name,
                residue,
                seq_num,
            } => write!(
                f,
                "atom name {name:?} in {residue} {seq_num} is not 1-4 ASCII characters"
            ),
            IoError::Write(m) => write!(f, "could not write structure: {m}"),
            IoError::NoSuchPose(id) => write!(f, "no pose {} in the arena", id.0),
        }
    }
}

impl std::error::Error for IoError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            IoError::Io(e) => Some(e),
            _ => None,
        }
    }
}

impl From<std::io::Error> for IoError {
    fn from(e: std::io::Error) -> Self {
        IoError::Io(e)
    }
}

fn parse_errors(errors: Vec<pdbtbx::PDBError>) -> IoError {
    IoError::Parse(errors.iter().map(|e| e.to_string()).collect())
}

fn pdbtbx_options(options: &LoadOptions) -> pdbtbx::ReadOptions {
    let mut o = pdbtbx::ReadOptions::new();
    o.set_level(StrictnessLevel::Loose)
        // Skip SEQRES & co: with them pdbtbx inserts atom-less residues for unmodelled loops.
        .set_only_atomic_coords(true)
        .set_only_first_model(options.first_model_only);
    o
}

/// Read a PDB or mmCIF file (optionally `.gz`) and append one pose per model.
/// Pose names are the file stem, with `" model N"` appended when there are several.
pub fn read_file(
    arena: &mut Arena,
    path: impl AsRef<Path>,
    options: &LoadOptions,
) -> Result<Vec<PoseId>, IoError> {
    let path = path.as_ref();
    let unknown = || IoError::UnknownFormat(path.to_path_buf());
    let (format, gz) = Format::from_path(path).ok_or_else(unknown)?;
    let path_str = path.to_str().ok_or_else(unknown)?;
    let mut o = pdbtbx_options(options);
    o.set_format(format.to_pdbtbx()).set_decompress(gz);
    let (pdb, _warnings) = o.read(path_str).map_err(parse_errors)?;
    append_pdb(arena, &pdb, &file_stem(path), options)
}

/// Read an uncompressed PDB or mmCIF stream and append one pose per model.
pub fn read_from<R: Read>(
    arena: &mut Arena,
    reader: R,
    format: Format,
    name: &str,
    options: &LoadOptions,
) -> Result<Vec<PoseId>, IoError> {
    let mut o = pdbtbx_options(options);
    o.set_format(format.to_pdbtbx());
    let (pdb, _warnings) = o.read_raw(BufReader::new(reader)).map_err(parse_errors)?;
    append_pdb(arena, &pdb, name, options)
}

fn file_stem(path: &Path) -> String {
    let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("");
    name.split('.').next().unwrap_or(name).to_string()
}

fn append_pdb(
    arena: &mut Arena,
    pdb: &pdbtbx::PDB,
    name: &str,
    options: &LoadOptions,
) -> Result<Vec<PoseId>, IoError> {
    let model_count = pdb.models().count();
    arena.reserve_atoms(pdb.models().next().map_or(0, |m| m.atom_count()) * model_count);
    let mut ids = Vec::with_capacity(model_count);
    for model in pdb.models() {
        let pose_name = if model_count > 1 {
            format!("{name} model {}", model.serial_number())
        } else {
            name.to_string()
        };
        ids.push(append_model(arena, model, pose_name, options.altloc)?);
    }
    Ok(ids)
}

fn append_model(
    arena: &mut Arena,
    model: &pdbtbx::Model,
    name: String,
    policy: AltLocPolicy,
) -> Result<PoseId, IoError> {
    let mut b = arena.begin_pose(name);
    for chain in model.chains() {
        let mut chain_open = false;
        for residue in chain.residues() {
            let Some(picked) = pick_conformer(residue, policy) else {
                continue;
            };
            if !chain_open {
                b.add_chain(chain.id());
                chain_open = true;
            }
            let seq_num = i32::try_from(residue.serial_number()).unwrap_or(i32::MAX);
            let icode = residue.insertion_code().and_then(|s| s.chars().next());
            let hetero = picked.atoms.first().is_some_and(|a| a.hetero());
            b.add_residue(picked.name, seq_num, icode, hetero);
            for atom in &picked.atoms {
                let name = AtomName::new(atom.name()).ok_or_else(|| IoError::InvalidAtomName {
                    name: atom.name().to_string(),
                    residue: picked.name.to_string(),
                    seq_num,
                })?;
                let occupancy = if picked.shared.contains(&atom.serial_number()) {
                    atom.occupancy() * picked.shared_occupancy_factor
                } else {
                    atom.occupancy()
                };
                b.add_atom(AtomRecord {
                    name,
                    element: atom
                        .element()
                        .and_then(|e| Element::from_pdbtbx(*e))
                        .unwrap_or(Element::UNKNOWN),
                    position: [atom.x() as f32, atom.y() as f32, atom.z() as f32],
                    b_factor: atom.b_factor() as f32,
                    occupancy: occupancy as f32,
                    formal_charge: i8::try_from(atom.charge()).unwrap_or(0),
                });
            }
        }
    }
    Ok(b.finish())
}

/// The atoms kept for one residue after applying the altloc policy.
struct Picked<'a> {
    name: &'a str,
    atoms: Vec<&'a pdbtbx::Atom>,
    /// Serials of atoms that had no altloc id in the file.
    shared: HashSet<usize>,
    shared_occupancy_factor: f64,
}

/// Apply the altloc policy to one residue. `None` if the residue has no atoms.
///
/// `pdbtbx` (`reshuffle_conformers`) copies every blank-altloc atom into each altloc
/// conformer and divides its occupancy by (altloc conformers + 1). We recognise those
/// atoms as the serials present in every conformer, score conformers on their own
/// atoms only, and undo the occupancy division.
fn pick_conformer(residue: &pdbtbx::Residue, policy: AltLocPolicy) -> Option<Picked<'_>> {
    let conformers: Vec<&pdbtbx::Conformer> = residue
        .conformers()
        .filter(|c| c.atom_count() > 0)
        .collect();
    let (chosen, shared, factor) = match conformers.as_slice() {
        [] => return None,
        [only] => (*only, HashSet::new(), 1.0),
        [first, rest @ ..] => {
            let shared: HashSet<usize> = first
                .atoms()
                .map(pdbtbx::Atom::serial_number)
                .filter(|s| {
                    rest.iter()
                        .all(|c| c.atoms().any(|a| a.serial_number() == *s))
                })
                .collect();
            let chosen = match policy {
                AltLocPolicy::First => *first,
                AltLocPolicy::HighestOccupancy => {
                    let score = |c: &pdbtbx::Conformer| {
                        let (sum, n) = c
                            .atoms()
                            .filter(|a| !shared.contains(&a.serial_number()))
                            .fold((0.0, 0u32), |(s, n), a| (s + a.occupancy(), n + 1));
                        if n == 0 { 0.0 } else { sum / f64::from(n) }
                    };
                    let mut best = *first;
                    let mut best_score = score(first);
                    for c in rest {
                        let s = score(c);
                        if s > best_score {
                            best = c;
                            best_score = s;
                        }
                    }
                    best
                }
            };
            let factor = if shared.is_empty() {
                1.0
            } else {
                (conformers.len() + 1) as f64
            };
            (chosen, shared, factor)
        }
    };
    let mut atoms: Vec<&pdbtbx::Atom> = chosen.atoms().collect();
    atoms.sort_by_key(|a| a.serial_number());
    Some(Picked {
        name: chosen.name(),
        atoms,
        shared,
        shared_occupancy_factor: factor,
    })
}

/// Write one pose to a file; the format comes from the extension (no `.gz`).
pub fn write_file(arena: &Arena, pose: PoseId, path: impl AsRef<Path>) -> Result<(), IoError> {
    let path = path.as_ref();
    let format = match Format::from_path(path) {
        Some((format, false)) => format,
        _ => return Err(IoError::UnknownFormat(path.to_path_buf())),
    };
    let file = std::fs::File::create(path)?;
    write_to(arena, pose, format, file)
}

/// Write one pose to a stream as a single-model PDB or mmCIF file.
pub fn write_to<W: Write>(
    arena: &Arena,
    pose: PoseId,
    format: Format,
    mut writer: W,
) -> Result<(), IoError> {
    let buf = match format {
        Format::Pdb => pdb_text(arena, pose)?.into_bytes(),
        Format::Mmcif => {
            let pdb = to_pdbtbx(arena, pose)?;
            // pdbtbx's writers unwrap I/O errors; give them an in-memory sink and do
            // the fallible write ourselves.
            let mut buf = Vec::new();
            pdbtbx::save_mmcif_raw(&pdb, BufWriter::new(&mut buf));
            buf
        }
    };
    writer.write_all(&buf)?;
    writer.flush()?;
    Ok(())
}

/// PDB-format text for one pose.
///
/// Written here rather than with `pdbtbx::save_pdb_raw`, which (0.12) strips leading
/// zeros from every fixed-width field and so turns CCD codes like `0WN` into `WN`.
/// Fields that do not fit the fixed-width format are an error: use mmCIF for them.
fn pdb_text(arena: &Arena, id: PoseId) -> Result<String, IoError> {
    use std::fmt::Write as _;

    let pose = arena.pose(id).ok_or(IoError::NoSuchPose(id))?;
    let too_wide = |what: &str, value: &dyn fmt::Display| {
        IoError::Write(format!(
            "{what} {value} does not fit the PDB format; write mmCIF"
        ))
    };
    let mut out = String::with_capacity(pose.atoms.len() * 81 + 4);
    let mut serial = 0usize;
    for chain in &arena.chains()[pose.chains.clone()] {
        if chain.id.chars().count() > 1 {
            return Err(too_wide("chain id", &chain.id));
        }
        for residue in &arena.residues()[chain.residues.clone()] {
            if residue.name.len() > 3 {
                return Err(too_wide("residue name", &residue.name));
            }
            if !(-999..=9999).contains(&residue.seq_num) {
                return Err(too_wide("residue number", &residue.seq_num));
            }
            for i in residue.atoms.clone() {
                serial += 1;
                if serial > 99_999 {
                    return Err(too_wide("atom serial", &serial));
                }
                let [x, y, z] = arena.positions()[i];
                if [x, y, z].iter().any(|v| !(-999.999..=9999.999).contains(v)) {
                    return Err(too_wide("coordinate", &format!("{x},{y},{z}")));
                }
                let element = arena.elements()[i].symbol();
                let name = arena.atom_names()[i].as_str();
                // Columns 13-16: a one-letter element sits in column 14 (" CA ").
                let name = if name.len() < 4 && element.len() == 1 {
                    format!(" {name:<3}")
                } else {
                    format!("{name:<4}")
                };
                let charge = match arena.formal_charges()[i] {
                    0 => String::new(),
                    c if c > 0 => format!("{c}+"),
                    c => format!("{}-", -c),
                };
                let _ = writeln!(
                    out,
                    "{record}{serial:>5} {name} {resn:>3} {chain:1}{seq:>4}{icode:1}   \
                     {x:>8.3}{y:>8.3}{z:>8.3}{occ:>6.2}{b:>6.2}          {element:>2}{charge:>2}",
                    record = if residue.hetero { "HETATM" } else { "ATOM  " },
                    resn = residue.name,
                    chain = chain.id,
                    seq = residue.seq_num,
                    icode = residue.insertion_code.unwrap_or(' '),
                    occ = arena.occupancies()[i],
                    b = arena.b_factors()[i].min(999.99),
                );
            }
        }
    }
    out.push_str("END\n");
    Ok(out)
}

fn to_pdbtbx(arena: &Arena, id: PoseId) -> Result<pdbtbx::PDB, IoError> {
    let pose = arena.pose(id).ok_or(IoError::NoSuchPose(id))?;
    let mut model = pdbtbx::Model::new(1);
    let mut serial = 0usize;
    for chain in &arena.chains()[pose.chains.clone()] {
        let mut out_chain = pdbtbx::Chain::new(&chain.id)
            .ok_or_else(|| IoError::Write(format!("invalid chain id {:?}", chain.id)))?;
        for residue in &arena.residues()[chain.residues.clone()] {
            let mut conformer =
                pdbtbx::Conformer::new(&residue.name, None, None).ok_or_else(|| {
                    IoError::Write(format!("invalid residue name {:?}", residue.name))
                })?;
            for i in residue.atoms.clone() {
                serial += 1;
                let [x, y, z] = arena.positions()[i];
                let name = arena.atom_names()[i];
                let atom = pdbtbx::Atom::new(
                    residue.hetero,
                    serial,
                    name.as_str(),
                    f64::from(x),
                    f64::from(y),
                    f64::from(z),
                    f64::from(arena.occupancies()[i]),
                    f64::from(arena.b_factors()[i]),
                    arena.elements()[i].symbol(),
                    isize::from(arena.formal_charges()[i]),
                )
                .ok_or_else(|| IoError::Write(format!("invalid atom {name} (serial {serial})")))?;
                conformer.add_atom(atom);
            }
            let icode = residue.insertion_code.map(String::from);
            let out_residue =
                pdbtbx::Residue::new(residue.seq_num as isize, icode.as_deref(), Some(conformer))
                    .ok_or_else(|| {
                    IoError::Write(format!(
                        "invalid insertion code {:?}",
                        residue.insertion_code
                    ))
                })?;
            out_chain.add_residue(out_residue);
        }
        model.add_chain(out_chain);
    }
    let mut pdb = pdbtbx::PDB::new();
    pdb.identifier = Some(
        pose.name
            .chars()
            .map(|c| if c.is_ascii_alphanumeric() { c } else { '_' })
            .collect(),
    );
    pdb.add_model(model);
    Ok(pdb)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn format_from_extension() {
        let f = |p: &str| Format::from_path(Path::new(p));
        assert_eq!(f("a/1M17.cif"), Some((Format::Mmcif, false)));
        assert_eq!(f("1M17.CIF.gz"), Some((Format::Mmcif, true)));
        assert_eq!(f("x.pdb"), Some((Format::Pdb, false)));
        assert_eq!(f("x.ent.gz"), Some((Format::Pdb, true)));
        assert_eq!(f("x.sdf"), None);
        assert_eq!(f("x"), None);
    }

    fn one_atom_arena(resn: &str, seq: i32, hetero: bool, atom: &str, el: Element) -> Arena {
        let mut arena = Arena::new();
        let mut b = arena.begin_pose("t");
        b.add_chain("A");
        b.add_residue(resn, seq, None, hetero);
        b.add_atom(AtomRecord {
            name: AtomName::new(atom).unwrap(),
            element: el,
            position: [50.463, 7.017, -17.184],
            b_factor: 57.75,
            occupancy: 1.0,
            formal_charge: 0,
        });
        b.finish();
        arena
    }

    #[test]
    fn pdb_lines_use_standard_columns() {
        // Verbatim from 4G5J.pdb, apart from the serial number.
        let arena = one_atom_arena("CYS", 797, false, "SG", Element::S);
        let text = pdb_text(&arena, PoseId(0)).unwrap();
        assert_eq!(
            text,
            "ATOM      1  SG  CYS A 797      50.463   7.017 -17.184  1.00 57.75           S  \nEND\n"
        );
        // Two-letter element starts in column 13; leading zero in a CCD code survives.
        let arena = one_atom_arena("0WN", 1101, true, "CL1", Element::CL);
        let line = pdb_text(&arena, PoseId(0)).unwrap();
        assert_eq!(&line[..27], "HETATM    1 CL1  0WN A1101 ");
        assert_eq!(&line[76..78], "CL");
    }

    #[test]
    fn pdb_refuses_fields_that_do_not_fit() {
        let arena = one_atom_arena("ABCDE", 1, true, "C1", Element::C);
        assert!(matches!(
            pdb_text(&arena, PoseId(0)),
            Err(IoError::Write(_))
        ));
    }
}
