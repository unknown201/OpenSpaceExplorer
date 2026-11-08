//! A Kerbal's EVA animation as KSP2's `animtree_kerbal_eva` animator runs it: its states (IDLES, MOVEMENT,
//! ZERO_G, JUMP_START, JUMP_END, LADDER, FLAG_PLANT), the transitions between them and their cross-fade times
//! and exit times, read from the controller in `kerbal_shared_assets_all.bundle`. Each frame gives the clips to
//! blend and their weights; the walk and the run play in step, as the animator's blend tree keeps them.

/// One of the animator's states we play.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum State {
    Idle,
    Movement,
    /// ZERO_G: off the ground (falling, floating, between a jump's take-off and landing).
    Air,
    JumpStart,
    JumpEnd,
    Ladder,
    FlagPlant,
}

/// What the animator reads each frame (its parameters, from what the Kerbal does).
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Inputs {
    /// fMoveBF: forward (+) or back (-), -1..1.
    pub move_fb: f32,
    /// fMoveLR: right (+) or left (-), -1..1.
    pub move_lr: f32,
    /// fSprint: walk (0) to run (1).
    pub sprint: f32,
    /// fMoveDU on a ladder: climbing up (+) or down (-), -1..1.
    pub climb: f32,
    /// bOnLadder.
    pub on_ladder: bool,
    /// bFalling: not standing on anything.
    pub falling: bool,
    /// tJump: a jump started this frame.
    pub jump: bool,
    /// tPlantFlag: a flag is being planted (from this frame).
    pub plant_flag: bool,
}

/// The cross-fade (seconds) and, for a transition that waits for its clip, the share of the clip it waits for.
#[derive(Clone, Copy, Debug, PartialEq)]
struct Transition {
    to: State,
    fade_s: f32,
}

/// The animator: the state, when it was entered, and the state it is fading from.
#[derive(Clone, Debug)]
pub struct Machine {
    state: State,
    since: f64,
    from: Option<(State, f64)>,
    fade_start: f64,
    fade_s: f32,
    /// The walk-run cycle's phase (0..1), shared by the clips of the MOVEMENT blend tree.
    phase: f32,
    last: Option<f64>,
}

/// Clip lengths, s (the exported clips; fallbacks if a model lacks one).
pub trait Durations {
    fn duration(&self, clip: &str) -> f32;
}

impl<F: Fn(&str) -> f32> Durations for F {
    fn duration(&self, clip: &str) -> f32 {
        self(clip)
    }
}

/// KSP2's EVA animator times (animtree_kerbal_eva).
pub mod timing {
    /// IDLES -> MOVEMENT when |fMoveBF| or |fMoveLR| > 0.1.
    pub const IDLE_TO_MOVE_S: f32 = 0.05;
    /// MOVEMENT -> IDLES once both are within 0.01 of zero.
    pub const MOVE_TO_IDLE_S: f32 = 0.5;
    /// IDLES -> JUMP_START (MOVEMENT: 0.05 s).
    pub const IDLE_TO_JUMP_S: f32 = 0.0;
    pub const MOVE_TO_JUMP_S: f32 = 0.05;
    /// JUMP_START -> ZERO_G at 0.59 of the take-off clip, fading 0.21 s.
    pub const JUMP_START_EXIT: f32 = 0.59;
    pub const JUMP_START_TO_AIR_S: f32 = 0.21;
    /// Any state -> ZERO_G when it starts falling (tStartFloating).
    pub const START_FLOATING_S: f32 = 0.10;
    /// ZERO_G -> JUMP_END when it stops falling.
    pub const AIR_TO_LAND_S: f32 = 0.05;
    /// JUMP_END -> IDLES at 0.56 of the landing clip, or -> MOVEMENT at once when moving; 0.25 s either way.
    pub const LAND_EXIT: f32 = 0.56;
    pub const LAND_TO_NEXT_S: f32 = 0.25;
    /// Any state -> LADDER on grabbing one; LADDER -> IDLES on letting go.
    pub const LADDER_S: f32 = 0.25;
    /// Any state -> FLAG_PLANT; FLAG_PLANT -> IDLES at 0.97 of the clip, fading 0.02 s.
    pub const TO_FLAG_S: f32 = 0.05;
    pub const FLAG_EXIT: f32 = 0.97;
    pub const FLAG_TO_IDLE_S: f32 = 0.02;
}

