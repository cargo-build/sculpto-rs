//! The timeline at the bottom of an interactive window: where the clip is, in seconds and
//! keyframes, and a bar to scrub it by hand.
//!
//! The operator asked for a stopwatch like Blender's (2026-11-26): to catch the moment the
//! body comes through the clothing and name it to the second and millisecond.

use bevy::animation::graph::AnimationNodeType;
use bevy::prelude::*;
use bevy::ui::{ComputedNode, UiGlobalTransform};

use crate::{AssetProfile, ClipSelection, Playback, PlayerGraph, ViewerConfig};

/// Keyframes a second for a clip whose file does not say how many it holds.
const DEFAULT_FPS: f32 = 30.0;
const BAR_WIDTH_PX: f32 = 440.0;
const BAR_HEIGHT_PX: f32 = 12.0;
const AMBER: Color = Color::srgb(1.0, 0.82, 0.15);

/// The strip, shown while a clip is selected.
#[derive(Component)]
pub(crate) struct TimelinePanel;

/// The bar that takes a click or a drag: the place under the cursor is the clip's new time.
#[derive(Component)]
pub(crate) struct TimelineBar;

#[derive(Component)]
pub(crate) struct TimelineFill;

#[derive(Component)]
pub(crate) struct TimelinePlayhead;

#[derive(Component)]
pub(crate) struct TimelineClock;

/// What the input asks of the clip this frame, and the selected clip's keyframes.
#[derive(Resource, Default)]
pub(crate) struct Timeline {
    /// The left button went down on the bar and is still held: the bar takes the drag and the orbit stands still.
    pub scrubbing: bool,
    /// The cursor's place on the bar, 0 at the start of the clip and 1 at its end.
    pub place: Option<f32>,
    /// The asset digest and the clip the keyframe count belongs to.
    counted: Option<(String, usize)>,
    keyframes: Option<usize>,
}

impl Timeline {
    /// The keyframes of the selected clip, read from the file once per asset and clip.
    fn keyframes(&mut self, profile: &AssetProfile, clip: usize) -> Option<usize> {
        if !self
            .counted
            .as_ref()
            .is_some_and(|(digest, index)| *digest == profile.sha256 && *index == clip)
        {
            self.keyframes = std::fs::read(&profile.path)
                .ok()
                .and_then(|bytes| crate::glb_json(&bytes))
                .and_then(|json| frames_of(&json, clip));
            self.counted = Some((profile.sha256.clone(), clip));
        }
        self.keyframes
    }
}

/// `mm:ss.mmm`, as a stopwatch reads.
pub fn clock(seconds: f32) -> String {
    let milliseconds = (seconds.max(0.0) * 1000.0).round() as u64;
    format!(
        "{:02}:{:02}.{:03}",
        milliseconds / 60_000,
        milliseconds % 60_000 / 1000,
        milliseconds % 1000
    )
}

/// The seconds between two keyframes of a clip.
fn between(duration: f32, keyframes: Option<usize>) -> f32 {
    keyframes
        .filter(|keyframes| *keyframes > 1)
        .map_or(1.0 / DEFAULT_FPS, |keyframes| {
            duration / (keyframes - 1) as f32
        })
}

/// The keyframe a time is on, counted from 1, and how many the clip holds.
pub fn frame(seconds: f32, duration: f32, keyframes: Option<usize>) -> Option<(usize, usize)> {
    let between = between(duration, keyframes);
    let keyframes = keyframes?.max(1);
    Some((
        ((seconds / between).round() as usize + 1).min(keyframes),
        keyframes,
    ))
}

/// The time one keyframe away from `seconds`, held inside the clip.
pub fn step(seconds: f32, duration: f32, keyframes: Option<usize>, forward: bool) -> f32 {
    let between = between(duration, keyframes);
    let index = (seconds / between).round() as i64 + i64::from(forward) * 2 - 1;
    (index.max(0) as f32 * between).clamp(0.0, duration)
}

/// The cursor's place on the bar from its place across it, the centre at 0 and the ends at ±0.5.
pub fn place(across: f32) -> f32 {
    (across + 0.5).clamp(0.0, 1.0)
}

/// The most keyframes any channel of a clip holds.
pub fn frames_of(json: &serde_json::Value, clip: usize) -> Option<usize> {
    let accessors = json.get("accessors")?.as_array()?;
    json.get("animations")?
        .as_array()?
        .get(clip)?
        .get("samplers")?
        .as_array()?
        .iter()
        .filter_map(|sampler| sampler.get("input")?.as_u64())
        .filter_map(|input| accessors.get(input as usize))
        .filter_map(|accessor| accessor.get("count")?.as_u64())
        .max()
        .map(|count| count as usize)
}

