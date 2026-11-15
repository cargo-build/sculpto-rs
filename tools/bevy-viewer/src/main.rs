//! Headless Bevy proof: load the accepted actor GLB and the baked cloth GLB
//! (morph animation), render an offscreen frame, save it as PNG, exit.
//! Also provides `--headless --report --glb <path>` count reporting for consumer acceptance.
use bevy::asset::RenderAssetUsages;
use bevy::input::mouse::{MouseMotion, MouseWheel};
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
    pub joints: usize,
    pub parts: Vec<PartReport>,
}

#[derive(Resource, Clone, Debug)]
struct ViewerConfig {
    report_mode: bool,
    debug_scene: bool,
    interactive: bool,
    /// Seed for the review lighting: flat hides the light rig and renders
    /// unlit materials. `L` flips it at runtime in the interactive viewer.
    flat: bool,
    asset_file: String,
}

/// Runtime lit/flat state; `SCULPTO_VIEWER_FLAT` seeds it and `L` flips it.
#[derive(Resource, Clone, Copy)]
struct FlatMode(bool);

/// Marks the review lights so flat mode can hide the rig without touching it.
#[derive(Component)]
struct ReviewLight;

/// Fill and rim lights: visible only in the optional three-point rig, so the
/// default is one key light and its honest runtime shadow.
#[derive(Component)]
struct FillLight;

/// The key light is the only default source; it casts the shadow.
#[derive(Component)]
struct KeyLight;

/// Review ground: it exists only to receive the key light's shadow in the
/// interactive window, which is how a turntable turn proves the light is
/// runtime rather than baked onto the model.
#[derive(Component)]
struct ReviewGround;

/// Review rig: one key light by default; `3` adds the fill/rim pair.
#[derive(Resource, Clone, Copy)]
struct RigMode {
    three_point: bool,
}

/// Materials flattened by the flat mode, so switching back restores them.
#[derive(Resource, Default)]
struct FlatFlipped(Vec<AssetId<StandardMaterial>>);

/// Marks a spawned review model so the turntable can rotate it while the key
/// light stays fixed in world space — that is what makes the shading and the
/// cast shadows sweep across the figure as it turns.
#[derive(Component)]
struct ReviewModel;

/// Key-light orbit: Shift+LMB moves the light around the figure while the
/// character stays put, so the shading and cast shadow follow the light.
#[derive(Resource, Clone, Copy)]
struct LightOrbit {
    azimuth: f32,
    elevation: f32,
    radius: f32,
}

impl LightOrbit {
    fn from_position(position: Vec3) -> Self {
        let radius = position.length().max(0.2);
        Self {
            azimuth: position.x.atan2(position.z),
            elevation: (position.y / radius).asin(),
            radius,
        }
    }

    fn position(self) -> Vec3 {
        let horizontal = self.radius * self.elevation.cos();
        Vec3::new(
            horizontal * self.azimuth.sin(),
            self.radius * self.elevation.sin(),
            horizontal * self.azimuth.cos(),
        )
    }
}

/// Character pose under the fixed light: Ctrl+LMB rotates the figure, Ctrl+RMB
/// moves it sideways; the default mouse keeps the camera controls.
#[derive(Resource, Clone, Copy)]
struct CharacterPose {
    yaw: f32,
    pitch: f32,
    offset: Vec3,
}

/// The model's spawn translation, so a character offset is relative to home.
#[derive(Component, Clone, Copy)]
struct ModelHome(Vec3);

/// The top-left controls card; `H` hides or shows it.
#[derive(Component)]
struct HelpOverlay;

/// Selected actor clip: `0` idle, `1` walk. Cloth keeps its own clip.
#[derive(Resource, Clone, Copy)]
struct ClipSelection(usize);

