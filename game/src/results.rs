use std::{
    path::PathBuf,
    rc::Rc,
    sync::{mpsc::Sender, Arc, Mutex},
};

use kson::score_ticks::ScoreTick;
use rodio::dynamic_mixer::DynamicMixerController;
use serde::Serialize;

use crate::{
    button_codes::UscButton,
    game::{HitRating, HitWindow},
    game_data::GameData,
    input_state::InputState,
    scene::{Scene, SceneData},
    songselect::{Difficulty, Song},
    vg_ui::Vgfx,
    ControlMessage,
};
use tealr::{
    mlu::{
        mlua::{Function, Lua, LuaSerdeExt},
        TealData, UserData,
    },
    TypeName,
};

#[derive(Debug, TypeName, Clone, Serialize, Default)]
#[serde(rename_all = "camelCase")]
struct HidSud {}

#[derive(Debug, TypeName, Clone, Serialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct SongResultData {
    score: u32,
    gauge_type: i32, // 0 = normal, 1 = hard. Should be defined in constants sometime
    gauge_option: i32, // type specific, such as difficulty level for the same gauge type if available
    mirror: bool,
    random: bool,
    auto_flags: i32, //bits for autoplay settings, 0 = no autoplay
    gauge: f32,      // value of the gauge at the end of the song
    misses: i32,
    goods: i32,
    perfects: i32,
    max_combo: i32,
    level: u8,
    difficulty: u8,
    title: String,      // With the player name in multiplayer
    real_title: String, // Always without the player name
    artist: String,
    effector: String,
    illustrator: String,
    bpm: String,
    duration: i32, // Length of the chart in milliseconds
    jacket_path: PathBuf,
    median_hit_delta: i32,
    mean_hit_delta: f32,
    median_hit_delta_abs: i32,
    mean_hit_delta_abs: f32,
    earlies: i32,
    lates: i32,
    badge: i32, // same as song wheel badge (except 0 which means the user manually exited)
    gauge_samples: Vec<f32>, // gauge values sampled throughout the song
    grade: String, // "S", "AAA+", "AAA", etc.
    high_scores: Vec<Score>, // Same as song wheel scores
    player_name: String,
    display_index: i32, // Only on multiplayer; which player's score (not necessarily the viewer's) is being shown right not
    uid: String,        // Only on multiplayer; the UID of the viewer
    hit_window: HitWindow, // Same as gameplay HitWindow
    autoplay: bool,
    playback_speed: f32,
    mission: String,               // Only on practice mode
    retry_count: i32,              // Only on practice mode
    is_self: bool, // Whether this score is viewer's in multiplayer; always true for singleplayer
    speed_mod_type: i32, // Only when isSelf is true; 0 for XMOD, 1 for MMOD, 2 for CMOD
    speed_mod_value: i32, // Only when isSelf is true; HiSpeed for XMOD, ModSpeed for MMOD and CMOD
    hidsud: HidSud, // Only when isSelf is true
    note_hit_stats: Vec<HitStat>, // Only when isSelf is true; contains HitStat for notes (excluding hold notes and lasers)
    hold_hit_stats: Vec<HitStat>, // Only when isSelf is true; contains HitStat for holds
    laser_hit_stats: Vec<HitStat>, // Only when isSelf is true; contains HitStat for lasers
    is_local: bool,               // Whether this score was set locally
}

