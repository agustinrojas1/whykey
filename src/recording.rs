//! Record and replay of the answers whykey takes from outside the process.
//!
//! Every question whykey asks its environment goes through one of the
//! functions here: a child process (`command_output`), an environment
//! variable (`env_var`, `env_var_os`), a file (`read`, `read_to_string`), a
//! directory listing (`read_dir`), a symbolic link (`read_link`), a path's
//! metadata, existence or canonical form (`metadata`, `symlink_metadata`,
//! `PathInputs`), or whether a device opens (`can_open`). Without the
//! variables below each function is the standard library call it wraps.
//!
//! `WHYKEY_RECORD=<file>` records every answer as one JSON line appended to
//! `<file>` and returns it unchanged. `lab/run.py --record DIR` sets it while
//! the reality lab runs whykey, then stores the lines as
//! `lab/recordings/<scenario>.json`.
//!
//! `WHYKEY_REPLAY=<recording.json>` answers the same questions from that file
//! and touches nothing outside the process. A question the recording holds no
//! answer to ends whykey with status 97 and `unrecorded input: ...` on stderr,
//! so a changed query can never be guessed at. An answer the run never asked
//! for ends it with status 98 (see `finish`). Both variables set is an error
//! (status 96).
//!
//! Answers are exact: each one is encoded to JSON, and in recording as well as
//! replay the value whykey receives is decoded from that JSON. A recorded run
//! therefore sees exactly what a replay will show it. Known losses: environment
//! values, paths and directory entry names that are not UTF-8 are recorded
//! lossily, and an entry whose type cannot be read is replayed as an error.
//! See `docs/lab-replay.md`.

use std::collections::{BTreeMap, HashMap, VecDeque};
use std::ffi::{OsStr, OsString};
use std::fs::{self, File, OpenOptions};
use std::io::{self, Write};
use std::os::unix::process::ExitStatusExt;
use std::path::{Path, PathBuf};
use std::process::{Command, ExitCode, ExitStatus, Output};
use std::sync::{Mutex, MutexGuard, OnceLock};
use std::time::Duration;

use serde_json::{Value, json};

use crate::command::CommandError;

/// Environment variable naming the file a recording run appends to.
pub const RECORD_VARIABLE: &str = "WHYKEY_RECORD";
/// Environment variable naming the recording document a replay answers from.
pub const REPLAY_VARIABLE: &str = "WHYKEY_REPLAY";
/// Exit status of a replay that was asked a question the recording lacks.
pub const UNRECORDED_EXIT: u8 = 97;
/// Exit status of a replay whose recording held answers the run never used.
pub const UNCONSUMED_EXIT: u8 = 98;
/// Exit status when a recording cannot be opened, loaded, or written.
pub const SETUP_EXIT: u8 = 96;

enum Mode {
    Live,
    Record(Mutex<File>),
    Replay(Mutex<Replay>),
}

#[derive(Default)]
struct Replay {
    /// Recorded answers per question, oldest first.
    answers: HashMap<String, VecDeque<Value>>,
}

fn mode() -> &'static Mode {
    static MODE: OnceLock<Mode> = OnceLock::new();
    MODE.get_or_init(open_mode)
}

fn open_mode() -> Mode {
    let record = std::env::var_os(RECORD_VARIABLE).filter(|value| !value.is_empty());
    let replay = std::env::var_os(REPLAY_VARIABLE).filter(|value| !value.is_empty());
    match (record, replay) {
        (None, None) => Mode::Live,
        (Some(_), Some(_)) => fail(
            SETUP_EXIT,
            &format!("{RECORD_VARIABLE} and {REPLAY_VARIABLE} cannot both be set"),
        ),
        (Some(path), None) => {
            match OpenOptions::new()
                .create(true)
                .write(true)
                .truncate(true)
                .open(&path)
            {
                Ok(file) => Mode::Record(Mutex::new(file)),
                Err(error) => fail(
                    SETUP_EXIT,
                    &format!(
                        "cannot open recording {}: {error}",
                        Path::new(&path).display()
                    ),
                ),
            }
        }
        (None, Some(path)) => match load_replay(Path::new(&path)) {
            Ok(replay) => Mode::Replay(Mutex::new(replay)),
            Err(message) => fail(SETUP_EXIT, &message),
        },
    }
}

fn load_replay(path: &Path) -> Result<Replay, String> {
    let bytes = fs::read(path)
        .map_err(|error| format!("cannot read replay recording {}: {error}", path.display()))?;
    let document: Value = serde_json::from_slice(&bytes)
        .map_err(|error| format!("replay recording {} is not JSON: {error}", path.display()))?;
    let inputs = document
        .get("inputs")
        .and_then(Value::as_array)
        .ok_or_else(|| {
            format!(
                "replay recording {} has no \"inputs\" array",
                path.display()
            )
        })?;
    let mut replay = Replay::default();
    for input in inputs {
        let (Some(kind), Some(key), Some(response)) = (
            input.get("kind").and_then(Value::as_str),
            input.get("key"),
            input.get("response"),
        ) else {
            return Err(format!(
                "replay recording {} has an input without kind, key, or response",
                path.display()
            ));
        };
        replay
            .answers
            .entry(question(kind, key))
            .or_default()
            .push_back(response.clone());
    }
    Ok(replay)
}

