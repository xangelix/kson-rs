#![allow(dead_code)]
use crate::{
    parameter::{BoolParameter, EffectParameter},
    Chart, Interval, Side, Track,
};

use kson_effect_param_macro::Effect;
use serde::{Deserialize, Serialize};
use thiserror::Error;

use std::f32;

#[cfg(feature = "schema")]
use schemars::JsonSchema;

trait Effect {
    fn derive(&self, key: &str, param: &str) -> Self;
    fn param_list() -> &'static [&'static str];
}

#[derive(Deserialize, Serialize, Clone, Effect)]
#[serde(tag = "type", content = "v")]
#[serde(rename_all = "snake_case")]
pub enum AudioEffect {
    ReTrigger(ReTrigger),
    Gate(Gate),
    Flanger(Flanger),
    PitchShift(PitchShift),
    BitCrusher(BitCrusher),
    Phaser(Phaser),
    Wobble(Wobble),
    TapeStop(TapeStop),
    Echo(Echo),
    SideChain(SideChain),
    AudioSwap(String),
    HighPassFilter(HighPassFilter),
    LowPassFilter(LowPassFilter),
    PeakingFilter(PeakingFilter),
}
#[derive(Debug, Error)]
pub enum EffectError {
    #[error("Tried to apply effect changes with differing effect types.")]
    EffectTypeMismatchError,
}

impl Effect for String {
    fn derive(&self, _key: &str, param: &str) -> Self {
        param.to_string()
    }

    fn param_list() -> &'static [&'static str] {
        &[]
    }
}

#[derive(Deserialize, Serialize, Clone, Default, Effect)]
pub struct ReTrigger {
    pub update_period: EffectParameter<f32>,
    pub wave_length: EffectParameter<f32>,
    pub rate: EffectParameter<f32>,
    pub update_trigger: BoolParameter,
    pub mix: EffectParameter<f32>,
}

#[derive(Deserialize, Serialize, Clone, Default, Effect)]
pub struct Gate {
    pub wave_length: EffectParameter<f32>,
    pub rate: EffectParameter<f32>,
    pub mix: EffectParameter<f32>,
}

#[derive(Deserialize, Serialize, Clone, Default, Effect)]
pub struct Flanger {
    pub period: EffectParameter<f32>,
    pub delay: EffectParameter<i64>,
    pub depth: EffectParameter<i64>,
    pub feedback: EffectParameter<f32>,
    pub stereo_width: EffectParameter<f32>,
    pub vol: EffectParameter<f32>,
    pub mix: EffectParameter<f32>,
}

#[derive(Deserialize, Serialize, Clone, Default, Effect)]
pub struct PitchShift {
    pub pitch: EffectParameter<f32>,
    pub pitch_quantize: BoolParameter,
    pub chunk_size: EffectParameter<i64>,
    pub overlap: EffectParameter<f32>,
    pub mix: EffectParameter<f32>,
}

#[derive(Deserialize, Serialize, Clone, Default, Effect)]
pub struct BitCrusher {
    pub reduction: EffectParameter<i64>,
    pub mix: EffectParameter<f32>,
}

#[derive(Deserialize, Serialize, Clone, Default, Effect)]
pub struct Phaser {
    pub period: EffectParameter<f32>,
    pub stage: EffectParameter<i64>,
    pub lo_freq: EffectParameter<f32>,
    pub hi_freq: EffectParameter<f32>,
    pub q: EffectParameter<f32>,
    pub feedback: EffectParameter<f32>,
    pub stereo_width: EffectParameter<f32>,
    pub mix: EffectParameter<f32>,
}

#[derive(Deserialize, Serialize, Clone, Default, Effect)]
pub struct Wobble {
    pub wave_length: EffectParameter<f32>,
    pub lo_freq: EffectParameter<f32>,
    pub hi_freq: EffectParameter<f32>,
    pub q: EffectParameter<f32>,
    pub mix: EffectParameter<f32>,
}

#[derive(Deserialize, Serialize, Clone, Default, Effect)]
pub struct TapeStop {
    pub speed: EffectParameter<f32>,
    pub trigger: BoolParameter,
    pub mix: EffectParameter<f32>,
}

#[derive(Deserialize, Serialize, Clone, Default, Effect)]
pub struct Echo {
    pub update_period: EffectParameter<f32>,
    pub wave_length: EffectParameter<f32>,
    pub update_trigger: BoolParameter,
    pub feedback_level: EffectParameter<f32>,
    pub mix: EffectParameter<f32>,
}