struct CliOptions {
    report: bool,
    headless: bool,
    debug_scene: bool,
    interactive: bool,
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
        interactive: std::env::var("SCULPTO_VIEWER_INTERACTIVE").is_ok(),
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
            "--interactive" => options.interactive = true,
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
        interactive: options.interactive,
        flat: std::env::var("SCULPTO_VIEWER_FLAT").is_ok(),
        asset_file,
    };
    let asset_plugin = AssetPlugin {
        file_path: asset_dir,
        ..Default::default()
    };

    let mut app = App::new();
    if options.interactive {
        app.add_plugins(
            DefaultPlugins
                .set(WindowPlugin {
                    primary_window: Some(Window {
                        title:
                            "Sculpto cloth review — drag orbit (Ctrl+drag figure), Shift+drag light, RMB pan, wheel zoom, R reset, L lit/flat, 3 rig, H help"
                                .to_string(),
                        resolution: (1280, 900).into(),
                        ..default()
                    }),
                    ..default()
                })
                .set(ImagePlugin::default())
                .set(asset_plugin),
        );
    } else {
        app.add_plugins(bevy::app::ScheduleRunnerPlugin::run_loop(interval))
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
            );
    }

    app.insert_resource(FlatFlipped::default())
        .insert_resource(ClipSelection(
            std::env::var("SCULPTO_VIEWER_ACTOR_ANIMATION")
                .ok()
                .and_then(|value| value.parse().ok())
                .unwrap_or(0),
        ))
        .insert_resource(LightOrbit::from_position(Vec3::new(3.0, 5.0, 2.0)))
        .insert_resource(CharacterPose {
            yaw: env_f32("SCULPTO_VIEWER_TURNTABLE_YAW", 0.0),
            pitch: 0.0,
            offset: Vec3::ZERO,
        })
        .insert_resource(FlatMode(config.flat))
        .insert_resource(RigMode {
            three_point: std::env::var("SCULPTO_VIEWER_THREE_POINT").is_ok(),
        })
        .insert_resource(Units(options.units))
        .insert_resource(Bench(options.bench))
        .insert_resource(config)
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
            brightness: env_f32(
                "SCULPTO_VIEWER_AMBIENT",
                if options.interactive { 60.0 } else { 0.0 },
            ),
            ..default()
        })
        .add_systems(Startup, setup)
        .add_systems(
            Update,
            (
                report_counts,
                advance_animation,
                select_clip,
                capture_once,
                benchmark,
                debug_scene,
                debug_playback,
                fit_camera_to_scene,
                orbit_controls,
                light_controls,
                toggle_flat,
                apply_flat_mode,
                toggle_rig,
                apply_review_visibility,
                character_controls,
                apply_character_pose,
                toggle_help,
            ),
        )
        .run();
}

/// Ctrl+LMB turns the figure and Ctrl+RMB moves it sideways; the default mouse
/// keeps the camera controls. The light stays world-fixed, so a character move
/// changes the shading and the cast shadow.
fn character_controls(
    config: Res<ViewerConfig>,
    buttons: Res<ButtonInput<MouseButton>>,
    keys: Res<ButtonInput<KeyCode>>,
    mut motion: MessageReader<MouseMotion>,
    pose: Option<ResMut<CharacterPose>>,
    cameras: Query<&Transform, With<Camera3d>>,
) {
    if !config.interactive {
        return;
    }
    let Some(mut pose) = pose else {
        return;
    };
    let control = keys.any_pressed([KeyCode::ControlLeft, KeyCode::ControlRight]);
    if !control {
        return;
    }
    let mut delta = Vec2::ZERO;
    for event in motion.read() {
        delta += event.delta;
    }
    if buttons.pressed(MouseButton::Left) {
        pose.yaw -= delta.x * 0.005;
        pose.pitch = (pose.pitch - delta.y * 0.005).clamp(-1.2, 1.2);
    }
    if buttons.pressed(MouseButton::Right) {
        let Ok(camera) = cameras.single() else {
            return;
        };
        let right = camera.rotation * Vec3::X;
        let up = camera.rotation * Vec3::Y;
        pose.offset += right * (delta.x * 0.002) - up * (delta.y * 0.002);
    }
}

/// Applies the character pose every frame. The model's home is its spawn
/// translation, so an offset moves it relative to where the scene put it.
fn apply_character_pose(
    pose: Res<CharacterPose>,
    mut models: Query<(&mut Transform, &ModelHome), With<ReviewModel>>,
) {
    let rotation = Quat::from_euler(bevy::math::EulerRot::YXZ, pose.yaw, pose.pitch, 0.0);
    for (mut transform, home) in &mut models {
        transform.rotation = rotation;
        transform.translation = home.0 + pose.offset;
    }
}

/// `L` flips lit/flat in the interactive viewer; the capture path keeps its
/// seeded state.
fn toggle_flat(
    config: Res<ViewerConfig>,
    keys: Res<ButtonInput<KeyCode>>,
    mut flat: ResMut<FlatMode>,
) {
    if config.interactive && keys.just_pressed(KeyCode::KeyL) {
        flat.0 = !flat.0;
    }
}

/// `3` adds the fill/rim pair to the single key light; the default rig stays
/// one source so the runtime shading and shadow are not filled away.
fn toggle_rig(
    config: Res<ViewerConfig>,
    keys: Res<ButtonInput<KeyCode>>,
    mut rig: ResMut<RigMode>,
) {
    if config.interactive && keys.just_pressed(KeyCode::Digit3) {
        rig.three_point = !rig.three_point;
    }
}