impl SongResultData {
    pub fn from_diff(
        song: Arc<Song>,
        diff_idx: usize,
        score: u32,
        hit_ratings: Vec<HitRating>,
        gauge: f32,
    ) -> Self {
        let Difficulty {
            jacket_path,
            level,
            difficulty,
            id: _,
            effector,
            top_badge: _,
            scores,
            hash: _,
        } = song.difficulties[diff_idx].clone();

        let Song {
            title,
            artist,
            bpm,
            id: _,
            difficulties: _,
        } = (*song).clone();

        let grade = match score {
            99_00000.. => "S",
            98_00000.. => "AAA+",
            97_00000.. => "AAA",
            95_00000.. => "AA+",
            93_00000.. => "AA",
            90_00000.. => "A+",
            87_00000.. => "A",
            75_00000.. => "B",
            65_00000.. => "C",
            0.. => "D",
        }
        .to_string();
        let (laser_hit_stats, note_hit_stats, hold_hit_stats) = hit_ratings.iter().fold(
            (vec![], vec![], vec![]),
            |(mut laser, mut note, mut hold), x| {
                let rating = (*x).try_into();
                match x {
                    HitRating::None => {}
                    HitRating::Crit {
                        tick,
                        delta: _,
                        time: _,
                    }
                    | HitRating::Good {
                        tick,
                        delta: _,
                        time: _,
                    }
                    | HitRating::Miss {
                        tick,
                        delta: _,
                        time: _,
                    } => match tick.tick {
                        ScoreTick::Laser { lane: _, pos: _ }
                        | ScoreTick::Slam {
                            lane: _,
                            start: _,
                            end: _,
                        } => laser.push(rating.unwrap()),

                        ScoreTick::Chip { lane: _ } => note.push(rating.unwrap()),
                        ScoreTick::Hold { lane: _ } => hold.push(rating.unwrap()),
                    },
                }
                (laser, note, hold)
            },
        );
        Self {
            score,
            jacket_path,
            artist,
            title,
            effector,
            high_scores: scores,
            level,
            difficulty,
            bpm,
            grade,
            gauge_samples: vec![0.0; 256],
            gauge,
            goods: hit_ratings
                .iter()
                .filter(|x| {
                    matches!(
                        x,
                        HitRating::Good {
                            tick: _,
                            delta: _,
                            time: _
                        }
                    )
                })
                .count() as i32,
            perfects: hit_ratings
                .iter()
                .filter(|x| {
                    matches!(
                        x,
                        HitRating::Crit {
                            tick: _,
                            delta: _,
                            time: _
                        }
                    )
                })
                .count() as i32,
            misses: hit_ratings
                .iter()
                .filter(|x| {
                    matches!(
                        x,
                        HitRating::Miss {
                            tick: _,
                            delta: _,
                            time: _
                        }
                    )
                })
                .count() as i32,

            earlies: hit_ratings
                .iter()
                .filter(
                    |x| matches!(x, HitRating::Good { tick: _, delta, time: _ } if *delta < 0.0),
                )
                .count() as i32,
            lates: hit_ratings
                .iter()
                .filter(
                    |x| matches!(x, HitRating::Good { tick: _, delta, time: _ } if *delta > 0.0),
                )
                .count() as i32,
            laser_hit_stats,
            note_hit_stats,
            hold_hit_stats,

            ..Default::default()
        }
    }
}

impl SceneData for SongResultData {
    fn make_scene(
        self: Box<Self>,
        _input_state: InputState,
        _: Rc<Mutex<Vgfx>>,
        _: Rc<Mutex<GameData>>,
    ) -> anyhow::Result<Box<dyn Scene>> {
        Ok(Box::new(SongResult {
            close: false,
            control_tx: None,
            data: *self,
            lua: Rc::new(Lua::new()),
        }))
    }
}

#[derive(Debug, TypeName, Clone, Serialize, Default)]
#[serde(rename_all = "camelCase")]
struct HitStat {
    rating: i32,    // 0 for miss, 1 for near, 2 for crit
    lane: i32,      // 0-3 btn, 4-5 fx, 6-7 lasers
    time: i32,      // In milliseconds
    time_frac: f32, // Between 0 and 1
    delta: i32,
    hold: i32, // 0 for chip or laser, otherwise # of ticks in hold
}

impl TryFrom<HitRating> for HitStat {
    type Error = anyhow::Error;

