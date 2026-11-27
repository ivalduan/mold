//! SDF / MDL molfile reading (V2000 and V3000) and V2000 writing. No RDKit.
//!
//! # What a read keeps
//! - **One pose per record.** Each record becomes one pose with one chain `L` and
//!   one hetero residue `LIG` (seq 1). Atom names are generated (`C1`, `N2`,
//!   `CL1`: element symbol + per-element counter), because molfiles have none.
//! - **Bonds** (types 1–4) go into the arena's bond table ([`Arena::pose_bonds`]).
//!   Query bond types (5–8) make the record fail as [`SdfError::Unsupported`].
//! - **Charges**: the atom-block charge field, unless the record has `M  CHG`
//!   lines, which (per the CTfile spec) replace every atom-block charge. V3000
//!   `CHG=` atom properties.
//! - **Title** (header line 1) becomes the pose name; a blank title falls back to
//!   `"<name> record <index>"`. The title and every `> <field>` data item are
//!   also returned, as strings, in [`SdfRecord`]: they feed the pose table, and
//!   the arena holds no metrics.
//! - Coordinates are read as written; stereo flags, isotopes, radicals, atom
//!   lists, S-groups and the like are ignored. `D` and `T` read as hydrogen; other
//!   pseudo-atoms (`*`, `A`, `Q`, `R#`, `LP`) as [`Element::UNKNOWN`].
//!
//! # Errors
//! A bad record never aborts the file: [`SdfRead`] holds the good records **and**
//! one [`SdfRecordError`] per bad one (0-based record index plus the 1-based
//! line in the input). A bad record leaves nothing behind in the arena. Only
//! I/O errors on the stream itself are returned as `Err`.

use std::collections::{HashMap, HashSet};
use std::fmt;
use std::io::{Read, Write};
use std::path::Path;

use crate::arena::{Arena, AtomName, AtomRecord, BondOrder};
use crate::io::IoError;
use crate::{Element, PoseId};

/// Chain id given to every pose read from an SDF.
pub const CHAIN_ID: &str = "L";
/// Residue name given to every pose read from an SDF.
pub const RESIDUE_NAME: &str = "LIG";

/// One record that loaded.
#[derive(Debug, Clone, PartialEq)]
pub struct SdfRecord {
    /// 0-based position of the record in the input.
    pub index: usize,
    pub pose: PoseId,
    /// Header line 1, trimmed (may be empty).
    pub title: String,
    /// The `> <name>` data items in file order; multi-line values joined by `\n`.
    pub properties: Vec<(String, String)>,
}

impl SdfRecord {
    /// The first data item called `name`.
    pub fn property(&self, name: &str) -> Option<&str> {
        self.properties
            .iter()
            .find(|(k, _)| k == name)
            .map(|(_, v)| v.as_str())
    }
}

/// Why one record did not load.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SdfError {
    /// The record holds no non-blank line (e.g. `$$$$` twice in a row).
    Empty,
    /// The record breaks the CTfile format. `line` is 1-based in the input.
    Malformed { line: usize, message: String },
    /// Valid CTfile, but a feature this reader does not handle (query bonds,
    /// atom lists, an unknown counts-line version).
    Unsupported { line: usize, message: String },
}

impl fmt::Display for SdfError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            SdfError::Empty => f.write_str("empty record"),
            SdfError::Malformed { line, message } => write!(f, "line {line}: {message}"),
            SdfError::Unsupported { line, message } => {
                write!(f, "line {line}: unsupported: {message}")
            }
        }
    }
}

impl std::error::Error for SdfError {}

/// One record that did not load.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SdfRecordError {
    /// 0-based position of the record in the input.
    pub index: usize,
    pub error: SdfError,
}

impl fmt::Display for SdfRecordError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "record {}: {}", self.index, self.error)
    }
}

/// The outcome of reading an SDF: the records that loaded and the ones that did not.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct SdfRead {
    pub records: Vec<SdfRecord>,
    pub errors: Vec<SdfRecordError>,
}

