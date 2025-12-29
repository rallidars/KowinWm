use serde::Deserialize;

#[derive(Deserialize)]
pub struct Config {
    pub border: Border,
}

#[derive(Deserialize)]
pub struct Border {
    pub thickness: i32,
    pub gap: i32,
    pub active: u32,
    pub inactive: u32,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            border: Border {
                thickness: 2,
                gap: 2,
                active: 0x8B4000,
                inactive: 0x2A2A2A,
            },
        }
    }
}

impl Config {
    pub fn get_config() -> Option<Config> {
        let home_path = std::env::var("HOME").ok()?;
        let config_path = ".config/kowinwm/config.toml";
        let config_path = format!("{}/{}", home_path, config_path);
        let file_data = std::fs::read_to_string(config_path).ok()?;
        toml::from_str(&file_data).ok()
    }
}
