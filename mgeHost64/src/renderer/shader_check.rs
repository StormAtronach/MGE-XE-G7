//! The check of a WGSL shader without a GPU, as the device checks it.
//!
//! The helper asks its device for a set of features and limits (`nirender_wgpu`,
//! `host/src/renderer.rs`, the request of the device). wgpu turns the features into the
//! capabilities of naga and refuses a shader that needs more; a pipeline refuses a shader
//! with more bindings of a class than the limits give a stage. This module does the same
//! with the smallest set the request can get, so that a shader which uses a feature the
//! request lacks, or more bindings than the limits allow, fails a unit test here, or the
//! load of a mod's water shader, and not a player's game. [`features`] and [`limits`] copy
//! the request: a change there changes them.

use nirender_wgpu::wgpu::{self, naga};

/// The features the request requires of every adapter (`nirender_wgpu::required_features`).
/// The optional ones of the request (the line mode, the timestamps, the indirect draws, the
/// float depth) map to no capability of naga, so they play no part in a shader.
fn features() -> wgpu::Features {
    nirender_wgpu::required_features()
}

/// The limits that this check compares: the bind groups, and the bindings of each class in
/// one stage, as the request fixes them (`nirender_wgpu::fixed_limits`: the sampled textures
/// at 32, the default of wgpu for the rest). The elements of a binding array, the workgroup
/// size and the varyings are not compared here.
fn limits() -> wgpu::Limits {
    nirender_wgpu::fixed_limits()
}

/// Reads and checks a shader as the device would: the capabilities of [`features`], every
/// validation of naga, the bind groups within [`limits`], and for each entry point the
/// bindings it uses within the limits of a stage. The error names the shader by `label`.
pub(crate) fn check(label: &str, source: &str) -> Result<naga::Module, String> {
    check_with(label, source, features())
}

/// [`check`] with the features of one texture tier of the request
/// (`nirender_wgpu::required_features_of`): the world shader of tier A takes the binding
/// arrays of its tier.
pub(crate) fn check_with(label: &str, source: &str, features: wgpu::Features) -> Result<naga::Module, String> {
    // No downlevel flag: the request asks for none, so a shader gets no capability from one.
    let downlevel = wgpu::DownlevelFlags::empty();
    let capabilities = wgpu_naga_bridge::features_to_naga_capabilities(features, downlevel);
    let mut options = naga::front::wgsl::Options::new();
    options.capabilities = capabilities;
    let module = naga::front::wgsl::Frontend::new_with_options(options).parse(source).map_err(|error| format!("{label}: {}", error.emit_to_string(source)))?;
    let limits = limits();
    let over_the_groups = module.global_variables.iter().filter_map(|(_, variable)| variable.binding.as_ref()).find(|binding| binding.group >= limits.max_bind_groups);
    if let Some(binding) = over_the_groups {
        return Err(format!("{label}: the binding group {} is at or over the limit of {} groups", binding.group, limits.max_bind_groups));
    }
    let mut validator = wgpu_naga_bridge::create_validator(features, downlevel, naga::valid::ValidationFlags::all());
    let info = validator.validate(&module).map_err(|error| format!("{label}: {}", error.emit_to_string(source)))?;
    for (index, entry) in module.entry_points.iter().enumerate() {
        let uses = info.get_entry_point(index);
        let mut counts = Counts::default();
        for (handle, variable) in module.global_variables.iter() {
            if variable.binding.is_some() && !uses[handle].is_empty() {
                counts.add(&module.types[variable.ty].inner, variable.space);
            }
        }
        counts.within(&limits).map_err(|problem| format!("{label}, the entry point {}: {problem}", entry.name))?;
    }
    Ok(module)
}

/// The bindings an entry point uses, by the class that has a limit for each stage.
#[derive(Default)]
struct Counts {
    uniform_buffers: u32,
    storage_buffers: u32,
    sampled_textures: u32,
    storage_textures: u32,
    samplers: u32,
}

impl Counts {
    fn add(&mut self, kind: &naga::TypeInner, space: naga::AddressSpace) {
        match kind {
            // A binding array has its own limit on its elements, which the request takes from
            // the adapter: nothing to compare here.
            naga::TypeInner::BindingArray { .. } => {}
            naga::TypeInner::Image { class: naga::ImageClass::Storage { .. }, .. } => self.storage_textures += 1,
            naga::TypeInner::Image { .. } => self.sampled_textures += 1,
            naga::TypeInner::Sampler { .. } => self.samplers += 1,
            _ => match space {
                naga::AddressSpace::Uniform => self.uniform_buffers += 1,
                naga::AddressSpace::Storage { .. } => self.storage_buffers += 1,
                _ => {}
            },
        }
    }

