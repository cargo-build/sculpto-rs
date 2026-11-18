//! Buffered Ctrl+digits. Bevy keyboard messages preserve the order of presses within one frame.
use crate::bank::Selection;
use bevy::{
    input::{
        keyboard::{KeyboardFocusLost, KeyboardInput},
        ButtonState,
    },
    prelude::*,
};
use std::time::Duration;

const IDLE: Duration = Duration::from_millis(1350);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Pick {
    Quick(usize),
    Typed(usize),
}
impl Pick {
    pub fn resolve(self, count: usize, inspection: bool) -> Option<Selection> {
        match self {
            Self::Typed(0) if inspection => Some(Selection::Inspection),
            Self::Typed(n) => count
                .checked_sub(1)
                .map(|last| Selection::Slot(n.min(last))),
            Self::Quick(n) if n < count => Some(Selection::Slot(n)),
            Self::Quick(_) => None,
        }
    }
}
#[derive(Resource, Default)]
pub struct NumberEntry {
    digits: String,
    last: Option<Duration>,
    control: [bool; 2],
    pub pick: Option<Pick>,
}
impl NumberEntry {
    pub fn digits(&self) -> &str {
        &self.digits
    }
    fn tick(&mut self, now: Duration) {
        self.pick = None;
        if self.last.is_some_and(|t| now.saturating_sub(t) >= IDLE) {
            let n = self.digits.bytes().fold(0usize, |v, ch| {
                v.saturating_mul(10).saturating_add(usize::from(ch - b'0'))
            });
            self.pick = Some(Pick::Typed(n));
            self.digits.clear();
            self.last = None;
        }
    }
    fn event(&mut self, key: KeyCode, pressed: bool, repeat: bool, now: Duration) {
        match key {
            KeyCode::ControlLeft => {
                self.control[0] = pressed;
                return;
            }
            KeyCode::ControlRight => {
                self.control[1] = pressed;
                return;
            }
            _ => {}
        }
        if !pressed || repeat {
            return;
        }
        let digit = match key {
            KeyCode::Digit0 | KeyCode::Numpad0 => 0,
            KeyCode::Digit1 | KeyCode::Numpad1 => 1,
            KeyCode::Digit2 | KeyCode::Numpad2 => 2,
            KeyCode::Digit3 | KeyCode::Numpad3 => 3,
            KeyCode::Digit4 | KeyCode::Numpad4 => 4,
            KeyCode::Digit5 | KeyCode::Numpad5 => 5,
            KeyCode::Digit6 | KeyCode::Numpad6 => 6,
            KeyCode::Digit7 | KeyCode::Numpad7 => 7,
            KeyCode::Digit8 | KeyCode::Numpad8 => 8,
            KeyCode::Digit9 | KeyCode::Numpad9 => 9,
            _ => return,
        };
        if self.control.iter().any(|v| *v) {
            self.digits.push(char::from(b'0' + digit));
            self.last = Some(now);
        } else {
            self.digits.clear();
            self.last = None;
            self.pick = Some(Pick::Quick(usize::from(digit)));
        }
    }
}
pub fn read(
    mut entry: ResMut<NumberEntry>,
    time: Res<Time<Real>>,
    mut events: MessageReader<KeyboardInput>,
    mut lost: MessageReader<KeyboardFocusLost>,
) {
    let now = time.elapsed();
    entry.tick(now);
    for e in events.read() {
        entry.event(e.key_code, e.state == ButtonState::Pressed, e.repeat, now);
    }
    if lost.read().count() > 0 {
        entry.control = [false; 2];
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn ms(n: u64) -> Duration {
        Duration::from_millis(n)
    }
    #[test]
    fn timeout_restarts_on_last_digit_and_control_release_does_not_submit() {
        let mut n = NumberEntry::default();
        n.event(KeyCode::ControlLeft, true, false, ms(0));
        n.event(KeyCode::Digit1, true, false, ms(0));
        n.tick(ms(1200));
        n.event(KeyCode::Digit2, true, false, ms(1200));
        n.event(KeyCode::ControlLeft, false, false, ms(1250));
        n.tick(ms(2549));
        assert_eq!(n.digits(), "12");
        assert_eq!(n.pick, None);
        n.tick(ms(2550));
        assert_eq!(n.pick, Some(Pick::Typed(12)));
        assert_eq!(n.pick.unwrap().resolve(33, true), Some(Selection::Slot(12)));
    }
    #[test]
    fn leading_zero_disassembly_and_overflow_have_distinct_destinations() {
        assert_eq!(Pick::Quick(0).resolve(33, true), Some(Selection::Slot(0)));
        assert_eq!(
            Pick::Typed(0).resolve(33, true),
            Some(Selection::Inspection)
        );
        assert_eq!(Pick::Typed(10).resolve(33, true), Some(Selection::Slot(10)));
        assert_eq!(
            Pick::Typed(167).resolve(33, true),
            Some(Selection::Slot(32))
        );
        let mut n = NumberEntry::default();
        n.event(KeyCode::ControlRight, true, false, ms(0));
        n.event(KeyCode::Numpad0, true, false, ms(1));
        n.event(KeyCode::Numpad1, true, false, ms(1));
        n.event(KeyCode::Digit1, true, true, ms(2));
        assert_eq!(n.digits(), "01");
        for _ in 0..40 {
            n.event(KeyCode::Digit9, true, false, ms(2));
        }
        n.tick(ms(1352));
        assert_eq!(n.pick.unwrap().resolve(33, true), Some(Selection::Slot(32)));
    }

    #[test]
    fn keyboard_messages_select_extended_slots_in_input_order() {
        use bevy::input::keyboard::Key;
        let mut app = App::new();
        app.init_resource::<NumberEntry>()
            .init_resource::<Time<Real>>()
            .add_message::<KeyboardInput>()
            .add_message::<KeyboardFocusLost>()
            .add_systems(Update, read);
        let key = |app: &mut App, key_code, state| {
            app.world_mut().write_message(KeyboardInput {
                key_code,
                state,
                logical_key: Key::Control,
                text: None,
                repeat: false,
                window: Entity::PLACEHOLDER,
            });
        };
        key(&mut app, KeyCode::ControlLeft, ButtonState::Pressed);
        key(&mut app, KeyCode::Digit1, ButtonState::Pressed);
        app.update();
        app.world_mut()
            .resource_mut::<Time<Real>>()
            .advance_by(ms(1200));
        key(&mut app, KeyCode::Numpad2, ButtonState::Pressed);
        key(&mut app, KeyCode::ControlLeft, ButtonState::Released);
        app.update();
        assert_eq!(app.world().resource::<NumberEntry>().digits(), "12");
        app.world_mut()
            .resource_mut::<Time<Real>>()
            .advance_by(ms(1349));
        app.update();
        assert_eq!(app.world().resource::<NumberEntry>().pick, None);
        app.world_mut()
            .resource_mut::<Time<Real>>()
            .advance_by(ms(1));
        app.update();
        assert_eq!(
            app.world().resource::<NumberEntry>().pick,
            Some(Pick::Typed(12))
        );
    }
}