/// Stop the process with `code`, after telling stderr why.
fn fail(code: u8, message: &str) -> ! {
    eprintln!("whykey: {message}");
    std::process::exit(i32::from(code))
}

/// The identity of one question: its kind and its key in canonical JSON, so
/// the same question has the same text whatever order its fields were built in.
fn question(kind: &str, key: &Value) -> String {
    format!("{kind} {}", canonical(key))
}

fn canonical(value: &Value) -> String {
    match value {
        Value::Object(map) => {
            let mut names: Vec<&String> = map.keys().collect();
            names.sort();
            let fields: Vec<String> = names
                .into_iter()
                .map(|name| {
                    format!(
                        "{}:{}",
                        Value::String(name.clone()),
                        canonical(&map[name.as_str()])
                    )
                })
                .collect();
            format!("{{{}}}", fields.join(","))
        }
        Value::Array(items) => {
            let items: Vec<String> = items.iter().map(canonical).collect();
            format!("[{}]", items.join(","))
        }
        other => other.to_string(),
    }
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// Whether whykey runs live: neither `WHYKEY_RECORD` nor `WHYKEY_REPLAY` is set.
pub fn is_live() -> bool {
    matches!(mode(), Mode::Live)
}

/// Answer one question. Live: `live()`. Recording: `live()`, appended to the
/// recording. Replay: the next recorded answer to this question, or exit 97.
/// In recording and replay the value returned is decoded from the encoded
/// answer, so both see the same thing.
fn answer<T>(
    kind: &str,
    key: Value,
    live: impl FnOnce() -> T,
    encode: impl FnOnce(&T) -> Value,
    decode: impl FnOnce(&Value) -> Option<T>,
) -> T {
    let response = match mode() {
        Mode::Live => return live(),
        Mode::Record(file) => {
            let response = encode(&live());
            let line = json!({"kind": kind, "key": key, "response": response}).to_string();
            let mut file = lock(file);
            if let Err(error) = writeln!(file, "{line}").and_then(|()| file.flush()) {
                fail(SETUP_EXIT, &format!("cannot write the recording: {error}"));
            }
            response
        }
        Mode::Replay(replay) => {
            let id = question(kind, &key);
            let recorded = lock(replay)
                .answers
                .get_mut(&id)
                .and_then(VecDeque::pop_front);
            match recorded {
                Some(response) => response,
                None => fail(
                    UNRECORDED_EXIT,
                    &format!(
                        "unrecorded input: {kind} {}; whykey asked a question the recording \
                         holds no answer to. Refresh the recording with a lab run \
                         (python3 lab/run.py --record lab/recordings --only <scenario>)",
                        canonical(&key)
                    ),
                ),
            }
        }
    };
    match decode(&response) {
        Some(value) => value,
        None => fail(
            SETUP_EXIT,
            &format!(
                "the recorded answer to {kind} {} is malformed",
                canonical(&key)
            ),
        ),
    }
}

/// Call this with whykey's result before exiting. In replay mode it refuses
/// to report success when recorded answers went unused: whykey no longer asks
/// everything the lab saw, so its verdict was not checked against the same
/// evidence.
pub fn finish(code: ExitCode) -> ExitCode {
    let Mode::Replay(replay) = mode() else {
        return code;
    };
    let replay = lock(replay);
    let mut unused: Vec<&String> = replay
        .answers
        .iter()
        .filter(|(_, answers)| !answers.is_empty())
        .map(|(question, _)| question)
        .collect();
    if unused.is_empty() {
        return code;
    }
    unused.sort();
    let names: Vec<&str> = unused.iter().map(|question| question.as_str()).collect();
    eprintln!(
        "whykey: recorded input never requested: {}; the recording no longer matches whykey's queries",
        names.join(", ")
    );
    ExitCode::from(UNCONSUMED_EXIT)
}

// ---- bytes, OS errors, results ----------------------------------------------

fn hex_encode(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

fn hex_decode(text: &str) -> Option<Vec<u8>> {
    if text.len() % 2 != 0 || !text.is_ascii() {
        return None;
    }
    (0..text.len())
        .step_by(2)
        .map(|index| u8::from_str_radix(&text[index..index + 2], 16).ok())
        .collect()
}

/// Bytes as `{"utf8": text}` when they are UTF-8, else `{"hex": ...}`.
fn bytes_value(bytes: &[u8]) -> Value {
    match std::str::from_utf8(bytes) {
        Ok(text) => json!({ "utf8": text }),
        Err(_) => json!({ "hex": hex_encode(bytes) }),
    }
}

fn bytes_from(value: &Value) -> Option<Vec<u8>> {
    if let Some(text) = value.get("utf8").and_then(Value::as_str) {
        return Some(text.as_bytes().to_vec());
    }
    hex_decode(value.get("hex")?.as_str()?)
}

/// An io error as its OS code when it has one, so the kind and message come
/// back exactly; otherwise as its kind and message.
fn io_value(error: &io::Error) -> Value {
    match error.raw_os_error() {
        Some(code) => json!({ "os": code }),
        None => json!({ "kind": format!("{:?}", error.kind()), "message": error.to_string() }),
    }
}

fn io_from(value: &Value) -> Option<io::Error> {
    if let Some(code) = value.get("os").and_then(Value::as_i64) {
        return Some(io::Error::from_raw_os_error(i32::try_from(code).ok()?));
    }
    let kind = match value.get("kind")?.as_str()? {
        "NotFound" => io::ErrorKind::NotFound,
        "PermissionDenied" => io::ErrorKind::PermissionDenied,
        "InvalidData" => io::ErrorKind::InvalidData,
        "InvalidInput" => io::ErrorKind::InvalidInput,
        "UnexpectedEof" => io::ErrorKind::UnexpectedEof,
        "WouldBlock" => io::ErrorKind::WouldBlock,
        _ => io::ErrorKind::Other,
    };
    Some(io::Error::new(
        kind,
        value.get("message")?.as_str()?.to_owned(),
    ))
}

/// `{"ok": encoded}` or `{"err": io error}`.
fn io_result_value<T>(result: &io::Result<T>, ok: impl FnOnce(&T) -> Value) -> Value {
    match result {
        Ok(value) => json!({ "ok": ok(value) }),
        Err(error) => json!({ "err": io_value(error) }),
    }
}

fn io_result_from<T>(
    response: &Value,
    ok: impl FnOnce(&Value) -> Option<T>,
) -> Option<io::Result<T>> {
    match response.get("ok") {
        Some(inner) => ok(inner).map(Ok),
        None => io_from(response.get("err")?).map(Err),
    }
}

// ---- environment ------------------------------------------------------------

/// `std::env::var_os`, recorded.
pub fn env_var_os<K: AsRef<OsStr>>(name: K) -> Option<OsString> {
    let name = name.as_ref();
    if is_live() {
        return std::env::var_os(name);
    }
    answer(
        "env",
        Value::String(name.to_string_lossy().into_owned()),
        || std::env::var_os(name),
        |value| match value {
            None => Value::Null,
            Some(value) => match value.to_str() {
                Some(text) => Value::String(text.to_owned()),
                None => json!({ "non_unicode": value.to_string_lossy() }),
            },
        },
        |response| match response {
            Value::Null => Some(None),
            Value::String(text) => Some(Some(OsString::from(text))),
            Value::Object(_) => Some(Some(OsString::from(response.get("non_unicode")?.as_str()?))),
            _ => None,
        },
    )
}

/// `std::env::var`, recorded, with the same results including `NotUnicode`.
pub fn env_var<K: AsRef<OsStr>>(name: K) -> Result<String, std::env::VarError> {
    match env_var_os(name) {
        None => Err(std::env::VarError::NotPresent),
        Some(value) => value.into_string().map_err(std::env::VarError::NotUnicode),
    }
}

// ---- files, links, directories ---------------------------------------------

fn path_key(path: &Path) -> Value {
    Value::String(path.to_string_lossy().into_owned())
}

/// `std::fs::read`, recorded.
pub fn read(path: impl AsRef<Path>) -> io::Result<Vec<u8>> {
    let path = path.as_ref();
    if is_live() {
        return fs::read(path);
    }
    answer(
        "file",
        path_key(path),
        || fs::read(path),
        |result| io_result_value(result, |bytes| bytes_value(bytes)),
        |response| io_result_from(response, bytes_from),
    )
}

/// `std::fs::read_to_string`, recorded: the bytes are read and checked as
/// UTF-8 with the error the standard library reports.
pub fn read_to_string(path: impl AsRef<Path>) -> io::Result<String> {
    read(path).and_then(|bytes| {
        String::from_utf8(bytes).map_err(|_| {
            io::Error::new(
                io::ErrorKind::InvalidData,
                "stream did not contain valid UTF-8",
            )
        })
    })
}

fn path_result_value(result: &io::Result<PathBuf>) -> Value {
    io_result_value(result, |target| path_key(target))
}

fn path_result_from(response: &Value) -> Option<io::Result<PathBuf>> {
    io_result_from(response, |value| value.as_str().map(PathBuf::from))
}

/// `std::fs::read_link`, recorded.
pub fn read_link(path: impl AsRef<Path>) -> io::Result<PathBuf> {
    let path = path.as_ref();
    if is_live() {
        return fs::read_link(path);
    }
    answer(
        "read_link",
        path_key(path),
        || fs::read_link(path),
        path_result_value,
        path_result_from,
    )
}

/// `std::fs::canonicalize`, recorded.
pub fn canonicalize(path: impl AsRef<Path>) -> io::Result<PathBuf> {
    let path = path.as_ref();
    if is_live() {
        return fs::canonicalize(path);
    }
    answer(
        "canonicalize",
        path_key(path),
        || fs::canonicalize(path),
        path_result_value,
        path_result_from,
    )
}

/// The parts of `std::fs::Metadata` whykey reads.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RecordedMetadata {
    size: u64,
    is_file: bool,
    is_dir: bool,
    is_symlink: bool,
}

impl RecordedMetadata {
    fn from_std(metadata: &fs::Metadata) -> Self {
        Self {
            size: metadata.len(),
            is_file: metadata.is_file(),
            is_dir: metadata.is_dir(),
            is_symlink: metadata.file_type().is_symlink(),
        }
    }

    /// The file's length in bytes, `std::fs::Metadata::len`.
    pub fn size(&self) -> u64 {
        self.size
    }

    pub fn is_file(&self) -> bool {
        self.is_file
    }

    pub fn is_dir(&self) -> bool {
        self.is_dir
    }

    pub fn is_symlink(&self) -> bool {
        self.is_symlink
    }

    fn encode(&self) -> Value {
        json!({
            "size": self.size,
            "is_file": self.is_file,
            "is_dir": self.is_dir,
            "is_symlink": self.is_symlink,
        })
    }

    fn decode(value: &Value) -> Option<Self> {
        Some(Self {
            size: value.get("size")?.as_u64()?,
            is_file: value.get("is_file")?.as_bool()?,
            is_dir: value.get("is_dir")?.as_bool()?,
            is_symlink: value.get("is_symlink")?.as_bool()?,
        })
    }
}

fn metadata_answer(
    kind: &str,
    path: &Path,
    live: impl FnOnce() -> io::Result<fs::Metadata>,
) -> io::Result<RecordedMetadata> {
    answer(
        kind,
        path_key(path),
        || live().map(|metadata| RecordedMetadata::from_std(&metadata)),
        |result| io_result_value(result, RecordedMetadata::encode),
        |response| io_result_from(response, RecordedMetadata::decode),
    )
}

/// `std::fs::metadata` (follows links), recorded.
pub fn metadata(path: impl AsRef<Path>) -> io::Result<RecordedMetadata> {
    let path = path.as_ref();
    if is_live() {
        return fs::metadata(path).map(|metadata| RecordedMetadata::from_std(&metadata));
    }
    metadata_answer("metadata", path, || fs::metadata(path))
}

/// `std::fs::symlink_metadata` (does not follow links), recorded.
pub fn symlink_metadata(path: impl AsRef<Path>) -> io::Result<RecordedMetadata> {
    let path = path.as_ref();
    if is_live() {
        return fs::symlink_metadata(path).map(|metadata| RecordedMetadata::from_std(&metadata));
    }
    metadata_answer("symlink_metadata", path, || fs::symlink_metadata(path))
}

/// Whether `File::open(path)` succeeds, recorded. The live probe is the only
/// time the device is opened.
pub fn can_open(path: impl AsRef<Path>) -> bool {
    let path = path.as_ref();
    if is_live() {
        return File::open(path).is_ok();
    }
    answer(
        "open",
        path_key(path),
        || File::open(path).is_ok(),
        |opened| Value::Bool(*opened),
        Value::as_bool,
    )
}

/// Up to `limit + 1` bytes of a regular file, for a caller that rejects a file
/// longer than `limit`. `None` when the path is not a regular file or cannot
/// be read. The file is opened without blocking, so a FIFO is never waited on.
pub fn read_regular_prefix(path: impl AsRef<Path>, limit: usize) -> Option<Vec<u8>> {
    let path = path.as_ref();
    if is_live() {
        return live_regular_prefix(path, limit);
    }
    let key = json!({ "path": path.to_string_lossy(), "limit": limit as u64 });
    answer(
        "regular_prefix",
        key,
        || live_regular_prefix(path, limit),
        |bytes| match bytes {
            Some(bytes) => json!({ "bytes": bytes_value(bytes) }),
            None => json!({ "none": true }),
        },
        |response| match response.get("bytes") {
            Some(bytes) => bytes_from(bytes).map(Some),
            None => Some(None),
        },
    )
}

/// Up to `limit + 1` bytes of a file, for a caller that rejects anything
/// longer. Errors are returned as the standard library reports them.
pub fn read_prefix(path: impl AsRef<Path>, limit: usize) -> io::Result<Vec<u8>> {
    use std::io::Read;
    let path = path.as_ref();
    let live = || -> io::Result<Vec<u8>> {
        let mut bytes = Vec::new();
        File::open(path)?
            .take(limit as u64 + 1)
            .read_to_end(&mut bytes)?;
        Ok(bytes)
    };
    if is_live() {
        return live();
    }
    let key = json!({ "path": path.to_string_lossy(), "limit": limit as u64 });
    answer(
        "file_prefix",
        key,
        live,
        |result| io_result_value(result, |bytes| bytes_value(bytes)),
        |response| io_result_from(response, bytes_from),
    )
}

fn live_regular_prefix(path: &Path, limit: usize) -> Option<Vec<u8>> {
    use std::io::Read;
    use std::os::unix::fs::OpenOptionsExt;
    // O_NONBLOCK so opening a FIFO cannot block; only regular files are read.
    let file = OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NONBLOCK)
        .open(path)
        .ok()?;
    if !file.metadata().ok()?.is_file() {
        return None;
    }
    let mut bytes = Vec::new();
    file.take(limit as u64 + 1).read_to_end(&mut bytes).ok()?;
    Some(bytes)
}

