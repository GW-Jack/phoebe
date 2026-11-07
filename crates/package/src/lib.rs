//! Deterministic local package graph and content-pinned lockfile.

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Component, Path, PathBuf};

use sha2::{Digest, Sha256};

#[derive(Clone, Debug)]
pub struct Package {
    pub locator: String,
    pub name: String,
    pub version: String,
    pub root: String,
    pub manifest: PathBuf,
    pub modules: BTreeMap<String, PathBuf>,
}

#[derive(Clone, Debug)]
pub struct Graph {
    /// The root appears first. Dependencies follow in deterministic DFS order.
    pub packages: Vec<Package>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct Pin {
    locator: String,
    name: String,
    version: String,
    sha256: String,
}

fn identifier(name: &str) -> bool {
    !name.is_empty()
        && name.as_bytes()[0].is_ascii_alphabetic()
        && name
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'_')
}

fn table<'a>(parent: &'a toml::Table, key: &str) -> Result<&'a toml::Table, String> {
    parent
        .get(key)
        .and_then(toml::Value::as_table)
        .ok_or_else(|| format!("manifest requires [{key}]"))
}

fn string<'a>(parent: &'a toml::Table, key: &str) -> Result<&'a str, String> {
    parent
        .get(key)
        .and_then(toml::Value::as_str)
        .ok_or_else(|| format!("requires string `{key}`"))
}

fn manifest_path(path: &Path) -> PathBuf {
    if path.is_dir() {
        path.join("phoebe.toml")
    } else {
        path.to_owned()
    }
}

fn update_hash(hash: &mut Sha256, label: &str, bytes: &[u8]) {
    hash.update((label.len() as u64).to_le_bytes());
    hash.update(label.as_bytes());
    hash.update((bytes.len() as u64).to_le_bytes());
    hash.update(bytes);
}

struct Walk {
    packages: Vec<Package>,
    pins: BTreeMap<String, Pin>,
    active: BTreeSet<PathBuf>,
}

impl Walk {
    fn visit(
        &mut self,
        path: &Path,
        locator: String,
        required_version: Option<&str>,
    ) -> Result<(), String> {
        let manifest = fs::canonicalize(manifest_path(path))
            .map_err(|error| format!("{}: {error}", path.display()))?;
        if !self.active.insert(manifest.clone()) {
            return Err(format!(
                "package dependency cycle at {}",
                manifest.display()
            ));
        }
        let result = self.visit_open(&manifest, locator, required_version);
        self.active.remove(&manifest);
        result
    }

