//! Headless Bevy proof: load the accepted actor GLB and the baked cloth GLB
//! (morph animation), render an offscreen frame, save it as PNG, exit.
//! Also provides `--headless --report --glb <path>` count reporting for consumer acceptance.
use bevy::asset::RenderAssetUsages;
use bevy::mesh::morph::{MorphWeights, MAX_MORPH_WEIGHTS};
use bevy::prelude::*;
use bevy::render::render_resource::{Extent3d, TextureDimension, TextureFormat};
use bevy::render::view::screenshot::{save_to_disk, Screenshot};
use serde::{Deserialize, Serialize};

fn env_f32(key: &str, default: f32) -> f32 {
    std::env::var(key)
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(default)
}

fn env_usize(key: &str, default: usize) -> usize {
    std::env::var(key)
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(default)
}

#[derive(Resource)]
struct Units(usize);

#[derive(Resource)]
struct Bench(bool);

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct PartReport {
    pub name: String,
    pub mesh_index: usize,
    pub primitive_index: usize,
    pub vertices: usize,
    pub triangles: usize,
    pub morph_targets: usize,
    pub joints: usize,
    pub material: bool,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct CountReport {
    pub profile: String,
    pub bevy_version: String,
    pub morph_limit: usize,
    pub morph_limit_verified: bool,
    pub nodes: usize,
    pub meshes: usize,
    pub vertices: usize,
    pub triangles: usize,
    pub materials: usize,
    pub parts: Vec<PartReport>,
}

#[derive(Resource, Clone, Debug)]
struct ViewerConfig {
    report_mode: bool,
    debug_scene: bool,
    asset_file: String,
}

struct CliOptions {
    report: bool,
    headless: bool,
    debug_scene: bool,
    glb_path: Option<String>,
    units: usize,
    bench: bool,
    uncapped: bool,
}

fn parse_args() -> CliOptions {
    let mut args = std::env::args().skip(1);
    let mut options = CliOptions {
        report: std::env::var("SCULPTO_VIEWER_REPORT").is_ok(),
        headless: std::env::var("SCULPTO_VIEWER_HEADLESS").is_ok(),
        debug_scene: std::env::var("SCULPTO_VIEWER_DEBUG_SCENE").is_ok(),
        glb_path: std::env::var("SCULPTO_VIEWER_GLB")
            .ok()
            .or_else(|| std::env::var("SCULPTO_VIEWER_ASSET").ok()),
        units: env_usize("SCULPTO_VIEWER_UNITS", 1),
        bench: std::env::var("SCULPTO_VIEWER_BENCH").is_ok(),
        uncapped: std::env::var("SCULPTO_VIEWER_UNCAPPED").is_ok(),
    };
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--report" => options.report = true,
            "--headless" => options.headless = true,
            "--debug-scene" => options.debug_scene = true,
            "--bench" => options.bench = true,
            "--uncapped" => options.uncapped = true,
            "--glb" => {
                if let Some(val) = args.next() {
                    options.glb_path = Some(val);
                }
            }
            "--asset" => {
                if let Some(val) = args.next() {
                    options.glb_path = Some(val);
                }
            }
            "--units" => {
                if let Some(val) = args.next().and_then(|v| v.parse().ok()) {
                    options.units = val;
                }
            }
            _ => {}
        }
    }
    options
}

