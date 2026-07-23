//! Minimal pure-Rust MATBIN parser.
//!
//! MATBIN (`material/allmaterial.matbinbnd.dcx` members, magic `MAB\0`) binds a
//! FLVER material to a shader (`shader_path`, an SPX material-shader like
//! `C[DetailBlend].spx`) plus its sampler->texture map and `FC_*` parameter values.
//! Layout verified 2026-06-25 against the live archives; mirrors the field order of
//! `fstools_formats::matbin` but with no external dependency so the M1 trace builds
//! and tests fast.
//!
//! Strings are UTF-16LE wide C-strings at absolute byte offsets from the start of
//! the buffer.

use thiserror::Error;

const HEADER_SIZE: usize = 56;
const PARAMETER_SIZE: usize = 40;
const SAMPLER_SIZE: usize = 48;
const MAGIC: &[u8; 4] = b"MAB\0";

#[derive(Debug, Error, PartialEq, Eq)]
pub enum MatbinError {
    #[error("buffer too small ({got} bytes, need {need})")]
    TooSmall { got: usize, need: usize },
    #[error("bad MATBIN magic {0:02x?} (expected MAB\\0)")]
    BadMagic([u8; 4]),
    #[error("offset {offset} out of bounds (len {len})")]
    OutOfBounds { offset: usize, len: usize },
    #[error("unknown parameter value type {0:#x}")]
    UnknownParamType(u32),
}