impl SdfRead {
    /// The poses appended to the arena, in file order.
    pub fn poses(&self) -> Vec<PoseId> {
        self.records.iter().map(|r| r.pose).collect()
    }
}

/// Read an SDF or molfile and append one pose per good record. Pose names fall
/// back to the file stem when a record has no title.
pub fn read_sdf_file(arena: &mut Arena, path: impl AsRef<Path>) -> Result<SdfRead, IoError> {
    let path = path.as_ref();
    let name = path
        .file_name()
        .and_then(|n| n.to_str())
        .map_or("", |n| n.split('.').next().unwrap_or(n));
    let bytes = std::fs::read(path)?;
    Ok(read_sdf_str(arena, &String::from_utf8_lossy(&bytes), name))
}

/// Read an SDF or molfile stream (not compressed). Invalid UTF-8 is replaced,
/// not an error: it can only sit in titles and data values.
pub fn read_sdf_from<R: Read>(
    arena: &mut Arena,
    mut reader: R,
    name: &str,
) -> Result<SdfRead, IoError> {
    let mut bytes = Vec::new();
    reader.read_to_end(&mut bytes)?;
    Ok(read_sdf_str(arena, &String::from_utf8_lossy(&bytes), name))
}

/// Read SDF text. See the [module docs](self) for what is kept and the error policy.
pub fn read_sdf_str(arena: &mut Arena, text: &str, name: &str) -> SdfRead {
    let mut out = SdfRead::default();
    let mut record: Vec<&str> = Vec::new();
    let mut first_line = 1;
    let mut index = 0;
    let mut finish =
        |arena: &mut Arena, lines: &[&str], first_line: usize, index: usize| match parse_record(
            lines, first_line,
        ) {
            Ok(mol) => {
                let fallback = format!("{name} record {index}");
                let pose = mol.append(arena, &fallback);
                out.records.push(SdfRecord {
                    index,
                    pose,
                    title: mol.title,
                    properties: mol.properties,
                });
            }
            Err(error) => out.errors.push(SdfRecordError { index, error }),
        };
    for (i, line) in text.lines().enumerate() {
        if line.trim_end() == "$$$$" {
            finish(arena, &record, first_line, index);
            index += 1;
            record.clear();
            first_line = i + 2;
        } else {
            record.push(line);
        }
    }
    // A molfile, or an SDF whose last record lacks `$$$$`. Trailing blank lines
    // after the last `$$$$` are not a record.
    if record.iter().any(|l| !l.trim().is_empty()) {
        finish(arena, &record, first_line, index);
    }
    out
}

// ---- Parsing -------------------------------------------------------------------

struct ParsedAtom {
    element: Element,
    position: [f32; 3],
    charge: i8,
}

struct Molecule {
    title: String,
    atoms: Vec<ParsedAtom>,
    /// 0-based atom indices within the record.
    bonds: Vec<(u32, u32, BondOrder)>,
    properties: Vec<(String, String)>,
}

impl Molecule {
    fn append(&self, arena: &mut Arena, fallback_name: &str) -> PoseId {
        let name = if self.title.is_empty() {
            fallback_name.to_string()
        } else {
            self.title.clone()
        };
        arena.reserve_atoms(self.atoms.len());
        let mut b = arena.begin_pose(name);
        b.add_chain(CHAIN_ID);
        b.add_residue(RESIDUE_NAME, 1, None, true);
        let mut per_element: HashMap<Element, u32> = HashMap::new();
        let mut first = 0;
        for (i, atom) in self.atoms.iter().enumerate() {
            let n = per_element.entry(atom.element).or_insert(0);
            *n += 1;
            let symbol = atom.element.symbol();
            let name = AtomName::new(&format!("{symbol}{n}"))
                .or_else(|| AtomName::new(symbol))
                .unwrap_or_default();
            let index = b.add_atom(AtomRecord {
                name,
                element: atom.element,
                position: atom.position,
                b_factor: 0.0,
                occupancy: 1.0,
                formal_charge: atom.charge,
            });
            if i == 0 {
                first = index;
            }
        }
        for &(x, y, order) in &self.bonds {
            b.add_bond(first + x, first + y, order);
        }
        b.finish()
    }
}