    fn within(&self, limits: &wgpu::Limits) -> Result<(), String> {
        let classes = [
            (self.uniform_buffers, limits.max_uniform_buffers_per_shader_stage, "uniform buffers"),
            (self.storage_buffers, limits.max_storage_buffers_per_shader_stage, "storage buffers"),
            (self.sampled_textures, limits.max_sampled_textures_per_shader_stage, "sampled textures"),
            (self.storage_textures, limits.max_storage_textures_per_shader_stage, "storage textures"),
            (self.samplers, limits.max_samplers_per_shader_stage, "samplers"),
        ];
        for (count, limit, name) in classes {
            if count > limit {
                return Err(format!("{count} {name} in one stage, the limit is {limit}"));
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const VERTEX: &str = "@vertex fn vs(@builtin(vertex_index) i: u32) -> @builtin(position) vec4f { return vec4f(f32(i), 0.0, 0.0, 1.0); }\n";

    #[test]
    fn a_shader_within_the_request_passes() {
        let source = format!("{VERTEX}@group(0) @binding(0) var<uniform> u: vec4f;\n@fragment fn fs() -> @location(0) vec4f {{ return u; }}");
        let module = check("plain", &source).unwrap();
        assert_eq!(module.entry_points.len(), 2);
    }

    #[test]
    fn binding_arrays_fail() {
        // An adapter of texture tier C has no binding arrays: the request of every adapter
        // lacks them.
        let source = format!(
            "enable wgpu_binding_array;\n{VERTEX}@group(0) @binding(0) var textures: binding_array<texture_2d<f32>>;\n@group(0) @binding(1) var samplers: binding_array<sampler>;\n\
             @fragment fn fs(@builtin(position) p: vec4f) -> @location(0) vec4f {{ let i = u32(p.x); return textureSample(textures[i], samplers[i], p.xy); }}"
        );
        let problem = check("arrays", &source).unwrap_err();
        assert!(problem.starts_with("arrays:"), "{problem}");
    }

    #[test]
    fn a_feature_the_request_lacks_fails() {
        // 16-bit floats need SHADER_F16, which the request does not ask for.
        let source = format!("enable f16;\n{VERTEX}@fragment fn fs() -> @location(0) vec4f {{ let h: f16 = 1.0h; return vec4f(f32(h)); }}");
        let problem = check("f16", &source).unwrap_err();
        assert!(problem.starts_with("f16:"), "{problem}");
        // The index of the primitive needs PRIMITIVE_INDEX.
        let source = format!("{VERTEX}@fragment fn fs(@builtin(primitive_index) i: u32) -> @location(0) vec4f {{ return vec4f(f32(i)); }}");
        assert!(check("primitive", &source).is_err());
    }

    #[test]
    fn a_group_at_the_limit_fails() {
        let source = format!("{VERTEX}@group(4) @binding(0) var<uniform> u: vec4f;\n@fragment fn fs() -> @location(0) vec4f {{ return u; }}");
        let problem = check("groups", &source).unwrap_err();
        assert!(problem.contains("the binding group 4 is at or over the limit of 4 groups"), "{problem}");
    }

    #[test]
    fn more_storage_buffers_than_a_stage_allows_fail() {
        let buffers: String = (0..9).map(|i| format!("@group(0) @binding({i}) var<storage, read> b{i}: array<f32>;\n")).collect();
        let sum: Vec<String> = (0..9).map(|i| format!("b{i}[0]")).collect();
        let source = format!("{VERTEX}{buffers}@fragment fn fs() -> @location(0) vec4f {{ return vec4f({}); }}", sum.join(" + "));
        let problem = check("storage", &source).unwrap_err();
        assert!(problem.contains("the entry point fs: 9 storage buffers in one stage, the limit is 8"), "{problem}");
        // The same buffers unused by the fragment stage count for nothing.
        let source = format!("{VERTEX}{buffers}@fragment fn fs() -> @location(0) vec4f {{ return vec4f(b0[0]); }}");
        check("one used", &source).unwrap();
    }
}