    fn visit_open(
        &mut self,
        manifest: &Path,
        locator: String,
        required_version: Option<&str>,
    ) -> Result<(), String> {
        let bytes =
            fs::read(manifest).map_err(|error| format!("{}: {error}", manifest.display()))?;
        let source = std::str::from_utf8(&bytes)
            .map_err(|error| format!("{}: {error}", manifest.display()))?;
        let parsed: toml::Table =
            toml::from_str(source).map_err(|error| format!("{}: {error}", manifest.display()))?;
        for key in parsed.keys() {
            if !matches!(key.as_str(), "package" | "modules" | "dependencies") {
                return Err(format!("{}: unknown table `{key}`", manifest.display()));
            }
        }
        let package = table(&parsed, "package")?;
        let name = string(package, "name")?;
        let version = string(package, "version")?;
        let root = string(package, "root")?;
        if !identifier(name) || !identifier(root) || !valid_version(version) {
            return Err(format!("{}: invalid package identity", manifest.display()));
        }
        if let Some(expected) = required_version
            && version != expected
        {
            return Err(format!(
                "{}: version {version} does not match required {expected}",
                manifest.display()
            ));
        }
        let directory = manifest.parent().expect("canonical manifest has parent");
        let mut hash = Sha256::new();
        update_hash(&mut hash, "phoebe.toml", &bytes);
        let module_table = table(&parsed, "modules")?;
        if !module_table.contains_key(root) {
            return Err(format!(
                "{}: root module `{root}` is absent",
                manifest.display()
            ));
        }
        let mut modules = BTreeMap::new();
        for (module_name, value) in module_table {
            if !identifier(module_name) {
                return Err(format!(
                    "{}: invalid module `{module_name}`",
                    manifest.display()
                ));
            }
            let relative = value
                .as_str()
                .ok_or_else(|| format!("{}: module path must be a string", manifest.display()))?;
            let relative_path = Path::new(relative);
            if relative_path.is_absolute()
                || relative_path
                    .components()
                    .any(|part| !matches!(part, Component::Normal(_)))
            {
                return Err(format!(
                    "{}: module path must stay inside package",
                    manifest.display()
                ));
            }
            let module_path = fs::canonicalize(directory.join(relative_path))
                .map_err(|error| format!("{}: {error}", directory.join(relative).display()))?;
            if !module_path.starts_with(directory) {
                return Err(format!(
                    "{}: module path escapes package",
                    manifest.display()
                ));
            }
            let content = fs::read(&module_path)
                .map_err(|error| format!("{}: {error}", module_path.display()))?;
            update_hash(&mut hash, module_name, &content);
            modules.insert(module_name.clone(), module_path);
        }
        let package_record = Package {
            locator: locator.clone(),
            name: name.to_owned(),
            version: version.to_owned(),
            root: root.to_owned(),
            manifest: manifest.to_owned(),
            modules,
        };
        self.packages.push(package_record);
        if locator != "." {
            self.pins.insert(
                locator.clone(),
                Pin {
                    locator: locator.clone(),
                    name: name.to_owned(),
                    version: version.to_owned(),
                    sha256: format!("{:x}", hash.finalize()),
                },
            );
        }
        if let Some(dependencies) = parsed.get("dependencies") {
            let dependencies = dependencies
                .as_table()
                .ok_or_else(|| format!("{}: [dependencies] must be a table", manifest.display()))?;
            for (alias, value) in dependencies {
                if !identifier(alias) || module_table.contains_key(alias) {
                    return Err(format!(
                        "{}: invalid dependency alias `{alias}`",
                        manifest.display()
                    ));
                }
                let spec = value.as_table().ok_or_else(|| {
                    format!(
                        "{}: dependency `{alias}` must be a table",
                        manifest.display()
                    )
                })?;
                if spec.len() != 2 || !spec.contains_key("path") || !spec.contains_key("version") {
                    return Err(format!(
                        "{}: dependency `{alias}` requires path and exact version",
                        manifest.display()
                    ));
                }
                let path = string(spec, "path")?;
                let required = string(spec, "version")?;
                if Path::new(path).is_absolute() || !valid_version(required) {
                    return Err(format!(
                        "{}: invalid path or required version for `{alias}`",
                        manifest.display()
                    ));
                }
                let child_locator = if locator == "." {
                    alias.clone()
                } else {
                    format!("{locator}/{alias}")
                };
                self.visit(&directory.join(path), child_locator, Some(required))?;
            }
        }
        Ok(())
    }
}

fn valid_version(version: &str) -> bool {
    let parts = version.split('.').collect::<Vec<_>>();
    parts.len() == 3
        && parts
            .iter()
            .all(|part| !part.is_empty() && part.bytes().all(|byte| byte.is_ascii_digit()))
}

fn walk(path: &Path) -> Result<(Graph, BTreeMap<String, Pin>), String> {
    let mut state = Walk {
        packages: Vec::new(),
        pins: BTreeMap::new(),
        active: BTreeSet::new(),
    };
    state.visit(path, ".".into(), None)?;
    Ok((
        Graph {
            packages: state.packages,
        },
        state.pins,
    ))
}

/// Create or refresh the lockfile from the exact content of each local dependency.
pub fn write_lock(path: &Path) -> Result<Graph, String> {
    let (graph, pins) = walk(path)?;
    let root_manifest = &graph.packages[0].manifest;
    let lock_path = root_manifest.with_file_name("phoebe.lock");
    let mut content = String::from("version = 1\n");
    for pin in pins.values() {
        content.push_str(&format!(
            "\n[[package]]\nlocator = \"{}\"\nname = \"{}\"\nversion = \"{}\"\nsha256 = \"{}\"\n",
            pin.locator, pin.name, pin.version, pin.sha256
        ));
    }
    let temp = lock_path.with_extension(format!("lock.tmp.{}", std::process::id()));
    fs::write(&temp, content).map_err(|error| format!("{}: {error}", temp.display()))?;
    fs::rename(&temp, &lock_path).map_err(|error| format!("{}: {error}", lock_path.display()))?;
    Ok(graph)
}

