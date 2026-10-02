use serde::{Deserialize, Serialize};
use std::collections::HashMap;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Scenario {
    pub name: String,
    pub description: String,
    pub state: HashMap<String, toml::Value>,
    pub indicators: HashMap<String, toml::Value>,
    #[serde(default)]
    pub map_info: HashMap<String, toml::Value>,
    #[serde(default)]
    pub map_objects: Vec<HashMap<String, toml::Value>>,
}

impl Default for Scenario {
    fn default() -> Self {
        Self {
            name: "empty".to_string(),
            description: "Empty scenario".to_string(),
            state: HashMap::new(),
            indicators: HashMap::new(),
            map_info: HashMap::new(),
            map_objects: Vec::new(),
        }
    }
}
