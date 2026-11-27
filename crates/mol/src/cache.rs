//! Binary arena cache: a compact, versioned, checksummed snapshot of an [`Arena`].
//!
//! Re-parsing mmCIF is far too slow for the "load 300 poses from cache in < 2 s"
//! budget (pdbtbx needs ~3 s for the 59 MB, 300-pose set). This format stores the
//! arena's arrays almost as they sit in memory, so loading is one read, one hash
//! and a few bulk little-endian conversions.
//!
//! **When** to write a cache and **where** to keep it is not `mol`'s business:
//! the run-directory cache belongs to `graph` (WS-04). `mol` only provides
//! [`save`] / [`load`] and the path helpers.
//!
//! # Format (version 1)
//! All integers are little-endian.
//!
//! | Bytes | Field |
//! |---|---|
//! | 16 | magic [`MAGIC`] (`"\x89MOLDARENA\r\n\x1a\n\0\0"`: catches text-mode mangling) |
//! | 4  | format version, `u32` ([`FORMAT_VERSION`]) |
//! | 8  | payload length, `u64` |
//! | 32 | BLAKE3 hash of the payload |
//! | …  | payload: sections in a fixed order |
//!
//! Each section is a 4-byte ASCII tag, a `u64` byte length and the bytes:
//! `POSN` positions (3×`f32`), `ELEM` elements (`u8`), `NAME` atom names (4 bytes),
//! `RESI` / `CHNI` residue / chain index (`u32`), `BFAC` / `OCCU` (`f32`), `CHRG`
//! (`i8`), `RESD` residues, `CHNS` chains, `POSE` poses (each with its bond range),
//! `BOND` bonds (`u32`, `u32`, `u8` order code). Strings are `u32` length + UTF-8;
//! ranges are two `u64`; an absent insertion code is `u32::MAX`.
//!
//! # Versioning policy
//! Any change to the layout bumps [`FORMAT_VERSION`], and a reader accepts **only**
//! its own version ([`CacheError::Version`]). A cache is derived data: on a
//! mismatch the caller re-creates it from the source file; there is no migration.
//!
//! # Untrusted input
//! A load never panics and never trusts a length: the payload hash is checked
//! first, then every array length and every index or range is validated against
//! the arena before it is built, so a consumer indexing with them cannot go out
//! of bounds. No `unsafe`.

use std::fmt;
use std::io::{BufReader, BufWriter, Read, Write};
use std::ops::Range;
use std::path::Path;

use crate::arena::{Arena, AtomName, Bond, BondOrder, Chain, Pose, Residue};
use crate::{Element, PoseId};

/// File magic, 16 bytes.
pub const MAGIC: [u8; 16] = *b"\x89MOLDARENA\r\n\x1a\n\0\0";
/// The only format version this build reads and writes.
pub const FORMAT_VERSION: u32 = 1;
/// Conventional file extension for arena caches.
pub const EXTENSION: &str = "moldarena";

const HEADER_LEN: usize = 16 + 4 + 8 + 32;

const SECTIONS: [&[u8; 4]; 12] = [
    b"POSN", b"ELEM", b"NAME", b"RESI", b"CHNI", b"BFAC", b"OCCU", b"CHRG", b"RESD", b"CHNS",
    b"POSE", b"BOND",
];

/// Why a cache could not be written or read.
#[derive(Debug)]
pub enum CacheError {
    Io(std::io::Error),
    /// The input does not start with [`MAGIC`]: not an arena cache.
    NotACache,
    /// An arena cache, but written by a different format version. Rebuild it.
    Version {
        found: u32,
        supported: u32,
    },
    /// The input ends before the length its header declares.
    Truncated,
    /// The payload does not match its BLAKE3 hash.
    Checksum,
    /// The hash matched but the content is inconsistent (e.g. an index out of range).
    Corrupt(String),
}