    fn try_from(value: HitRating) -> Result<Self, Self::Error> {
        let mut ret = match value {
            HitRating::None => return Err(anyhow::anyhow!("HitRating was None")),
            HitRating::Crit { tick, delta, time }
            | HitRating::Good { tick, delta, time }
            | HitRating::Miss { tick, delta, time } => Self {
                rating: 0,
                lane: tick.tick.lane() as i32,
                time: time as i32,
                time_frac: time.fract() as f32,
                delta: delta as i32,
                hold: match tick.tick {
                    kson::score_ticks::ScoreTick::Laser { lane: _, pos: _ } => 1,
                    kson::score_ticks::ScoreTick::Slam {
                        lane: _,
                        start: _,
                        end: _,
                    } => 0,
                    kson::score_ticks::ScoreTick::Chip { lane: _ } => 0,
                    kson::score_ticks::ScoreTick::Hold { lane: _ } => 1,
                },
            },
        };

        ret.rating = match value {
            HitRating::None => unreachable!(),
            HitRating::Crit {
                tick: _,
                delta: _,
                time: _,
            } => 2,
            HitRating::Good {
                tick: _,
                delta: _,
                time: _,
            } => 1,
            HitRating::Miss {
                tick: _,
                delta: _,
                time: _,
            } => 0,
        };

        Ok(ret)
    }
}

#[derive(Debug, TypeName, Clone, Serialize, UserData, Default)]
#[serde(rename_all = "camelCase")]
pub struct Score {
    pub gauge: f32,        //range 0.0 -> 1.0
    pub gauge_type: i32,   // 0 = normal, 1 = hard. Should be defined in constants sometime
    pub gauge_option: i32, // type specific, such as difficulty level for the same gauge type if available
    pub mirror: bool,
    pub random: bool,
    pub auto_flags: i32, //bits for autoplay settings, 0 = no autoplay
    pub score: i32,
    pub perfects: i32,
    pub goods: i32,
    pub misses: i32,
    pub badge: i32,
    pub timestamp: i32, //timestamp in POSIX time (seconds since Jan 1 1970 00:00:00 UTC)
    pub player_name: String,
    pub is_local: bool, // Whether this score was set locally
}

impl TealData for Score {}

pub struct SongResult {
    data: SongResultData,
    lua: Rc<Lua>,
    control_tx: Option<Sender<ControlMessage>>,
    close: bool,
}

impl Scene for SongResult {
    fn init(
        &mut self,
        load_lua: std::rc::Rc<
            dyn Fn(std::rc::Rc<Lua>, &'static str) -> anyhow::Result<generational_arena::Index>,
        >,
        app_control_tx: Sender<ControlMessage>,
        _mixer: Arc<DynamicMixerController<f32>>,
    ) -> anyhow::Result<()> {
        load_lua(self.lua.clone(), "result.lua")?;

        self.lua
            .globals()
            .set("result", self.lua.to_value(&self.data)?)?;

        if let Ok(result_set) = self.lua.globals().get::<_, Function>("result_set") {
            result_set.call::<_, ()>(())?;
        }
        self.control_tx = Some(app_control_tx);
        Ok(())
    }

    fn render_ui(&mut self, dt: f64) -> anyhow::Result<()> {
        let render_fn: Function = self.lua.globals().get("render")?;
        render_fn.call(dt / 1000.0)?;
        Ok(())
    }

    fn is_suspended(&self) -> bool {
        false
    }

    fn on_button_pressed(&mut self, button: crate::button_codes::UscButton) {
        if let UscButton::Start = button {
            self.close = true;
        }
    }

    fn debug_ui(&mut self, ctx: &egui::Context) -> anyhow::Result<()> {
        egui::Window::new("Song Results").show(ctx, |ui| {
            if ui.button("Close").clicked() {
                self.close = true;
            }
        });

        Ok(())
    }

    fn closed(&self) -> bool {
        self.close
    }

    fn name(&self) -> &str {
        "Song Result"
    }
}
