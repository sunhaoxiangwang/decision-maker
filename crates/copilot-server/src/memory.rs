//! Confirmed-preference memory: single actor, versioned edits, durable write before
//! publication, shared publication lock with lunch decisions, recovery state
//! (PLAN v3 §7, v4 §5).

use copilot_core::limits::{CORRUPT_FILES_KEPT, MEMORY_FILE_MAX, MEMORY_PREFS_MAX, PREF_TEXT_MAX};
use copilot_core::lunch::{MemoryView, PrefKind, Preference, Strength};
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use tokio::sync::{mpsc, oneshot};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct MemoryDoc {
    pub schema_version: u32,
    pub memory_version: u64,
    pub next_id: u64,
    pub prefs: Vec<Preference>,
}

impl MemoryDoc {
    fn empty() -> MemoryDoc {
        MemoryDoc {
            schema_version: 1,
            memory_version: 0,
            next_id: 1,
            prefs: vec![],
        }
    }

    /// Full validation of a loaded document (PLAN v3 §7; review item 8).
    pub fn validate(&self) -> Result<(), String> {
        if self.schema_version != 1 {
            return Err("unsupported schema version".into());
        }
        if self.prefs.len() > MEMORY_PREFS_MAX {
            return Err("too many preferences".into());
        }
        let mut ids = BTreeSet::new();
        for p in &self.prefs {
            p.validate()?;
            if p.id >= self.next_id {
                return Err("preference id not below next_id".into());
            }
            if !ids.insert(p.id) {
                return Err("duplicate preference id".into());
            }
        }
        if self.memory_version == 0 && !self.prefs.is_empty() {
            return Err("memory_version 0 with stored preferences".into());
        }
        Ok(())
    }
}

#[derive(Debug, Clone)]
pub struct Published {
    pub doc: Arc<MemoryDoc>,
    /// Monotonic publication counter (never decreases, even for older files).
    pub publication: u64,
    pub pending_change: bool,
    pub recovery: Option<String>,
    pub fingerprint: Option<String>,
}