/// One visibility rule for the review scene: flat hides everything, the
/// default rig shows only the key, and `3` shows the fill/rim pair as well.
/// The ground is interactive-only and hides in flat so the silhouette stays
/// against the black backdrop.
fn apply_review_visibility(
    flat: Res<FlatMode>,
    rig: Res<RigMode>,
    mut lights: Query<
        (
            &mut Visibility,
            Option<&KeyLight>,
            Option<&FillLight>,
            Option<&ReviewGround>,
        ),
        Or<(With<ReviewLight>, With<ReviewGround>)>,
    >,
) {
    for (mut visibility, key, fill, ground) in &mut lights {
        let wanted = if flat.0 {
            Visibility::Hidden
        } else if ground.is_some() {
            Visibility::Inherited
        } else if key.is_some() {
            Visibility::Inherited
        } else if fill.is_some() && !rig.three_point {
            Visibility::Hidden
        } else {
            Visibility::Inherited
        };
        if *visibility != wanted {
            *visibility = wanted;
        }
    }
}

/// Flat review renders unlit materials: silhouette, seams and layer colours,
/// with nothing that reads as a defect. Runs every frame because glTF
/// materials stream in after startup.
fn apply_flat_mode(
    flat: Res<FlatMode>,
    mut materials: ResMut<Assets<StandardMaterial>>,
    mut flipped: ResMut<FlatFlipped>,
) {
    if flat.0 {
        // Remember which materials we flattened so `L` can restore the lit
        // look; leaving them unlit made the second press look like a no-op.
        for (id, material) in materials.iter_mut() {
            if !material.unlit {
                material.unlit = true;
                flipped.0.push(id);
            }
        }
    } else {
        for id in flipped.0.drain(..) {
            if let Some(mut material) = materials.get_mut(id) {
                material.unlit = false;
            }
        }
    }
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
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
    asset_server: Res<AssetServer>,
    mut graphs: ResMut<Assets<AnimationGraph>>,
    units: Res<Units>,
    config: Res<ViewerConfig>,
) {
    let mut target_handle = None;
    if !config.interactive {
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
        image.texture_descriptor.usage =
            bevy::render::render_resource::TextureUsages::RENDER_ATTACHMENT
                | bevy::render::render_resource::TextureUsages::COPY_SRC
                | bevy::render::render_resource::TextureUsages::COPY_DST
                | bevy::render::render_resource::TextureUsages::TEXTURE_BINDING;
        let handle = images.add(image);
        commands.insert_resource(Target(handle.clone()));
        target_handle = Some(handle);
    }
    if config.debug_scene {
        let at_cap = MorphWeights::new(vec![0.0; MAX_MORPH_WEIGHTS], None).is_ok();
        let over_cap = MorphWeights::new(vec![0.0; MAX_MORPH_WEIGHTS + 1], None)
            .err()
            .map(|error| error.to_string());
        info!(
            "viewer: morph_max={MAX_MORPH_WEIGHTS} at_cap_ok={at_cap} over_cap_error={over_cap:?}"
        );
    }
    // Lab fixtures are authored in BodySlide units (71.4286 per metre), so the
    // fixed review camera sits inside them. Scaling the loaded scene keeps the
    // camera, lights and orbit speeds in metres for any source unit.
    let scene_scale = env_f32("SCULPTO_VIEWER_SCALE", 1.0);
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
    let mut camera = commands.spawn((
        Camera3d::default(),
        bevy::core_pipeline::tonemapping::Tonemapping::None,
        Transform::from_translation(camera_eye).looking_at(camera_target, Vec3::Y),
    ));
    if let Some(handle) = target_handle {
        camera.insert(bevy::camera::RenderTarget::Image(handle.into()));
    } else {
        commands.insert_resource(OrbitCamera::looking_at(camera_eye, camera_target));
    }
    // One key light by default: the shading and the cast shadow are computed
    // at runtime, so turning the figure under it changes both. `3` adds the
    // fill/rim pair when the far side is too dark to read.
    let rig = [
        (10_000.0_f32, Vec3::new(3.0, 5.0, 2.0), true),
        (3_500.0, Vec3::new(-3.0, 2.0, -2.5), false),
        (6_000.0, Vec3::new(0.0, 3.0, -4.0), false),
    ];
    for (index, (illuminance, position, key)) in rig.into_iter().enumerate() {
        let mut entity = commands.spawn((
            DirectionalLight {
                illuminance,
                shadow_maps_enabled: key && std::env::var("SCULPTO_VIEWER_NO_SHADOWS").is_err(),
                ..default()
            },
            Transform::from_translation(position).looking_at(Vec3::ZERO, Vec3::Y),
            ReviewLight,
        ));
        match index {
            0 => entity.insert(KeyLight),
            1 | 2 => entity.insert(FillLight),
            _ => unreachable!(),
        };
    }
    if config.interactive || std::env::var("SCULPTO_VIEWER_GROUND").is_ok() {
        // A matte ground receives the key's shadow. It is off in the archived
        // headless captures unless `SCULPTO_VIEWER_GROUND` asks for the shadow
        // proof, so the contact-sheet backdrop stays black.
        commands.spawn((
            Mesh3d(meshes.add(Cuboid::new(4.0, 0.02, 4.0))),
            MeshMaterial3d(materials.add(StandardMaterial {
                base_color: Color::srgb(0.06, 0.06, 0.07),
                perceptual_roughness: 0.95,
                ..default()
            })),
            Transform::from_xyz(0.0, -0.01, 0.0),
            ReviewGround,
        ));
    }

    let gltf_handle: Handle<bevy::gltf::Gltf> = asset_server.load(config.asset_file.clone());
    commands.insert_resource(GltfRoot(gltf_handle));

    if !config.report_mode {
        let actor_idle = asset_server.load(format!("{}#Animation0", config.asset_file));
        let actor_walk = asset_server.load(format!("{}#Animation1", config.asset_file));
        let (mut graph, actor_idle_node) = AnimationGraph::from_clip(actor_idle);
        let actor_walk_node = graph.add_clip(actor_walk, 1.0, graph.root);
        // A separate cloth clip exists only when the file carries one and the
        // caller names it. Defaulting it to `Animation0` played the actor's own
        // idle clip a second time at full weight, blending two clips on the same
        // skeleton — the figure trembled and the walk was diluted by half.
        let mut nodes = vec![actor_idle_node, actor_walk_node];
        if let Ok(index) = std::env::var("SCULPTO_VIEWER_CLOTH_ANIMATION") {
            let cloth_clip = asset_server.load(format!("{}#Animation{index}", config.asset_file));
            nodes.push(graph.add_clip(cloth_clip, 1.0, graph.root));
        }
        commands.insert_resource(PlayerGraph {
            handle: graphs.add(graph),
            nodes,
        });
    }

    let scene = asset_server.load(format!("{}#Scene0", config.asset_file));
    for index in 0..count {
        let column = index % columns;
        let row = index / columns;
        let x = (column as f32 - (columns as f32 - 1.0) * 0.5) * spacing;
        let z = (row as f32 - (rows as f32 - 1.0) * 0.5) * spacing;
        let home = Vec3::new(x, 0.0, z);
        commands.spawn((
            bevy::world_serialization::WorldAssetRoot(scene.clone()),
            Transform::from_translation(home).with_scale(Vec3::splat(scene_scale)),
            ModelHome(home),
            ReviewModel,
        ));
    }
    if config.interactive {
        commands
            .spawn((
                Node {
                    position_type: PositionType::Absolute,
                    left: Val::Px(12.0),
                    top: Val::Px(12.0),
                    padding: UiRect::all(Val::Px(10.0)),
                    flex_direction: FlexDirection::Column,
                    row_gap: Val::Px(2.0),
                    ..default()
                },
                BackgroundColor(Color::srgba(0.0, 0.0, 0.0, 0.55)),
                HelpOverlay,
            ))
            .with_children(|card| {
                for line in [
                    "Drag LMB orbit · RMB pan · wheel zoom",
                    "Ctrl+LMB turn figure · Ctrl+RMB move sideways",
                    "Shift+LMB move key light",
                    "0 idle · 1 walk",
                    "R reset · L lit/flat · 3 three-point rig · H hide help",
                ] {
                    card.spawn((
                        Text::new(line),
                        TextFont {
                            font_size: bevy::text::FontSize::Px(13.0),
                            ..default()
                        },
                        TextColor(Color::WHITE),
                    ));
                }
            });
    }
}

