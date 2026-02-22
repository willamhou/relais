use std::collections::HashMap;

use crate::adapter::Adapter;
use crate::types::SiteManifest;

pub struct Router {
    adapters: HashMap<String, Box<dyn Adapter>>,
}

impl Router {
    pub fn new() -> Self {
        Self {
            adapters: HashMap::new(),
        }
    }

    pub fn register(&mut self, adapter: Box<dyn Adapter>) {
        let id = adapter.manifest().id.clone();
        self.adapters.insert(id, adapter);
    }

    pub fn get(&self, site_id: &str) -> Option<&dyn Adapter> {
        self.adapters.get(site_id).map(|a| a.as_ref())
    }

    pub fn sites(&self) -> Vec<SiteManifest> {
        self.adapters.values().map(|a| a.manifest()).collect()
    }
}

impl Default for Router {
    fn default() -> Self {
        Self::new()
    }
}
