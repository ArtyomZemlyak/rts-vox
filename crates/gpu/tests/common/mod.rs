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
    /// Read WGSL's actual layout, including nested structures, arrays and
    /// padding. Use the same parser as wgpu instead of splitting generic
    /// types at commas or maintaining a second set of alignment rules.
    pub fn of(wgsl: &str) -> Layout {
        let module = wgpu::naga::front::wgsl::parse_str(wgsl).expect("valid WGSL");
        let (_, params) = module
            .types
            .iter()
            .find(|(_, ty)| ty.name.as_deref() == Some("Params"))
            .expect("the shader declares Params");
        let wgpu::naga::TypeInner::Struct { members, span } = &params.inner else {
            panic!("Params must be a structure");
        };
        assert_eq!(span % 4, 0, "uniform size must fit float storage");
        let fields = members
            .iter()
            .map(|member| {
                assert_eq!(member.offset % 4, 0, "field offset must fit float storage");
                (
                    member.name.clone().expect("named Params field"),
                    member.offset as usize / 4,
                )
            })
            .collect();
        Layout {
            fields,
            floats: *span as usize / 4,
        }
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
