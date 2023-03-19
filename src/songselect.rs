use anyhow::{ensure, Result};
use generational_arena::Index;
use puffin::profile_function;
use serde::Serialize;
use std::{
    fmt::Debug,
    path::PathBuf,
    rc::Rc,
    sync::{mpsc::Sender, Arc, Mutex},
};
use tealr::{
    mlu::{
        mlua::{Function, Lua, LuaSerdeExt},
        TealData, UserData,
    },
    TypeName,
};

use crate::{
    button_codes::{LaserAxis, LaserState, UscButton},
    config::GameConfig,
    scene::{Scene, SceneData},
    song_provider::{FileSongProvider, NauticaSongProvider, SongProvider, SongProviderEvent},
    ControlMessage,
};

#[derive(Debug, TypeName, Clone, Serialize, UserData)]
#[serde(rename_all = "camelCase")]
pub struct Difficulty {
    pub jacket_path: PathBuf,
    pub level: u8,
    pub difficulty: u8, // 0 = nov, 1 = adv, etc.
    pub id: u64,        //unique static identifier
    pub effector: String,
    pub best_badge: i32,  //top badge for this difficulty
    pub scores: Vec<i32>, //array of all scores on this diff
}

impl TealData for Difficulty {
    fn add_fields<'lua, F: tealr::mlu::TealDataFields<'lua, Self>>(fields: &mut F) {
        fields.add_field_method_get("jacketPath", |_, diff| {
            Ok(diff
                .jacket_path
                .clone()
                .into_os_string()
                .into_string()
                .unwrap())
        });
        fields.add_field_method_get("level", |_, diff| Ok(diff.level));
        fields.add_field_method_get("difficulty", |_, diff| Ok(diff.difficulty));
        fields.add_field_method_get("id", |_, diff| Ok(diff.id));
        fields.add_field_method_get("effector", |_, diff| Ok(diff.effector.clone()));
        fields.add_field_method_get("bestBadge", |_, diff| Ok(diff.best_badge));
        fields.add_field_method_get("scores", |_, diff| Ok(diff.scores.clone()));
    }
}

#[derive(Debug, TypeName, UserData, Clone, Serialize)]
pub struct Song {
    pub title: String,
    pub artist: String,
    pub bpm: String,                   //ex. "170-200"
    pub id: u64,                       //unique static identifier
    pub difficulties: Vec<Difficulty>, //array of all difficulties for this song
}

//Keep tealdata for generating type definitions
impl TealData for Song {
    fn add_fields<'lua, F: tealr::mlu::TealDataFields<'lua, Self>>(fields: &mut F) {
        fields.add_field_method_get("title", |_, song| Ok(song.title.clone()));
        fields.add_field_method_get("artist", |_, song| Ok(song.artist.clone()));
        fields.add_field_method_get("bpm", |_, song| Ok(song.bpm.clone()));
        fields.add_field_method_get("id", |_, song| Ok(song.id));
        fields.add_field_method_get("difficulties", |_, song| Ok(song.difficulties.clone()));
    }
}

#[derive(Debug, Serialize, UserData)]
pub struct SongSelect {
    songs: Vec<Arc<Song>>,
    searchInputActive: bool, //true when the user is currently inputting search text
    searchText: String,      //current string used by the song search
    selected_index: i32,
    selected_diff_index: i32,
    #[serde(skip_serializing)]
    song_provider: Box<dyn SongProvider + Send>,
}

impl TealData for SongSelect {
    fn add_fields<'lua, F: tealr::mlu::TealDataFields<'lua, Self>>(fields: &mut F) {
        fields.add_field_method_get("songs", |_, _| Ok([] as [Song; 0]));
        fields.add_field_method_get("searchInputActive", |_, songwheel| {
            Ok(songwheel.searchInputActive)
        });
        fields.add_field_method_get("searchText", |_, songwheel| {
            Ok(songwheel.searchText.clone())
        });
        fields.add_field_method_get(
            "searchStatus",
            |_, _| -> Result<Option<String>, tealr::mlu::mlua::Error> { Ok(None) },
        );
    }
}

impl TypeName for SongSelect {
    fn get_type_parts() -> std::borrow::Cow<'static, [tealr::NamePart]> {
        use std::borrow::Cow;