/// Resolve a graph only if its committed lockfile matches every dependency.
pub fn resolve(path: &Path) -> Result<Graph, String> {
    let (graph, expected) = walk(path)?;
    let lock_path = graph.packages[0].manifest.with_file_name("phoebe.lock");
    let source = fs::read_to_string(&lock_path)
        .map_err(|error| format!("{}: {error}; run `phoebe lock`", lock_path.display()))?;
    let parsed: toml::Table =
        toml::from_str(&source).map_err(|error| format!("{}: {error}", lock_path.display()))?;
    if parsed.get("version").and_then(toml::Value::as_integer) != Some(1) {
        return Err(format!(
            "{}: unsupported lockfile version",
            lock_path.display()
        ));
    }
    if parsed
        .keys()
        .any(|key| key != "version" && key != "package")
    {
        return Err(format!("{}: unknown lockfile field", lock_path.display()));
    }
    let mut actual = BTreeMap::new();
    if let Some(packages) = parsed.get("package") {
        let packages = packages
            .as_array()
            .ok_or_else(|| format!("{}: package must be an array", lock_path.display()))?;
        for value in packages {
            let table = value
                .as_table()
                .ok_or_else(|| format!("{}: invalid package entry", lock_path.display()))?;
            if table.len() != 4 {
                return Err(format!("{}: invalid package fields", lock_path.display()));
            }
            let pin = Pin {
                locator: string(table, "locator")?.to_owned(),
                name: string(table, "name")?.to_owned(),
                version: string(table, "version")?.to_owned(),
                sha256: string(table, "sha256")?.to_owned(),
            };
            if actual.insert(pin.locator.clone(), pin).is_some() {
                return Err(format!("{}: duplicate locator", lock_path.display()));
            }
        }
    }
    if expected != actual {
        return Err(format!(
            "{}: stale dependency pins; run `phoebe lock`",
            lock_path.display()
        ));
    }
    Ok(graph)
}

#[cfg(test)]
mod tests {
    use super::{resolve, write_lock};
    use std::fs;
    use std::sync::atomic::{AtomicUsize, Ordering};

    static NEXT: AtomicUsize = AtomicUsize::new(0);

    #[test]
    fn transitive_path_graph_is_pinned_by_source_content() {
        let directory = std::env::temp_dir().join(format!(
            "phoebe-package-test-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        let app = directory.join("app");
        let math = directory.join("math");
        let util = directory.join("util");
        for path in [&app, &math, &util] {
            fs::create_dir_all(path).unwrap();
        }
        fs::write(
            app.join("phoebe.toml"),
            "[package]\nname = 'app'\nversion = '0.0.1'\nroot = 'main'\n\
             [modules]\nmain = 'main.phoebe'\n\
             [dependencies]\nmath = { path = '../math', version = '0.0.1' }\n",
        )
        .unwrap();
        fs::write(app.join("main.phoebe"), "fn main() -> I64 = 1\n").unwrap();
        fs::write(
            math.join("phoebe.toml"),
            "[package]\nname = 'math'\nversion = '0.0.1'\nroot = 'api'\n\
             [modules]\napi = 'api.phoebe'\n\
             [dependencies]\nutil = { path = '../util', version = '0.0.1' }\n",
        )
        .unwrap();
        fs::write(math.join("api.phoebe"), "fn answer() -> I64 = 42\n").unwrap();
        fs::write(
            util.join("phoebe.toml"),
            "[package]\nname = 'util'\nversion = '0.0.1'\nroot = 'api'\n\
             [modules]\napi = 'api.phoebe'\n",
        )
        .unwrap();
        fs::write(util.join("api.phoebe"), "fn id(x: I64) -> I64 = x\n").unwrap();

        assert!(resolve(&app).is_err());
        let graph = write_lock(&app).unwrap();
        assert_eq!(graph.packages.len(), 3);
        assert_eq!(graph.packages[1].locator, "math");
        assert_eq!(graph.packages[2].locator, "math/util");
        assert_eq!(resolve(&app).unwrap().packages.len(), 3);
        fs::write(util.join("api.phoebe"), "fn id(x: I64) -> I64 = x + 0\n").unwrap();
        assert!(resolve(&app).unwrap_err().contains("stale dependency pins"));
        write_lock(&app).unwrap();
        assert_eq!(resolve(&app).unwrap().packages.len(), 3);
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn rejects_path_dependency_cycle() {
        let directory = std::env::temp_dir().join(format!(
            "phoebe-package-cycle-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir_all(&directory).unwrap();
        fs::write(
            directory.join("phoebe.toml"),
            "[package]\nname='loop'\nversion='0.0.1'\nroot='app'\n\
             [modules]\napp='app.phoebe'\n\
             [dependencies]\nself_dep={path='.',version='0.0.1'}\n",
        )
        .unwrap();
        fs::write(directory.join("app.phoebe"), "fn main() -> I64 = 1\n").unwrap();
        assert!(write_lock(&directory).unwrap_err().contains("cycle"));
        fs::remove_dir_all(directory).unwrap();
    }
}