impl fmt::Display for CacheError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            CacheError::Io(e) => write!(f, "I/O error: {e}"),
            CacheError::NotACache => f.write_str("not a mold arena cache (bad magic)"),
            CacheError::Version { found, supported } => write!(
                f,
                "arena cache format version {found}, this build reads only version {supported}; rebuild the cache"
            ),
            CacheError::Truncated => f.write_str("arena cache is truncated"),
            CacheError::Checksum => f.write_str("arena cache checksum mismatch (corrupted)"),
            CacheError::Corrupt(m) => write!(f, "arena cache is corrupt: {m}"),
        }
    }
}

impl std::error::Error for CacheError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            CacheError::Io(e) => Some(e),
            _ => None,
        }
    }
}

impl From<std::io::Error> for CacheError {
    fn from(e: std::io::Error) -> Self {
        if e.kind() == std::io::ErrorKind::UnexpectedEof {
            CacheError::Truncated
        } else {
            CacheError::Io(e)
        }
    }
}

fn corrupt(m: impl Into<String>) -> CacheError {
    CacheError::Corrupt(m.into())
}

// ---- Writing ---------------------------------------------------------------------

/// Serialise `arena` to `writer`.
pub fn save<W: Write>(arena: &Arena, mut writer: W) -> Result<(), CacheError> {
    let payload = encode(arena);
    let hash = blake3::hash(&payload);
    writer.write_all(&MAGIC)?;
    writer.write_all(&FORMAT_VERSION.to_le_bytes())?;
    writer.write_all(&(payload.len() as u64).to_le_bytes())?;
    writer.write_all(hash.as_bytes())?;
    writer.write_all(&payload)?;
    writer.flush()?;
    Ok(())
}

/// Serialise `arena` to a file (created or truncated).
pub fn save_file(arena: &Arena, path: impl AsRef<Path>) -> Result<(), CacheError> {
    let file = std::fs::File::create(path)?;
    save(arena, BufWriter::new(file))
}

fn put_str(out: &mut Vec<u8>, s: &str) {
    out.extend_from_slice(&(s.len() as u32).to_le_bytes());
    out.extend_from_slice(s.as_bytes());
}

fn put_range(out: &mut Vec<u8>, r: &Range<usize>) {
    out.extend_from_slice(&(r.start as u64).to_le_bytes());
    out.extend_from_slice(&(r.end as u64).to_le_bytes());
}

fn section(out: &mut Vec<u8>, tag: &[u8; 4], body: impl FnOnce(&mut Vec<u8>)) {
    out.extend_from_slice(tag);
    let len_at = out.len();
    out.extend_from_slice(&[0; 8]);
    body(out);
    let len = (out.len() - len_at - 8) as u64;
    out[len_at..len_at + 8].copy_from_slice(&len.to_le_bytes());
}

fn encode(a: &Arena) -> Vec<u8> {
    let n = a.atom_count();
    let mut out = Vec::with_capacity(n * 34 + a.residues.len() * 48 + 1024);
    section(&mut out, b"POSN", |o| {
        for v in a.positions.as_flattened() {
            o.extend_from_slice(&v.to_le_bytes());
        }
    });
    section(&mut out, b"ELEM", |o| {
        o.extend(a.elements.iter().map(|e| e.0));
    });
    section(&mut out, b"NAME", |o| {
        for name in &a.atom_names {
            o.extend_from_slice(&name.0);
        }
    });
    for (tag, values) in [(b"RESI", &a.residue_indices), (b"CHNI", &a.chain_indices)] {
        section(&mut out, tag, |o| {
            for v in values {
                o.extend_from_slice(&v.to_le_bytes());
            }
        });
    }
    for (tag, values) in [(b"BFAC", &a.b_factors), (b"OCCU", &a.occupancies)] {
        section(&mut out, tag, |o| {
            for v in values {
                o.extend_from_slice(&v.to_le_bytes());
            }
        });
    }
    section(&mut out, b"CHRG", |o| {
        o.extend(a.formal_charges.iter().map(|&c| c as u8));
    });
    section(&mut out, b"RESD", |o| {
        o.extend_from_slice(&(a.residues.len() as u64).to_le_bytes());
        for r in &a.residues {
            put_str(o, &r.name);
            o.extend_from_slice(&r.seq_num.to_le_bytes());
            let icode = r.insertion_code.map_or(u32::MAX, u32::from);
            o.extend_from_slice(&icode.to_le_bytes());
            o.push(u8::from(r.hetero));
            o.extend_from_slice(&r.chain.to_le_bytes());
            put_range(o, &r.atoms);
        }
    });
    section(&mut out, b"CHNS", |o| {
        o.extend_from_slice(&(a.chains.len() as u64).to_le_bytes());
        for c in &a.chains {
            put_str(o, &c.id);
            o.extend_from_slice(&c.pose.0.to_le_bytes());
            put_range(o, &c.residues);
            put_range(o, &c.atoms);
        }
    });
    section(&mut out, b"POSE", |o| {
        o.extend_from_slice(&(a.poses.len() as u64).to_le_bytes());
        for (p, bonds) in a.poses.iter().zip(&a.pose_bonds) {
            put_str(o, &p.name);
            put_range(o, &p.atoms);
            put_range(o, &p.residues);
            put_range(o, &p.chains);
            put_range(o, bonds);
        }
    });
    section(&mut out, b"BOND", |o| {
        for b in &a.bonds {
            o.extend_from_slice(&b.a.to_le_bytes());
            o.extend_from_slice(&b.b.to_le_bytes());
            o.push(b.order.mdl_code());
        }
    });
    out
}