fn malformed(line: usize, message: impl Into<String>) -> SdfError {
    SdfError::Malformed {
        line,
        message: message.into(),
    }
}

fn unsupported(line: usize, message: impl Into<String>) -> SdfError {
    SdfError::Unsupported {
        line,
        message: message.into(),
    }
}

/// A fixed-width field, trimmed; empty if the line is too short.
fn field(line: &str, range: std::ops::Range<usize>) -> &str {
    let end = range.end.min(line.len());
    line.get(range.start.min(end)..end).unwrap_or("").trim()
}

/// `lines` is one record without its `$$$$`; `first` is the 1-based input line
/// of `lines[0]`.
fn parse_record(lines: &[&str], first: usize) -> Result<Molecule, SdfError> {
    if lines.iter().all(|l| l.trim().is_empty()) {
        return Err(SdfError::Empty);
    }
    let line_no = |i: usize| first + i;
    if lines.len() < 4 {
        return Err(malformed(
            line_no(lines.len().saturating_sub(1)),
            "record ends before the counts line",
        ));
    }
    let title = lines[0].trim().to_string();
    let counts = lines[3];
    // Columns 34-39, but tolerate writers that misalign the counts line.
    let version = match counts.trim_end() {
        c if c.ends_with("V3000") => "V3000",
        c if c.ends_with("V2000") => "V2000",
        _ => field(counts, 33..39),
    };
    let (atoms, bonds, end) = match version {
        "V3000" => parse_v3000(lines, first)?,
        "V2000" | "" => parse_v2000(lines, first)?,
        other => {
            return Err(unsupported(
                line_no(3),
                format!("counts-line version {other:?}"),
            ));
        }
    };
    let properties = parse_data_items(&lines[end..]);
    Ok(Molecule {
        title,
        atoms,
        bonds,
        properties,
    })
}

type Parsed = (Vec<ParsedAtom>, Vec<(u32, u32, BondOrder)>, usize);

fn parse_count(counts: &str, range: std::ops::Range<usize>, nth: usize) -> Option<usize> {
    field(counts, range)
        .parse()
        .ok()
        .or_else(|| counts.split_whitespace().nth(nth)?.parse().ok())
}