/// `H` hides or shows the controls card. Agent-opened windows start unfocused,
/// so the first key press needs one click on the figure.
fn toggle_help(
    config: Res<ViewerConfig>,
    keys: Res<ButtonInput<KeyCode>>,
    mut card: Query<&mut Visibility, With<HelpOverlay>>,
) {
    if !config.interactive || !keys.just_pressed(KeyCode::KeyH) {
        return;
    }
    for mut visibility in &mut card {
        *visibility = match *visibility {
            Visibility::Hidden => Visibility::Inherited,
            _ => Visibility::Hidden,
        };
    }
}

fn env_or(key: &str, default: &str) -> String {
    std::env::var(key).unwrap_or_else(|_| default.to_string())
}

/// Orbit state for the interactive review window.
///
/// `looking_at` seeds the state from the same eye/target the headless camera
/// uses, so the first interactive frame matches the captured review view.
#[derive(Resource)]
struct OrbitCamera {
    target: Vec3,
    yaw: f32,
    pitch: f32,
    distance: f32,
    min_distance: f32,
    max_distance: f32,
    home: (Vec3, f32, f32, f32),
}

impl OrbitCamera {
    fn looking_at(eye: Vec3, target: Vec3) -> Self {
        let offset = eye - target;
        let distance = offset.length().max(0.2);
        let yaw = offset.x.atan2(offset.z);
        let pitch = (offset.y / distance).asin();
        Self {
            target,
            yaw,
            pitch,
            distance,
            min_distance: 0.2,
            max_distance: 30.0,
            home: (target, yaw, pitch, distance),
        }
    }