        Cow::Borrowed(&[tealr::NamePart::Type(tealr::TealType {
            name: Cow::Borrowed("songwheel"),
            type_kind: tealr::KindOfType::External,
            generics: None,
        })])
    }
}

impl SongSelect {
    pub fn new() -> Self {
        let song_path = { GameConfig::get().unwrap().songs_path.clone() };

        let mut provider: Box<dyn SongProvider + Send> = if song_path == PathBuf::from("nautica") {
            Box::new(NauticaSongProvider::new())
        } else {
            Box::new(FileSongProvider::new())
        };

        let songs = if let Some(SongProviderEvent::SongsAdded(songs)) = provider.poll() {
            songs
        } else {
            vec![]
        };
        let charts = song_walker
            .into_iter()
            .filter_map(|a| a.ok())
            .filter(|e| e.file_type().is_file())
            .filter_map(|e| {
                if let Ok(data) = std::fs::read_to_string(e.path()) {
                    Some((e, data))
                } else {
                    None
                }
            })
            .filter_map(|(dir, data)| {
                if let Ok(chart) = kson::Chart::from_ksh(&data) {
                    Some((dir, chart))
                } else {
                    None
                }
            });

        let song_folders = charts.fold(
            HashMap::<PathBuf, Vec<(PathBuf, Chart)>>::new(),
            |mut acc, (dir, chart)| {
                if let Some(parent_folder) = dir.path().parent() {
                    acc.entry(parent_folder.to_path_buf())
                        .and_modify(|v| v.push((dir.clone().into_path(), chart.clone())))
                        .or_insert_with(|| vec![(dir.into_path(), chart)]);
                }
                acc
            },
        );

        let mut songs: Vec<Song> = song_folders
            .into_iter()
            .enumerate()
            .map(|(id, (song_folder, charts))| Song {
                title: charts[0].1.meta.title.clone(),
                artist: charts[0].1.meta.artist.clone(),
                bpm: charts[0].1.meta.disp_bpm.clone(),
                id: id as i32,
                path: song_folder.clone(),
                difficulties: charts
                    .iter()
                    .enumerate()
                    .map(|(id, (p, c))| Difficulty {
                        best_badge: 0,
                        difficulty: c.meta.difficulty,
                        effector: c.meta.chart_author.clone(),
                        id: id as i32,
                        jacket_path: song_folder.join(&c.meta.jacket_filename),
                        level: c.meta.level,
                        scores: vec![99],
                        file_path: p.clone(),
                    })
                    .collect(),
            })
            .collect();

        songs.sort_by_key(|s| s.title.to_lowercase());

        Self {
            songs,
            searchInputActive: false,
            searchText: String::new(),
            selected_index: 0,
            selected_diff_index: 0,
            song_provider: provider,
        }
    }
}

impl SceneData for SongSelect {
    fn make_scene(self: Box<Self>) -> Box<dyn Scene> {
        Box::new(SongSelectScene::new(self))
    }
}

pub struct SongSelectScene {
    state: Arc<Mutex<Box<SongSelect>>>,
    lua: Rc<Lua>,
    background_lua: Rc<Lua>,
    program_control: Option<Sender<ControlMessage>>,
    song_advance: f32,
    diff_advance: f32,
}

impl SongSelectScene {
    pub fn new(song_select: Box<SongSelect>) -> Self {
        Self {
            background_lua: Rc::new(Lua::new()),
            lua: Rc::new(Lua::new()),
            state: Arc::new(Mutex::new(song_select)),
            program_control: None,
            diff_advance: 0.0,
            song_advance: 0.0,
        }
    }
}

impl Scene for SongSelectScene {
    fn render_ui(&mut self, dt: f64) -> Result<bool> {
        profile_function!();
        let render_bg: Function = self.background_lua.globals().get("render")?;
        render_bg.call(dt / 1000.0)?;

        let render_wheel: Function = self.lua.globals().get("render")?;
        render_wheel.call(dt / 1000.0)?;

        Ok(false)
    }

    fn is_suspended(&self) -> bool {
        false
    }