fn parse_v2000(lines: &[&str], first: usize) -> Result<Parsed, SdfError> {
    let line_no = |i: usize| first + i;
    let counts = lines[3];
    let n_atoms = parse_count(counts, 0..3, 0)
        .ok_or_else(|| malformed(line_no(3), "counts line: no atom count"))?;
    let n_bonds = parse_count(counts, 3..6, 1)
        .ok_or_else(|| malformed(line_no(3), "counts line: no bond count"))?;
    let atoms_end = 4 + n_atoms;
    let bonds_end = atoms_end + n_bonds;
    if lines.len() < bonds_end {
        return Err(malformed(
            line_no(lines.len() - 1),
            format!(
                "record ends inside the atom/bond block ({n_atoms} atoms, {n_bonds} bonds declared)"
            ),
        ));
    }

    let mut atoms = Vec::with_capacity(n_atoms);
    for (i, line) in lines[4..atoms_end].iter().enumerate() {
        atoms.push(parse_v2000_atom(line).map_err(|m| malformed(line_no(4 + i), m))?);
    }

    let mut bonds = Vec::with_capacity(n_bonds);
    let mut seen = HashSet::new();
    for (i, line) in lines[atoms_end..bonds_end].iter().enumerate() {
        let at = line_no(atoms_end + i);
        let ints = |range: std::ops::Range<usize>, nth: usize| -> Option<u32> {
            field(line, range)
                .parse()
                .ok()
                .or_else(|| line.split_whitespace().nth(nth)?.parse().ok())
        };
        let (Some(a), Some(b), Some(code)) = (ints(0..3, 0), ints(3..6, 1), ints(6..9, 2)) else {
            return Err(malformed(at, format!("bond line {line:?}")));
        };
        bonds.push(check_bond(a, b, code, n_atoms, &mut seen, at)?);
    }

    // Properties block, up to `M  END`. Without one (old files), it ends at the
    // first data item.
    let mut charges: Option<Vec<i8>> = None;
    let mut i = bonds_end;
    while i < lines.len() {
        let line = lines[i];
        if line.trim_end() == "M  END" {
            i += 1;
            break;
        }
        if line.starts_with('>') {
            break;
        }
        if let Some(rest) = line.strip_prefix("M  CHG") {
            let charges = charges.get_or_insert_with(|| vec![0; n_atoms]);
            apply_m_chg(rest, charges).map_err(|m| malformed(line_no(i), m))?;
        } else if line.starts_with("A  ") || line.starts_with("G  ") {
            // Atom alias / group abbreviation: the next line is its text.
            i += 1;
        } else if let Some(n) = line.strip_prefix("S  SKP") {
            i += n.trim().parse::<usize>().unwrap_or(0);
        }
        i += 1;
    }
    if let Some(charges) = charges {
        for (atom, c) in atoms.iter_mut().zip(charges) {
            atom.charge = c;
        }
    }
    Ok((atoms, bonds, i.min(lines.len())))
}

fn parse_v2000_atom(line: &str) -> Result<ParsedAtom, String> {
    let fixed = || -> Option<([f32; 3], &str)> {
        let x = field(line, 0..10).parse().ok()?;
        let y = field(line, 10..20).parse().ok()?;
        let z = field(line, 20..30).parse().ok()?;
        let symbol = field(line, 31..34);
        (!symbol.is_empty()).then_some(([x, y, z], symbol))
    };
    // Some writers do not keep the columns; fall back to whitespace tokens.
    let loose = || -> Option<([f32; 3], &str)> {
        let mut t = line.split_whitespace();
        let x = t.next()?.parse().ok()?;
        let y = t.next()?.parse().ok()?;
        let z = t.next()?.parse().ok()?;
        Some(([x, y, z], t.next()?))
    };
    let (position, symbol) = fixed()
        .or_else(loose)
        .ok_or_else(|| format!("atom line {line:?}"))?;
    check_finite(position)?;
    let charge = match field(line, 36..39) {
        "1" => 3,
        "2" => 2,
        "3" => 1,
        "5" => -1,
        "6" => -2,
        "7" => -3,
        _ => 0, // 0 = uncharged, 4 = doublet radical
    };
    Ok(ParsedAtom {
        element: element_from_mdl(symbol),
        position,
        charge,
    })
}

fn check_finite(p: [f32; 3]) -> Result<(), String> {
    if p.iter().all(|v| v.is_finite()) {
        Ok(())
    } else {
        Err(format!("non-finite coordinate {p:?}"))
    }
}

fn element_from_mdl(symbol: &str) -> Element {
    match symbol {
        "D" | "T" => Element::H,
        s => Element::from_symbol(s).unwrap_or(Element::UNKNOWN),
    }
}

/// 1-based `a`, `b` → 0-based, validated.
fn check_bond(
    a: u32,
    b: u32,
    code: u32,
    n_atoms: usize,
    seen: &mut HashSet<(u32, u32)>,
    line: usize,
) -> Result<(u32, u32, BondOrder), SdfError> {
    for atom in [a, b] {
        if atom == 0 || atom as usize > n_atoms {
            return Err(malformed(
                line,
                format!("bond names atom {atom}, but the record has {n_atoms}"),
            ));
        }
    }
    if a == b {
        return Err(malformed(line, format!("atom {a} bonded to itself")));
    }
    if !seen.insert((a.min(b), a.max(b))) {
        return Err(malformed(line, format!("bond {a}-{b} listed twice")));
    }
    let order = u8::try_from(code)
        .ok()
        .and_then(BondOrder::from_mdl_code)
        .ok_or_else(|| unsupported(line, format!("bond type {code} (query bond)")))?;
    Ok((a - 1, b - 1, order))
}

