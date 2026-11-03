//! Headless Bevy proof: load the accepted actor GLB and the baked cloth GLB
//! (morph animation), render an offscreen frame, save it as PNG, exit.
use bevy::asset::RenderAssetUsages;
use bevy::prelude::*;
use bevy::render::render_resource::{Extent3d, TextureDimension, TextureFormat};
use bevy::render::view::screenshot::{save_to_disk, Screenshot};

fn env_f32(key: &str, default: f32) -> f32 {
    std::env::var(key).ok().and_then(|v| v.parse().ok()).unwrap_or(default)
}

fn main() {
    App::new()
        .add_plugins(bevy::app::ScheduleRunnerPlugin::run_loop(
            std::time::Duration::from_secs_f64(1.0 / 60.0),
        ))
        .add_plugins(
            DefaultPlugins
                .set(WindowPlugin {
                    primary_window: None,
                    primary_cursor_options: None,
                    exit_condition: bevy::window::ExitCondition::DontExit,
                    close_when_requested: false,
                })
                .set(ImagePlugin::default()),
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
        .register_type::<bevy::animation::AnimationTarget>()
        .register_type::<bevy::camera::primitives::Aabb>()
        .register_type::<bevy::gltf::GltfMaterialExtras>()
        .register_type::<bevy::gltf::GltfMeshName>()
        .register_type::<bevy::gltf::GltfMaterialName>()
        .register_type::<bevy::mesh::skinning::SkinnedMesh>()
        .register_type::<bevy::mesh::morph::MeshMorphWeights>()
        .register_type::<bevy::gltf::GltfMeshExtras>()
        .register_type::<bevy::mesh::morph::MorphWeights>()

        .register_type::<Name>()
        .register_type::<Visibility>()
        .register_type::<InheritedVisibility>()
        .register_type::<ViewVisibility>()
        .insert_resource(ClearColor(Color::srgb(0.02, 0.02, 0.03)))
        .insert_resource(AmbientLight {
            brightness: env_f32("SCULPTO_VIEWER_AMBIENT", 0.0),
            ..default()
        })
        .add_systems(Startup, setup)
        .add_systems(Update, (advance_animation, capture_once))
        .run();
}

#[derive(Resource)]
struct Target(Handle<Image>);

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
) {
    let mut image = Image::new(
        Extent3d { width: 960, height: 900, depth_or_array_layers: 1 },
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
    commands.spawn((
        Camera3d::default(),
        bevy::core_pipeline::tonemapping::Tonemapping::None,
        Camera { target: handle.into(), ..default() },
        Transform::from_xyz(2.2, 1.3, 2.8).looking_at(Vec3::new(0.0, 0.9, 0.0), Vec3::Y),
    ));
    commands.spawn((
        DirectionalLight { illuminance: 10_000.0, ..default() },
        Transform::from_xyz(3.0, 5.0, 2.0).looking_at(Vec3::ZERO, Vec3::Y),
    ));
    // The glTF loader spawns AnimationPlayer on each animation root inside
    // the scene, and every AnimationTarget references that internal player
    // entity. A player on the SceneRoot wrapper would drive nothing. So: load
    // one scene instance, build a graph blending the actor clip (bones) and
    // the cloth clip (morph weights), and hand it to the scene's own players
    // once they appear. Clip indices are per-asset (the combined export
    // prepends the cloth weight track), so the capture names them instead of
    // hard-coding one asset's numbering.
    let asset = env_or("SCULPTO_VIEWER_ASSET", "combined.glb");
    let actor_animation = env_or("SCULPTO_VIEWER_ACTOR_ANIMATION", "4");
    let cloth_animation = env_or("SCULPTO_VIEWER_CLOTH_ANIMATION", "0");
    let actor_clip = asset_server.load(format!("{asset}#Animation{actor_animation}"));
    let cloth_clip = asset_server.load(format!("{asset}#Animation{cloth_animation}"));
    let (mut graph, actor_node) = AnimationGraph::from_clip(actor_clip);
    let cloth_node = graph.add_clip(cloth_clip, 1.0, graph.root);
    commands.insert_resource(PlayerGraph {
        handle: graphs.add(graph),
        nodes: vec![actor_node, cloth_node],
    });
    commands.spawn(SceneRoot(asset_server.load(format!("{asset}#Scene0"))));
}

fn env_or(key: &str, default: &str) -> String {
    std::env::var(key).unwrap_or_else(|_| default.to_string())
}

fn advance_animation(
    mut commands: Commands,
    graph: Option<Res<PlayerGraph>>,
    mut players: Query<(Entity, &mut AnimationPlayer), Added<AnimationPlayer>>,
) {
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

fn capture_once(
    target: Res<Target>,
    mut frames: Local<u32>,
    mut captured: Local<bool>,
    mut cameras: Query<&mut Transform, With<Camera3d>>,
    mut commands: Commands,
    mut exit: EventWriter<AppExit>,
) {
    *frames += 1;
    if *frames < 240 {
        return;
    }
    // Series mode: one pose every `stride` ticks, one view per following
    // tick, so a review gets 10-20 distinct poses across the clip without
    // rewinding the animation.
    if let Some((start, count, stride)) =
        parse_series(&env_or("SCULPTO_VIEWER_SERIES", ""))
    {
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
                info!("viewer: series pose {} view {} at tick {}", pose_tick, view, *frames);
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
