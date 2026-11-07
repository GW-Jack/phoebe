//! Maps a verified package graph to one collision-checked module namespace.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

pub(super) struct SourceSpec {
    pub name: String,
    pub path: PathBuf,
    pub source: Option<String>,
    pub package_root: String,
    pub available: HashMap<String, String>,
}

fn prefix(locator: &str) -> String {
    locator.replace('/', "__")
}

fn symbol(locator: &str, module: &str, root: &str) -> String {
    if locator == "." {
        module.to_owned()
    } else if module == root {
        prefix(locator)
    } else {
        format!("{}__{module}", prefix(locator))
    }
}

pub(super) fn sources(
    path: &Path,
    expected_manifest: &[u8],
) -> Result<(String, Vec<SourceSpec>), String> {
    let graph = phoebe_package::resolve(path)?;
    if graph.packages[0].manifest_bytes != expected_manifest {
        return Err("root manifest changed while resolving package graph".into());
    }
    let root = graph.packages[0].root.clone();
    let mut seen = HashSet::new();
    let mut sources = Vec::new();
    for package in &graph.packages {
        let mut available = package
            .modules
            .keys()
            .map(|name| (name.clone(), symbol(&package.locator, name, &package.root)))
            .collect::<HashMap<_, _>>();
        let child_prefix = if package.locator == "." {
            String::new()
        } else {
            format!("{}/", package.locator)
        };
        for child in &graph.packages {
            if child.locator != "."
                && let Some(alias) = child.locator.strip_prefix(&child_prefix)
                && !alias.is_empty()
                && !alias.contains('/')
            {
                available.insert(alias.to_owned(), prefix(&child.locator));
            }
        }
        let package_root = symbol(&package.locator, &package.root, &package.root);
        for (module, source) in &package.modules {
            let name = symbol(&package.locator, module, &package.root);
            if !seen.insert(name.clone()) {
                return Err(format!("module namespace collision at `{name}`"));
            }
            sources.push(SourceSpec {
                name,
                path: source.path.clone(),
                source: Some(
                    String::from_utf8(source.bytes.clone())
                        .map_err(|error| format!("{}: {error}", source.path.display()))?,
                ),
                package_root: package_root.clone(),
                available: available.clone(),
            });
        }
    }
    Ok((root, sources))
}
