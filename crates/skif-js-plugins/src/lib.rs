//! JavaScript and TypeScript tool knowledge for skif.
//!
//! Today this crate holds the [`scripts`] module, which turns a `package.json` script into the
//! binaries, entry files and dependencies it references. The declarative plugin table described in
//! ADR 0011 comes later and will live next to it.

pub mod scripts;