    /// Frame the measured scene once, keeping the review direction but widening
    /// the zoom range with the asset. A 100-unit Fox must not inherit the
    /// human-scale 30-unit cap.
    fn fit(&mut self, target: Vec3, distance: f32) {
        self.target = target;
        self.distance = distance;
        self.min_distance = (distance * 0.05).max(0.01);
        self.max_distance = (distance * 20.0).max(30.0);
        self.home = (target, self.yaw, self.pitch, distance);
    }

    fn reset(&mut self) {
        let (target, yaw, pitch, distance) = self.home;
        self.target = target;
        self.yaw = yaw;
        self.pitch = pitch;
        self.distance = distance;
    }
}

/// Frame the first loaded scene from its world bounds so a 1.8 m character and
/// a 100-unit Fox both open at a readable distance. The model itself is never
/// rescaled; `SCULPTO_VIEWER_SCALE` still controls source-unit conversion.
/// Interactive only, so headless captures keep their fixed review camera.
fn fit_camera_to_scene(
    config: Res<ViewerConfig>,
    mut fitted: Local<bool>,
    orbit: Option<ResMut<OrbitCamera>>,
    meshes: Query<
        (&bevy::camera::primitives::Aabb, &GlobalTransform),
        (With<Mesh3d>, Without<ReviewGround>),
    >,
    mut cameras: Query<&mut Transform, With<Camera3d>>,
    mut grounds: Query<&mut Transform, (With<ReviewGround>, Without<Camera3d>)>,
) {
    if *fitted || !config.interactive || env_f32("SCULPTO_VIEWER_AUTOFIT", 1.0) == 0.0 {
        return;
    }
    let Some(mut orbit) = orbit else {
        return;
    };
    let mut min = Vec3::splat(f32::INFINITY);
    let mut max = Vec3::splat(f32::NEG_INFINITY);
    for (aabb, transform) in &meshes {
        let center = aabb.center();
        let half = aabb.half_extents();
        for x in [-1.0_f32, 1.0] {
            for y in [-1.0_f32, 1.0] {
                for z in [-1.0_f32, 1.0] {
                    let local = center + Vec3::new(x * half.x, y * half.y, z * half.z);
                    let world = transform.transform_point(local);
                    min = min.min(world);
                    max = max.max(world);
                }
            }
        }
    }
    if !min.is_finite() || !max.is_finite() {
        return;
    }
    let radius = (max - min).length() * 0.5;
    if radius < 1e-6 {
        return;
    }
    let target = (min + max) * 0.5;
    let distance = radius / (std::f32::consts::FRAC_PI_8).tan() * 1.15;
    orbit.fit(target, distance);
    if let Ok(mut camera) = cameras.single_mut() {
        let rotation = Quat::from_euler(bevy::math::EulerRot::YXZ, orbit.yaw, orbit.pitch, 0.0);
        camera.translation = target + rotation * Vec3::Z * distance;
        camera.look_at(target, Vec3::Y);
    }
    let width = (max.x - min.x).max(max.z - min.z);
    for mut ground in &mut grounds {
        ground.translation = Vec3::new(target.x, min.y - 0.01, target.z);
        ground.scale = Vec3::new(width / 4.0, 1.0, width / 4.0);
    }
    *fitted = true;
    info!("viewer: auto-fit target={target:?} radius={radius:.3} distance={distance:.3}");
}