fn moving(i: &Inputs) -> bool {
    i.move_fb.abs() > 0.1 || i.move_lr.abs() > 0.1
}

fn still(i: &Inputs) -> bool {
    i.move_fb.abs() < 0.01 && i.move_lr.abs() < 0.01
}

impl Machine {
    pub fn new(now: f64) -> Self {
        Self {
            state: State::Idle,
            since: now,
            from: None,
            fade_start: now,
            fade_s: 0.0,
            phase: 0.0,
            last: None,
        }
    }

    pub fn state(&self) -> State {
        self.state
    }

    fn go(&mut self, now: f64, t: Transition) {
        if t.to == self.state {
            return;
        }
        self.from = Some((self.state, self.since));
        self.fade_start = now;
        self.fade_s = t.fade_s;
        self.state = t.to;
        self.since = now;
    }

    /// Normalized time of the current state's one-shot clip.
    fn progress(&self, now: f64, clip: &str, d: &dyn Durations) -> f32 {
        ((now - self.since) as f32 / d.duration(clip).max(1e-3)).max(0.0)
    }

    /// Advance to `now`: take the transitions the inputs and clip times call for.
    pub fn update(&mut self, now: f64, i: &Inputs, d: &dyn Durations) {
        use timing::*;
        let dt = self.last.map_or(0.0, |l| (now - l).max(0.0)) as f32;
        self.last = Some(now);
        // the walk-run cycle runs at the blended clip length
        let cycle = d.duration("walk")
            + (d.duration("run") - d.duration("walk")) * i.sprint.clamp(0.0, 1.0);
        self.phase = (self.phase + dt / cycle.max(1e-3)).fract();
        // any-state transitions
        if i.on_ladder {
            self.go(
                now,
                Transition {
                    to: State::Ladder,
                    fade_s: LADDER_S,
                },
            );
            return;
        }
        if i.plant_flag && self.state != State::FlagPlant {
            self.go(
                now,
                Transition {
                    to: State::FlagPlant,
                    fade_s: TO_FLAG_S,
                },
            );
            return;
        }
        let t = match self.state {
            State::Ladder => Some(Transition {
                to: State::Idle,
                fade_s: LADDER_S,
            }),
            State::Idle if i.jump => Some(Transition {
                to: State::JumpStart,
                fade_s: IDLE_TO_JUMP_S,
            }),
            State::Movement if i.jump => Some(Transition {
                to: State::JumpStart,
                fade_s: MOVE_TO_JUMP_S,
            }),
            State::Idle | State::Movement | State::JumpEnd | State::FlagPlant if i.falling => {
                Some(Transition {
                    to: State::Air,
                    fade_s: START_FLOATING_S,
                })
            }
            State::Idle if moving(i) => Some(Transition {
                to: State::Movement,
                fade_s: IDLE_TO_MOVE_S,
            }),
            State::Movement if still(i) => Some(Transition {
                to: State::Idle,
                fade_s: MOVE_TO_IDLE_S,
            }),
            State::JumpStart if self.progress(now, "jump_start", d) >= JUMP_START_EXIT => {
                Some(Transition {
                    to: State::Air,
                    fade_s: JUMP_START_TO_AIR_S,
                })
            }
            State::Air if !i.falling => Some(Transition {
                to: State::JumpEnd,
                fade_s: AIR_TO_LAND_S,
            }),
            State::JumpEnd if !still(i) => Some(Transition {
                to: State::Movement,
                fade_s: LAND_TO_NEXT_S,
            }),
            State::JumpEnd if self.progress(now, "jump_end", d) >= LAND_EXIT => Some(Transition {
                to: State::Idle,
                fade_s: LAND_TO_NEXT_S,
            }),
            State::FlagPlant if self.progress(now, "flag_plant", d) >= FLAG_EXIT => {
                Some(Transition {
                    to: State::Idle,
                    fade_s: FLAG_TO_IDLE_S,
                })
            }
            _ => None,
        };
        if let Some(t) = t {
            self.go(now, t);
        }
    }

