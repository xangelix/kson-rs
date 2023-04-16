use std::{collections::HashMap, fs::File, io::Read, path::PathBuf, sync::RwLock};

use log::{error, info};
use once_cell::sync::OnceCell;
use serde::{Deserialize, Serialize};

use crate::skin_settings::{SkinSettingEntry, SkinSettingValue};

#[derive(Debug, Deserialize, Serialize)]
pub struct GameConfig {
    #[serde(skip_serializing, skip_deserializing)]
    config_file: PathBuf,
    pub songs_path: PathBuf,
    pub skin: String,
    #[serde(skip_serializing, skip_deserializing)]
    pub skin_settings: HashMap<String, SkinSettingValue>,
}

impl Default for GameConfig {
    fn default() -> Self {
        Self {
            config_file: PathBuf::from_iter([".", "Main.cfg"]),
            songs_path: PathBuf::from_iter([".", "songs"]),
            skin: "Default".into(),
            skin_settings: HashMap::new(),
        }
    }
}

static INSTANCE: OnceCell<RwLock<GameConfig>> = OnceCell::new();

impl GameConfig {
    pub fn get() -> Option<std::sync::RwLockReadGuard<'static, GameConfig>> {
        INSTANCE.get().and_then(|i| i.read().ok())
    }
    pub fn get_mut() -> Option<std::sync::RwLockWriteGuard<'static, GameConfig>> {
        INSTANCE.get().and_then(|i| i.write().ok())
    }

    fn skin_config_path(&self) -> PathBuf {
        let mut skin_config_path = self.config_file.clone();
        skin_config_path.pop();
        skin_config_path.push("skins");
        skin_config_path.push(&self.skin);
        skin_config_path.push("skin_config.cfg");
        skin_config_path
    }

    fn init_skin_settings(&mut self) -> anyhow::Result<()> {
        let definition_path = self
            .skin_config_path()
            .with_file_name("config-definitions.json");

        let file = File::open(definition_path)?;
        let definitions: Vec<SkinSettingEntry> = serde_json::from_reader(file)?;

        for def in definitions {
            let entry = match def {
                SkinSettingEntry::Selection {
                    default,
                    label: _,
                    name,
                    values: _,
                } => (name, SkinSettingValue::Text(default)),
                SkinSettingEntry::Text {
                    default,
                    label: _,
                    name,
                    secret: _,
                } => (name, SkinSettingValue::Text(default)),
                SkinSettingEntry::Color {
                    default,
                    label: _,
                    name,
                } => (name, SkinSettingValue::Color(default)),
                SkinSettingEntry::Bool {
                    default,
                    label: _,
                    name,
                } => (name, SkinSettingValue::Bool(default)),
                SkinSettingEntry::Float {
                    default,
                    label: _,
                    name,
                    min: _,
                    max: _,
                } => (name, SkinSettingValue::Float(default)),
                SkinSettingEntry::Integer {
                    default,
                    label: _,
                    name,
                    min: _,
                    max: _,
                } => (name, SkinSettingValue::Integer(default)),
                _ => continue,
            };

            self.skin_settings.insert(entry.0, entry.1);
        }

        let mut file = File::open(self.skin_config_path())?;
        let mut skin_settings_string = String::new();
        file.read_to_string(&mut skin_settings_string)?;

        let skin_settings: HashMap<String, SkinSettingValue> =
            toml::from_str(&skin_settings_string)?;

        for (k, v) in skin_settings {
            self.skin_settings.insert(k, v);
        }

        Ok(())
    }

    pub fn init(path: PathBuf) {
        info!("Loading game config from: {:?}", &path);
        let file_content =
            std::fs::read_to_string(&path).map(|str| toml::from_str::<GameConfig>(&str));

        match file_content {
            Ok(Ok(mut config)) => {
                config.config_file = path;
                INSTANCE.set(RwLock::new(config));
            }
            Ok(Err(e)) => {
                error!("{}", e);
                INSTANCE.set(RwLock::new(GameConfig {
                    config_file: path,
                    songs_path: PathBuf::from_iter([".", "songs"]),
                    skin: "Default".into(),
                    ..Default::default()
                }));
            }
            Err(e) => {
                error!("{}", e);
                INSTANCE.set(RwLock::new(GameConfig {
                    config_file: path,
                    songs_path: PathBuf::from_iter([".", "songs"]),
                    skin: "Default".into(),
                    ..Default::default()
                }));
            }
        }

        if let Some(mut config) = GameConfig::get_mut() {
            if let Some(err) = config.init_skin_settings().err() {
                log::warn!("{:?}", err)
            };
        }
    }

    pub fn save(&self) {
        info!("Saving config");

        if let Ok(data) = toml::to_string_pretty(self) {
            std::fs::write(&self.config_file, data);
        }

        if let Ok(data) = toml::to_string_pretty(&self.skin_settings) {
            std::fs::write(&self.skin_config_path(), data);
        }
    }
}