/// The type of a directory entry, as far as whykey reads it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FileKind {
    File,
    Dir,
    Symlink,
    Other,
}

impl FileKind {
    fn of(file_type: &fs::FileType) -> Self {
        if file_type.is_symlink() {
            Self::Symlink
        } else if file_type.is_dir() {
            Self::Dir
        } else if file_type.is_file() {
            Self::File
        } else {
            Self::Other
        }
    }

    fn name(self) -> &'static str {
        match self {
            Self::File => "file",
            Self::Dir => "dir",
            Self::Symlink => "symlink",
            Self::Other => "other",
        }
    }

    fn from_name(name: &str) -> Option<Self> {
        match name {
            "file" => Some(Self::File),
            "dir" => Some(Self::Dir),
            "symlink" => Some(Self::Symlink),
            "other" => Some(Self::Other),
            _ => None,
        }
    }
}

/// The type of a directory entry, not following links. Unlike
/// `std::fs::FileType` it can be built from a recording.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RecordedFileType(FileKind);

impl RecordedFileType {
    pub fn is_file(&self) -> bool {
        self.0 == FileKind::File
    }

    pub fn is_dir(&self) -> bool {
        self.0 == FileKind::Dir
    }

    pub fn is_symlink(&self) -> bool {
        self.0 == FileKind::Symlink
    }
}