fn main() {
    let options = parse_args();
    let (asset_dir, asset_file) = if let Some(ref glb) = options.glb_path {
        let p = std::path::Path::new(glb);
        let abs = if p.is_absolute() {
            p.to_path_buf()
        } else {
            std::env::current_dir()
                .map(|cwd| cwd.join(p))
                .unwrap_or_else(|_| p.to_path_buf())
        };
        let parent = abs.parent().unwrap_or(std::path::Path::new("."));
        let filename = abs
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("model.glb")
            .to_string();
        (parent.to_string_lossy().to_string(), filename)
    } else {
        (
            "assets".to_string(),
            env_or("SCULPTO_VIEWER_ASSET", "combined.glb"),
        )
    };

    // The ordinary proof runs at 60 Hz so captures land on stable ticks. The
    // units benchmark and report modes must not be capped: a 16.7 ms sleep would
    // hide the real playback cost or slow down tests.
    let interval = if options.uncapped || options.report {
        std::time::Duration::ZERO
    } else {
        std::time::Duration::from_secs_f64(1.0 / 60.0)
    };

    let config = ViewerConfig {
        report_mode: options.report,
        debug_scene: options.debug_scene,
        asset_file,
    };
    let asset_plugin = AssetPlugin {
        file_path: asset_dir,
        ..Default::default()
    };

    App::new()
        .add_plugins(bevy::app::ScheduleRunnerPlugin::run_loop(interval))
        .insert_resource(Units(options.units))
        .insert_resource(Bench(options.bench))
        .insert_resource(config)
        .add_plugins(
            DefaultPlugins
                .set(WindowPlugin {
                    primary_window: None,
                    primary_cursor_options: None,
                    exit_condition: bevy::window::ExitCondition::DontExit,
                    close_when_requested: false,
                })
                .set(ImagePlugin::default())
                .set(asset_plugin),
        )
        .register_type::<Transform>()
        .register_type::<GlobalTransform>()
        .register_type::<bevy::transform::components::TransformTreeChanged>()
        .register_type::<bevy::ecs::hierarchy::ChildOf>()
        .register_type::<bevy::ecs::hierarchy::Children>()
        .register_type::<bevy::gltf::GltfExtras>()
        .register_type::<bevy::gltf::GltfSceneExtras>()
        .register_type::<Mesh3d>()
        .register_type::<MeshMaterial3d<StandardMaterial>>()
        .register_type::<bevy::camera::visibility::Visibility>()
        .register_type::<bevy::animation::AnimationPlayer>()
        .register_type::<bevy::animation::AnimationTargetId>()
        .register_type::<bevy::animation::AnimatedBy>()
        .register_type::<bevy::camera::primitives::Aabb>()
        .register_type::<bevy::gltf::GltfMaterialExtras>()
        .register_type::<bevy::gltf::GltfMeshName>()
        .register_type::<bevy::gltf::GltfMaterialName>()
        .register_type::<bevy::gltf::GltfSceneName>()
        .register_type::<bevy::mesh::skinning::SkinnedMesh>()
        .register_type::<bevy::mesh::skinning::SkinnedMeshBounds>()
        .register_type::<bevy::camera::visibility::DynamicSkinnedMeshBounds>()
        .register_type::<bevy::mesh::morph::MeshMorphWeights>()
        .register_type::<bevy::gltf::GltfMeshExtras>()
        .register_type::<bevy::mesh::morph::MorphWeights>()
        .register_type::<Name>()
        .register_type::<Visibility>()
        .register_type::<InheritedVisibility>()
        .register_type::<ViewVisibility>()
        .insert_resource(ClearColor(Color::srgb(0.02, 0.02, 0.03)))
        .insert_resource(GlobalAmbientLight {
            brightness: env_f32("SCULPTO_VIEWER_AMBIENT", 0.0),
            ..default()
        })
        .add_systems(Startup, setup)
        .add_systems(
            Update,
            (
                report_counts,
                advance_animation,
                capture_once,
                benchmark,
                debug_scene,
                debug_playback,
            ),
        )
        .run();
}

#[derive(Resource)]
struct Target(Handle<Image>);

#[derive(Resource)]
struct GltfRoot(Handle<bevy::gltf::Gltf>);

/// Graph (actor bones + cloth morph clips) to attach to the scene's players.
#[derive(Resource)]
struct PlayerGraph {
    handle: Handle<AnimationGraph>,
    nodes: Vec<AnimationNodeIndex>,
}