// ---- Reading ---------------------------------------------------------------------

/// Read an arena from `reader`. See the [module docs](self) for what is checked.
pub fn load<R: Read>(mut reader: R) -> Result<Arena, CacheError> {
    let len = read_header(&mut reader)?;
    let (declared_len, hash) = len;
    let mut payload = Vec::new();
    reader.take(declared_len).read_to_end(&mut payload)?;
    finish_load(payload, declared_len, hash)
}

/// Read an arena from a file.
pub fn load_file(path: impl AsRef<Path>) -> Result<Arena, CacheError> {
    let file = std::fs::File::open(path)?;
    let size = file.metadata()?.len();
    let mut reader = BufReader::new(file);
    let (declared_len, hash) = read_header(&mut reader)?;
    // The file size bounds the allocation, whatever the header claims.
    let available = size.saturating_sub(HEADER_LEN as u64);
    let mut payload = Vec::with_capacity(declared_len.min(available) as usize);
    reader.take(declared_len).read_to_end(&mut payload)?;
    finish_load(payload, declared_len, hash)
}

fn read_header<R: Read>(reader: &mut R) -> Result<(u64, [u8; 32]), CacheError> {
    let mut magic = [0u8; 16];
    // A short read of the magic is "not a cache" only if what is there differs.
    let got = read_up_to(reader, &mut magic)?;
    if magic[..got] != MAGIC[..got] {
        return Err(CacheError::NotACache);
    }
    if got < magic.len() {
        return Err(CacheError::Truncated);
    }
    let mut word = [0u8; 4];
    reader.read_exact(&mut word)?;
    let version = u32::from_le_bytes(word);
    if version != FORMAT_VERSION {
        return Err(CacheError::Version {
            found: version,
            supported: FORMAT_VERSION,
        });
    }
    let mut len = [0u8; 8];
    reader.read_exact(&mut len)?;
    let mut hash = [0u8; 32];
    reader.read_exact(&mut hash)?;
    Ok((u64::from_le_bytes(len), hash))
}

fn read_up_to<R: Read>(reader: &mut R, buf: &mut [u8]) -> Result<usize, CacheError> {
    let mut got = 0;
    while got < buf.len() {
        match reader.read(&mut buf[got..]) {
            Ok(0) => break,
            Ok(n) => got += n,
            Err(e) if e.kind() == std::io::ErrorKind::Interrupted => {}
            Err(e) => return Err(e.into()),
        }
    }
    Ok(got)
}

fn finish_load(payload: Vec<u8>, declared: u64, hash: [u8; 32]) -> Result<Arena, CacheError> {
    if (payload.len() as u64) < declared {
        return Err(CacheError::Truncated);
    }
    if blake3::hash(&payload) != blake3::Hash::from_bytes(hash) {
        return Err(CacheError::Checksum);
    }
    decode(&payload)
}

/// A bounds-checked cursor over a byte slice.
struct Cursor<'a> {
    bytes: &'a [u8],
    at: usize,
    what: &'static str,
}