    fn debug_ui(&mut self, ctx: &three_d::egui::Context) -> Result<()> {
        use three_d::egui;
        let set_song_idx: Function = self.lua.globals().get("set_index").unwrap();
        if let Ok(state) = &mut self.state.lock() {
            let song_count = state.songs.len();

            egui::Window::new("Songsel").show(ctx, |ui| {
                egui::Grid::new("songsel-grid")
                    .num_columns(2)
                    .striped(true)
                    .show(ui, |ui| -> Result<()> {
                        if song_count > 0 {
                            ui.label("Song");
                            if ui
                                .add(
                                    egui::DragValue::new(&mut state.selected_index)
                                        .clamp_range(0..=(song_count - 1))
                                        .speed(0.1),
                                )
                                .changed()
                            {
                                set_song_idx.call::<_, i32>(state.selected_index + 1)?;
                            }

                            ui.end_row();
                            if ui.button("Start").clicked() {
                                let song = state.songs[state.selected_index as usize].clone();
                                let diff = state.selected_diff_index as usize;

                                let loader = state
                                    .song_provider
                                    .load_song(song.id, song.difficulties[diff].id);
                                ensure!(self
                                    .program_control
                                    .as_ref()
                                    .unwrap()
                                    .send(ControlMessage::Song { diff, song, loader })
                                    .is_ok());
                            }
                            ui.end_row();
                            Ok(())
                        } else {
                            ui.label("No songs");
                            Ok(())
                        }
                    })
            });
        }

        Ok(())
    }

    fn init(
        &mut self,
        load_lua: Box<dyn Fn(Rc<Lua>, &'static str) -> anyhow::Result<Index>>,
        app_control_tx: Sender<ControlMessage>,
    ) -> anyhow::Result<()> {
        self.lua
            .globals()
            .set("songwheel", self.lua.to_value(&self.state)?)?;
        self.program_control = Some(app_control_tx);
        load_lua(self.lua.clone(), "songselect/songwheel.lua")?;
        load_lua(self.background_lua.clone(), "songselect/background.lua")?;
        Ok(())
    }

    fn tick(&mut self, _dt: f64, knob_state: LaserState) -> Result<bool> {
        self.song_advance += LaserAxis::from(knob_state.get(kson::Side::Right)).delta;
        self.diff_advance += LaserAxis::from(knob_state.get(kson::Side::Left)).delta;

        const KNOB_NAV_THRESHOLD: f32 = std::f32::consts::PI / 3.0;
        let song_advance_steps = (self.song_advance / KNOB_NAV_THRESHOLD).trunc() as i32;
        self.song_advance -= song_advance_steps as f32 * KNOB_NAV_THRESHOLD;
        if let Ok(state) = &mut self.state.lock() {
            let mut songs_dirty = false;
            while let Some(provider_event) = state.song_provider.poll() {
                songs_dirty = true;
                match provider_event {
                    SongProviderEvent::SongsAdded(mut new_songs) => {
                        state.songs.append(&mut new_songs)
                    }
                    SongProviderEvent::SongsRemoved(removed_ids) => {
                        state.songs.retain(|s| !removed_ids.contains(&s.id))
                    }
                    SongProviderEvent::OrderChanged(_) => todo!(),
                }
            }

            if songs_dirty {
                self.lua
                    .globals()
                    .set("songwheel", self.lua.to_value(state.as_ref())?)?;
            }

            if !state.songs.is_empty() {
                state.selected_index = (state.selected_index + song_advance_steps)
                    .rem_euclid(state.songs.len() as i32);
                let song_idx = state.selected_index as usize;
                let song_id = state.songs[song_idx].id;
                state.song_provider.set_current_index(song_id);

                if song_advance_steps != 0 {
                    let set_song_idx: Function = self.lua.globals().get("set_index").unwrap();

                    set_song_idx.call::<_, ()>(state.selected_index + 1)?;
                }
            }
        }

        Ok(false)
    }

    fn on_event(&mut self, _event: &mut three_d::Event) {}

    fn on_button_pressed(&mut self, button: crate::button_codes::UscButton) {
        if let UscButton::Start = button {
            let state = self.state.lock().unwrap();
            if let Some(pc) = &self.program_control {
                let song = state.songs[state.selected_index as usize].clone();
                let diff = state.selected_diff_index as usize;
                let loader = state
                    .song_provider
                    .load_song(song.id, song.difficulties[diff].id);
                pc.send(ControlMessage::Song { diff, loader, song });
            }
        }
    }

    fn suspend(&mut self) {}

    fn resume(&mut self) {}
}