fn setup(
    mut commands: Commands,
    mut images: ResMut<Assets<Image>>,
    asset_server: Res<AssetServer>,
    mut graphs: ResMut<Assets<AnimationGraph>>,
    units: Res<Units>,
    config: Res<ViewerConfig>,
) {
    let mut image = Image::new(
        Extent3d {
            width: 960,
            height: 900,
            depth_or_array_layers: 1,
        },
        TextureDimension::D2,
        vec![0; 960 * 900 * 4],
        TextureFormat::Rgba8UnormSrgb,
        RenderAssetUsages::default(),
    );
    image.texture_descriptor.usage = bevy::render::render_resource::TextureUsages::RENDER_ATTACHMENT
        | bevy::render::render_resource::TextureUsages::COPY_SRC
        | bevy::render::render_resource::TextureUsages::COPY_DST
        | bevy::render::render_resource::TextureUsages::TEXTURE_BINDING;
    let handle = images.add(image);
    commands.insert_resource(Target(handle.clone()));
    if config.debug_scene {
        let at_cap = MorphWeights::new(vec![0.0; MAX_MORPH_WEIGHTS], None).is_ok();
        let over_cap = MorphWeights::new(vec![0.0; MAX_MORPH_WEIGHTS + 1], None)
            .err()
            .map(|error| error.to_string());
        info!(
            "viewer: morph_max={MAX_MORPH_WEIGHTS} at_cap_ok={at_cap} over_cap_error={over_cap:?}"
        );
    }
    let count = units.0.max(1);
    let columns = (count as f32).sqrt().ceil() as usize;
    let rows = count.div_ceil(columns);
    let spacing = 0.9_f32;
    let (camera_eye, camera_target) = if count > 1 {
        let extent = (columns.max(rows) as f32 - 1.0) * spacing + 1.5;
        (
            Vec3::new(extent * 1.4, 1.4, extent * 1.8),
            Vec3::new(0.0, 0.9, 0.0),
        )
    } else {
        (Vec3::new(2.2, 1.3, 2.8), Vec3::new(0.0, 0.9, 0.0))
    };
    commands.spawn((
        Camera3d::default(),
        bevy::core_pipeline::tonemapping::Tonemapping::None,
        bevy::camera::RenderTarget::Image(handle.into()),
        Transform::from_translation(camera_eye).looking_at(camera_target, Vec3::Y),
    ));
    commands.spawn((
        DirectionalLight {
            illuminance: 10_000.0,
            ..default()
        },
        Transform::from_xyz(3.0, 5.0, 2.0).looking_at(Vec3::ZERO, Vec3::Y),
    ));

    let gltf_handle: Handle<bevy::gltf::Gltf> = asset_server.load(config.asset_file.clone());
    commands.insert_resource(GltfRoot(gltf_handle));

    if !config.report_mode {
        let actor_animation = env_or("SCULPTO_VIEWER_ACTOR_ANIMATION", "4");
        let cloth_animation = env_or("SCULPTO_VIEWER_CLOTH_ANIMATION", "0");
        let actor_clip =
            asset_server.load(format!("{}#Animation{actor_animation}", config.asset_file));
        let cloth_clip =
            asset_server.load(format!("{}#Animation{cloth_animation}", config.asset_file));
        let (mut graph, actor_node) = AnimationGraph::from_clip(actor_clip);
        let cloth_node = graph.add_clip(cloth_clip, 1.0, graph.root);
        commands.insert_resource(PlayerGraph {
            handle: graphs.add(graph),
            nodes: vec![actor_node, cloth_node],
        });
    }

    let scene = asset_server.load(format!("{}#Scene0", config.asset_file));
    for index in 0..count {
        let column = index % columns;
        let row = index / columns;
        let x = (column as f32 - (columns as f32 - 1.0) * 0.5) * spacing;
        let z = (row as f32 - (rows as f32 - 1.0) * 0.5) * spacing;
        commands.spawn((
            bevy::world_serialization::WorldAssetRoot(scene.clone()),
            Transform::from_xyz(x, 0.0, z),
        ));
    }
}

fn env_or(key: &str, default: &str) -> String {
    std::env::var(key).unwrap_or_else(|_| default.to_string())
}