/// The strip at the bottom of the window, hidden until a clip plays.
pub(crate) fn spawn(commands: &mut Commands) {
    commands
        .spawn((
            Node {
                position_type: PositionType::Absolute,
                left: Val::Px(0.0),
                right: Val::Px(0.0),
                bottom: Val::Px(14.0),
                justify_content: JustifyContent::Center,
                ..default()
            },
            TimelinePanel,
            Visibility::Hidden,
        ))
        .with_children(|panel| {
            panel
                .spawn((
                    Node {
                        flex_direction: FlexDirection::Column,
                        align_items: AlignItems::Center,
                        row_gap: Val::Px(5.0),
                        padding: UiRect::axes(Val::Px(12.0), Val::Px(7.0)),
                        ..default()
                    },
                    BackgroundColor(Color::srgba(0.0, 0.0, 0.0, 0.55)),
                ))
                .with_children(|strip| {
                    strip.spawn((
                        Text::new(String::new()),
                        TextFont {
                            font_size: bevy::text::FontSize::Px(13.0),
                            ..default()
                        },
                        TextColor(Color::WHITE),
                        TimelineClock,
                    ));
                    strip
                        .spawn((
                            Node {
                                width: Val::Px(BAR_WIDTH_PX),
                                height: Val::Px(BAR_HEIGHT_PX),
                                border_radius: BorderRadius::all(Val::Px(BAR_HEIGHT_PX / 2.0)),
                                ..default()
                            },
                            BackgroundColor(Color::srgba(1.0, 1.0, 1.0, 0.14)),
                            TimelineBar,
                        ))
                        .with_children(|bar| {
                            bar.spawn((
                                Node {
                                    position_type: PositionType::Absolute,
                                    left: Val::Px(0.0),
                                    top: Val::Px(0.0),
                                    bottom: Val::Px(0.0),
                                    width: Val::Percent(0.0),
                                    border_radius: BorderRadius::all(Val::Px(BAR_HEIGHT_PX / 2.0)),
                                    ..default()
                                },
                                BackgroundColor(AMBER),
                                TimelineFill,
                            ));
                            bar.spawn((
                                Node {
                                    position_type: PositionType::Absolute,
                                    top: Val::Px(-3.0),
                                    bottom: Val::Px(-3.0),
                                    left: Val::Percent(0.0),
                                    width: Val::Px(2.0),
                                    ..default()
                                },
                                BackgroundColor(Color::WHITE),
                                TimelinePlayhead,
                            ));
                        });
                });
        });
}

/// A plain left button on the bar scrubs the clip. The gesture lasts until the button is
/// released, wherever the cursor has gone, and holds the orbit off meanwhile.
pub(crate) fn read(
    config: Res<ViewerConfig>,
    keys: Res<ButtonInput<KeyCode>>,
    buttons: Res<ButtonInput<MouseButton>>,
    windows: Query<&Window>,
    bar: Query<(&ComputedNode, &UiGlobalTransform), With<TimelineBar>>,
    mut timeline: ResMut<Timeline>,
) {
    let held = timeline.scrubbing;
    timeline.scrubbing = false;
    timeline.place = None;
    if !config.interactive {
        return;
    }
    let (Ok((node, transform)), Some(cursor)) = (
        bar.single(),
        windows.single().ok().and_then(Window::cursor_position),
    ) else {
        return;
    };
    let plain = !keys.any_pressed([
        KeyCode::ShiftLeft,
        KeyCode::ShiftRight,
        KeyCode::ControlLeft,
        KeyCode::ControlRight,
    ]);
    let begun =
        node.contains_point(*transform, cursor) && plain && buttons.just_pressed(MouseButton::Left);
    if !(begun || (held && buttons.pressed(MouseButton::Left))) {
        return;
    }
    let across = transform.inverse().transform_point2(cursor).x / node.size().x;
    timeline.scrubbing = true;
    timeline.place = Some(place(across));
}

