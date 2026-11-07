//! Reuses parsed modules and computes conservative interface invalidation.

use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

use super::{Loaded, Module, ModuleInterface, load_with_cache};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LoadReport {
    pub parsed: Vec<String>,
    pub reused: Vec<String>,
    pub changed_interfaces: Vec<String>,
    /// Conservative source-level dependency invalidation. This is not a
    /// checked Core cache hit: the current compiler still lowers every module.
    pub affected: Vec<String>,
}

#[derive(Default)]
pub struct ProjectSession {
    modules: BTreeMap<String, Module>,
    interfaces: BTreeMap<String, ModuleInterface>,
}

impl ProjectSession {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn load(&mut self, path: &Path) -> Result<(Loaded, LoadReport), String> {
        let super::LoadParts {
            loaded,
            modules,
            mut parsed,
        } = load_with_cache(path, Some(&self.modules))?;
        parsed.sort();
        let parsed_set = parsed.iter().map(String::as_str).collect::<BTreeSet<_>>();
        let reused = modules
            .keys()
            .filter(|name| !parsed_set.contains(name.as_str()))
            .cloned()
            .collect();
        let current = loaded
            .interfaces
            .iter()
            .map(|interface| (interface.name.clone(), interface.clone()))
            .collect::<BTreeMap<_, _>>();
        let changed_interfaces = current
            .iter()
            .filter(|(name, interface)| self.interfaces.get(*name) != Some(*interface))
            .map(|(name, _)| name.clone())
            .collect::<Vec<_>>();
        let mut affected = parsed.iter().cloned().collect::<BTreeSet<_>>();
        let mut frontier = changed_interfaces.iter().cloned().collect::<BTreeSet<_>>();
        loop {
            let mut added = false;
            for module in modules.values() {
                if !frontier.contains(&module.name)
                    && module
                        .ast
                        .imports
                        .iter()
                        .any(|import| frontier.contains(&import.name))
                {
                    frontier.insert(module.name.clone());
                    affected.insert(module.name.clone());
                    added = true;
                }
            }
            if !added {
                break;
            }
        }
        self.modules = modules;
        self.interfaces = current;
        Ok((
            loaded,
            LoadReport {
                parsed,
                reused,
                changed_interfaces,
                affected: affected.into_iter().collect(),
            },
        ))
    }
}
