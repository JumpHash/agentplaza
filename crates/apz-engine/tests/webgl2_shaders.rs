//! Every WGSL shader in the engine must validate, must translate to GLSL ES 3.00
//! (WebGL2) through naga, and must fit the WebGL2 limits that wgpu's
//! `downlevel_webgl2_defaults()` describes. The browser fallback cannot be run
//! here, so this is the native proof of the shader side.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use naga::back::glsl;
use naga::valid::{Capabilities, ValidationFlags, Validator};
use naga::{AddressSpace, Block, Expression, Function, ImageClass, Module, Statement, TypeInner};
use wgpu::Limits;

/// Collects every `.wgsl` file under `dir`, recursively.
fn wgsl_files(dir: &Path, out: &mut Vec<PathBuf>) {
    for entry in std::fs::read_dir(dir).expect("read src dir") {
        let path = entry.expect("dir entry").path();
        if path.is_dir() {
            wgsl_files(&path, out);
        } else if path.extension().is_some_and(|e| e == "wgsl") {
            out.push(path);
        }
    }
}

/// Collects the functions called directly from `block`, including inside nested blocks.
fn collect_calls(block: &Block, calls: &mut Vec<naga::Handle<Function>>) {
    for stmt in block.iter() {
        match stmt {
            Statement::Block(b) => collect_calls(b, calls),
            Statement::If { accept, reject, .. } => {
                collect_calls(accept, calls);
                collect_calls(reject, calls);
            }
            Statement::Switch { cases, .. } => {
                for case in cases {
                    collect_calls(&case.body, calls);
                }
            }
            Statement::Loop {
                body, continuing, ..
            } => {
                collect_calls(body, calls);
                collect_calls(continuing, calls);
            }
            Statement::Call { function, .. } => calls.push(*function),
            _ => {}
        }
    }
}

/// Indices of the global variables an entry point can reach, through the functions it calls.
fn used_globals(module: &Module, root: &Function) -> BTreeSet<usize> {
    let mut globals = BTreeSet::new();
    let mut visited = BTreeSet::new();
    let mut pending: Vec<&Function> = vec![root];
    while let Some(func) = pending.pop() {
        for (_, expr) in func.expressions.iter() {
            if let Expression::GlobalVariable(handle) = expr {
                globals.insert(handle.index());
            }
        }
        let mut calls = Vec::new();
        collect_calls(&func.body, &mut calls);
        for callee in calls {
            if visited.insert(callee.index()) {
                pending.push(&module.functions[callee]);
            }
        }
    }
    globals
}

/// Checks the WebGL2 resource limits for each entry point of `module`.
fn check_limits(name: &str, module: &Module, limits: &Limits) {
    for ep in &module.entry_points {
        let used = used_globals(module, &ep.function);
        let label = format!("{name}::{} ({:?})", ep.name, ep.stage);
        let (mut textures, mut samplers, mut uniforms) = (0u32, 0u32, 0u32);
        let mut groups = BTreeSet::new();
        for (handle, var) in module.global_variables.iter() {
            if !used.contains(&handle.index()) {
                continue;
            }
            match var.space {
                AddressSpace::Storage { .. } => {
                    panic!(
                        "{label}: storage buffer `{:?}` is not available on WebGL2",
                        var.name
                    )
                }
                AddressSpace::PushConstant => {
                    panic!("{label}: push constants are not available on WebGL2")
                }
                AddressSpace::Uniform => uniforms += 1,
                _ => {}
            }
            match module.types[var.ty].inner {
                TypeInner::Image {
                    class: ImageClass::Storage { .. },
                    ..
                } => panic!("{label}: storage textures are not available on WebGL2"),
                TypeInner::Image { .. } => textures += 1,
                TypeInner::Sampler { .. } => samplers += 1,
                _ => {}
            }
            if let Some(binding) = var.binding {
                groups.insert(binding.group);
            }
        }
        assert!(
            groups.len() as u32 <= limits.max_bind_groups,
            "{label}: uses {} bind groups, WebGL2 allows {}",
            groups.len(),
            limits.max_bind_groups
        );
        assert!(
            textures <= limits.max_sampled_textures_per_shader_stage,
            "{label}: uses {textures} sampled textures, WebGL2 allows {}",
            limits.max_sampled_textures_per_shader_stage
        );
        assert!(
            samplers <= limits.max_samplers_per_shader_stage,
            "{label}: uses {samplers} samplers, WebGL2 allows {}",
            limits.max_samplers_per_shader_stage
        );
        assert!(
            uniforms <= limits.max_uniform_buffers_per_shader_stage,
            "{label}: uses {uniforms} uniform buffers, WebGL2 allows {}",
            limits.max_uniform_buffers_per_shader_stage
        );
    }
}

#[test]
fn every_engine_shader_fits_webgl2() {
    let src = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    let mut files = Vec::new();
    wgsl_files(&src, &mut files);
    assert!(!files.is_empty(), "no .wgsl files under {}", src.display());
    files.sort();

    let limits = Limits::downlevel_webgl2_defaults();
    for file in &files {
        let name = file
            .strip_prefix(&src)
            .unwrap_or(file)
            .display()
            .to_string();
        let source = std::fs::read_to_string(file).expect("read shader");
        let module = naga::front::wgsl::parse_str(&source).unwrap_or_else(|e| {
            panic!("{name}: WGSL parse failed:\n{}", e.emit_to_string(&source))
        });
        let info = Validator::new(ValidationFlags::all(), Capabilities::default())
            .validate(&module)
            .unwrap_or_else(|e| panic!("{name}: validation failed: {e:?}"));

        check_limits(&name, &module, &limits);

        let options = glsl::Options {
            version: glsl::Version::Embedded {
                version: 300,
                is_webgl: true,
            },
            ..Default::default()
        };
        for ep in &module.entry_points {
            let pipeline = glsl::PipelineOptions {
                shader_stage: ep.stage,
                entry_point: ep.name.clone(),
                multiview: None,
            };
            let mut out = String::new();
            let mut writer = glsl::Writer::new(
                &mut out,
                &module,
                &info,
                &options,
                &pipeline,
                naga::proc::BoundsCheckPolicies::default(),
            )
            .unwrap_or_else(|e| panic!("{name}::{}: GLSL writer setup failed: {e:?}", ep.name));
            writer.write().unwrap_or_else(|e| {
                panic!("{name}::{}: GLSL ES 3.00 write failed: {e:?}", ep.name)
            });
            assert!(
                out.contains("#version 300 es"),
                "{name}::{}: not GLSL ES 3.00",
                ep.name
            );
        }
    }
}