impl Published {
    pub fn view(&self) -> MemoryView {
        MemoryView {
            version: self.doc.memory_version,
            publication: self.publication,
            pending_change: self.pending_change,
            recovery_required: self.recovery.clone(),
            prefs: self.doc.prefs.clone(),
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum Change {
    Add {
        kind: PrefKind,
        strength: Strength,
        note: String,
    },
    Delete {
        id: u64,
        version: u64,
    },
}

#[derive(Debug, Clone, PartialEq)]
pub enum SaveResult {
    Saved(u64),
    NotSaved(String),
    Stale,
    Uncertain(String),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FailPoint {
    BeforeRename,
    Rename,
    DirSync,
}

enum Cmd {
    Apply {
        expected: u64,
        change: Change,
        reply: oneshot::Sender<SaveResult>,
    },
    Recheck {
        reply: oneshot::Sender<SaveResult>,
    },
    NewProfile {
        fingerprint: String,
        reply: oneshot::Sender<SaveResult>,
    },
}

/// Called while holding the memory publication lock, with the published version.
pub type Hook = Arc<dyn Fn(u64) + Send + Sync>;

pub struct MemoryHub {
    pub publ: Arc<Mutex<Published>>,
    tx: mpsc::Sender<Cmd>,
    pub path: PathBuf,
}

/// Shared gate for deterministic tests: the actor waits on it between the durable
/// write and publication when present.
pub type Gate = Arc<tokio::sync::Mutex<Option<oneshot::Receiver<()>>>>;

pub fn fingerprint(bytes: &[u8]) -> String {
    // FNV-1a 64-bit: identifies the exact bytes the user saw when confirming recovery.
    let mut h: u64 = 0xcbf29ce484222325;
    for b in bytes {
        h ^= *b as u64;
        h = h.wrapping_mul(0x100000001b3);
    }
    format!("{h:016x}-{}", bytes.len())
}

pub enum FileRead {
    Missing,
    TooLarge(u64),
    Bytes(Vec<u8>),
    Error(String),
}

/// Read at most `MEMORY_FILE_MAX + 1` bytes; never buffers an oversized file.
pub fn read_bounded(path: &Path) -> FileRead {
    let f = match std::fs::File::open(path) {
        Ok(f) => f,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return FileRead::Missing,
        Err(e) => return FileRead::Error(format!("{:?}", e.kind())),
    };
    let mut buf = Vec::new();
    match f.take(MEMORY_FILE_MAX as u64 + 1).read_to_end(&mut buf) {
        Ok(_) if buf.len() > MEMORY_FILE_MAX => FileRead::TooLarge(buf.len() as u64),
        Ok(_) => FileRead::Bytes(buf),
        Err(e) => FileRead::Error(format!("{:?}", e.kind())),
    }
}

fn load(path: &Path) -> Published {
    // start each process above any earlier process's numbers (forms from a
    // previous process are also rejected by the per-process form token)
    let start = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(1)
        .max(1);
    let mk = |doc: MemoryDoc, rec: Option<String>, fp: Option<String>| Published {
        doc: Arc::new(doc),
        publication: start,
        pending_change: false,
        recovery: rec,
        fingerprint: fp,
    };
    match read_bounded(path) {
        FileRead::Missing => mk(MemoryDoc::empty(), None, None),
        FileRead::Error(e) => mk(
            MemoryDoc::empty(),
            Some(format!("cannot read memory file ({e})")),
            None,
        ),
        FileRead::TooLarge(_) => mk(
            MemoryDoc::empty(),
            Some(format!("memory file over {MEMORY_FILE_MAX} bytes")),
            file_fingerprint(path),
        ),
        FileRead::Bytes(bytes) => {
            let fp = fingerprint(&bytes);
            match serde_json::from_slice::<MemoryDoc>(&bytes) {
                Ok(d) => match d.validate() {
                    Ok(()) => mk(d, None, None),
                    Err(e) => mk(
                        MemoryDoc::empty(),
                        Some(format!("memory file failed validation: {e}")),
                        Some(fp),
                    ),
                },
                Err(_) => mk(
                    MemoryDoc::empty(),
                    Some("memory file is corrupt".into()),
                    Some(fp),
                ),
            }
        }
    }
}

/// Fingerprint of the whole file, streamed in chunks (no full buffering). Identical
/// to `fingerprint(bytes)` for the same content. Files beyond the streaming cap also
/// bind their size, modification time and inode so any change is detected.
pub fn file_fingerprint(path: &Path) -> Option<String> {
    const STREAM_CAP: u64 = 256 * 1024 * 1024;
    let f = std::fs::File::open(path).ok()?;
    let meta = f.metadata().ok()?;
    let mut h: u64 = 0xcbf29ce484222325;
    let mut n: u64 = 0;
    let mut r = std::io::BufReader::with_capacity(64 * 1024, f.take(STREAM_CAP));
    let mut buf = [0u8; 64 * 1024];
    loop {
        let k = r.read(&mut buf).ok()?;
        if k == 0 {
            break;
        }
        for b in &buf[..k] {
            h ^= *b as u64;
            h = h.wrapping_mul(0x100000001b3);
        }
        n += k as u64;
    }
    if meta.len() > STREAM_CAP {
        use std::os::unix::fs::MetadataExt;
        return Some(format!(
            "{h:016x}-{}-m{}.{}-i{}",
            meta.len(),
            meta.mtime(),
            meta.mtime_nsec(),
            meta.ino()
        ));
    }
    Some(format!("{h:016x}-{n}"))
}

fn write_durable(path: &Path, bytes: &[u8], fail: Option<FailPoint>) -> Result<(), (bool, String)> {
    // Err((renamed, reason))
    let dir = path.parent().unwrap_or(Path::new("."));
    std::fs::create_dir_all(dir).map_err(|e| (false, format!("create dir: {:?}", e.kind())))?;
    let tmp = dir.join(format!(".memory.json.tmp-{}", std::process::id()));
    let _ = std::fs::remove_file(&tmp);
    let res = (|| -> std::io::Result<()> {
        let mut f = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&tmp)?;
        f.write_all(bytes)?;
        f.sync_all()?;
        if fail == Some(FailPoint::BeforeRename) {
            return Err(std::io::Error::other("injected failure before rename"));
        }
        Ok(())
    })();
    if let Err(e) = res {
        let _ = std::fs::remove_file(&tmp);
        return Err((false, format!("write: {e}")));
    }
    if fail == Some(FailPoint::Rename) {
        let _ = std::fs::remove_file(&tmp);
        return Err((false, "rename: injected failure".into()));
    }
    if let Err(e) = std::fs::rename(&tmp, path) {
        let _ = std::fs::remove_file(&tmp);
        return Err((false, format!("rename: {:?}", e.kind())));
    }
    let sync_dir = || -> std::io::Result<()> {
        if fail == Some(FailPoint::DirSync) {
            return Err(std::io::Error::other("injected directory sync failure"));
        }
        std::fs::File::open(dir)?.sync_all()
    };
    if sync_dir().is_err() && sync_dir().is_err() {
        return Err((true, "directory flush failed after rename".into()));
    }
    Ok(())
}

impl MemoryHub {
    pub fn start(
        path: PathBuf,
        hook: Hook,
        fail: Option<FailPoint>,
        gate: Option<Gate>,
    ) -> MemoryHub {
        let publ = Arc::new(Mutex::new(load(&path)));
        let (tx, rx) = mpsc::channel(16);
        tokio::spawn(actor(path.clone(), publ.clone(), rx, hook, fail, gate));
        MemoryHub { publ, tx, path }
    }

    pub fn view(&self) -> MemoryView {
        self.publ.lock().expect("memory lock").view()
    }

    async fn send(&self, mk: impl FnOnce(oneshot::Sender<SaveResult>) -> Cmd) -> SaveResult {
        let (rtx, rrx) = oneshot::channel();
        if self.tx.try_send(mk(rtx)).is_err() {
            return SaveResult::NotSaved("memory writer busy".into());
        }
        match tokio::time::timeout(std::time::Duration::from_secs(5), rrx).await {
            Ok(Ok(r)) => r,
            _ => SaveResult::Uncertain("save outcome unknown; load latest".into()),
        }
    }

    pub async fn apply(&self, expected: u64, change: Change) -> SaveResult {
        self.send(|reply| Cmd::Apply {
            expected,
            change,
            reply,
        })
        .await
    }
    pub async fn recheck(&self) -> SaveResult {
        self.send(|reply| Cmd::Recheck { reply }).await
    }
    pub async fn new_profile(&self, fingerprint: String) -> SaveResult {
        self.send(|reply| Cmd::NewProfile { fingerprint, reply })
            .await
    }
}

/// Publish under the memory lock. The publication counter always increases, even
/// when `f` replaces the whole state (restored older file, profile reset).
fn publish_with(publ: &Mutex<Published>, hook: &Hook, f: impl FnOnce(&mut Published)) {
    let mut p = publ.lock().expect("memory lock");
    let next = p.publication + 1;
    f(&mut p);
    p.publication = next;
    hook(next);
}

async fn actor(
    path: PathBuf,
    publ: Arc<Mutex<Published>>,
    mut rx: mpsc::Receiver<Cmd>,
    hook: Hook,
    fail: Option<FailPoint>,
    gate: Option<Gate>,
) {
    while let Some(cmd) = rx.recv().await {
        match cmd {
            Cmd::Apply {
                expected,
                change,
                reply,
            } => {
                let current = publ.lock().expect("memory lock").clone();
                if let Some(r) = &current.recovery {
                    let _ = reply.send(SaveResult::NotSaved(format!("recovery required: {r}")));
                    continue;
                }
                // edits are bound to the monotonic publication number, never the
                // reusable file version (a reset or restore can repeat file versions)
                if current.publication != expected || current.pending_change {
                    let _ = reply.send(SaveResult::Stale);
                    continue;
                }
                let mut doc = (*current.doc).clone();
                match &change {
                    Change::Add {
                        kind,
                        strength,
                        note,
                    } => {
                        if doc.prefs.len() >= MEMORY_PREFS_MAX {
                            let _ = reply.send(SaveResult::NotSaved("memory_full".into()));
                            continue;
                        }
                        let (n, _) = copilot_core::text::bounded(note, PREF_TEXT_MAX);
                        let pref = Preference {
                            id: doc.next_id,
                            version: 1,
                            kind: kind.clone(),
                            strength: *strength,
                            note: n,
                            provenance: "user entry via UI".into(),
                        };
                        if let Err(e) = pref.validate() {
                            let _ = reply
                                .send(SaveResult::NotSaved(format!("invalid preference: {e}")));
                            continue;
                        }
                        doc.prefs.push(pref);
                        doc.next_id += 1;
                    }
                    Change::Delete { id, version } => {
                        let before = doc.prefs.len();
                        doc.prefs
                            .retain(|p| !(p.id == *id && p.version == *version));
                        if doc.prefs.len() == before {
                            let _ = reply.send(SaveResult::Stale);
                            continue;
                        }
                    }
                }
                doc.memory_version += 1;
                let bytes = match serde_json::to_vec_pretty(&doc) {
                    Ok(b) if b.len() <= MEMORY_FILE_MAX => b,
                    _ => {
                        let _ = reply.send(SaveResult::NotSaved("memory_full".into()));
                        continue;
                    }
                };
                // pending change: blocks lunch commits and clears any selection
                publish_with(&publ, &hook, |p| p.pending_change = true);
                let path2 = path.clone();
                let bytes2 = bytes.clone();
                let res = tokio::task::spawn_blocking(move || write_durable(&path2, &bytes2, fail))
                    .await
                    .unwrap_or(Err((false, "writer task failed".into())));
                if let Some(g) = &gate {
                    if let Some(rx) = g.lock().await.take() {
                        let _ = rx.await;
                    }
                }
                let result = match res {
                    Ok(()) => {
                        publish_with(&publ, &hook, |p| {
                            p.doc = Arc::new(doc.clone());
                            p.pending_change = false;
                        });
                        SaveResult::Saved(doc.memory_version)
                    }
                    Err((false, why)) => {
                        publish_with(&publ, &hook, |p| p.pending_change = false);
                        SaveResult::NotSaved(why)
                    }
                    Err((true, why)) => {
                        reconcile_after_rename(&path, &publ, &hook, &doc, &why);
                        SaveResult::Uncertain(why)
                    }
                };
                let _ = reply.send(result);
            }
            Cmd::Recheck { reply } => {
                let current = publ.lock().expect("memory lock").clone();
                let durability_only = current
                    .recovery
                    .as_deref()
                    .is_some_and(|r| r.starts_with("durability_unknown"));
                let fresh = load(&path);
                let ok_sync = path
                    .parent()
                    .map(|d| std::fs::File::open(d).and_then(|f| f.sync_all()).is_ok())
                    .unwrap_or(false);
                if fresh.recovery.is_none() && (!durability_only || ok_sync) {
                    let mut v = 0;
                    publish_with(&publ, &hook, |p| {
                        *p = fresh;
                        v = p.doc.memory_version;
                    });
                    let _ = reply.send(SaveResult::Saved(v));
                } else {
                    if fresh.recovery.is_some() {
                        // show the user the file as it is now (reason and identity)
                        let (r, fp) = (fresh.recovery.clone(), fresh.fingerprint.clone());
                        publish_with(&publ, &hook, |p| {
                            p.recovery = r;
                            p.fingerprint = fp;
                        });
                    }
                    let _ = reply.send(SaveResult::NotSaved(
                        fresh
                            .recovery
                            .unwrap_or_else(|| "directory flush still failing".into()),
                    ));
                }
            }
            Cmd::NewProfile {
                fingerprint: fp,
                reply,
            } => {
                let current = publ.lock().expect("memory lock").clone();
                if current.recovery.is_none() || current.fingerprint.as_deref() != Some(fp.as_str())
                {
                    let _ = reply.send(SaveResult::Stale);
                    continue;
                }
                // the file must still be exactly what the user saw when confirming
                if file_fingerprint(&path).as_deref() != Some(fp.as_str()) {
                    let _ = reply.send(SaveResult::Stale);
                    continue;
                }
                let dir = path.parent().unwrap_or(Path::new(".")).to_path_buf();
                let corrupt_count = std::fs::read_dir(&dir)
                    .map(|r| {
                        r.filter_map(|e| e.ok())
                            .filter(|e| {
                                e.file_name()
                                    .to_string_lossy()
                                    .starts_with("memory.corrupt-")
                            })
                            .count()
                    })
                    .unwrap_or(0);
                if corrupt_count >= CORRUPT_FILES_KEPT {
                    let _ = reply.send(SaveResult::NotSaved(
                        "three corrupt copies already kept; remove one first".into(),
                    ));
                    continue;
                }
                // 1) keep a copy of the bad file under a collision-safe name (the
                //    original stays in place, so recovery survives any failure)
                let quarantine = match quarantine_copy(&path, &dir) {
                    Ok(q) => q,
                    Err(why) => {
                        let _ = reply.send(SaveResult::NotSaved(why));
                        continue;
                    }
                };
                // 2) atomically replace memory.json with the new empty profile; the
                //    file is never missing in between
                let doc = MemoryDoc::empty();
                let bytes = serde_json::to_vec_pretty(&doc).unwrap_or_default();
                match write_durable(&path, &bytes, fail) {
                    Ok(()) => {
                        publish_with(&publ, &hook, |p| {
                            *p = Published {
                                doc: Arc::new(doc),
                                publication: 0,
                                pending_change: false,
                                recovery: None,
                                fingerprint: None,
                            };
                        });
                        let _ = reply.send(SaveResult::Saved(0));
                    }
                    Err((true, why)) => {
                        // the rename happened: memory.json already holds the new
                        // profile; only durability is unknown. Publish what is on disk
                        // and keep (non-persisted) recovery until a successful Re-check.
                        reconcile_after_rename(&path, &publ, &hook, &doc, &why);
                        let _ = reply.send(SaveResult::Uncertain(why));
                    }
                    Err((false, why)) => {
                        // nothing replaced: the corrupt file is still memory.json
                        let _ = std::fs::remove_file(&quarantine);
                        let _ = reply.send(SaveResult::NotSaved(format!(
                            "new profile not written ({why}); recovery still required"
                        )));
                    }
                }
            }
        }
    }
}

/// Copy the current file to `memory.corrupt-<secs>-<nanos>-<pid>-<n>.json`, created
/// with `create_new` so an existing quarantine copy is never overwritten.
fn quarantine_copy(path: &Path, dir: &Path) -> Result<PathBuf, String> {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default();
    for n in 0..100u32 {
        let q = dir.join(format!(
            "memory.corrupt-{}-{:09}-{}-{n}.json",
            now.as_secs(),
            now.subsec_nanos(),
            std::process::id()
        ));
        let mut out = match std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&q)
        {
            Ok(f) => f,
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(e) => return Err(format!("quarantine: {:?}", e.kind())),
        };
        let res = std::fs::File::open(path)
            .and_then(|mut src| std::io::copy(&mut src, &mut out))
            .and_then(|_| out.sync_all());
        return match res {
            Ok(()) => Ok(q),
            Err(e) => {
                let _ = std::fs::remove_file(&q);
                Err(format!("quarantine copy failed: {:?}", e.kind()))
            }
        };
    }
    Err("no free quarantine name".into())
}

/// After a successful rename whose durability is unknown: publish exactly what is on
/// disk. If it is the intended document, publish it with a (non-persisted)
/// `durability_unknown` recovery that a successful Re-check clears; otherwise mark
/// the disk state unknown with the file's current identity. Lunch stays blocked
/// either way until Re-check succeeds.
fn reconcile_after_rename(
    path: &Path,
    publ: &Mutex<Published>,
    hook: &Hook,
    doc: &MemoryDoc,
    why: &str,
) {
    let on_disk = match read_bounded(path) {
        FileRead::Bytes(b) => serde_json::from_slice::<MemoryDoc>(&b).ok(),
        _ => None,
    };
    let fp = file_fingerprint(path);
    publish_with(publ, hook, |p| {
        p.pending_change = false;
        if on_disk.as_ref() == Some(doc) {
            p.doc = Arc::new(doc.clone());
            p.recovery = Some(format!("durability_unknown: {why}; press Re-check"));
            p.fingerprint = None;
        } else {
            p.recovery = Some(format!("disk_state_unknown: {why}"));
            p.fingerprint = fp;
        }
    });
}