/// One entry of a directory listing, live or recorded.
pub enum RecordedDirEntry {
    Live(fs::DirEntry),
    Recorded {
        path: PathBuf,
        name: OsString,
        kind: Option<FileKind>,
    },
}

impl RecordedDirEntry {
    pub fn path(&self) -> PathBuf {
        match self {
            Self::Live(entry) => entry.path(),
            Self::Recorded { path, .. } => path.clone(),
        }
    }

    pub fn file_name(&self) -> OsString {
        match self {
            Self::Live(entry) => entry.file_name(),
            Self::Recorded { name, .. } => name.clone(),
        }
    }

    /// `std::fs::DirEntry::file_type`: the type, not following links.
    pub fn file_type(&self) -> io::Result<RecordedFileType> {
        match self {
            Self::Live(entry) => entry
                .file_type()
                .map(|file_type| RecordedFileType(FileKind::of(&file_type))),
            Self::Recorded {
                kind: Some(kind), ..
            } => Ok(RecordedFileType(*kind)),
            Self::Recorded { kind: None, .. } => Err(io::Error::other(
                "the entry's file type is not in the recording",
            )),
        }
    }
}

/// The entries of a directory, in the order the listing gave them.
pub struct ReadDirEntries {
    inner: Entries,
}

enum Entries {
    Live(fs::ReadDir),
    Recorded(VecDeque<io::Result<RecordedDirEntry>>),
}

