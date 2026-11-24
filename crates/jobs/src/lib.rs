//! Worker supervision. Workers are separate processes, never linked in.
//!
//! A job spec goes in as JSON on stdin. Events come back as newline-delimited
//! JSON on stdout, one object per line. Artifacts are written to the run
//! directory and referenced by path, never streamed through the pipe.
//!
//! See `docs/worker-protocol.md` and `CLAUDE.md` in this crate.

pub mod protocol;