/// `rest` follows `M  CHG`: a count, then (atom, charge) pairs.
fn apply_m_chg(rest: &str, charges: &mut [i8]) -> Result<(), String> {
    let bad = || format!("M  CHG{rest}");
    let mut t = rest.split_whitespace().map(str::parse::<i64>);
    let n = t.next().and_then(Result::ok).ok_or_else(bad)?;
    for _ in 0..n {
        let (Some(Ok(atom)), Some(Ok(charge))) = (t.next(), t.next()) else {
            return Err(bad());
        };
        let slot = usize::try_from(atom)
            .ok()
            .and_then(|a| a.checked_sub(1))
            .and_then(|a| charges.get_mut(a))
            .ok_or_else(|| format!("M  CHG names atom {atom}"))?;
        *slot = i8::try_from(charge).map_err(|_| format!("M  CHG charge {charge}"))?;
    }
    Ok(())
}

/// V3000: the CTAB is `M  V30` lines from line 4 to `M  END`. Lines ending in `-`
/// continue on the next one.
fn parse_v3000(lines: &[&str], first: usize) -> Result<Parsed, SdfError> {
    // (line number of the logical line's start, joined content)
    let mut logical: Vec<(usize, String)> = Vec::new();
    let mut end = lines.len();
    let mut continuing = false;
    for (i, line) in lines.iter().enumerate().skip(4) {
        if line.trim_end() == "M  END" {
            end = i + 1;
            break;
        }
        let Some(content) = line.strip_prefix("M  V30 ") else {
            if line.starts_with('>') {
                end = i;
                break;
            }
            continue;
        };
        let (content, more) = match content.trim_end().strip_suffix('-') {
            Some(c) => (c, true),
            None => (content.trim_end(), false),
        };
        if continuing {
            if let Some(last) = logical.last_mut() {
                last.1.push_str(content);
            }
        } else {
            logical.push((first + i, content.to_string()));
        }
        continuing = more;
    }

    #[derive(PartialEq)]
    enum Block {
        None,
        Atom,
        Bond,
        Other,
    }
    let mut block = Block::None;
    let mut ctab_done = false;
    let mut declared: Option<(usize, usize)> = None;
    let mut atoms = Vec::new();
    let mut ids: HashMap<u32, u32> = HashMap::new();
    let mut raw_bonds: Vec<(usize, u32, u32, u32)> = Vec::new();
    for (at, text) in &logical {
        let at = *at;
        let mut t = text.split_whitespace();
        let Some(head) = t.next() else { continue };
        if ctab_done {
            continue; // templates, R-group CTABs: not read
        }
        match (head, &block) {
            ("BEGIN", _) => {
                block = match t.next() {
                    Some("ATOM") => Block::Atom,
                    Some("BOND") => Block::Bond,
                    Some("CTAB") => Block::None,
                    _ => Block::Other,
                }
            }
            ("END", _) => {
                if t.next() == Some("CTAB") {
                    ctab_done = true;
                }
                block = Block::None;
            }
            ("COUNTS", Block::None) => {
                let mut n = || t.next().and_then(|v| v.parse::<usize>().ok());
                let (Some(na), Some(nb)) = (n(), n()) else {
                    return Err(malformed(at, format!("COUNTS line {text:?}")));
                };
                declared = Some((na, nb));
            }
            (id, Block::Atom) => {
                let id: u32 = id
                    .parse()
                    .map_err(|_| malformed(at, format!("atom line {text:?}")))?;
                let symbol = t.next().unwrap_or("");
                if symbol.starts_with('[') || symbol == "NOT" {
                    return Err(unsupported(at, format!("atom list {symbol}")));
                }
                let mut coord = || t.next().and_then(|v| v.parse::<f32>().ok());
                let (Some(x), Some(y), Some(z)) = (coord(), coord(), coord()) else {
                    return Err(malformed(at, format!("atom line {text:?}")));
                };
                check_finite([x, y, z]).map_err(|m| malformed(at, m))?;
                let _aamap = t.next();
                let mut charge = 0i8;
                for prop in t {
                    if let Some(v) = prop.strip_prefix("CHG=") {
                        charge = v
                            .parse()
                            .map_err(|_| malformed(at, format!("charge {prop}")))?;
                    }
                }
                if ids.insert(id, atoms.len() as u32).is_some() {
                    return Err(malformed(at, format!("atom id {id} used twice")));
                }
                atoms.push(ParsedAtom {
                    element: element_from_mdl(symbol),
                    position: [x, y, z],
                    charge,
                });
            }
            (_, Block::Bond) => {
                let mut n = || t.next().and_then(|v| v.parse::<u32>().ok());
                let (Some(code), Some(a), Some(b)) = (n(), n(), n()) else {
                    return Err(malformed(at, format!("bond line {text:?}")));
                };
                raw_bonds.push((at, code, a, b));
            }
            _ => {}
        }
    }

    let Some((na, nb)) = declared else {
        return Err(malformed(first + 3, "V3000 record without a COUNTS line"));
    };
    if atoms.len() != na || raw_bonds.len() != nb {
        return Err(malformed(
            first + 3,
            format!(
                "COUNTS declares {na} atoms and {nb} bonds, the CTAB holds {} and {}",
                atoms.len(),
                raw_bonds.len()
            ),
        ));
    }
    let mut seen = HashSet::new();
    let mut bonds = Vec::with_capacity(nb);
    for (at, code, a, b) in raw_bonds {
        // Map atom ids to 1-based positions, then validate like V2000.
        let pos = |id: u32| ids.get(&id).map_or(0, |&i| i + 1);
        bonds.push(check_bond(pos(a), pos(b), code, na, &mut seen, at)?);
    }
    Ok((atoms, bonds, end))
}