impl Iterator for ReadDirEntries {
    type Item = io::Result<RecordedDirEntry>;

    fn next(&mut self) -> Option<Self::Item> {
        match &mut self.inner {
            Entries::Live(read_dir) => read_dir
                .next()
                .map(|entry| entry.map(RecordedDirEntry::Live)),
            Entries::Recorded(items) => items.pop_front(),
        }
    }
}

/// A whole listing: each entry's name and type, or the error that stopped it.
type Listing = io::Result<Vec<io::Result<(OsString, Option<FileKind>)>>>;

fn encode_listing(listing: &Listing) -> Value {
    match listing {
        Err(error) => json!({ "err": io_value(error) }),
        Ok(items) => json!({
            "ok": items
                .iter()
                .map(|item| match item {
                    Ok((name, kind)) => json!({
                        "name": name.to_string_lossy(),
                        "type": kind.map_or("unknown", |kind| kind.name()),
                    }),
                    Err(error) => json!({ "err": io_value(error) }),
                })
                .collect::<Vec<_>>(),
        }),
    }
}

fn decode_listing(response: &Value) -> Option<Listing> {
    if let Some(error) = response.get("err") {
        return Some(Err(io_from(error)?));
    }
    let items = response.get("ok")?.as_array()?;
    let decoded = items
        .iter()
        .map(|item| -> Option<io::Result<(OsString, Option<FileKind>)>> {
            if let Some(error) = item.get("err") {
                return io_from(error).map(Err);
            }
            let name = OsString::from(item.get("name")?.as_str()?);
            let kind = match item.get("type")?.as_str()? {
                "unknown" => None,
                other => Some(FileKind::from_name(other)?),
            };
            Some(Ok((name, kind)))
        })
        .collect::<Option<Vec<_>>>()?;
    Some(Ok(decoded))
}