/// Interactive-only orbit camera: LMB drag orbits, RMB drag pans, wheel zooms,
/// R resets. It never runs in the headless capture path.
fn orbit_controls(
    config: Res<ViewerConfig>,
    orbit: Option<ResMut<OrbitCamera>>,
    buttons: Res<ButtonInput<MouseButton>>,
    keys: Res<ButtonInput<KeyCode>>,
    mut motion: MessageReader<MouseMotion>,
    mut wheel: MessageReader<MouseWheel>,
    mut cameras: Query<&mut Transform, With<Camera3d>>,
) {
    if !config.interactive {
        return;
    }
    let Some(mut orbit) = orbit else {
        return;
    };
    let mut delta = Vec2::ZERO;
    for event in motion.read() {
        delta += event.delta;
    }
    let mut zoom = 0.0;
    for event in wheel.read() {
        zoom += match event.unit {
            bevy::input::mouse::MouseScrollUnit::Line => event.y,
            bevy::input::mouse::MouseScrollUnit::Pixel => event.y / 50.0,
        };
    }

    if keys.just_pressed(KeyCode::KeyR) {
        orbit.reset();
    }
    let moving_light = keys.any_pressed([KeyCode::ShiftLeft, KeyCode::ShiftRight]);
    let moving_character = keys.any_pressed([KeyCode::ControlLeft, KeyCode::ControlRight]);
    if buttons.pressed(MouseButton::Left) && !moving_light && !moving_character {
        orbit.yaw -= delta.x * 0.005;
        orbit.pitch = (orbit.pitch - delta.y * 0.005).clamp(-1.45, 1.45);
    }
    if buttons.pressed(MouseButton::Right) && !moving_character {
        let rotation = Quat::from_euler(bevy::math::EulerRot::YXZ, orbit.yaw, orbit.pitch, 0.0);
        let right = rotation * Vec3::X;
        let up = rotation * Vec3::Y;
        let pan_scale = orbit.distance * 0.0015;
        orbit.target += (-right * delta.x + up * delta.y) * pan_scale;
    }
    if zoom.abs() > f32::EPSILON {
        orbit.distance =
            (orbit.distance * (1.0 - zoom * 0.12)).clamp(orbit.min_distance, orbit.max_distance);
    }

    let Ok(mut transform) = cameras.single_mut() else {
        return;
    };
    let rotation = Quat::from_euler(bevy::math::EulerRot::YXZ, orbit.yaw, orbit.pitch, 0.0);
    transform.translation = orbit.target + rotation * Vec3::Z * orbit.distance;
    transform.look_at(orbit.target, Vec3::Y);
}

/// Shift+LMB moves the key light around the figure; the character stays put so
/// a shadow comparison is a light change, not a pose change.
fn light_controls(
    config: Res<ViewerConfig>,
    buttons: Res<ButtonInput<MouseButton>>,
    keys: Res<ButtonInput<KeyCode>>,
    mut motion: MessageReader<MouseMotion>,
    orbit: Option<ResMut<LightOrbit>>,
    mut lights: Query<&mut Transform, With<KeyLight>>,
) {
    if !config.interactive {
        return;
    }
    let Some(mut orbit) = orbit else {
        return;
    };
    let mut delta = Vec2::ZERO;
    for event in motion.read() {
        delta += event.delta;
    }
    if keys.any_pressed([KeyCode::ShiftLeft, KeyCode::ShiftRight])
        && buttons.pressed(MouseButton::Left)
    {
        orbit.azimuth -= delta.x * 0.005;
        orbit.elevation = (orbit.elevation - delta.y * 0.005).clamp(-0.2, 1.45);
    }
    let Ok(mut transform) = lights.single_mut() else {
        return;
    };
    transform.translation = orbit.position();
    transform.look_at(Vec3::ZERO, Vec3::Y);
}

