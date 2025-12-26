use serde::Deserialize;

#[derive(Deserialize)]
pub struct Config {
    border: Border,
}

#[derive(Deserialize)]
struct Border {
    thickness: i32,
    gap: i32,
    active: u32,
    inactive: u32,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            border: Border {
                thickness: 2,
                gap: 2,
                active: 222222,
                inactive: 000000,
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
    pub fn border_thickness(&self) -> i32 {
        self.border.thickness
    }
    pub fn border_gap(&self) -> i32 {
        self.border.gap
    }
    pub fn border_acitve_color(&self) -> u32 {
        self.border.active
    }
    pub fn border_inacitve_color(&self) -> u32 {
        self.border.inactive
    }
}