impl<'a> Cursor<'a> {
    fn take(&mut self, n: usize) -> Result<&'a [u8], CacheError> {
        let end = self
            .at
            .checked_add(n)
            .filter(|&e| e <= self.bytes.len())
            .ok_or_else(|| corrupt(format!("{} section overruns", self.what)))?;
        let s = &self.bytes[self.at..end];
        self.at = end;
        Ok(s)
    }

    fn array<const N: usize>(&mut self) -> Result<[u8; N], CacheError> {
        let mut out = [0u8; N];
        out.copy_from_slice(self.take(N)?);
        Ok(out)
    }

    fn u8(&mut self) -> Result<u8, CacheError> {
        Ok(self.array::<1>()?[0])
    }

    fn u32(&mut self) -> Result<u32, CacheError> {
        Ok(u32::from_le_bytes(self.array()?))
    }

    fn i32(&mut self) -> Result<i32, CacheError> {
        Ok(i32::from_le_bytes(self.array()?))
    }

    fn u64(&mut self) -> Result<u64, CacheError> {
        Ok(u64::from_le_bytes(self.array()?))
    }

    fn usize(&mut self) -> Result<usize, CacheError> {
        usize::try_from(self.u64()?).map_err(|_| corrupt(format!("{}: value too large", self.what)))
    }

    fn range(&mut self) -> Result<Range<usize>, CacheError> {
        Ok(self.usize()?..self.usize()?)
    }

    fn string(&mut self) -> Result<String, CacheError> {
        let len = self.u32()? as usize;
        let bytes = self.take(len)?;
        String::from_utf8(bytes.to_vec())
            .map_err(|_| corrupt(format!("{}: string is not UTF-8", self.what)))
    }

    /// A record count, sanity-checked against the bytes left (each record ≥ `min_size`).
    fn count(&mut self, min_size: usize) -> Result<usize, CacheError> {
        let n = self.usize()?;
        if n.saturating_mul(min_size) > self.bytes.len() - self.at {
            return Err(corrupt(format!(
                "{}: count {n} exceeds the section",
                self.what
            )));
        }
        Ok(n)
    }

    fn done(&self) -> Result<(), CacheError> {
        if self.at == self.bytes.len() {
            Ok(())
        } else {
            Err(corrupt(format!("{}: trailing bytes", self.what)))
        }
    }
}

/// Split the payload into its sections, in the fixed order.
fn sections(payload: &[u8]) -> Result<Vec<&[u8]>, CacheError> {
    let mut c = Cursor {
        bytes: payload,
        at: 0,
        what: "payload",
    };
    let mut out = Vec::with_capacity(SECTIONS.len());
    for tag in SECTIONS {
        let found = c.array::<4>()?;
        if &found != tag {
            return Err(corrupt(format!(
                "expected section {}, found {:?}",
                String::from_utf8_lossy(tag),
                String::from_utf8_lossy(&found)
            )));
        }
        let len = c.usize()?;
        out.push(c.take(len)?);
    }
    c.done()?;
    Ok(out)
}

/// A per-atom array of fixed-size items.
fn per_atom<const N: usize, T>(
    bytes: &[u8],
    what: &str,
    f: impl Fn([u8; N]) -> T,
) -> Result<Vec<T>, CacheError> {
    let (items, rest) = bytes.as_chunks::<N>();
    if !rest.is_empty() {
        return Err(corrupt(format!("{what}: length not a multiple of {N}")));
    }
    Ok(items.iter().map(|&a| f(a)).collect())
}

fn check_range(r: &Range<usize>, len: usize, what: &str) -> Result<(), CacheError> {
    if r.start <= r.end && r.end <= len {
        Ok(())
    } else {
        Err(corrupt(format!("{what} range {r:?} outside 0..{len}")))
    }
}