#[allow(clippy::too_many_arguments)]
fn report_counts(
    config: Res<ViewerConfig>,
    gltf_root: Option<Res<GltfRoot>>,
    asset_server: Res<AssetServer>,
    gltf_assets: Res<Assets<bevy::gltf::Gltf>>,
    gltf_mesh_assets: Res<Assets<bevy::gltf::GltfMesh>>,
    mesh_assets: Res<Assets<Mesh>>,
    gltf_skin_assets: Res<Assets<bevy::gltf::GltfSkin>>,
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
    let total_joints: usize = gltf
        .skins
        .iter()
        .filter_map(|handle| gltf_skin_assets.get(handle))
        .map(|skin| skin.joints.len())
        .sum();

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
                joints: total_joints,
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
        joints: total_joints,
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
    selection: Res<ClipSelection>,
    mut players: Query<(Entity, &mut AnimationPlayer), Added<AnimationPlayer>>,
) {
    if config.report_mode {
        return;
    }
    // The glTF scene spawns its players asynchronously; attach our graph and
    // start both clips the moment each player appears.
    let Some(graph) = graph else { return };
    let selection = selection.0.min(1);
    for (entity, mut player) in &mut players {
        player.play(graph.nodes[selection]).repeat();
        if let Some(cloth) = graph.nodes.get(2) {
            player.play(*cloth).repeat();
        }
        commands
            .entity(entity)
            .insert(AnimationGraphHandle(graph.handle.clone()));
        info!("viewer: started 1 animation players");
    }
}

