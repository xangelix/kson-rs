use std::{collections::HashSet, fmt::Debug, sync::Arc};

use kson::Chart;

use crate::{results::Score, songselect::Song};
mod files;
mod nautica;

#[derive(Debug)]
pub enum SongProviderEvent {
    SongsAdded(Vec<Arc<Song>>),
    SongsRemoved(HashSet<u64>),
    OrderChanged(Vec<u64>),
}

#[derive(Debug)]
pub enum ScoreProviderEvent {
    NewScore(u64, Score), //(diff.id, score)
}

pub enum ScoreFilter {
    Local,
    Online,
    Mixed,
}

pub enum SortDir {
    Asc,
    Desc,
}

pub enum SongSort {
    Title(SortDir),
}

pub enum SongFilter {
    Level(u8),
    Folder(String),
    Collection(String),
}

pub trait SongProvider: Debug {
    fn poll(&mut self) -> Option<SongProviderEvent>;
    fn set_search(&mut self, query: &str);
    fn set_sort(&mut self, sort: SongSort);
    fn set_filter(&mut self, filter: SongFilter);
    fn set_current_index(&mut self, index: u64);
    fn load_song(
        &self,
        song_index: u64,
        diff_index: u64,
    ) -> Box<dyn FnOnce() -> (Chart, Box<dyn rodio::Source<Item = f32> + Send>) + Send>;
}

pub trait ScoreProvider: Debug {
    fn poll(&mut self) -> Option<ScoreProviderEvent>;
    fn get_scores(&mut self, id: u64) -> Vec<Score>;
    fn insert_score(&mut self, id: u64, score: Score) -> anyhow::Result<()>;
}

pub use files::FileSongProvider;
pub use nautica::NauticaSongProvider;