/// `std::fs::read_dir`, recorded. A recording takes the whole listing with
/// each entry's type, then returns it as a recorded listing.
pub fn read_dir(path: impl AsRef<Path>) -> io::Result<ReadDirEntries> {
    let path = path.as_ref();
    if is_live() {
        return fs::read_dir(path).map(|read_dir| ReadDirEntries {
            inner: Entries::Live(read_dir),
        });
    }
    let listing: Listing = answer(
        "read_dir",
        path_key(path),
        || {
            fs::read_dir(path).map(|read_dir| {
                read_dir
                    .map(|item| {
                        item.map(|entry| {
                            let kind = entry
                                .file_type()
                                .ok()
                                .map(|file_type| FileKind::of(&file_type));
                            (entry.file_name(), kind)
                        })
                    })
                    .collect()
            })
        },
        encode_listing,
        decode_listing,
    );
    let dir = path.to_path_buf();
    listing.map(|items| ReadDirEntries {
        inner: Entries::Recorded(
            items
                .into_iter()
                .map(|item| {
                    item.map(|(name, kind)| RecordedDirEntry::Recorded {
                        path: dir.join(&name),
                        name,
                        kind,
                    })
                })
                .collect(),
        ),
    })
}

// ---- path tests -------------------------------------------------------------

/// The `Path` methods that test a path, recorded. Implemented for `Path`, so
/// `PathBuf` and references get them through deref.
pub trait PathInputs {
    /// `Path::exists`.
    fn recorded_exists(&self) -> bool;
    /// `Path::is_file`.
    fn recorded_is_file(&self) -> bool;
    /// `Path::is_dir`.
    fn recorded_is_dir(&self) -> bool;
    /// `Path::canonicalize`.
    fn recorded_canonicalize(&self) -> io::Result<PathBuf>;
}

impl PathInputs for Path {
    fn recorded_exists(&self) -> bool {
        metadata(self).is_ok()
    }

    fn recorded_is_file(&self) -> bool {
        metadata(self).is_ok_and(|metadata| metadata.is_file())
    }

    fn recorded_is_dir(&self) -> bool {
        metadata(self).is_ok_and(|metadata| metadata.is_dir())
    }

    fn recorded_canonicalize(&self) -> io::Result<PathBuf> {
        canonicalize(self)
    }
}

// ---- this process ------------------------------------------------------------

/// `std::process::id`, recorded. A pid names a process in `/proc`, so a replay
/// must see the pid the recording saw, not its own.
pub fn process_id() -> u32 {
    if is_live() {
        return std::process::id();
    }
    answer(
        "process_id",
        Value::Null,
        std::process::id,
        |pid| json!(pid),
        |response| response.as_u64().and_then(|pid| u32::try_from(pid).ok()),
    )
}

/// `std::os::unix::process::parent_id`, recorded: the pid of the parent, which
/// whykey walks up from. Like `process_id`, it must match the recording.
pub fn parent_process_id() -> u32 {
    if is_live() {
        return std::os::unix::process::parent_id();
    }
    answer(
        "parent_pid",
        Value::Null,
        std::os::unix::process::parent_id,
        |pid| json!(pid),
        |response| response.as_u64().and_then(|pid| u32::try_from(pid).ok()),
    )
}

// ---- the controlling terminal -------------------------------------------------

/// The termios settings of the controlling terminal (`/dev/tty`), recorded.
/// Errors keep the text the caller reports.
pub fn tty_termios() -> Result<libc::termios, String> {
    use std::os::unix::io::AsRawFd;
    let live = || -> Result<libc::termios, String> {
        let tty =
            File::open("/dev/tty").map_err(|error| format!("failed to open /dev/tty: {error}"))?;
        let mut termios = unsafe { std::mem::zeroed::<libc::termios>() };
        if unsafe { libc::tcgetattr(tty.as_raw_fd(), &mut termios) } != 0 {
            return Err(format!(
                "failed to read terminal settings: {}",
                io::Error::last_os_error()
            ));
        }
        Ok(termios)
    };
    if is_live() {
        return live();
    }
    answer(
        "tty_termios",
        Value::Null,
        live,
        encode_termios,
        decode_termios,
    )
}

fn encode_termios(result: &Result<libc::termios, String>) -> Value {
    match result {
        Ok(termios) => json!({ "ok": {
            "iflag": termios.c_iflag,
            "oflag": termios.c_oflag,
            "cflag": termios.c_cflag,
            "lflag": termios.c_lflag,
            "line": termios.c_line,
            "cc": termios.c_cc.to_vec(),
            "ispeed": termios.c_ispeed,
            "ospeed": termios.c_ospeed,
        } }),
        Err(message) => json!({ "err": message }),
    }
}