/// Show where the clip is, step it a keyframe with Left and Right, and take what the bar asked.
#[allow(clippy::too_many_arguments, clippy::type_complexity)]
pub(crate) fn show(
    keys: Res<ButtonInput<KeyCode>>,
    mut timeline: ResMut<Timeline>,
    selection: Res<ClipSelection>,
    profile: Res<AssetProfile>,
    graph: Option<Res<PlayerGraph>>,
    mut playback: ResMut<Playback>,
    graphs: Res<Assets<AnimationGraph>>,
    clips: Res<Assets<AnimationClip>>,
    mut players: Query<(&mut AnimationPlayer, &AnimationGraphHandle)>,
    mut panel: Query<&mut Visibility, With<TimelinePanel>>,
    mut clock_text: Query<&mut Text, With<TimelineClock>>,
    mut strip: Query<
        (&mut Node, Has<TimelinePlayhead>),
        Or<(With<TimelineFill>, With<TimelinePlayhead>)>,
    >,
) {
    let clip = graph.as_ref().and_then(|graph| {
        let node = *graph.nodes.get(selection.0)?;
        let entry = graphs.get(&graph.handle)?.get(node)?;
        let AnimationNodeType::Clip(handle) = &entry.node_type else {
            return None;
        };
        Some((node, clips.get(handle)?.duration()))
    });
    let mut hide = || {
        panel
            .iter_mut()
            .for_each(|mut visibility| *visibility = Visibility::Hidden)
    };
    let Some((node, duration)) = clip else {
        hide();
        return;
    };
    let handle = graph
        .as_ref()
        .map(|graph| &graph.handle)
        .expect("a clip needs its graph");
    let Some(now) = players
        .iter()
        .filter(|(_, bound)| bound.0 == *handle)
        .find_map(|(player, _)| {
            player
                .animation(node)
                .map(|animation| animation.seek_time())
        })
    else {
        hide();
        return;
    };
    let keyframes = timeline.keyframes(&profile, selection.0);
    let stepped = [(KeyCode::ArrowLeft, false), (KeyCode::ArrowRight, true)]
        .into_iter()
        .find(|(key, _)| keys.just_pressed(*key))
        .map(|(_, forward)| {
            playback.paused = true;
            step(now, duration, keyframes, forward)
        });
    let to = timeline.place.map(|place| place * duration).or(stepped);
    if let Some(to) = to {
        for (mut player, _) in players.iter_mut().filter(|(_, bound)| bound.0 == *handle) {
            if let Some(animation) = player.animation_mut(node) {
                animation.set_seek_time(to);
            }
        }
    }
    let at = to.unwrap_or(now);
    let along = if duration > 0.0 {
        (at / duration).clamp(0.0, 1.0)
    } else {
        0.0
    };
    let keyframes = frame(at, duration, keyframes)
        .map_or_else(String::new, |(on, of)| format!("   {on}/{of} frames"));
    let line = format!(
        "{} {:.2}x   {} / {}{keyframes}",
        if playback.paused { "||" } else { ">" },
        playback.speed,
        clock(at),
        clock(duration)
    );
    panel
        .iter_mut()
        .for_each(|mut visibility| *visibility = Visibility::Inherited);
    clock_text.iter_mut().for_each(|mut text| {
        if text.0 != line {
            text.0.clone_from(&line);
        }
    });
    for (mut node, playhead) in &mut strip {
        let place = Val::Percent(along * 100.0);
        if playhead {
            node.left = place;
        } else {
            node.width = place;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn close(left: f32, right: f32) -> bool {
        (left - right).abs() < 1e-5
    }

    #[test]
    fn the_clock_reads_minutes_seconds_and_milliseconds() {
        assert_eq!(clock(0.0), "00:00.000");
        assert_eq!(clock(4.1204), "00:04.120");
        assert_eq!(clock(64.5), "01:04.500");
        assert_eq!(clock(-1.0), "00:00.000");
    }

    #[test]
    fn a_step_moves_one_keyframe_and_stays_inside_the_clip() {
        let (duration, keyframes) = (1.0, Some(31));
        assert!(close(
            step(15.0 / 30.0, duration, keyframes, true),
            16.0 / 30.0
        ));
        assert!(close(step(16.0 / 30.0, duration, keyframes, false), 0.5));
        assert_eq!(step(0.0, duration, keyframes, false), 0.0);
        assert_eq!(step(1.0, duration, keyframes, true), 1.0);
        assert!(close(
            step(1.0, duration, None, false),
            1.0 - 1.0 / DEFAULT_FPS
        ));
        assert!(close(
            step(0.4, duration, Some(1), false),
            0.4 - 1.0 / DEFAULT_FPS
        ));
    }

    #[test]
    fn a_frame_is_counted_from_one_up_to_the_clip_s_keyframes() {
        assert_eq!(frame(0.0, 1.0, Some(31)), Some((1, 31)));
        assert_eq!(frame(0.5, 1.0, Some(31)), Some((16, 31)));
        assert_eq!(frame(1.0, 1.0, Some(31)), Some((31, 31)));
        assert_eq!(frame(0.5, 1.0, None), None);
    }

    #[test]
    fn the_cursor_s_place_on_the_bar_stops_at_its_ends() {
        assert_eq!(place(-0.9), 0.0);
        assert_eq!(place(-0.5), 0.0);
        assert_eq!(place(0.0), 0.5);
        assert_eq!(place(0.25), 0.75);
        assert_eq!(place(0.9), 1.0);
    }

    #[test]
    fn the_keyframes_are_the_longest_channel_of_the_chosen_clip() {
        let json = serde_json::json!({
            "accessors": [{"count": 31}, {"count": 7}, {"count": 31}],
            "animations": [{"samplers": [{"input": 1}, {"input": 2}]}, {"samplers": [{"input": 1}]}]
        });
        assert_eq!(frames_of(&json, 0), Some(31));
        assert_eq!(frames_of(&json, 1), Some(7));
        assert_eq!(frames_of(&json, 2), None);
        assert_eq!(frames_of(&serde_json::json!({}), 0), None);
    }
}