    /// The clips a state plays: (clip, time s, looping, weight within the state).
    fn state_clips(
        &self,
        s: State,
        since: f64,
        now: f64,
        i: &Inputs,
        d: &dyn Durations,
    ) -> Vec<(&'static str, f32, bool, f32)> {
        let t = (now - since).max(0.0) as f32;
        match s {
            State::Idle => vec![("idle", t, true, 1.0)],
            State::Air => vec![("air", t, true, 1.0)],
            State::JumpStart => vec![("jump_start", t, false, 1.0)],
            State::JumpEnd => vec![("jump_end", t, false, 1.0)],
            State::FlagPlant => vec![("flag_plant", t, false, 1.0)],
            State::Ladder => {
                let c = i.climb.clamp(-1.0, 1.0);
                vec![
                    ("ladder_up", t, true, c.max(0.0)),
                    ("ladder_idle", t, true, 1.0 - c.abs()),
                    ("ladder_down", t, true, (-c).max(0.0)),
                ]
            }
            State::Movement => {
                // in step: every clip at the shared phase
                let at = |clip: &'static str| self.phase * d.duration(clip);
                if i.move_lr.abs() > i.move_fb.abs() {
                    let side = if i.move_lr > 0.0 { "walk_r" } else { "walk_l" };
                    vec![(side, at(side), true, 1.0)]
                } else if i.move_fb < -0.01 {
                    vec![("walk_back", at("walk_back"), true, 1.0)]
                } else {
                    let s = i.sprint.clamp(0.0, 1.0);
                    vec![
                        ("walk", at("walk"), true, 1.0 - s),
                        ("run", at("run"), true, s),
                    ]
                }
            }
        }
    }

    /// The clips to blend now and their weights (summing to 1): the state faded in over the one it left.
    pub fn clips(
        &self,
        now: f64,
        i: &Inputs,
        d: &dyn Durations,
    ) -> Vec<(&'static str, f32, bool, f32)> {
        let a = if self.fade_s > 0.0 {
            (((now - self.fade_start) as f32) / self.fade_s).clamp(0.0, 1.0)
        } else {
            1.0
        };
        let mut out: Vec<_> = self
            .state_clips(self.state, self.since, now, i, d)
            .into_iter()
            .map(|(c, t, l, w)| (c, t, l, w * a))
            .collect();
        if let Some((s, since)) = self.from.filter(|_| a < 1.0) {
            out.extend(
                self.state_clips(s, since, now, i, d)
                    .into_iter()
                    .map(|(c, t, l, w)| (c, t, l, w * (1.0 - a))),
            );
        }
        out.retain(|c| c.3 > 1e-4);
        out
    }

