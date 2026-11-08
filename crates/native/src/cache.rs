//! Exact-program native object cache for repeated command-line builds.

use std::fs::{self, File, OpenOptions};
use std::io::{Read, Write};
#[cfg(unix)]
use std::os::unix::fs::MetadataExt;
use std::path::Path;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::UNIX_EPOCH;

use phoebe_core::{self as core, Function, Operation, Program, Terminator};
use sha2::{Digest, Sha256};

use super::{ObjectEmission, emit_object_with_poll_layout, link_executable};

const MAGIC: &[u8] = b"PHOEBE-NATIVE-OBJECTS\0\x04";
const CACHE_FILE: &str = "objects.v4";
const MAX_CACHE_BYTES: u64 = 256 * 1024 * 1024;
const MAX_KEY_BYTES: usize = 16 * 1024 * 1024;
const MAX_OBJECT_BYTES: usize = 64 * 1024 * 1024;
const MAX_CHILDREN: usize = 10_000;

/// Local compiler file identity. Build tools replace this file when codegen
/// changes; a stamp avoids hashing a large debug executable per invocation.
fn compiler_stamp() -> Result<[u8; 32], String> {
    let executable = std::env::current_exe().map_err(|error| error.to_string())?;
    let executable = fs::canonicalize(executable).map_err(|error| error.to_string())?;
    let metadata = fs::metadata(&executable).map_err(|error| error.to_string())?;
    let modified = metadata
        .modified()
        .map_err(|error| error.to_string())?
        .duration_since(UNIX_EPOCH)
        .map_err(|error| error.to_string())?;
    let mut hasher = Sha256::new();
    hasher.update(executable.as_os_str().as_encoded_bytes());
    hasher.update(metadata.len().to_le_bytes());
    hasher.update(modified.as_secs().to_le_bytes());
    hasher.update(modified.subsec_nanos().to_le_bytes());
    #[cfg(unix)]
    {
        hasher.update(metadata.dev().to_le_bytes());
        hasher.update(metadata.ino().to_le_bytes());
    }
    Ok(hasher.finalize().into())
}

/// A dead leaf with no runtime operation cannot contribute an object or
/// change the native call graph. Keep its signature in the key because the
/// module still declares every function, but discard its unobserved body.
fn dead_pure_leaf(function: &Function) -> bool {
    function.blocks.iter().all(|block| {
        block.instructions.iter().all(|instruction| {
            !matches!(
                instruction.operation,
                Operation::Call(_, _)
                    | Operation::CloseHandle(_)
                    | Operation::CloseReply(_)
                    | Operation::PackRequest { .. }
                    | Operation::CloseRequest(_)
                    | Operation::ScopeEnter(_)
            )
        }) && matches!(
            block.terminator,
            Terminator::Return(_)
                | Terminator::Jump(_, _)
                | Terminator::Branch { .. }
                | Terminator::Match { .. }
        )
    })
}

fn program_key(program: &Program) -> Result<String, String> {
    let (main, _) = program
        .function("main")
        .ok_or("standalone executable requires `fn main() -> I64`")?;
    let mut reachable = vec![false; program.functions.len()];
    let mut pending = vec![main];
    while let Some(id) = pending.pop() {
        let Some(function) = program.functions.get(id.0) else {
            return Err("invalid function target in native cache key".into());
        };
        if reachable[id.0] {
            continue;
        }
        reachable[id.0] = true;
        for block in &function.blocks {
            for instruction in &block.instructions {
                if let Operation::Call(target, _) = instruction.operation {
                    pending.push(target);
                }
            }
            if let Terminator::Spawn { child, .. } = block.terminator {
                pending.push(child);
            }
        }
    }
    let mut normalized = program.clone();
    for (index, function) in normalized.functions.iter_mut().enumerate() {
        if !reachable[index] && dead_pure_leaf(function) {
            let entry = function
                .blocks
                .first()
                .ok_or("function has no entry block")?;
            let entry_types = entry
                .params
                .iter()
                .map(|id| {
                    function
                        .value_types
                        .get(id.0)
                        .copied()
                        .ok_or_else(|| "invalid entry parameter in native cache key".to_owned())
                })
                .collect::<Result<Vec<_>, _>>()?;
            function.blocks.clear();
            function.value_types = entry_types;
            function.place_types.clear();
            function.loan_regions.clear();
        }
        function.for_each_source_span_mut(|span| {
            span.start = 0;
            span.end = 0;
        });
    }
    Ok(format!("{normalized:?}"))
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

    fn object(&mut self) -> Option<Vec<u8>> {
        let length = self.length(MAX_OBJECT_BYTES)?;
        let digest = self.take(32)?;
        let bytes = self.take(length)?;
        (Sha256::digest(bytes).as_slice() == digest).then(|| bytes.to_vec())
    }
}