fn report_counts(
    config: Res<ViewerConfig>,
    gltf_root: Option<Res<GltfRoot>>,
    asset_server: Res<AssetServer>,
    gltf_assets: Res<Assets<bevy::gltf::Gltf>>,
    gltf_mesh_assets: Res<Assets<bevy::gltf::GltfMesh>>,
    mesh_assets: Res<Assets<Mesh>>,
    mut frames: Local<u32>,
) {
    if !config.report_mode {
        return;
    }
    *frames += 1;
    let Some(root) = gltf_root else { return };
    let state = asset_server.load_state(&root.0);
    if state.is_failed() {
        eprintln!(
            "REPORT_ERROR: failed to load glTF asset {}: {state:?}",
            config.asset_file
        );
        unsafe {
            libc::_exit(1);
        }
    }
    let Some(gltf) = gltf_assets.get(&root.0) else {
        if *frames > 300 {
            eprintln!(
                "REPORT_ERROR: timed out waiting for glTF asset {} to load (state={state:?})",
                config.asset_file
            );
            unsafe {
                libc::_exit(2);
            }
        }
        return;
    };

    let mut total_vertices = 0;
    let mut total_triangles = 0;
    let mut parts = Vec::new();

    for (mesh_idx, mesh_handle) in gltf.meshes.iter().enumerate() {
        let Some(gltf_mesh) = gltf_mesh_assets.get(mesh_handle) else {
            return;
        };
        for (prim_idx, prim) in gltf_mesh.primitives.iter().enumerate() {
            let Some(mesh) = mesh_assets.get(&prim.mesh) else {
                return;
            };
            let verts = mesh.count_vertices();
            let tris = mesh.indices().map_or(verts / 3, |ind| ind.len() / 3);
            let morphs = mesh.morph_target_names().map_or(0, <[String]>::len);
            total_vertices += verts;
            total_triangles += tris;
            parts.push(PartReport {
                name: if !prim.name.is_empty() {
                    prim.name.clone()
                } else {
                    gltf_mesh.name.clone()
                },
                mesh_index: mesh_idx,
                primitive_index: prim_idx,
                vertices: verts,
                triangles: tris,
                morph_targets: morphs,
                joints: 0,
                material: prim.material.is_some(),
            });
        }
    }

    let at_cap_ok = MorphWeights::new(vec![0.0; MAX_MORPH_WEIGHTS], None).is_ok();
    let over_cap_err = MorphWeights::new(vec![0.0; MAX_MORPH_WEIGHTS + 1], None).is_err();

    let report = CountReport {
        profile: "bevy".to_string(),
        bevy_version: "0.20.1".to_string(),
        morph_limit: MAX_MORPH_WEIGHTS,
        morph_limit_verified: at_cap_ok && over_cap_err,
        nodes: gltf.nodes.len(),
        meshes: gltf.meshes.len(),
        vertices: total_vertices,
        triangles: total_triangles,
        materials: gltf.materials.len(),
        parts,
    };

    use std::io::Write;
    let json = serde_json::to_string_pretty(&report).unwrap();
    println!("{json}");
    let _ = std::io::stdout().flush();
    unsafe {
        // Skip glibc atexit handlers that crash inside NVIDIA driver cleanup.
        libc::_exit(0);
    }
}

fn advance_animation(
    mut commands: Commands,
    config: Res<ViewerConfig>,
    graph: Option<Res<PlayerGraph>>,
    mut players: Query<(Entity, &mut AnimationPlayer), Added<AnimationPlayer>>,
) {
    if config.report_mode {
        return;
    }
    // The glTF scene spawns its players asynchronously; attach our graph and
    // start both clips the moment each player appears.
    let Some(graph) = graph else { return };
    for (entity, mut player) in &mut players {
        for node in &graph.nodes {
            player.play(*node).repeat();
        }
        commands
            .entity(entity)
            .insert(AnimationGraphHandle(graph.handle.clone()));
        info!("viewer: started 1 animation players");
    }
}

/// Camera positions for the review views; the glTF actor faces +Z, so the
/// +Z camera is the front view and (2.2, ., 2.8) the three-quarter front.
fn view_location(name: &str) -> Vec3 {
    match name {
        "front" => Vec3::new(0.0, 1.3, 2.8),
        "side" => Vec3::new(2.8, 1.3, 0.0),
        "back" => Vec3::new(0.0, 1.3, -2.8),
        _ => Vec3::new(2.2, 1.3, 2.8),
    }
}