    /// The share the current state has of the pose (1 once its cross-fade is over).
    pub fn fade_in(&self, now: f64) -> f32 {
        if self.fade_s > 0.0 {
            (((now - self.fade_start) as f32) / self.fade_s).clamp(0.0, 1.0)
        } else {
            1.0
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn d(clip: &str) -> f32 {
        // the exported clips' lengths (tools/export_kerbal_anim.py)
        match clip {
            "walk" | "walk_back" | "walk_l" | "walk_r" => 0.83,
            "run" => 0.53,
            "jump_start" => 0.50,
            "jump_end" => 0.57,
            "flag_plant" => 8.13,
            "ladder_up" | "ladder_down" => 2.0,
            _ => 6.0,
        }
    }

    fn run(m: &mut Machine, from: f64, to: f64, i: &Inputs) {
        let mut t = from;
        while t < to - 1e-9 {
            t += 0.02;
            m.update(t, i, &d);
        }
    }

    #[test]
    fn walking_fades_in_over_a_twentieth_of_a_second_and_out_over_half_a_second() {
        let mut m = Machine::new(0.0);
        let walk = Inputs {
            move_fb: 1.0,
            ..Default::default()
        };
        m.update(0.02, &walk, &d);
        assert_eq!(m.state(), State::Movement);
        assert!(
            (m.fade_in(0.02 + 0.025) - 0.5).abs() < 1e-3,
            "half way at 25 ms"
        );
        assert_eq!(m.fade_in(0.02 + 0.05), 1.0);
        run(&mut m, 0.02, 1.0, &walk);
        let stop = Inputs::default();
        m.update(1.02, &stop, &d);
        assert_eq!(m.state(), State::Idle);
        assert!(
            (m.fade_in(1.02 + 0.25) - 0.5).abs() < 1e-3,
            "MOVEMENT -> IDLES takes 0.5 s"
        );
        // the walk still shows half way through the fade
        let clips = m.clips(1.27, &stop, &d);
        let w = |n: &str| clips.iter().filter(|c| c.0 == n).map(|c| c.3).sum::<f32>();
        assert!(
            (w("idle") - 0.5).abs() < 1e-3 && (w("walk") - 0.5).abs() < 1e-3,
            "{clips:?}"
        );
    }

    #[test]
    fn a_jump_takes_off_flies_and_lands_on_the_animator_s_times() {
        let mut m = Machine::new(0.0);
        let jump = Inputs {
            jump: true,
            falling: true,
            ..Default::default()
        };
        m.update(0.02, &jump, &d);
        assert_eq!(m.state(), State::JumpStart);
        let air = Inputs {
            falling: true,
            ..Default::default()
        };
        // JUMP_START -> ZERO_G at 0.59 of the 0.5 s take-off
        run(&mut m, 0.02, 0.30, &air);
        assert_eq!(m.state(), State::JumpStart);
        run(&mut m, 0.30, 0.34, &air);
        assert_eq!(m.state(), State::Air);
        run(&mut m, 0.34, 1.2, &air);
        let down = Inputs::default();
        m.update(1.22, &down, &d);
        assert_eq!(m.state(), State::JumpEnd);
        // JUMP_END -> IDLES at 0.56 of the 0.57 s landing
        run(&mut m, 1.22, 1.50, &down);
        assert_eq!(m.state(), State::JumpEnd);
        run(&mut m, 1.50, 1.56, &down);
        assert_eq!(m.state(), State::Idle);
        // walking out of a landing goes straight to MOVEMENT
        let mut m2 = m.clone();
        let fall = Inputs {
            falling: true,
            ..Default::default()
        };
        m2.update(2.0, &fall, &d);
        assert_eq!(
            m2.state(),
            State::Air,
            "walking off an edge floats (tStartFloating)"
        );
        m2.update(2.5, &down, &d);
        m2.update(
            2.52,
            &Inputs {
                move_fb: 1.0,
                ..Default::default()
            },
            &d,
        );
        assert_eq!(m2.state(), State::Movement);
    }

    #[test]
    fn ladder_and_flag_are_entered_from_anywhere_and_left_as_the_animator_does() {
        let mut m = Machine::new(0.0);
        let ladder = Inputs {
            on_ladder: true,
            climb: 1.0,
            ..Default::default()
        };
        m.update(0.02, &ladder, &d);
        assert_eq!(m.state(), State::Ladder);
        let clips = m.clips(0.5, &ladder, &d);
        assert_eq!(
            clips.iter().find(|c| c.0 == "ladder_up").map(|c| c.3),
            Some(1.0)
        );
        m.update(
            0.6,
            &Inputs {
                falling: true,
                ..Default::default()
            },
            &d,
        );
        assert_eq!(m.state(), State::Idle, "LADDER -> IDLES on letting go");
        m.update(
            0.62,
            &Inputs {
                falling: true,
                ..Default::default()
            },
            &d,
        );
        assert_eq!(m.state(), State::Air, "then floats while it falls");
        m.update(2.0, &Inputs::default(), &d);
        let flag = Inputs {
            plant_flag: true,
            ..Default::default()
        };
        m.update(3.0, &flag, &d);
        assert_eq!(m.state(), State::FlagPlant);
        run(&mut m, 3.0, 3.0 + 8.13 * 0.96, &Inputs::default());
        assert_eq!(m.state(), State::FlagPlant);
        run(
            &mut m,
            3.0 + 8.13 * 0.96,
            3.0 + 8.13 * 0.98,
            &Inputs::default(),
        );
        assert_eq!(m.state(), State::Idle);
    }

    #[test]
    fn the_walk_and_the_run_blend_in_step_by_the_sprint() {
        let mut m = Machine::new(0.0);
        let half = Inputs {
            move_fb: 1.0,
            sprint: 0.5,
            ..Default::default()
        };
        run(&mut m, 0.0, 2.0, &half);
        let clips = m.clips(2.0, &half, &d);
        let walk = clips.iter().find(|c| c.0 == "walk").unwrap();
        let run_c = clips.iter().find(|c| c.0 == "run").unwrap();
        assert!((walk.3 - 0.5).abs() < 1e-3 && (run_c.3 - 0.5).abs() < 1e-3);
        assert!(
            (walk.1 / 0.83 - run_c.1 / 0.53).abs() < 1e-4,
            "same phase: {walk:?} {run_c:?}"
        );
    }
}