#[derive(Deserialize, Serialize, Clone, Default, Effect)]
pub struct SideChain {
    pub period: EffectParameter<f32>,
    pub hold_time: EffectParameter<f32>,
    pub attack_time: EffectParameter<f32>,
    pub release_time: EffectParameter<f32>,
    pub ratio: EffectParameter<f32>,
}

#[derive(Deserialize, Serialize, Clone, Default, Effect)]
pub struct HighPassFilter {
    pub v: EffectParameter<f32>,
    pub freq: EffectParameter<f32>,
    pub freq_max: EffectParameter<f32>,
    pub q: EffectParameter<f32>,
    pub delay: EffectParameter<f32>,
    pub mix: EffectParameter<f32>,
}

#[derive(Deserialize, Serialize, Clone, Default, Effect)]
pub struct LowPassFilter {
    pub v: EffectParameter<f32>,
    pub freq: EffectParameter<f32>,
    pub freq_max: EffectParameter<f32>,
    pub q: EffectParameter<f32>,
    pub delay: EffectParameter<f32>,
    pub mix: EffectParameter<f32>,
}

#[derive(Deserialize, Serialize, Clone, Default, Effect)]
pub struct PeakingFilter {
    pub v: EffectParameter<f32>,
    pub freq: EffectParameter<f32>,
    pub freq_max: EffectParameter<f32>,
    pub q: EffectParameter<f32>,
    pub delay: EffectParameter<f32>,
    pub mix: EffectParameter<f32>,
}

pub struct EffectInterval {
    pub interval: Interval,
    pub effect: AudioEffect,
    pub track: Option<Track>,
    pub dom: bool,
}

impl Chart {
    pub fn get_effect_tracks(&self) -> Vec<EffectInterval> {
        let audio_effect = if let Some(a) = &self.audio.audio_effect {
            a
        } else {
            return vec![];
        };
        let sides = [Side::Left, Side::Right];
        let mut result = vec![];
        for (name, root_effect) in &audio_effect.fx.def {
            if let Some(long_event) = audio_effect.fx.long_event.get(name) {
                for fx_side in 0..2 {
                    for event in &long_event[fx_side] {
                        if let Ok(note_index) =
                            self.note.fx[fx_side].binary_search_by_key(&event.0, |n| n.y)
                        {
                            let mut effect = audio_effect
                                .fx
                                .param_change
                                .get(name)
                                .map(|params_map| {
                                    params_map
                                        .iter()
                                        .flat_map(|(key, param_changes)| {
                                            param_changes
                                                .iter()
                                                .take_while(|(tick, _)| *tick <= event.0)
                                                .map(move |(tick, param)| (key, tick, param))
                                        })
                                        .fold(root_effect.clone(), |a, (key, _, param)| {
                                            a.derive(key, param)
                                        })
                                })
                                .unwrap_or_else(|| root_effect.clone());

                            if let Some(long_params) = &event.1 {
                                effect = long_params
                                    .iter()
                                    .fold(effect, |e, (key, param)| e.derive(key, param));
                            }
                            result.push(EffectInterval {
                                interval: self.note.fx[fx_side][note_index],
                                effect,
                                track: Some(Track::FX(sides[fx_side])),
                                dom: true,
                            });
                        }
                    }
                }
            }
        }

        for (i, side) in sides.iter().enumerate() {
            let intervals = self.note.laser[i].iter().map(|ls| Interval {
                y: ls.0,
                l: ls.1.last().map(|s| s.ry).unwrap_or(0),
            });

            for interval in intervals {
                if let Some((effect_key, Some(effect))) = audio_effect
                    .laser
                    .pulse_event
                    .iter()
                    .flat_map(|(a, b)| b.iter().map(move |(c, _)| (a, c)))
                    .take_while(|(_, tick)| **tick <= interval.y)
                    .max_by_key(|(_, tick)| **tick)
                    .map(|(k, _)| (k, audio_effect.laser.def.get(k)))
                {
                    let effect = audio_effect
                        .laser
                        .param_change
                        .get(effect_key)
                        .map(|a| {
                            a.iter()
                                .flat_map(|(param_name, changes)| {
                                    changes
                                        .iter()
                                        .map(move |(tick, param)| (*tick, param_name, param))
                                })
                                .take_while(|(tick, _, _)| *tick <= interval.y)
                                .fold(effect.clone(), |e, (_, key, param)| e.derive(key, param))
                        })
                        .unwrap_or_else(|| effect.clone());

                    result.push(EffectInterval {
                        interval,
                        effect,
                        track: Some(Track::Laser(*side)),
                        dom: true,
                    })
                }
            }

            //TODO: Mid-section effect changes
        }

        result.sort_by_key(|e| e.interval.y);
        result
    }
}