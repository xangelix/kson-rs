use std::{collections::HashMap, fs::FileType, path::PathBuf};

use kson::{Chart, Ksh};
use tealr::{
    mlu::{
        mlua::{Function, Lua, ToLua},
        TealData, UserData,
    },
    TypeName,
};

#[derive(Debug, TypeName, UserData, Clone)]
pub struct Difficulty {
    jacket_path: PathBuf,
    level: u8,
    difficulty: u8, // 0 = nov, 1 = adv, etc.
    id: i32,        //unique static identifier
    effector: String,
    best_badge: i32,  //top badge for this difficulty
    scores: Vec<i32>, //array of all scores on this diff
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

#[derive(Debug, TypeName, UserData, Clone)]
pub struct Song {
    title: String,
    artist: String,
    bpm: String,                   //ex. "170-200"
    id: i32,                       //unique static identifier
    path: PathBuf,                 //folder the song is stored in
    difficulties: Vec<Difficulty>, //array of all difficulties for this song
}
//TODO: Investigate lifetimes
impl TealData for Song {
    fn add_fields<'lua, F: tealr::mlu::TealDataFields<'lua, Self>>(fields: &mut F) {
        fields.add_field_method_get("title", |_, song| Ok(song.title.clone()));
        fields.add_field_method_get("artist", |_, song| Ok(song.artist.clone()));
        fields.add_field_method_get("bpm", |_, song| Ok(song.bpm.clone()));
        fields.add_field_method_get("id", |_, song| Ok(song.id));
        fields.add_field_method_get("path", |_, song| {
            Ok(song.path.clone().into_os_string().into_string().unwrap())
        });
        fields.add_field_method_get("difficulties", |_, song| Ok(song.difficulties.clone()));
    }
}

#[derive(Debug, UserData)]
pub struct SongSelect {
    songs: Vec<Song>,
    searchInputActive: bool, //true when the user is currently inputting search text
    searchText: String,      //current string used by the song search
    selected_index: i32,
}

impl TealData for SongSelect {
    fn add_fields<'lua, F: tealr::mlu::TealDataFields<'lua, Self>>(fields: &mut F) {
        fields.add_field_method_get("songs", |_, songwheel| Ok(songwheel.songs.clone()));
        fields.add_field_method_get("searchInputActive", |_, songwheel| {
            Ok(songwheel.searchInputActive)
        });
        fields.add_field_method_get("searchText", |_, songwheel| {
            Ok(songwheel.searchText.clone())
        });
        fields.add_field_method_get(
            "searchStatus",
            |_, songwheel| -> Result<Option<String>, tealr::mlu::mlua::Error> { Ok(None) },
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
    pub fn new(song_path: impl std::convert::AsRef<std::path::Path>) -> Self {
        let song_walker = walkdir::WalkDir::new(song_path);

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
            HashMap::<PathBuf, Vec<Chart>>::new(),
            |mut acc, (dir, chart)| {
                if let Some(parent_folder) = dir.path().parent() {
                    acc.entry(parent_folder.to_path_buf())
                        .and_modify(|v| v.push(chart.clone()))
                        .or_insert_with(|| vec![chart]);
                }
                acc
            },
        );

        Self {
            songs: song_folders
                .into_iter()
                .enumerate()
                .map(|(id, (song_folder, charts))| Song {
                    title: charts[0].meta.title.clone(),
                    artist: charts[0].meta.artist.clone(),
                    bpm: charts[0].meta.disp_bpm.clone(),
                    id: id as i32,
                    path: song_folder.clone(),
                    difficulties: charts
                        .iter()
                        .enumerate()
                        .map(|(id, c)| Difficulty {
                            best_badge: 0,
                            difficulty: c.meta.difficulty,
                            effector: c.meta.chart_author.clone(),
                            id: id as i32,
                            jacket_path: song_folder.join(&c.meta.jacket_filename),
                            level: c.meta.level,
                            scores: vec![99],
                        })
                        .collect(),
                })
                .collect(),
            searchInputActive: false,
            searchText: String::new(),
            selected_index: 0,
        }
    }

    pub fn debug_ui(&mut self, ctx: &three_d::egui::Context, lua: &Lua) {
        use three_d::egui;
        let set_song_idx: Function = lua.globals().get("set_index").unwrap();

        egui::Window::new("Songsel").show(ctx, |ui| {
            egui::Grid::new("songsel-grid")
                .num_columns(2)
                .striped(true)
                .show(ui, |ui| {
                    ui.label("Song");
                    if ui
                        .add(
                            egui::DragValue::new(&mut self.selected_index)
                                .clamp_range(1..=self.songs.len())
                                .speed(0.1),
                        )
                        .changed()
                    {
                        set_song_idx.call::<_, i32>(self.selected_index);
                    }

                    ui.end_row()
                })
        });
    }
}