/// A parsed MATBIN: the shader binding plus its samplers and parameters.
#[derive(Debug, Clone, PartialEq)]
pub struct Matbin {
    /// SPX material-shader path, e.g. `N:\GR\...\SPX\C[DetailBlend].spx`.
    pub shader_path: String,
    /// Source `.matxml` path the matbin was built from.
    pub source_path: String,
    pub samplers: Vec<Sampler>,
    pub parameters: Vec<Parameter>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Sampler {
    pub name: String,
    /// Texture path (TPF), often empty when the FLVER supplies the texture.
    pub path: String,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Parameter {
    pub name: String,
    pub value: ParamValue,
}

#[derive(Debug, Clone, PartialEq)]
pub enum ParamValue {
    Bool(bool),
    Int(i32),
    Int2([i32; 2]),
    Float(f32),
    Float2([f32; 2]),
    Float3([f32; 3]),
    Float4([f32; 4]),
    Float5([f32; 5]),
}

impl Matbin {
    /// Parse a MATBIN from its (already DCX-decompressed) member bytes.
    pub fn parse(bytes: &[u8]) -> Result<Self, MatbinError> {
        if bytes.len() < HEADER_SIZE {
            return Err(MatbinError::TooSmall {
                got: bytes.len(),
                need: HEADER_SIZE,
            });
        }
        let magic: [u8; 4] = bytes[0..4].try_into().unwrap();
        if &magic != MAGIC {
            return Err(MatbinError::BadMagic(magic));
        }
        let shader_path_off = u64_at(bytes, 8)? as usize;
        let source_path_off = u64_at(bytes, 16)? as usize;
        let param_count = u32_at(bytes, 28)? as usize;
        let sampler_count = u32_at(bytes, 32)? as usize;

        let shader_path = wide_cstr(bytes, shader_path_off)?;
        let source_path = wide_cstr(bytes, source_path_off)?;

        let mut parameters = Vec::with_capacity(param_count);
        for i in 0..param_count {
            let base = HEADER_SIZE + i * PARAMETER_SIZE;
            let name_off = u64_at(bytes, base)? as usize;
            let value_off = u64_at(bytes, base + 8)? as usize;
            let value_type = u32_at(bytes, base + 20)?;
            parameters.push(Parameter {
                name: wide_cstr(bytes, name_off)?,
                value: ParamValue::parse(value_type, bytes, value_off)?,
            });
        }

        let samplers_base = HEADER_SIZE + param_count * PARAMETER_SIZE;
        let mut samplers = Vec::with_capacity(sampler_count);
        for i in 0..sampler_count {
            let base = samplers_base + i * SAMPLER_SIZE;
            let name_off = u64_at(bytes, base)? as usize;
            let path_off = u64_at(bytes, base + 8)? as usize;
            samplers.push(Sampler {
                name: wide_cstr(bytes, name_off)?,
                path: wide_cstr(bytes, path_off)?,
            });
        }

        Ok(Matbin {
            shader_path,
            source_path,
            samplers,
            parameters,
        })
    }

    /// The shader's leaf name without directories or the `.spx` extension, e.g.
    /// `N:\...\SPX\C[DetailBlend].spx` -> `C[DetailBlend]`. This is the key used to
    /// group materials by shader.
    pub fn shader_name(&self) -> String {
        shader_leaf(&self.shader_path)
    }
}

/// Leaf of an SPX shader path, normalized across `\`/`/` separators and stripped of
/// the `.spx` extension.
pub fn shader_leaf(shader_path: &str) -> String {
    let leaf = shader_path
        .rsplit(|c| c == '\\' || c == '/')
        .next()
        .unwrap_or(shader_path);
    leaf.strip_suffix(".spx").unwrap_or(leaf).to_owned()
}

impl ParamValue {
    fn parse(value_type: u32, bytes: &[u8], off: usize) -> Result<Self, MatbinError> {
        Ok(match value_type {
            0x0 => ParamValue::Bool(*byte_at(bytes, off)? != 0),
            0x4 => ParamValue::Int(i32_at(bytes, off)?),
            0x5 => ParamValue::Int2([i32_at(bytes, off)?, i32_at(bytes, off + 4)?]),
            0x8 => ParamValue::Float(f32_at(bytes, off)?),
            0x9 => ParamValue::Float2(read_f32s::<2>(bytes, off)?),
            0xA => ParamValue::Float3(read_f32s::<3>(bytes, off)?),
            0xB => ParamValue::Float4(read_f32s::<4>(bytes, off)?),
            0xC => ParamValue::Float5(read_f32s::<5>(bytes, off)?),
            other => return Err(MatbinError::UnknownParamType(other)),
        })
    }
}

// --- little-endian primitive reads (bounds-checked) -------------------------

fn byte_at(b: &[u8], off: usize) -> Result<&u8, MatbinError> {
    b.get(off).ok_or(MatbinError::OutOfBounds {
        offset: off,
        len: b.len(),
    })
}

fn arr<const N: usize>(b: &[u8], off: usize) -> Result<[u8; N], MatbinError> {
    b.get(off..off + N)
        .and_then(|s| s.try_into().ok())
        .ok_or(MatbinError::OutOfBounds {
            offset: off + N,
            len: b.len(),
        })
}

fn u32_at(b: &[u8], off: usize) -> Result<u32, MatbinError> {
    Ok(u32::from_le_bytes(arr::<4>(b, off)?))
}
fn i32_at(b: &[u8], off: usize) -> Result<i32, MatbinError> {
    Ok(i32::from_le_bytes(arr::<4>(b, off)?))
}
fn u64_at(b: &[u8], off: usize) -> Result<u64, MatbinError> {
    Ok(u64::from_le_bytes(arr::<8>(b, off)?))
}
fn f32_at(b: &[u8], off: usize) -> Result<f32, MatbinError> {
    Ok(f32::from_le_bytes(arr::<4>(b, off)?))
}
fn read_f32s<const N: usize>(b: &[u8], off: usize) -> Result<[f32; N], MatbinError> {
    let mut out = [0.0f32; N];
    for (i, slot) in out.iter_mut().enumerate() {
        *slot = f32_at(b, off + i * 4)?;
    }
    Ok(out)
}

/// Read a UTF-16LE NUL-terminated string at `off`. Unpaired surrogates / invalid
/// units are replaced (display-only strings, never used as keys for binary data).
fn wide_cstr(b: &[u8], off: usize) -> Result<String, MatbinError> {
    if off > b.len() {
        return Err(MatbinError::OutOfBounds {
            offset: off,
            len: b.len(),
        });
    }
    let mut units = Vec::new();
    let mut i = off;
    while i + 1 < b.len() {
        let u = u16::from_le_bytes([b[i], b[i + 1]]);
        if u == 0 {
            break;
        }
        units.push(u);
        i += 2;
    }
    Ok(String::from_utf16_lossy(&units))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Hand-build a minimal valid MATBIN: 1 float param "FC_X"=2.5, 1 sampler
    /// "g_Diffuse"->"tex.tif", shader "C[DetailBlend].spx". Strings packed after
    /// the param/sampler tables.
    fn synth() -> Vec<u8> {
        fn wide(s: &str) -> Vec<u8> {
            let mut v: Vec<u8> = s.encode_utf16().flat_map(|u| u.to_le_bytes()).collect();
            v.extend_from_slice(&[0, 0]);
            v
        }
        let param_count = 1usize;
        let sampler_count = 1usize;
        let strings_base =
            HEADER_SIZE + param_count * PARAMETER_SIZE + sampler_count * SAMPLER_SIZE;

        // Lay out strings and remember their offsets.
        let mut strings = Vec::new();
        let mut off = strings_base;
        let push = |s: &str, strings: &mut Vec<u8>, off: &mut usize| -> u64 {
            let at = *off as u64;
            let w = wide(s);
            *off += w.len();
            strings.extend_from_slice(&w);
            at
        };
        let shader_off = push("N:\\GR\\SPX\\C[DetailBlend].spx", &mut strings, &mut off);
        let source_off = push("c4800_Body.matxml", &mut strings, &mut off);
        let pname_off = push("FC_X", &mut strings, &mut off);
        let pval_off = {
            let at = off as u64;
            strings.extend_from_slice(&2.5f32.to_le_bytes());
            off += 4;
            at
        };
        let sname_off = push("g_Diffuse", &mut strings, &mut off);
        let spath_off = push("tex.tif", &mut strings, &mut off);

        let mut b = vec![0u8; strings_base];
        b[0..4].copy_from_slice(MAGIC);
        b[8..16].copy_from_slice(&shader_off.to_le_bytes());
        b[16..24].copy_from_slice(&source_off.to_le_bytes());
        b[28..32].copy_from_slice(&(param_count as u32).to_le_bytes());
        b[32..36].copy_from_slice(&(sampler_count as u32).to_le_bytes());
        // Parameter 0
        let p0 = HEADER_SIZE;
        b[p0..p0 + 8].copy_from_slice(&pname_off.to_le_bytes());
        b[p0 + 8..p0 + 16].copy_from_slice(&pval_off.to_le_bytes());
        b[p0 + 20..p0 + 24].copy_from_slice(&0x8u32.to_le_bytes()); // float
        // Sampler 0
        let s0 = HEADER_SIZE + PARAMETER_SIZE;
        b[s0..s0 + 8].copy_from_slice(&sname_off.to_le_bytes());
        b[s0 + 8..s0 + 16].copy_from_slice(&spath_off.to_le_bytes());

        b.extend_from_slice(&strings);
        b
    }

    #[test]
    fn parses_shader_samplers_and_params() {
        let m = Matbin::parse(&synth()).expect("parse");
        assert_eq!(m.shader_path, "N:\\GR\\SPX\\C[DetailBlend].spx");
        assert_eq!(m.shader_name(), "C[DetailBlend]");
        assert_eq!(m.source_path, "c4800_Body.matxml");
        assert_eq!(m.parameters.len(), 1);
        assert_eq!(m.parameters[0].name, "FC_X");
        assert_eq!(m.parameters[0].value, ParamValue::Float(2.5));
        assert_eq!(m.samplers.len(), 1);
        assert_eq!(m.samplers[0].name, "g_Diffuse");
        assert_eq!(m.samplers[0].path, "tex.tif");
    }

    #[test]
    fn rejects_bad_magic() {
        let mut b = synth();
        b[0] = b'X';
        assert!(matches!(Matbin::parse(&b), Err(MatbinError::BadMagic(_))));
    }

    #[test]
    fn rejects_truncated() {
        assert!(matches!(
            Matbin::parse(&[0u8; 8]),
            Err(MatbinError::TooSmall { .. })
        ));
    }

    #[test]
    fn shader_leaf_normalizes_separators_and_ext() {
        assert_eq!(shader_leaf("a/b\\C[Fur]_cloth.spx"), "C[Fur]_cloth");
        assert_eq!(shader_leaf("M[AMSN_V][Ov_N].spx"), "M[AMSN_V][Ov_N]");
        assert_eq!(shader_leaf("plain"), "plain");
    }
}