fn parse_series(spec: &str) -> Option<(u32, u32, u32)> {
    let mut parts = spec.split(':');
    let start = parts.next()?.parse().ok()?;
    let count = parts.next()?.parse().ok()?;
    let stride = parts.next().unwrap_or("1").parse().ok()?;
    (count > 0 && stride > 0).then_some((start, count, stride))
}

#[allow(clippy::too_many_arguments)]
fn capture_once(
    config: Res<ViewerConfig>,
    target: Res<Target>,
    bench: Res<Bench>,
    mut frames: Local<u32>,
    mut captured: Local<bool>,
    mut cameras: Query<&mut Transform, With<Camera3d>>,
    mut commands: Commands,
    mut exit: MessageWriter<AppExit>,
) {
    if bench.0 || config.report_mode || config.debug_scene {
        return;
    }
    *frames += 1;
    if *frames < 240 {
        return;
    }
    // Series mode: one pose every `stride` ticks, one view per following
    // tick, so a review gets 10-20 distinct poses across the clip without
    // rewinding the animation.
    if let Some((start, count, stride)) = parse_series(&env_or("SCULPTO_VIEWER_SERIES", "")) {
        let views: Vec<String> = env_or("SCULPTO_VIEWER_VIEWS", "quarter")
            .split(',')
            .map(str::to_string)
            .collect();
        let dir = env_or("SCULPTO_VIEWER_DIR", "/tmp/sculpto-classify/cloth/series");
        let last = start + (count - 1) * stride;
        if *frames >= start && *frames <= last + stride {
            let pose = (*frames - start) % stride;
            // ticks after the last pose tick belong to its trailing views only
            if pose < views.len() as u32 && *frames - pose <= last {
                let view = &views[pose as usize];
                let location = view_location(view);
                if let Ok(mut transform) = cameras.single_mut() {
                    transform.translation = location;
                    transform.look_at(Vec3::new(0.0, 0.95, 0.0), Vec3::Y);
                }
                let pose_tick = *frames - pose;
                info!(
                    "viewer: series pose {} view {} at tick {}",
                    pose_tick, view, *frames
                );
                commands
                    .spawn(Screenshot::image(target.0.clone()))
                    .observe(save_to_disk(format!("{dir}/pose{pose_tick:04}_{view}.png")));
            }
        }
        if *frames > last + stride + views.len() as u32 + 30 {
            info!("viewer: series done, exiting at {}", *frames);
            exit.write(AppExit::Success);
        }
        return;
    }
    if *captured {
        if *frames > 300 {
            info!("viewer: captured at 240, exiting at {}", *frames);
            exit.write(AppExit::Success);
        }
        return;
    }
    *captured = true;
    info!("viewer: requesting screenshot at frame {}", *frames);
    commands
        .spawn(Screenshot::image(target.0.clone()))
        .observe(save_to_disk(env_or(
            "SCULPTO_VIEWER_PNG",
            "/tmp/sculpto-classify/cloth/bevy_combined_240.png",
        )));
}

/// One-shot scene dump (SCULPTO_VIEWER_DEBUG_SCENE=1): every mesh entity
/// with its glTF name, visibility components, bounding box and world
/// transform — catches assets that load silently but never draw. When
/// `SCULPTO_VIEWER_DEBUG_SCENE` is set it also reports the loader facts the
/// glTF writer must satisfy: vertex count, morph targets, material and skin
/// joints.
#[allow(clippy::type_complexity)]
fn debug_scene(
    config: Res<ViewerConfig>,
    mut done: Local<bool>,
    meshes: Query<
        (
            Option<&Name>,
            Option<&bevy::gltf::GltfMeshName>,
            Option<&Visibility>,
            Option<&InheritedVisibility>,
            Option<&bevy::camera::primitives::Aabb>,
            &GlobalTransform,
            &Mesh3d,
            Option<&MeshMaterial3d<StandardMaterial>>,
            Option<&bevy::mesh::skinning::SkinnedMesh>,
        ),
        With<Mesh3d>,
    >,
    assets: Res<Assets<Mesh>>,
) {
    if *done || config.report_mode || !config.debug_scene {
        return;
    }
    if meshes.is_empty() {
        return; // scene spawns asynchronously; wait for it
    }
    *done = true;
    for (name, gltf_name, vis, inherited, aabb, transform, mesh3d, material, skin) in &meshes {
        let mesh = assets.get(&mesh3d.0);
        let morphs = mesh
            .and_then(Mesh::morph_target_names)
            .map_or(0, <[String]>::len);
        info!(
            "mesh name={:?} gltf={:?} vis={:?} inherited={} verts={} morph_targets={} material={} joints={} aabb={} at {:?}",
            name.map(|n| n.to_string()),
            gltf_name.map(|g| g.0.clone()),
            vis.copied(),
            inherited
                .copied()
                .map(|v| format!("{}", v.get()))
                .unwrap_or_else(|| "ABSENT".into()),
            mesh.map_or(0, Mesh::count_vertices),
            morphs,
            material.is_some(),
            skin.map_or_else(|| "none".into(), |s| s.joints.len().to_string()),
            aabb.map(|a| format!("{:.2}..={:.2}", a.min(), a.max()))
                .unwrap_or_else(|| "NONE".into()),
            transform.translation()
        );
    }
}

