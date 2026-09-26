//! Layout of the shared gas/terrain uniform, read from the shader itself.
//!
//! `Params` is declared in WGSL (`gas.wgsl`, `terrain_common.wgsl`) and
//! filled by the renderer. These tests cannot depend on `render`, and a
//! hard-coded size or float index goes stale the moment the struct grows:
//! ADR-414 appended two vectors, and every GPU gas and terrain test failed
//! validation on a 352-byte buffer. Offsets are looked up by
//! field name instead: appended fields keep working, and a renamed or
//! removed field fails with its name.
// Each test binary compiles this module on its own and uses part of it.
#![allow(dead_code)]

/// Fields of `struct Params` in declaration order, in floats.
pub struct Layout {
    fields: Vec<(String, usize)>,
    /// Size of the whole struct in floats.
    pub floats: usize,
}

impl Layout {
    /// Parse `struct Params { ... }` of a WGSL source. Every field is a
    /// `vec4<f32>` or a `mat4x4<f32>`: both are 16-byte aligned, so the
    /// fields follow each other without padding. Another type is refused
    /// rather than guessed.
    pub fn of(wgsl: &str) -> Layout {
        let start = wgsl
            .find("struct Params {")
            .expect("the shader declares struct Params");
        let body = &wgsl[start + "struct Params {".len()..];
        let body = &body[..body.find('}').expect("struct Params is closed")];
        // Comments carry commas of their own ("xyz world origin, w cell edge").
        let code: String = body
            .lines()
            .map(|l| l.split("//").next().unwrap_or(""))
            .collect::<Vec<_>>()
            .join(" ");
        let mut fields = Vec::new();
        let mut at = 0usize;
        for field in code.split(',') {
            let field = field.trim();
            if field.is_empty() {
                continue;
            }
            let (name, ty) = field.split_once(':').expect("field is `name: type`");
            let ty = ty.trim();
            let size = if ty.starts_with("mat4x4") {
                16
            } else if ty.starts_with("vec4") {
                4
            } else {
                panic!("Params field {name} has type {ty}, not a vec4 or mat4x4");
            };
            fields.push((name.trim().to_string(), at));
            at += size;
        }
        Layout { fields, floats: at }
    }

    /// First float of the named field.
    pub fn at(&self, name: &str) -> usize {
        self.fields
            .iter()
            .find(|(n, _)| n == name)
            .unwrap_or_else(|| panic!("Params has no field {name}"))
            .1
    }

    /// Size of the uniform buffer in bytes.
    pub fn bytes(&self) -> u64 {
        (self.floats * 4) as u64
    }

    /// Field names and offsets, for comparing two declarations.
    pub fn fields(&self) -> &[(String, usize)] {
        &self.fields
    }
}