/// Data items: `> <name>` (anything before/after the brackets is ignored), then
/// value lines up to a blank line.
fn parse_data_items(lines: &[&str]) -> Vec<(String, String)> {
    let mut out = Vec::new();
    let mut i = 0;
    while i < lines.len() {
        let line = lines[i];
        i += 1;
        if !line.starts_with('>') {
            continue;
        }
        let name = match (line.find('<'), line.rfind('>')) {
            (Some(open), Some(close)) if close > open => &line[open + 1..close],
            _ => line[1..].trim(),
        };
        let mut value = Vec::new();
        while i < lines.len() && !lines[i].trim().is_empty() {
            value.push(lines[i]);
            i += 1;
        }
        out.push((name.to_string(), value.join("\n")));
    }
    out
}

// ---- Writing -------------------------------------------------------------------

/// Write one pose as a V2000 record (ending in `$$$$`), with `properties` as data
/// items. Charges go to `M  CHG`. Append several calls to make a multi-record SDF.
///
/// V2000 holds at most 999 atoms and 999 bonds: larger poses are an error.
pub fn write_sdf<W: Write>(
    arena: &Arena,
    pose: PoseId,
    properties: &[(String, String)],
    mut writer: W,
) -> Result<(), IoError> {
    use std::fmt::Write as _;

    let p = arena.pose(pose).ok_or(IoError::NoSuchPose(pose))?;
    let bonds = arena.pose_bonds(pose).unwrap_or(&[]);
    let (n_atoms, n_bonds) = (p.atoms.len(), bonds.len());
    if n_atoms > 999 || n_bonds > 999 {
        return Err(IoError::Write(format!(
            "pose has {n_atoms} atoms and {n_bonds} bonds; V2000 holds at most 999 of each"
        )));
    }
    let title: String = p
        .name
        .chars()
        .map(|c| if c.is_control() { ' ' } else { c })
        .take(80)
        .collect();
    let mut out = String::with_capacity(64 + 70 * n_atoms + 13 * n_bonds);
    let _ = write!(
        out,
        "{title}\n  mold          3D\n\n{n_atoms:>3}{n_bonds:>3}  0  0  0  0  0  0  0  0999 V2000\n"
    );
    let mut charged = Vec::new();
    for (local, i) in p.atoms.clone().enumerate() {
        let [x, y, z] = arena.positions()[i];
        let symbol = mdl_symbol(arena.elements()[i]);
        let _ = writeln!(
            out,
            "{x:>10.4}{y:>10.4}{z:>10.4} {symbol:<3} 0  0  0  0  0  0  0  0  0  0  0  0"
        );
        let c = arena.formal_charges()[i];
        if c != 0 {
            charged.push((local + 1, c));
        }
    }
    for bond in bonds {
        let b = bond.relative_to(p.atoms.start);
        let _ = writeln!(
            out,
            "{:>3}{:>3}{:>3}  0",
            b.a + 1,
            b.b + 1,
            b.order.mdl_code()
        );
    }
    for chunk in charged.chunks(8) {
        let _ = write!(out, "M  CHG{:>3}", chunk.len());
        for (atom, c) in chunk {
            let _ = write!(out, " {atom:>3} {c:>3}");
        }
        out.push('\n');
    }
    out.push_str("M  END\n");
    for (name, value) in properties {
        let _ = write!(out, "> <{name}>\n{value}\n\n");
    }
    out.push_str("$$$$\n");
    writer.write_all(out.as_bytes())?;
    Ok(())
}