fn decode_termios(response: &Value) -> Option<Result<libc::termios, String>> {
    if let Some(message) = response.get("err") {
        return Some(Err(message.as_str()?.to_owned()));
    }
    let ok = response.get("ok")?;
    let field = |name: &str| ok.get(name)?.as_u64();
    // SAFETY: termios is plain data; every field is set below.
    let mut termios = unsafe { std::mem::zeroed::<libc::termios>() };
    termios.c_iflag = field("iflag")? as libc::tcflag_t;
    termios.c_oflag = field("oflag")? as libc::tcflag_t;
    termios.c_cflag = field("cflag")? as libc::tcflag_t;
    termios.c_lflag = field("lflag")? as libc::tcflag_t;
    termios.c_line = field("line")? as libc::cc_t;
    termios.c_ispeed = field("ispeed")? as libc::speed_t;
    termios.c_ospeed = field("ospeed")? as libc::speed_t;
    let control = ok.get("cc")?.as_array()?;
    if control.len() != termios.c_cc.len() {
        return None;
    }
    for (slot, value) in termios.c_cc.iter_mut().zip(control) {
        *slot = value.as_u64()? as libc::cc_t;
    }
    Some(Ok(termios))
}

// ---- child processes --------------------------------------------------------

/// The question a child process run asks: its program, arguments, working
/// directory, environment changes and stdin.
pub fn command_question(command: &Command, input: Option<&[u8]>) -> Value {
    let envs: BTreeMap<String, Value> = command
        .get_envs()
        .map(|(name, value)| {
            (
                name.to_string_lossy().into_owned(),
                value.map_or(Value::Null, |value| {
                    Value::String(value.to_string_lossy().into_owned())
                }),
            )
        })
        .collect();
    json!({
        "program": command.get_program().to_string_lossy(),
        "args": command.get_args().map(|arg| arg.to_string_lossy().into_owned()).collect::<Vec<_>>(),
        "cwd": command.get_current_dir().map(|dir| dir.to_string_lossy().into_owned()),
        "env": envs,
        "stdin": input.map(bytes_value),
    })
}

/// Run a child process through the recording. `live` runs it for real.
pub fn command_output(
    command: &mut Command,
    input: Option<&[u8]>,
    live: impl FnOnce(&mut Command) -> Result<Output, CommandError>,
) -> Result<Output, CommandError> {
    if is_live() {
        return live(command);
    }
    let key = command_question(command, input);
    answer(
        "command",
        key,
        || live(command),
        encode_command_result,
        decode_command_result,
    )
}

fn encode_command_result(result: &Result<Output, CommandError>) -> Value {
    match result {
        Ok(output) => json!({
            "ok": {
                "status": { "code": output.status.code(), "signal": output.status.signal() },
                "stdout": bytes_value(&output.stdout),
                "stderr": bytes_value(&output.stderr),
            }
        }),
        Err(error) => json!({ "err": encode_command_error(error) }),
    }
}

fn decode_command_result(response: &Value) -> Option<Result<Output, CommandError>> {
    if let Some(ok) = response.get("ok") {
        let status = ok.get("status")?;
        let raw = match (
            status.get("signal").and_then(Value::as_i64),
            status.get("code").and_then(Value::as_i64),
        ) {
            (Some(signal), _) => i32::try_from(signal).ok()?,
            (None, Some(code)) => i32::try_from(code).ok()? << 8,
            (None, None) => 0,
        };
        return Some(Ok(Output {
            status: ExitStatus::from_raw(raw),
            stdout: bytes_from(ok.get("stdout")?)?,
            stderr: bytes_from(ok.get("stderr")?)?,
        }));
    }
    decode_command_error(response.get("err")?).map(Err)
}

fn encode_command_error(error: &CommandError) -> Value {
    match error {
        CommandError::Spawn { program, source } => {
            json!({ "variant": "Spawn", "program": program, "io": io_value(source) })
        }
        CommandError::Wait { program, source } => {
            json!({ "variant": "Wait", "program": program, "io": io_value(source) })
        }
        CommandError::Kill { program, source } => {
            json!({ "variant": "Kill", "program": program, "io": io_value(source) })
        }
        CommandError::Write { program, source } => {
            json!({ "variant": "Write", "program": program, "io": io_value(source) })
        }
        CommandError::Read {
            program,
            stream,
            source,
        } => json!({
            "variant": "Read", "program": program, "stream": stream, "io": io_value(source),
        }),
        CommandError::ReaderPanicked { program, stream } => json!({
            "variant": "ReaderPanicked", "program": program, "stream": stream,
        }),
        CommandError::TimedOut { program, timeout } => json!({
            "variant": "TimedOut", "program": program, "timeout_nanos": timeout.as_nanos() as u64,
        }),
        CommandError::BudgetExhausted { program } => {
            json!({ "variant": "BudgetExhausted", "program": program })
        }
        CommandError::OutputLimit {
            program,
            stream,
            limit,
        } => json!({
            "variant": "OutputLimit", "program": program, "stream": stream, "limit": *limit as u64,
        }),
    }
}