/// `0` idle, `1` walk: stop the player and restart the chosen actor clip while
/// the cloth clip keeps running.
fn select_clip(
    config: Res<ViewerConfig>,
    keys: Res<ButtonInput<KeyCode>>,
    graph: Option<Res<PlayerGraph>>,
    mut selection: ResMut<ClipSelection>,
    mut players: Query<&mut AnimationPlayer>,
) {
    if !config.interactive {
        return;
    }
    let wanted = if keys.just_pressed(KeyCode::Digit0) {
        Some(0)
    } else if keys.just_pressed(KeyCode::Digit1) {
        Some(1)
    } else {
        None
    };
    let Some(wanted) = wanted else {
        return;
    };
    if selection.0 == wanted {
        return;
    }
    selection.0 = wanted;
    let Some(graph) = graph else { return };
    for mut player in &mut players {
        player.stop_all();
        player.play(graph.nodes[wanted]).repeat();
        if let Some(cloth) = graph.nodes.get(2) {
            player.play(*cloth).repeat();
        }
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
    target: Option<Res<Target>>,
    bench: Res<Bench>,
    mut frames: Local<u32>,
    mut captured: Local<bool>,
    mut cameras: Query<&mut Transform, With<Camera3d>>,
    mut commands: Commands,
    mut exit: MessageWriter<AppExit>,
) {
    if bench.0 || config.report_mode || config.debug_scene || config.interactive {
        return;
    }
    let Some(target) = target else {
        return;
    };
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

#[cfg(test)]
mod tests {
    use super::*;

    fn viewer_config(interactive: bool, flat: bool) -> ViewerConfig {
        ViewerConfig {
            report_mode: false,
            debug_scene: false,
            interactive,
            flat,
            asset_file: String::new(),
        }
    }

    fn press(app: &mut App, key: KeyCode) {
        let mut input = app.world_mut().resource_mut::<ButtonInput<KeyCode>>();
        input.clear();
        input.release_all();
        input.press(key);
    }

    #[test]
    fn l_toggles_flat_and_back() {
        let mut app = App::new();
        app.insert_resource(viewer_config(true, false))
            .init_resource::<ButtonInput<KeyCode>>()
            .insert_resource(FlatMode(false))
            .add_systems(Update, toggle_flat);

        press(&mut app, KeyCode::KeyL);
        app.update();
        assert!(app.world().resource::<FlatMode>().0, "first L enters flat");

        press(&mut app, KeyCode::KeyL);
        app.update();
        assert!(
            !app.world().resource::<FlatMode>().0,
            "second L returns to lit"
        );
    }

    #[test]
    fn l_is_ignored_when_not_interactive() {
        let mut app = App::new();
        app.insert_resource(viewer_config(false, false))
            .init_resource::<ButtonInput<KeyCode>>()
            .insert_resource(FlatMode(false))
            .add_systems(Update, toggle_flat);

        press(&mut app, KeyCode::KeyL);
        app.update();
        assert!(!app.world().resource::<FlatMode>().0);
    }

    #[test]
    fn flat_restores_every_material_it_flattened() {
        let mut app = App::new();
        app.insert_resource(viewer_config(true, false))
            .insert_resource(FlatMode(false))
            .insert_resource(Assets::<StandardMaterial>::default())
            .init_resource::<FlatFlipped>()
            .add_systems(Update, apply_flat_mode);
        let id = app
            .world_mut()
            .resource_mut::<Assets<StandardMaterial>>()
            .add(StandardMaterial::default())
            .id();

        app.world_mut().resource_mut::<FlatMode>().0 = true;
        app.update();
        app.update();
        assert!(
            app.world()
                .resource::<Assets<StandardMaterial>>()
                .get(id)
                .expect("material")
                .unlit,
            "flat renders unlit"
        );
        assert_eq!(
            app.world().resource::<FlatFlipped>().0.len(),
            1,
            "a material is remembered once, not once per frame"
        );

        app.world_mut().resource_mut::<FlatMode>().0 = false;
        app.update();
        assert!(
            !app.world()
                .resource::<Assets<StandardMaterial>>()
                .get(id)
                .expect("material")
                .unlit,
            "lit restores what flat flattened"
        );
        assert!(app.world().resource::<FlatFlipped>().0.is_empty());
    }

    #[test]
    fn default_rig_shows_the_key_only() {
        let mut app = App::new();
        app.insert_resource(viewer_config(true, false))
            .insert_resource(FlatMode(false))
            .insert_resource(RigMode { three_point: false })
            .add_systems(Update, apply_review_visibility);
        let key = app
            .world_mut()
            .spawn((Visibility::Inherited, ReviewLight, KeyLight))
            .id();
        let fill = app
            .world_mut()
            .spawn((Visibility::Inherited, ReviewLight, FillLight))
            .id();

        app.update();
        assert_eq!(
            *app.world().get::<Visibility>(key).expect("key"),
            Visibility::Inherited
        );
        assert_eq!(
            *app.world().get::<Visibility>(fill).expect("fill"),
            Visibility::Hidden,
            "one source by default: the fill would flatten the shadow"
        );
    }

    #[test]
    fn three_point_shows_fill_and_flat_hides_everything() {
        let mut app = App::new();
        app.insert_resource(viewer_config(true, false))
            .insert_resource(FlatMode(false))
            .insert_resource(RigMode { three_point: true })
            .add_systems(Update, apply_review_visibility);
        let fill = app
            .world_mut()
            .spawn((Visibility::Inherited, ReviewLight, FillLight))
            .id();
        let ground = app
            .world_mut()
            .spawn((Visibility::Inherited, ReviewGround))
            .id();

        app.update();
        assert_eq!(
            *app.world().get::<Visibility>(fill).expect("fill"),
            Visibility::Inherited
        );

        app.world_mut().resource_mut::<FlatMode>().0 = true;
        app.update();
        assert_eq!(
            *app.world().get::<Visibility>(fill).expect("fill"),
            Visibility::Hidden
        );
        assert_eq!(
            *app.world().get::<Visibility>(ground).expect("ground"),
            Visibility::Hidden,
            "flat keeps the black backdrop for the silhouette"
        );
    }

    #[test]
    fn key_three_toggles_the_rig() {
        let mut app = App::new();
        app.insert_resource(viewer_config(true, false))
            .init_resource::<ButtonInput<KeyCode>>()
            .insert_resource(RigMode { three_point: false })
            .add_systems(Update, toggle_rig);

        press(&mut app, KeyCode::Digit3);
        app.update();
        assert!(app.world().resource::<RigMode>().three_point);
    }

    #[test]
    fn h_hides_and_shows_the_help_card() {
        let mut app = App::new();
        app.insert_resource(viewer_config(true, false))
            .init_resource::<ButtonInput<KeyCode>>()
            .add_systems(Update, toggle_help);
        let card = app
            .world_mut()
            .spawn((Visibility::Inherited, HelpOverlay))
            .id();

        press(&mut app, KeyCode::KeyH);
        app.update();
        assert_eq!(
            *app.world().get::<Visibility>(card).expect("card"),
            Visibility::Hidden
        );

        press(&mut app, KeyCode::KeyH);
        app.update();
        assert_eq!(
            *app.world().get::<Visibility>(card).expect("card"),
            Visibility::Inherited
        );
    }

    #[test]
    fn character_pose_rotates_and_offsets_from_home() {
        let mut app = App::new();
        app.insert_resource(CharacterPose {
            yaw: 0.75,
            pitch: 0.0,
            offset: Vec3::new(0.1, 0.2, 0.0),
        })
        .add_systems(Update, apply_character_pose);
        let model = app
            .world_mut()
            .spawn((
                Transform::default(),
                ModelHome(Vec3::new(1.0, 0.0, 2.0)),
                ReviewModel,
            ))
            .id();

        app.update();
        let transform = app.world().get::<Transform>(model).expect("model");
        assert!(
            (transform.rotation.to_euler(bevy::math::EulerRot::YXZ).0 - 0.75).abs() < 1e-4,
            "Ctrl+LMB turns the figure"
        );
        assert_eq!(
            transform.translation,
            Vec3::new(1.1, 0.2, 2.0),
            "Ctrl+RMB offsets the figure from its home"
        );
    }
}