/// Two playback snapshots on the debug run: the count of active clips and the
/// elapsed time prove animation is advancing, the summed morph weights prove
/// the cloth clip reaches the targets rather than only the skeleton.
fn debug_playback(
    config: Res<ViewerConfig>,
    mut tick: Local<u32>,
    players: Query<&AnimationPlayer>,
    weights: Query<&MorphWeights>,
    mut exit: MessageWriter<AppExit>,
) {
    if config.report_mode || !config.debug_scene {
        return;
    }
    *tick += 1;
    if players.is_empty() || weights.is_empty() {
        *tick = 249; // the scene is still loading; sample once it is ready
        return;
    }
    if *tick != 250 && *tick != 290 {
        return;
    }
    let active: usize = players
        .iter()
        .map(|player| player.playing_animations().count())
        .sum();
    let elapsed: Vec<f32> = players
        .iter()
        .flat_map(|player| player.playing_animations().map(|(_, a)| a.elapsed()))
        .collect();
    let sums: Vec<f32> = weights.iter().map(|w| w.weights().iter().sum()).collect();
    info!(
        "viewer: playback tick={} players={} active_clips={} elapsed={:?} morph_weight_sums={:?}",
        *tick,
        players.iter().count(),
        active,
        elapsed,
        sums
    );
    if *tick == 290 {
        info!("viewer: debug snapshots done, exiting");
        exit.write(AppExit::Success);
    }
}

/// Units benchmark: N scene instances, a warmup, then p50/p95 frame time and
/// the process RSS. It exits through the same `AppExit` path as the captures.
fn benchmark(
    time: Res<Time>,
    units: Res<Units>,
    bench: Res<Bench>,
    mut frames: Local<u32>,
    mut samples: Local<Vec<f32>>,
    mut exit: MessageWriter<AppExit>,
) {
    if !bench.0 {
        return;
    }
    *frames += 1;
    if *frames > 240 {
        samples.push(time.delta_secs());
    }
    if *frames <= 540 {
        return;
    }
    let mut sorted = samples.clone();
    sorted.sort_by(f32::total_cmp);
    let mean = sorted.iter().sum::<f32>() / sorted.len().max(1) as f32;
    let p50 = sorted.get(sorted.len() / 2).copied().unwrap_or(0.0);
    let p95 = sorted
        .get((sorted.len() as f32 * 0.95) as usize)
        .copied()
        .unwrap_or(0.0);
    let rss = std::fs::read_to_string("/proc/self/status")
        .ok()
        .and_then(|text| {
            text.lines()
                .find(|line| line.starts_with("VmRSS:"))
                .map(|line| line.trim().to_owned())
        })
        .unwrap_or_else(|| "VmRSS: unknown".to_owned());
    info!(
        "BENCH units={} ticks={} mean_ms={:.3} p50_ms={:.3} p95_ms={:.3} {}",
        units.0,
        sorted.len(),
        mean * 1000.0,
        p50 * 1000.0,
        p95 * 1000.0,
        rss
    );
    exit.write(AppExit::Success);
}