fn read_cache(directory: &Path, expected_key: &str) -> Option<ObjectEmission> {
    let expected_compiler = compiler_stamp().ok()?;
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
    let key = decoder.string()?;
    if key != expected_key {
        return None;
    }
    let raw_words = u64::from_le_bytes(decoder.take(8)?.try_into().ok()?);
    let poll_words = if raw_words == u64::MAX {
        None
    } else {
        Some(usize::try_from(raw_words).ok()?)
    };
    let root = decoder.object()?;
    let child_count = decoder.length(MAX_CHILDREN)?;
    let mut children = Vec::with_capacity(child_count);
    for _ in 0..child_count {
        children.push(decoder.object()?);
    }
    (decoder.offset == bytes.len()).then_some(ObjectEmission {
        root,
        poll_words,
        children,
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

fn write_object(file: &mut File, bytes: &[u8]) -> Result<(), String> {
    write_length(file, bytes.len(), MAX_OBJECT_BYTES)?;
    file.write_all(&Sha256::digest(bytes))
        .map_err(|error| error.to_string())?;
    file.write_all(bytes).map_err(|error| error.to_string())
}

fn write_cache(directory: &Path, key: &str, emitted: &ObjectEmission) -> Result<(), String> {
    let compiler = compiler_stamp()?;
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
        write_string(&mut file, key)?;
        let words = emitted
            .poll_words
            .map(|value| value as u64)
            .unwrap_or(u64::MAX);
        file.write_all(&words.to_le_bytes())
            .map_err(|error| error.to_string())?;
        write_object(&mut file, &emitted.root)?;
        write_length(&mut file, emitted.children.len(), MAX_CHILDREN)?;
        for child in &emitted.children {
            write_object(&mut file, child)?;
        }
        if file.metadata().map_err(|error| error.to_string())?.len() > MAX_CACHE_BYTES {
            return Err("native object cache exceeds total size limit".into());
        }
        // This is disposable cache data. Closing before the atomic rename is
        // sufficient; a truncated entry is rejected on the next read.
        Ok(())
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

pub struct CachedBuildReport {
    pub reused: bool,
    pub cache_write_error: Option<String>,
}

/// Reuse an emitted object set only when its span-free Core inputs and local
/// compiler file stamp match. Every build still verifies Core and runs the linker.
pub fn build_executable_cached(
    program: &Program,
    output: &Path,
    directory: &Path,
) -> Result<CachedBuildReport, String> {
    build_executable_cached_verified(core::VerifiedProgram::check(program)?, output, directory)
}

pub fn build_executable_cached_checked(
    program: &core::CheckedProgram,
    output: &Path,
    directory: &Path,
) -> Result<CachedBuildReport, String> {
    build_executable_cached_verified(program.verified(), output, directory)
}

fn build_executable_cached_verified(
    verified: core::VerifiedProgram<'_>,
    output: &Path,
    directory: &Path,
) -> Result<CachedBuildReport, String> {
    let program = verified.as_program();
    let key = program_key(program)?;
    if let Some(emitted) = read_cache(directory, &key) {
        link_executable(program, &emitted, output)?;
        return Ok(CachedBuildReport {
            reused: true,
            cache_write_error: None,
        });
    }
    let (emitted, _) = emit_object_with_poll_layout(verified, false, None)?;
    link_executable(program, &emitted, output)?;
    Ok(CachedBuildReport {
        reused: false,
        cache_write_error: write_cache(directory, &key, &emitted).err(),
    })
}
