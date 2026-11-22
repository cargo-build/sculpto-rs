use super::physics::{CapsulePose, Solver, DT};
use bevy::prelude::Vec3;
use std::sync::{mpsc, Arc, Mutex};
use std::time::{Duration, Instant};

pub(super) struct Goal {
    pub pins: Vec<Vec3>,
    pub caps: Vec<CapsulePose>,
    pub reset_to: Option<Vec<Vec3>>,
}

pub(super) struct Sample {
    pub previous: Vec<Vec3>,
    pub positions: Vec<Vec3>,
    pub finished: Instant,
    pub step_ms: f32,
    pub steps: u64,
    pub contacts: usize,
    pub pin_error: f32,
}

pub(super) struct Worker {
    goal: Arc<Mutex<Option<Goal>>>,
    sample: Arc<Mutex<Option<Sample>>>,
    keep_alive: mpsc::Sender<()>,
}

impl Worker {
    pub fn start(mut solver: Solver, mut current: Goal, pinned: Vec<u32>) -> Result<Self, String> {
        let goal: Arc<Mutex<Option<Goal>>> = Arc::default();
        let sample: Arc<Mutex<Option<Sample>>> = Arc::default();
        let (keep_alive, closed) = mpsc::channel::<()>();
        let mailbox = Arc::clone(&goal);
        let published = Arc::clone(&sample);
        std::thread::Builder::new()
            .name("garment-physics".into())
            .spawn(move || {
                let dt = Duration::from_secs_f32(DT);
                let mut last = solver.positions();
                let mut deadline = Instant::now();
                loop {
                    match closed.recv_timeout(deadline.saturating_duration_since(Instant::now())) {
                        Err(mpsc::RecvTimeoutError::Disconnected) => return,
                        Ok(()) => continue,
                        Err(mpsc::RecvTimeoutError::Timeout) => {}
                    }
                    if let Some(next) = mailbox.lock().expect("cloth pose mailbox").take() {
                        current = next;
                    }
                    if let Some(pose) = current.reset_to.take() {
                        solver.place(&pose);
                        last = pose;
                    }
                    let start = Instant::now();
                    solver.step(&current.pins, &current.caps);
                    let finished = Instant::now();
                    let positions = solver.positions();
                    let pin_error = pinned
                        .iter()
                        .zip(&current.pins)
                        .map(|(&i, p)| positions[i as usize].distance(*p))
                        .fold(0., f32::max);
                    let previous = std::mem::replace(&mut last, positions.clone());
                    let frame = Sample {
                        previous,
                        positions,
                        finished,
                        step_ms: (finished - start).as_secs_f32() * 1000.,
                        steps: solver.steps,
                        contacts: solver.contacts(),
                        pin_error,
                    };
                    *published.lock().expect("cloth sample mailbox") = Some(frame);
                    // A long contact solve must never queue a catch-up burst on the presentation thread.
                    deadline = (deadline + dt).max(finished);
                }
            })
            .map_err(|err| format!("start garment physics worker: {err}"))?;
        Ok(Self {
            goal,
            sample,
            keep_alive,
        })
    }
    pub fn submit(&self, mut next: Goal) {
        let mut mail = self.goal.lock().expect("cloth pose mailbox");
        if next.reset_to.is_none() {
            next.reset_to = mail.as_mut().and_then(|old| old.reset_to.take());
        }
        *mail = Some(next);
    }
    pub fn poll(&self) -> Option<Sample> {
        self.sample.lock().expect("cloth sample mailbox").take()
    }
}
impl Drop for Worker {
    fn drop(&mut self) {
        // Sending wakes the worker before the last sender disconnects.
        let _ = self.keep_alive.send(());
    }
}