fn decode_command_error(value: &Value) -> Option<CommandError> {
    let program = value.get("program")?.as_str()?.to_owned();
    let stream = || -> Option<&'static str> {
        match value.get("stream")?.as_str()? {
            "stdout" => Some("stdout"),
            "stderr" => Some("stderr"),
            _ => None,
        }
    };
    let source = || io_from(value.get("io")?);
    Some(match value.get("variant")?.as_str()? {
        "Spawn" => CommandError::Spawn {
            program,
            source: source()?,
        },
        "Wait" => CommandError::Wait {
            program,
            source: source()?,
        },
        "Kill" => CommandError::Kill {
            program,
            source: source()?,
        },
        "Write" => CommandError::Write {
            program,
            source: source()?,
        },
        "Read" => CommandError::Read {
            program,
            stream: stream()?,
            source: source()?,
        },
        "ReaderPanicked" => CommandError::ReaderPanicked {
            program,
            stream: stream()?,
        },
        "TimedOut" => CommandError::TimedOut {
            program,
            timeout: Duration::from_nanos(value.get("timeout_nanos")?.as_u64()?),
        },
        "BudgetExhausted" => CommandError::BudgetExhausted { program },
        "OutputLimit" => CommandError::OutputLimit {
            program,
            stream: stream()?,
            limit: usize::try_from(value.get("limit")?.as_u64()?).ok()?,
        },
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hex_round_trips_and_rejects_bad_input() {
        let bytes = [0u8, 0xff, 0x7f, 0x80];
        assert_eq!(hex_decode(&hex_encode(&bytes)).as_deref(), Some(&bytes[..]));
        assert_eq!(hex_decode("abc"), None);
        assert_eq!(hex_decode("zz"), None);
    }

    #[test]
    fn bytes_round_trip_as_text_or_hex() {
        for bytes in [b"plain text".as_slice(), &[0xff, 0x00, 0x41]] {
            assert_eq!(bytes_from(&bytes_value(bytes)).as_deref(), Some(bytes));
        }
    }

    #[test]
    fn io_errors_keep_their_os_code_and_message() {
        let error = io::Error::from_raw_os_error(2);
        let back = io_from(&io_value(&error)).unwrap();
        assert_eq!(back.raw_os_error(), Some(2));
        assert_eq!(back.to_string(), error.to_string());
        let custom = io::Error::new(
            io::ErrorKind::InvalidData,
            "stream did not contain valid UTF-8",
        );
        let back = io_from(&io_value(&custom)).unwrap();
        assert_eq!(back.kind(), io::ErrorKind::InvalidData);
        assert_eq!(back.to_string(), custom.to_string());
    }

    #[test]
    fn questions_do_not_depend_on_field_order() {
        let left = json!({"a": 1, "b": {"y": 2, "x": 3}});
        let right = json!({"b": {"x": 3, "y": 2}, "a": 1});
        assert_eq!(question("env", &left), question("env", &right));
    }

    #[test]
    fn command_errors_round_trip() {
        let errors = [
            CommandError::Spawn {
                program: "tmux".into(),
                source: io::Error::from_raw_os_error(2),
            },
            CommandError::TimedOut {
                program: "hyprctl".into(),
                timeout: Duration::from_millis(2000),
            },
            CommandError::OutputLimit {
                program: "gdbus".into(),
                stream: "stdout",
                limit: 1024,
            },
            CommandError::BudgetExhausted {
                program: "tmux".into(),
            },
        ];
        for error in &errors {
            let back = decode_command_error(&encode_command_error(error)).unwrap();
            assert_eq!(back.to_string(), error.to_string());
        }
    }

    #[test]
    fn command_output_round_trips_status_and_streams() {
        let output = Output {
            status: ExitStatus::from_raw(3 << 8),
            stdout: b"out\n".to_vec(),
            stderr: vec![0xff],
        };
        let back = decode_command_result(&encode_command_result(&Ok(output)))
            .unwrap()
            .unwrap();
        assert_eq!(back.status.code(), Some(3));
        assert_eq!(back.stdout, b"out\n");
        assert_eq!(back.stderr, vec![0xff]);
    }

    #[test]
    fn listings_round_trip() {
        let listing: Listing = Ok(vec![
            Ok((OsString::from("1"), Some(FileKind::Dir))),
            Ok((OsString::from("link"), Some(FileKind::Symlink))),
            Ok((OsString::from("odd"), None)),
            Err(io::Error::from_raw_os_error(5)),
        ]);
        let back = decode_listing(&encode_listing(&listing)).unwrap().unwrap();
        assert_eq!(back.len(), 4);
        assert_eq!(back[0].as_ref().unwrap().1, Some(FileKind::Dir));
        assert_eq!(back[2].as_ref().unwrap().1, None);
        assert_eq!(back[3].as_ref().unwrap_err().raw_os_error(), Some(5));
    }
}
