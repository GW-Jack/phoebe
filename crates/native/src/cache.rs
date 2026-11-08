//! Bounded, versioned direct-object cache for repeated CLI builds.

use std::fs::{self, File, OpenOptions};
use std::io::{Read, Write};
use std::path::Path;
use std::sync::OnceLock;
use std::sync::atomic::{AtomicU64, Ordering};

use sha2::{Digest, Sha256};

use super::NativeBuildSession;

const MAGIC: &[u8] = b"PHOEBE-NATIVE-OBJECTS\0\x01";
const MAX_CACHE_BYTES: u64 = 256 * 1024 * 1024;
const MAX_KEY_BYTES: usize = 16 * 1024 * 1024;
const MAX_OBJECT_BYTES: usize = 64 * 1024 * 1024;
const MAX_OBJECTS: usize = 10_000;
const CACHE_FILE: &str = "objects.v1";

fn compiler_digest() -> Result<[u8; 32], String> {
    static DIGEST: OnceLock<Result<[u8; 32], String>> = OnceLock::new();
    DIGEST
        .get_or_init(|| {
            let executable = std::env::current_exe().map_err(|error| error.to_string())?;
            let mut file = File::open(executable).map_err(|error| error.to_string())?;
            let mut hasher = Sha256::new();
            let mut buffer = [0u8; 64 * 1024];
            loop {
                let count = file.read(&mut buffer).map_err(|error| error.to_string())?;
                if count == 0 {
                    break;
                }
                hasher.update(&buffer[..count]);
            }
            Ok(hasher.finalize().into())
        })
        .clone()
}

struct Decoder<'a> {
    bytes: &'a [u8],
    offset: usize,
}

impl<'a> Decoder<'a> {
    fn take(&mut self, count: usize) -> Option<&'a [u8]> {
        let end = self.offset.checked_add(count)?;
        let value = self.bytes.get(self.offset..end)?;
        self.offset = end;
        Some(value)
    }

    fn length(&mut self, maximum: usize) -> Option<usize> {
        let raw = self.take(8)?;
        let value = usize::try_from(u64::from_le_bytes(raw.try_into().ok()?)).ok()?;
        (value <= maximum).then_some(value)
    }

    fn string(&mut self) -> Option<String> {
        let length = self.length(MAX_KEY_BYTES)?;
        Some(std::str::from_utf8(self.take(length)?).ok()?.to_owned())
    }
}

fn read_cache(directory: &Path) -> Option<NativeBuildSession> {
    let expected_compiler = compiler_digest().ok()?;
    let file = File::open(directory.join(CACHE_FILE)).ok()?;
    let mut bytes = Vec::new();
    file.take(MAX_CACHE_BYTES + 1)
        .read_to_end(&mut bytes)
        .ok()?;
    if bytes.len() as u64 > MAX_CACHE_BYTES {
        return None;
    }
    let mut decoder = Decoder {
        bytes: &bytes,
        offset: 0,
    };
    if decoder.take(MAGIC.len())? != MAGIC || decoder.take(32)? != expected_compiler {
        return None;
    }
    let global_key = decoder.string()?;
    let count = decoder.length(MAX_OBJECTS)?;
    let mut objects = Vec::with_capacity(count);
    for _ in 0..count {
        match decoder.take(1)?[0] {
            0 => objects.push(None),
            1 => {
                let key = decoder.string()?;
                let length = decoder.length(MAX_OBJECT_BYTES)?;
                let digest = decoder.take(32)?;
                let object = decoder.take(length)?;
                if Sha256::digest(object).as_slice() != digest {
                    return None;
                }
                objects.push(Some((key, object.to_vec())));
            }
            _ => return None,
        }
    }
    (decoder.offset == bytes.len()).then_some(NativeBuildSession {
        global_key: Some(global_key),
        objects,
    })
}

fn write_length(file: &mut File, value: usize, maximum: usize) -> Result<(), String> {
    if value > maximum {
        return Err("native object cache exceeds size limit".into());
    }
    file.write_all(&(value as u64).to_le_bytes())
        .map_err(|error| error.to_string())
}

fn write_string(file: &mut File, value: &str) -> Result<(), String> {
    write_length(file, value.len(), MAX_KEY_BYTES)?;
    file.write_all(value.as_bytes())
        .map_err(|error| error.to_string())
}

fn write_cache(session: &NativeBuildSession, directory: &Path) -> Result<(), String> {
    let Some(global_key) = &session.global_key else {
        return Ok(());
    };
    let compiler = compiler_digest()?;
    fs::create_dir_all(directory).map_err(|error| error.to_string())?;
    static NEXT_TEMP: AtomicU64 = AtomicU64::new(0);
    let mut temporary = None;
    for _ in 0..16 {
        let candidate = directory.join(format!(
            ".objects-{}-{}",
            std::process::id(),
            NEXT_TEMP.fetch_add(1, Ordering::Relaxed)
        ));
        match OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&candidate)
        {
            Ok(file) => {
                temporary = Some((candidate, file));
                break;
            }
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {}
            Err(error) => return Err(error.to_string()),
        }
    }
    let (path, mut file) = temporary.ok_or("cannot create native object cache temporary file")?;
    let written = (|| -> Result<(), String> {
        file.write_all(MAGIC).map_err(|error| error.to_string())?;
        file.write_all(&compiler)
            .map_err(|error| error.to_string())?;
        write_string(&mut file, global_key)?;
        write_length(&mut file, session.objects.len(), MAX_OBJECTS)?;
        for object in &session.objects {
            match object {
                None => file.write_all(&[0]).map_err(|error| error.to_string())?,
                Some((key, bytes)) => {
                    file.write_all(&[1]).map_err(|error| error.to_string())?;
                    write_string(&mut file, key)?;
                    write_length(&mut file, bytes.len(), MAX_OBJECT_BYTES)?;
                    file.write_all(&Sha256::digest(bytes))
                        .map_err(|error| error.to_string())?;
                    file.write_all(bytes).map_err(|error| error.to_string())?;
                }
            }
        }
        if file.metadata().map_err(|error| error.to_string())?.len() > MAX_CACHE_BYTES {
            return Err("native object cache exceeds total size limit".into());
        }
        file.sync_all().map_err(|error| error.to_string())
    })();
    drop(file);
    let result = written.and_then(|()| {
        fs::rename(&path, directory.join(CACHE_FILE)).map_err(|error| error.to_string())
    });
    if result.is_err() {
        let _ = fs::remove_file(path);
    }
    result
}

impl NativeBuildSession {
    /// Load only a complete cache produced by this exact compiler executable.
    /// Missing, stale, or malformed cache data is treated as a cold build.
    pub fn load_cache(directory: &Path) -> Self {
        read_cache(directory).unwrap_or_default()
    }

    /// Atomically publish direct objects from the last successful build.
    pub fn save_cache(&self, directory: &Path) -> Result<(), String> {
        write_cache(self, directory)
    }
}