/// Mixed-case symbol as molfiles write it (`Cl`), `*` for unknown.
fn mdl_symbol(e: Element) -> String {
    if e.is_unknown() {
        return "*".to_string();
    }
    let upper = e.symbol();
    let mut s = upper[..1].to_string();
    s.push_str(&upper[1..].to_ascii_lowercase());
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    const WATER: &str = "water\n  test\n\n  3  2  0  0  0  0  0  0  0  0999 V2000\n\
        \x20   0.0000    0.0000    0.0000 O   0  0  0  0  0  0  0  0  0  0  0  0\n\
        \x20   0.9572    0.0000    0.0000 H   0  0  0  0  0  0  0  0  0  0  0  0\n\
        \x20  -0.2400    0.9266    0.0000 H   0  0  0  0  0  0  0  0  0  0  0  0\n\
        \x20 1  2  1  0\n  1  3  1  0\nM  END\n> <id>\nw1\n\n> <note>\nline one\nline two\n\n$$$$\n";

    #[test]
    fn reads_a_record_with_properties() {
        let mut arena = Arena::new();
        let read = read_sdf_str(&mut arena, WATER, "t");
        assert!(read.errors.is_empty(), "{:?}", read.errors);
        let r = &read.records[0];
        assert_eq!((r.index, r.pose, r.title.as_str()), (0, PoseId(0), "water"));
        assert_eq!(r.property("id"), Some("w1"));
        assert_eq!(r.property("note"), Some("line one\nline two"));
        assert_eq!(arena.atom_count(), 3);
        assert_eq!(arena.atom_names()[2], "H2");
        assert_eq!(arena.residues()[0].name, RESIDUE_NAME);
        assert_eq!(arena.chains()[0].id, CHAIN_ID);
        assert_eq!(arena.pose_bonds(PoseId(0)).unwrap().len(), 2);
        assert_eq!(arena.poses()[0].name, "water");
    }

    #[test]
    fn m_chg_replaces_atom_block_charges() {
        // Atom block says O is +3 (code 1); M  CHG says -1 on O and nothing on H.
        let text = WATER
            .replace(
                "O   0  0  0",
                "O   0  1  0", // ccc = 1 (+3)
            )
            .replace("M  END", "M  CHG  1   1  -1\nM  END");
        let mut arena = Arena::new();
        let read = read_sdf_str(&mut arena, &text, "t");
        assert!(read.errors.is_empty(), "{:?}", read.errors);
        assert_eq!(arena.formal_charges(), &[-1, 0, 0]);
        // Without M  CHG the atom-block code counts.
        let text = WATER.replace("O   0  0  0", "O   0  5  0"); // ccc = 5 (-1)
        let mut arena = Arena::new();
        read_sdf_str(&mut arena, &text, "t");
        assert_eq!(arena.formal_charges(), &[-1, 0, 0]);
    }

    #[test]
    fn blank_title_falls_back_to_name_and_index() {
        let text = format!("{WATER}{}", WATER.replacen("water", "", 1));
        let mut arena = Arena::new();
        let read = read_sdf_str(&mut arena, &text, "set");
        assert_eq!(read.records.len(), 2);
        assert_eq!(read.records[1].title, "");
        assert_eq!(arena.poses()[1].name, "set record 1");
    }

    #[test]
    fn query_bonds_and_bad_versions_are_unsupported() {
        let text = WATER.replace("  1  3  1  0", "  1  3  8  0");
        let mut arena = Arena::new();
        let read = read_sdf_str(&mut arena, &text, "t");
        assert!(matches!(
            read.errors[0].error,
            SdfError::Unsupported { line: 9, .. }
        ));
        assert!(arena.is_empty());
        let text = WATER.replace("V2000", "V4000");
        let read = read_sdf_str(&mut arena, &text, "t");
        assert!(matches!(
            read.errors[0].error,
            SdfError::Unsupported { line: 4, .. }
        ));
    }

    const V3000: &str = "v3\n  test\n\n  0  0  0     0  0            999 V3000\n\
        M  V30 BEGIN CTAB\nM  V30 COUNTS 3 2 0 0 0\nM  V30 BEGIN ATOM\n\
        M  V30 10 N 1.0 2.0 3.0 0 CHG=1\nM  V30 20 C 2.5 2.0 -\nM  V30 3.0 0\n\
        M  V30 30 Cl 4.0 2.0 3.0 0\nM  V30 END ATOM\nM  V30 BEGIN BOND\n\
        M  V30 1 1 10 20\nM  V30 2 2 20 30\nM  V30 END BOND\nM  V30 END CTAB\nM  END\n\
        > <id>\nv\n\n$$$$\n";

    #[test]
    fn reads_v3000() {
        let mut arena = Arena::new();
        let read = read_sdf_str(&mut arena, V3000, "t");
        assert!(read.errors.is_empty(), "{:?}", read.errors);
        assert_eq!(read.records[0].property("id"), Some("v"));
        assert_eq!(arena.elements(), &[Element::N, Element::C, Element::CL]);
        assert_eq!(arena.positions()[1], [2.5, 2.0, 3.0]);
        assert_eq!(arena.formal_charges(), &[1, 0, 0]);
        let bonds = arena.pose_bonds(PoseId(0)).unwrap();
        assert_eq!(
            (bonds[1].a, bonds[1].b, bonds[1].order),
            (1, 2, BondOrder::Double)
        );
    }

    #[test]
    fn v3000_count_mismatch_is_malformed() {
        let text = V3000.replace("COUNTS 3 2", "COUNTS 4 2");
        let mut arena = Arena::new();
        let read = read_sdf_str(&mut arena, &text, "t");
        assert!(matches!(read.errors[0].error, SdfError::Malformed { .. }));
        assert!(arena.is_empty());
    }

    #[test]
    fn mdl_symbols_are_mixed_case() {
        assert_eq!(mdl_symbol(Element::CL), "Cl");
        assert_eq!(mdl_symbol(Element::C), "C");
        assert_eq!(mdl_symbol(Element::UNKNOWN), "*");
        assert_eq!(element_from_mdl("D"), Element::H);
        assert_eq!(element_from_mdl("R#"), Element::UNKNOWN);
    }
}