fn decode(payload: &[u8]) -> Result<Arena, CacheError> {
    let s = sections(payload)?;
    let f32_le = |b: [u8; 4]| f32::from_le_bytes(b);
    let u32_le = |b: [u8; 4]| u32::from_le_bytes(b);

    let positions = per_atom(s[0], "positions", |b: [u8; 12]| {
        std::array::from_fn(|k| {
            f32::from_le_bytes([b[4 * k], b[4 * k + 1], b[4 * k + 2], b[4 * k + 3]])
        })
    })?;
    let n = positions.len();
    let elements: Vec<Element> = s[1].iter().map(|&e| Element(e)).collect();
    let atom_names = per_atom(s[2], "atom names", AtomName)?;
    if let Some(bad) = atom_names.iter().find(|a| !a.0.is_ascii()) {
        return Err(corrupt(format!("atom name {:?} is not ASCII", bad.0)));
    }
    let residue_indices = per_atom(s[3], "residue indices", u32_le)?;
    let chain_indices = per_atom(s[4], "chain indices", u32_le)?;
    let b_factors = per_atom(s[5], "B-factors", f32_le)?;
    let occupancies = per_atom(s[6], "occupancies", f32_le)?;
    let formal_charges: Vec<i8> = s[7].iter().map(|&c| c as i8).collect();
    let lens = [
        elements.len(),
        atom_names.len(),
        residue_indices.len(),
        chain_indices.len(),
        b_factors.len(),
        occupancies.len(),
        formal_charges.len(),
    ];
    if lens.iter().any(|&l| l != n) {
        return Err(corrupt(format!(
            "per-atom arrays differ in length: {n} positions vs {lens:?}"
        )));
    }

    // Residues: name(≥4) seq(4) icode(4) hetero(1) chain(4) atoms(16) = ≥33 bytes.
    let mut c = Cursor {
        bytes: s[8],
        at: 0,
        what: "residues",
    };
    let count = c.count(33)?;
    let mut residues = Vec::with_capacity(count);
    for _ in 0..count {
        let name = c.string()?;
        let seq_num = c.i32()?;
        let icode = c.u32()?;
        let insertion_code = match icode {
            u32::MAX => None,
            v => Some(char::from_u32(v).ok_or_else(|| corrupt("invalid insertion code"))?),
        };
        let hetero = match c.u8()? {
            0 => false,
            1 => true,
            _ => return Err(corrupt("residue hetero flag not 0/1")),
        };
        let chain = c.u32()?;
        let atoms = c.range()?;
        check_range(&atoms, n, "residue atom")?;
        residues.push(Residue {
            name,
            seq_num,
            insertion_code,
            hetero,
            chain,
            atoms,
        });
    }
    c.done()?;

    // Chains: id(≥4) pose(4) residues(16) atoms(16) = ≥40 bytes.
    let mut c = Cursor {
        bytes: s[9],
        at: 0,
        what: "chains",
    };
    let count = c.count(40)?;
    let mut chains = Vec::with_capacity(count);
    for _ in 0..count {
        let id = c.string()?;
        let pose = PoseId(c.u32()?);
        let res = c.range()?;
        let atoms = c.range()?;
        check_range(&res, residues.len(), "chain residue")?;
        check_range(&atoms, n, "chain atom")?;
        chains.push(Chain {
            id,
            pose,
            residues: res,
            atoms,
        });
    }
    c.done()?;

    let bonds = per_atom(s[11], "bonds", |b: [u8; 9]| {
        let a = u32::from_le_bytes([b[0], b[1], b[2], b[3]]);
        let bb = u32::from_le_bytes([b[4], b[5], b[6], b[7]]);
        (a, bb, b[8])
    })?
    .into_iter()
    .map(|(a, b, code)| {
        let order =
            BondOrder::from_mdl_code(code).ok_or_else(|| corrupt(format!("bond order {code}")))?;
        if a as usize >= n || b as usize >= n {
            return Err(corrupt(format!("bond {a}-{b} outside 0..{n}")));
        }
        Ok(Bond { a, b, order })
    })
    .collect::<Result<Vec<_>, _>>()?;

    // Poses: name(≥4) + four ranges (64) = ≥68 bytes.
    let mut c = Cursor {
        bytes: s[10],
        at: 0,
        what: "poses",
    };
    let count = c.count(68)?;
    let mut poses = Vec::with_capacity(count);
    let mut pose_bonds = Vec::with_capacity(count);
    for _ in 0..count {
        let name = c.string()?;
        let atoms = c.range()?;
        let res = c.range()?;
        let ch = c.range()?;
        let bond_range = c.range()?;
        check_range(&atoms, n, "pose atom")?;
        check_range(&res, residues.len(), "pose residue")?;
        check_range(&ch, chains.len(), "pose chain")?;
        check_range(&bond_range, bonds.len(), "pose bond")?;
        poses.push(Pose {
            name,
            atoms,
            residues: res,
            chains: ch,
        });
        pose_bonds.push(bond_range);
    }
    c.done()?;

    if let Some(i) = residue_indices
        .iter()
        .position(|&r| r as usize >= residues.len())
    {
        return Err(corrupt(format!("atom {i}: residue index out of range")));
    }
    if let Some(i) = chain_indices
        .iter()
        .position(|&r| r as usize >= chains.len())
    {
        return Err(corrupt(format!("atom {i}: chain index out of range")));
    }
    if let Some(r) = residues.iter().find(|r| r.chain as usize >= chains.len()) {
        return Err(corrupt(format!("residue {} names a missing chain", r.name)));
    }
    if let Some(c) = chains.iter().find(|c| c.pose.0 as usize >= poses.len()) {
        return Err(corrupt(format!("chain {} names a missing pose", c.id)));
    }

    Ok(Arena {
        positions,
        elements,
        atom_names,
        residue_indices,
        chain_indices,
        b_factors,
        occupancies,
        formal_charges,
        residues,
        chains,
        poses,
        bonds,
        pose_bonds,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::arena::AtomRecord;

    fn small() -> Arena {
        let mut arena = Arena::new();
        let mut b = arena.begin_pose("p");
        b.add_chain("A");
        b.add_residue("LIG", 7, Some('B'), true);
        let x = b.add_atom(AtomRecord {
            name: AtomName::new("C1").unwrap(),
            element: Element::C,
            position: [1.0, -2.5, 3.25],
            b_factor: 12.5,
            occupancy: 0.5,
            formal_charge: -1,
        });
        let y = b.add_atom(AtomRecord {
            name: AtomName::new("N1").unwrap(),
            element: Element::N,
            position: [0.0, 0.0, 0.0],
            b_factor: 0.0,
            occupancy: 1.0,
            formal_charge: 1,
        });
        b.add_bond(x, y, BondOrder::Aromatic);
        b.finish();
        arena
    }

    fn bytes(arena: &Arena) -> Vec<u8> {
        let mut buf = Vec::new();
        save(arena, &mut buf).unwrap();
        buf
    }

    #[test]
    fn empty_and_small_round_trip() {
        for arena in [Arena::new(), small()] {
            assert_eq!(load(bytes(&arena).as_slice()).unwrap(), arena);
        }
    }

    #[test]
    fn header_errors() {
        let good = bytes(&small());
        assert!(matches!(load(&b"HELLO"[..]), Err(CacheError::NotACache)));
        assert!(matches!(load(&good[..5]), Err(CacheError::Truncated)));
        assert!(matches!(load(&[][..]), Err(CacheError::Truncated)));
        let mut v2 = good.clone();
        v2[16..20].copy_from_slice(&2u32.to_le_bytes());
        assert!(matches!(
            load(v2.as_slice()),
            Err(CacheError::Version {
                found: 2,
                supported: 1
            })
        ));
    }

    /// A payload that hashes correctly but lies: validation, not the hash, must catch it.
    #[test]
    fn rehashed_inconsistent_payload_is_corrupt() {
        let good = bytes(&small());
        let mut payload = good[HEADER_LEN..].to_vec();
        // The last 9 bytes are the one bond; point it past the atoms.
        let at = payload.len() - 9;
        payload[at..at + 4].copy_from_slice(&99u32.to_le_bytes());
        let mut forged = good[..HEADER_LEN - 32].to_vec();
        forged.extend_from_slice(blake3::hash(&payload).as_bytes());
        forged.extend_from_slice(&payload);
        assert!(matches!(
            load(forged.as_slice()),
            Err(CacheError::Corrupt(_))
        ));
    }
}
