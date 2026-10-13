//! SQLite catalog: an index over raw files for searching, filtering, sorting and albums.
//!
//! Sidecars stay the source of truth for adjustments, ratings, labels and keywords;
//! the catalog mirrors them so that queries across folders are fast.
//!
//! A session database (`.sksession`) stores paths relative to the session folder, with `/` as
//! separator, so that the session can be moved, renamed or copied to another computer.

use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result};
use rusqlite::{Connection, OptionalExtension, params, params_from_iter, types::Value};
use serde::{Deserialize, Serialize};

use crate::raw::{self, RawMeta};
use crate::sidecar::{self, ColorLabel, Sidecar, Stack};

#[derive(Clone, Debug, PartialEq)]
pub struct ImageRecord {
    pub id: i64,
    pub path: PathBuf,
    pub name: String,
    pub capture_date: Option<String>,
    pub camera: Option<String>,
    pub lens: Option<String>,
    pub iso: Option<u32>,
    pub rating: u8,
    pub color_label: ColorLabel,
    pub keywords: Vec<String>,
    pub stack: Option<Stack>,
    /// Result of the face and culling analysis, if the image was analysed.
    pub culling: Option<Culling>,
    /// People recognized in the image (ids of `Person`).
    pub persons: Vec<i64>,
}

/// Culling information of an image (see `store_analysis`).
#[derive(Clone, Copy, Debug, PartialEq, Default)]
pub struct Culling {
    pub faces: u32,
    /// Faces with closed eyes.
    pub eyes_closed: u32,
    /// Sharpness of the faces, or of the sharpest part (about 0.4 and more for sharp images).
    pub sharpness: f32,
}

/// Below this sharpness an image counts as blurry.
pub const BLURRY: f32 = 0.22;

impl Culling {
    pub fn blurry(&self) -> bool {
        self.sharpness < BLURRY
    }
}

/// What the culling filter shows.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum CullFilter {
    EyesClosed,
    Blurry,
    WithFaces,
    NotAnalyzed,
}

impl CullFilter {
    pub const ALL: [CullFilter; 4] = [CullFilter::EyesClosed, CullFilter::Blurry, CullFilter::WithFaces, CullFilter::NotAnalyzed];

    pub fn label(self) -> &'static str {
        match self {
            CullFilter::EyesClosed => "Augen geschlossen",
            CullFilter::Blurry => "Unscharf",
            CullFilter::WithFaces => "Mit Gesichtern",
            CullFilter::NotAnalyzed => "Nicht analysiert",
        }
    }

    pub fn matches(self, c: Option<&Culling>) -> bool {
        match (self, c) {
            (CullFilter::NotAnalyzed, c) => c.is_none(),
            (_, None) => false,
            (CullFilter::EyesClosed, Some(c)) => c.eyes_closed > 0,
            (CullFilter::Blurry, Some(c)) => c.blurry(),
            (CullFilter::WithFaces, Some(c)) => c.faces > 0,
        }
    }
}

/// A person recognized by face (named or not).
#[derive(Clone, Debug, PartialEq)]
pub struct Person {
    pub id: i64,
    pub name: Option<String>,
    /// Images showing the person.
    pub count: usize,
    /// A face of the person: image and box (normalized x, y, w, h).
    pub sample: Option<(PathBuf, [f32; 4])>,
}

/// A face found by the analysis.
#[derive(Clone, Debug, PartialEq)]
pub struct FaceRecord {
    pub bbox: [f32; 4],
    pub embedding: Vec<f32>,
    pub eyes_closed: f32,
}

fn embedding_to_blob(e: &[f32]) -> Vec<u8> {
    e.iter().flat_map(|v| v.to_le_bytes()).collect()
}

fn embedding_from_blob(b: &[u8]) -> Vec<f32> {
    b.as_chunks::<4>().0.iter().map(|c| f32::from_le_bytes(*c)).collect()
}

/// Eyes count as closed above this probability.
pub const EYES_CLOSED: f32 = 0.5;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum SortKey {
    #[default]
    Name,
    CaptureDate,
    Rating,
}

impl SortKey {
    pub const ALL: [SortKey; 3] = [SortKey::Name, SortKey::CaptureDate, SortKey::Rating];

    pub fn label(self) -> &'static str {
        match self {
            SortKey::Name => "Dateiname",
            SortKey::CaptureDate => "Aufnahmedatum",
            SortKey::Rating => "Bewertung",
        }
    }
}

/// Filter criteria; also the definition of a smart album.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct Filter {
    /// Free text: matched against file name, camera, lens and keywords.
    pub text: String,
    pub min_rating: u8,
    pub color_label: Option<ColorLabel>,
    pub keyword: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub culling: Option<CullFilter>,
}

impl Filter {
    pub fn is_empty(&self) -> bool {
        *self == Filter::default()
    }

    /// In-memory evaluation, consistent with the SQL query.
    pub fn matches(&self, r: &ImageRecord) -> bool {
        if r.rating < self.min_rating {
            return false;
        }
        if self.color_label.is_some_and(|l| l != r.color_label) {
            return false;
        }
        if let Some(k) = &self.keyword
            && !r.keywords.iter().any(|x| x.eq_ignore_ascii_case(k))
        {
            return false;
        }
        if let Some(c) = self.culling
            && !c.matches(r.culling.as_ref())
        {
            return false;
        }
        let text = self.text.trim().to_lowercase();
        if !text.is_empty() {
            let hay = [
                r.name.to_lowercase(),
                r.camera.clone().unwrap_or_default().to_lowercase(),
                r.lens.clone().unwrap_or_default().to_lowercase(),
                r.keywords.join(" ").to_lowercase(),
            ];
            if !text.split_whitespace().all(|t| hay.iter().any(|h| h.contains(t))) {
                return false;
            }
        }
        true
    }
}

#[derive(Clone, Debug, PartialEq)]
pub enum Scope {
    All,
    Folder { path: PathBuf, recursive: bool },
    Album(i64),
    /// Images added by the most recent import.
    LastImport,
    /// Images showing a person.
    Person(i64),
}

#[derive(Clone, Debug, PartialEq)]
pub struct Album {
    pub id: i64,
    pub name: String,
    /// `Some` for smart albums.
    pub smart: Option<Filter>,
    pub count: usize,
}

/// Format of the databases written by this build (`PRAGMA user_version`; older databases are
/// upgraded on opening, newer ones are refused). Raise it when an upgrade an older build
/// can't cope with is added to [`Catalog::init`].
pub const SCHEMA_VERSION: u32 = 1;

pub struct Catalog {
    conn: Connection,
    pub path: PathBuf,
    /// Folder that stored paths are relative to (the session folder); `None` for the catalog.
    base: Option<PathBuf>,
}

pub fn default_catalog_path() -> PathBuf {
    crate::paths::data_dir().join("catalog.sqlite")
}

fn now() -> i64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.as_secs() as i64)
}

fn file_stamp(path: &Path) -> (i64, i64) {
    std::fs::metadata(crate::variant::file_of(path))
        .map(|m| {
            let mtime = m.modified().ok().and_then(|t| t.duration_since(UNIX_EPOCH).ok()).map_or(0, |d| d.as_secs() as i64);
            (mtime, m.len() as i64)
        })
        .unwrap_or((0, 0))
}

fn label_to_str(l: ColorLabel) -> String {
    serde_json::to_string(&l).unwrap_or_default().trim_matches('"').to_string()
}

fn label_from_str(s: &str) -> ColorLabel {
    serde_json::from_str(&format!("\"{s}\"")).unwrap_or_default()
}

impl Catalog {
    pub fn open(path: &Path) -> Result<Self> {
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        let conn = Connection::open(path).with_context(|| format!("Katalog {} nicht lesbar", path.display()))?;
        let is_session = path.extension().is_some_and(|e| e.eq_ignore_ascii_case(crate::session::SESSION_EXT));
        let base = is_session.then(|| path.parent().map(|d| d.canonicalize().unwrap_or_else(|_| d.to_path_buf()))).flatten();
        Self::init(conn, path.to_path_buf(), base)
    }

    pub fn open_in_memory() -> Result<Self> {
        Self::init(Connection::open_in_memory()?, PathBuf::from(":memory:"), None)
    }

    /// Text stored for a path: relative to the base folder (joined with `/`) if inside it, else absolute.
    fn key(&self, path: &Path) -> String {
        match self.base.as_ref().and_then(|b| path.strip_prefix(b).ok()) {
            Some(rel) => rel.components().map(|c| c.as_os_str().to_string_lossy()).collect::<Vec<_>>().join("/"),
            None => path.to_string_lossy().to_string(),
        }
    }

    fn folder_key(&self, path: &Path) -> String {
        self.key(path.parent().unwrap_or(Path::new("")))
    }

    /// Inverse of [`Self::key`].
    fn resolve(&self, stored: &str) -> PathBuf {
        match &self.base {
            // A leading '/' is an absolute Unix path even when read on Windows.
            Some(b) if !Path::new(stored).is_absolute() && !stored.starts_with('/') => {
                stored.split('/').filter(|c| !c.is_empty()).fold(b.clone(), |p, c| p.join(c))
            }
            _ => PathBuf::from(stored),
        }
    }

    /// Converts absolute paths inside the base folder to relative ones (databases written
    /// before sessions stored relative paths).
    fn make_paths_relative(&self) -> Result<()> {
        let rows: Vec<(i64, String)> = self
            .conn
            .prepare("SELECT id, path FROM images")?
            .query_map([], |r| Ok((r.get(0)?, r.get(1)?)))?
            .collect::<rusqlite::Result<_>>()?;
        for (id, stored) in rows {
            let path = PathBuf::from(&stored);
            if path.is_absolute() && self.key(&path) != stored {
                self.conn.execute(
                    "UPDATE OR REPLACE images SET path = ?2, folder = ?3 WHERE id = ?1",
                    params![id, self.key(&path), self.folder_key(&path)],
                )?;
            }
        }
        let favs: Vec<String> =
            self.conn.prepare("SELECT path FROM favorites")?.query_map([], |r| r.get(0))?.collect::<rusqlite::Result<_>>()?;
        for stored in favs {
            self.conn.execute("UPDATE OR REPLACE favorites SET path = ?2 WHERE path = ?1", params![stored, self.key(Path::new(&stored))])?;
        }
        Ok(())
    }

    fn init(conn: Connection, path: PathBuf, base: Option<PathBuf>) -> Result<Self> {
        conn.busy_timeout(std::time::Duration::from_secs(5))?;
        let version: u32 = conn.query_row("PRAGMA user_version", [], |r| r.get(0))?;
        anyhow::ensure!(
            version <= SCHEMA_VERSION,
            "{} wurde von einer neueren Silberkorn-Version angelegt (Format {version}, unterstützt bis {SCHEMA_VERSION}). Bitte Silberkorn aktualisieren.",
            path.display()
        );
        conn.execute_batch(
            "PRAGMA journal_mode=WAL;
             PRAGMA foreign_keys=ON;
             CREATE TABLE IF NOT EXISTS images (
                 id INTEGER PRIMARY KEY,
                 path TEXT NOT NULL UNIQUE,
                 folder TEXT NOT NULL,
                 name TEXT NOT NULL,
                 capture_date TEXT,
                 camera TEXT,
                 lens TEXT,
                 iso INTEGER,
                 rating INTEGER NOT NULL DEFAULT 0,
                 color_label TEXT NOT NULL DEFAULT 'None',
                 file_mtime INTEGER NOT NULL DEFAULT 0,
                 file_size INTEGER NOT NULL DEFAULT 0,
                 import_id INTEGER NOT NULL DEFAULT 0
             );
             CREATE INDEX IF NOT EXISTS images_folder ON images(folder);
             CREATE TABLE IF NOT EXISTS keywords (
                 image_id INTEGER NOT NULL REFERENCES images(id) ON DELETE CASCADE,
                 keyword TEXT NOT NULL COLLATE NOCASE,
                 PRIMARY KEY (image_id, keyword)
             );
             CREATE TABLE IF NOT EXISTS albums (
                 id INTEGER PRIMARY KEY,
                 name TEXT NOT NULL,
                 smart TEXT
             );
             CREATE TABLE IF NOT EXISTS album_images (
                 album_id INTEGER NOT NULL REFERENCES albums(id) ON DELETE CASCADE,
                 image_id INTEGER NOT NULL REFERENCES images(id) ON DELETE CASCADE,
                 PRIMARY KEY (album_id, image_id)
             );
             CREATE TABLE IF NOT EXISTS meta (key TEXT PRIMARY KEY, value TEXT);
             CREATE TABLE IF NOT EXISTS favorites (path TEXT PRIMARY KEY);",
        )?;
        // Columns added later.
        let columns: Vec<String> =
            conn.prepare("SELECT name FROM pragma_table_info('images')")?.query_map([], |r| r.get(0))?.collect::<rusqlite::Result<_>>()?;
        conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS persons (id INTEGER PRIMARY KEY, name TEXT);
             CREATE TABLE IF NOT EXISTS faces (
                 id INTEGER PRIMARY KEY,
                 image_id INTEGER NOT NULL REFERENCES images(id) ON DELETE CASCADE,
                 x REAL, y REAL, w REAL, h REAL,
                 embedding BLOB,
                 eyes_closed REAL NOT NULL DEFAULT 0,
                 person_id INTEGER REFERENCES persons(id) ON DELETE SET NULL
             );
             CREATE INDEX IF NOT EXISTS faces_image ON faces(image_id);
             CREATE INDEX IF NOT EXISTS faces_person ON faces(person_id);",
        )?;
        for (name, def) in [
            ("stack", "TEXT"),
            ("stack_top", "INTEGER NOT NULL DEFAULT 0"),
            ("analyzed", "INTEGER NOT NULL DEFAULT 0"),
            ("sharpness", "REAL"),
            ("face_count", "INTEGER NOT NULL DEFAULT 0"),
            ("eyes_closed", "INTEGER NOT NULL DEFAULT 0"),
        ] {
            if !columns.iter().any(|c| c == name) {
                conn.execute_batch(&format!("ALTER TABLE images ADD COLUMN {name} {def}"))?;
            }
        }
        conn.execute_batch(&format!("PRAGMA user_version = {SCHEMA_VERSION}"))?;
        let catalog = Self { conn, path, base };
        if catalog.base.is_some() {
            catalog.make_paths_relative()?;
        }
        Ok(catalog)
    }

    /// True if the file is unknown or changed since it was indexed.
    pub fn needs_index(&self, path: &Path) -> Result<bool> {
        let (mtime, size) = file_stamp(path);
        let row: Option<(i64, i64)> = self
            .conn
            .query_row(
                "SELECT file_mtime, file_size FROM images WHERE path = ?1",
                [self.key(path)],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .optional()?;
        Ok(row != Some((mtime, size)))
    }

    /// Starts a new import; images indexed with the returned id form the "last import".
    pub fn begin_import(&self) -> Result<i64> {
        let id = now();
        self.conn.execute("INSERT OR REPLACE INTO meta(key, value) VALUES ('last_import', ?1)", [id.to_string()])?;
        Ok(id)
    }

    /// Reads metadata + sidecar of `path` and stores it.
    pub fn index_file(&self, path: &Path, import_id: i64) -> Result<i64> {
        let meta = raw::read_meta(path)?;
        let sc = sidecar::load(path);
        self.upsert(&meta, &sc, import_id)
    }

    pub fn upsert(&self, meta: &RawMeta, sc: &Sidecar, import_id: i64) -> Result<i64> {
        let path = &meta.path;
        let (mtime, size) = file_stamp(path);
        let folder = self.folder_key(path);
        let name = crate::variant::display_name(path);
        let info = &meta.info;
        let camera = format!("{} {}", info.make, info.model).trim().to_string();
        // A variant belongs to the import of its file.
        let import_id = match crate::variant::split(path) {
            (file, n) if n > 1 && import_id == 0 => self
                .conn
                .query_row("SELECT import_id FROM images WHERE path = ?1", [self.key(&file)], |r| r.get(0))
                .unwrap_or(0),
            _ => import_id,
        };
        // A variant is in the stack of its file.
        let stack = if crate::variant::number(path) > 1 { sidecar::load(path).stack } else { sc.stack.clone() };
        // Keep the original import id when re-indexing a changed file.
        let id: i64 = self.conn.query_row(
            "INSERT INTO images (path, folder, name, capture_date, camera, lens, iso, rating, color_label, file_mtime, file_size, import_id, stack, stack_top)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14)
             ON CONFLICT(path) DO UPDATE SET
                 folder=excluded.folder, name=excluded.name, capture_date=excluded.capture_date,
                 camera=excluded.camera, lens=excluded.lens, iso=excluded.iso, rating=excluded.rating,
                 color_label=excluded.color_label, file_mtime=excluded.file_mtime, file_size=excluded.file_size,
                 stack=excluded.stack, stack_top=excluded.stack_top
             RETURNING id",
            params![
                self.key(path),
                folder,
                name,
                info.date,
                camera,
                info.lens,
                info.iso,
                sc.rating,
                label_to_str(sc.color_label),
                mtime,
                size,
                import_id,
                stack.as_ref().map(|s| s.id.clone()),
                stack.as_ref().is_some_and(|s| s.top)
            ],
            |r| r.get(0),
        )?;
        self.write_keywords(id, &sc.keywords)?;
        Ok(id)
    }

    fn write_keywords(&self, id: i64, keywords: &[String]) -> Result<()> {
        self.conn.execute("DELETE FROM keywords WHERE image_id = ?1", [id])?;
        for k in keywords {
            self.conn.execute("INSERT OR IGNORE INTO keywords(image_id, keyword) VALUES (?1, ?2)", params![id, k])?;
        }
        Ok(())
    }

    /// Mirrors the editable metadata from a sidecar (no-op if the image is not indexed).
    pub fn sync_sidecar(&self, path: &Path, sc: &Sidecar) -> Result<()> {
        let id: Option<i64> =
            self.conn.query_row("SELECT id FROM images WHERE path = ?1", [self.key(path)], |r| r.get(0)).optional()?;
        if let Some(id) = id {
            self.conn.execute(
                "UPDATE images SET rating = ?2, color_label = ?3 WHERE id = ?1",
                params![id, sc.rating, label_to_str(sc.color_label)],
            )?;
            self.write_keywords(id, &sc.keywords)?;
        }
        Ok(())
    }

    /// Mirrors the stack of an image (no-op if it is not indexed).
    pub fn set_stack(&self, path: &Path, stack: Option<&Stack>) -> Result<()> {
        self.conn.execute(
            "UPDATE images SET stack = ?2, stack_top = ?3 WHERE path = ?1",
            params![self.key(path), stack.map(|s| s.id.clone()), stack.is_some_and(|s| s.top)],
        )?;
        Ok(())
    }

    /// Removes entries whose files no longer exist. Returns the number removed.
    ///
    /// Files in a folder that can't be reached (an unplugged drive, an offline share) and
    /// variants whose sidecar can't be read are kept: they are probably only out of reach, and
    /// removing them would lose their albums, keywords and faces.
    pub fn remove_missing(&self) -> Result<usize> {
        let paths: Vec<(i64, String)> = self
            .conn
            .prepare("SELECT id, path FROM images")?
            .query_map([], |r| Ok((r.get(0)?, r.get(1)?)))?
            .collect::<rusqlite::Result<_>>()?;
        let mut n = 0;
        for (id, p) in paths {
            let path = self.resolve(&p);
            let (file, variant) = crate::variant::split(&path);
            let reachable = file.parent().is_some_and(|d| d.is_dir());
            let gone = match file.try_exists() {
                Ok(false) => reachable,
                Ok(true) if variant > 1 => crate::sidecar::variant_ids(&file).is_ok_and(|ids| !ids.contains(&variant)),
                _ => false,
            };
            if gone {
                self.conn.execute("DELETE FROM images WHERE id = ?1", [id])?;
                n += 1;
            }
        }
        Ok(n)
    }

    /// Follows a file that was moved or renamed, with its variants; album memberships and
    /// keywords are kept.
    pub fn rename_path(&self, from: &Path, to: &Path) -> Result<()> {
        if crate::variant::number(from) == 1 {
            for (old, _) in self.variant_rows(from)? {
                let n = crate::variant::number(&old);
                self.rename_one(&old, &crate::variant::path_for(to, n))?;
            }
        }
        self.rename_one(from, to)
    }

    /// Stored further variants of a file: (path, id).
    fn variant_rows(&self, file: &Path) -> Result<Vec<(PathBuf, i64)>> {
        let prefix = format!("{}#", self.key(file));
        let rows = self
            .conn
            .prepare("SELECT path, id FROM images WHERE substr(path, 1, length(?1)) = ?1")?
            .query_map([&prefix], |r| Ok((r.get::<_, String>(0)?, r.get::<_, i64>(1)?)))?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(rows.into_iter().map(|(p, id)| (self.resolve(&p), id)).filter(|(p, _)| crate::variant::number(p) > 1).collect())
    }

    fn rename_one(&self, from: &Path, to: &Path) -> Result<()> {
        let (from, to_key) = (self.key(from), self.key(to));
        let name = crate::variant::display_name(to);
        // A stale entry at the destination would violate the unique path.
        self.conn.execute("DELETE FROM images WHERE path = ?1 AND path <> ?2", params![to_key, from])?;
        self.conn.execute(
            "UPDATE images SET path = ?2, folder = ?3, name = ?4 WHERE path = ?1",
            params![from, to_key, self.folder_key(to), name],
        )?;
        Ok(())
    }

    /// Removes an image; for a raw file also its variants.
    pub fn remove_path(&self, path: &Path) -> Result<()> {
        if crate::variant::number(path) == 1 {
            for (_, id) in self.variant_rows(path)? {
                self.conn.execute("DELETE FROM images WHERE id = ?1", [id])?;
            }
        }
        self.conn.execute("DELETE FROM images WHERE path = ?1", [self.key(path)])?;
        Ok(())
    }

    // ------------------------------------------------------------ faces and culling

    fn image_id(&self, path: &Path) -> Result<Option<i64>> {
        Ok(self.conn.query_row("SELECT id FROM images WHERE path = ?1", [self.key(path)], |r| r.get(0)).optional()?)
    }

    /// Has the image been analysed (faces, sharpness)?
    pub fn is_analyzed(&self, path: &Path) -> bool {
        self.conn
            .query_row("SELECT analyzed FROM images WHERE path = ?1", [self.key(path)], |r| r.get::<_, i64>(0))
            .is_ok_and(|a| a > 0)
    }

    /// Normalized mean embedding of each person.
    fn person_centroids(&self) -> Result<Vec<(i64, Vec<f32>)>> {
        let mut stmt = self.conn.prepare("SELECT person_id, embedding FROM faces WHERE person_id IS NOT NULL AND embedding IS NOT NULL")?;
        let rows: Vec<(i64, Vec<u8>)> = stmt.query_map([], |r| Ok((r.get(0)?, r.get(1)?)))?.collect::<rusqlite::Result<_>>()?;
        let mut sums: Vec<(i64, Vec<f32>)> = Vec::new();
        for (pid, blob) in rows {
            let e = embedding_from_blob(&blob);
            match sums.iter_mut().find(|(p, _)| *p == pid) {
                Some((_, s)) if s.len() == e.len() => s.iter_mut().zip(&e).for_each(|(a, b)| *a += b),
                Some(_) => {}
                None => sums.push((pid, e)),
            }
        }
        for (_, s) in &mut sums {
            let n = s.iter().map(|v| v * v).sum::<f32>().sqrt().max(1e-9);
            s.iter_mut().for_each(|v| *v /= n);
        }
        Ok(sums)
    }

    /// Stores the analysis of an image (and its variants): sharpness and faces. Each face goes to
    /// the most similar known person (cosine similarity of the embeddings above `same_person`)
    /// or becomes a new, unnamed person.
    pub fn store_analysis(&self, path: &Path, sharpness: f32, faces: &[FaceRecord], same_person: f32) -> Result<()> {
        let Some(id) = self.image_id(path)? else { return Ok(()) };
        let closed = faces.iter().filter(|f| f.eyes_closed > EYES_CLOSED).count();
        // All or nothing: an error halfway must not leave the image without its faces.
        let tx = self.conn.unchecked_transaction()?;
        // Before the old faces go, so that analysing again finds the same (named) persons even
        // when this image was their only one.
        let mut centroids = self.person_centroids()?;
        self.conn.execute("DELETE FROM faces WHERE image_id = ?1", [id])?;
        for f in faces {
            let best = centroids
                .iter()
                .map(|(pid, c)| (*pid, c.iter().zip(&f.embedding).map(|(a, b)| a * b).sum::<f32>()))
                .max_by(|a, b| a.1.total_cmp(&b.1))
                .filter(|(_, s)| *s > same_person);
            let pid = match best {
                Some((pid, _)) => pid,
                None => {
                    self.conn.execute("INSERT INTO persons(name) VALUES (NULL)", [])?;
                    let pid = self.conn.last_insert_rowid();
                    centroids.push((pid, f.embedding.clone()));
                    pid
                }
            };
            self.conn.execute(
                "INSERT INTO faces(image_id, x, y, w, h, embedding, eyes_closed, person_id) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
                params![id, f.bbox[0], f.bbox[1], f.bbox[2], f.bbox[3], embedding_to_blob(&f.embedding), f.eyes_closed, pid],
            )?;
        }
        // The file and its variants show the same.
        let prefix = format!("{}#", self.key(path));
        self.conn.execute(
            "UPDATE images SET analyzed = 1, sharpness = ?2, face_count = ?3, eyes_closed = ?4
             WHERE path = ?1 OR substr(path, 1, length(?5)) = ?5",
            params![self.key(path), sharpness, faces.len() as i64, closed as i64, prefix],
        )?;
        // Persons without faces are gone.
        self.conn.execute("DELETE FROM persons WHERE id NOT IN (SELECT DISTINCT person_id FROM faces WHERE person_id IS NOT NULL)", [])?;
        tx.commit()?;
        Ok(())
    }

    /// Recognized people: named ones and those seen in at least `min_count` images.
    pub fn persons(&self, min_count: usize) -> Result<Vec<Person>> {
        let mut stmt = self.conn.prepare(
            "SELECT p.id, p.name, count(DISTINCT f.image_id),
                    (SELECT i.path FROM faces g JOIN images i ON i.id = g.image_id WHERE g.person_id = p.id ORDER BY g.w * g.h DESC LIMIT 1),
                    (SELECT g.x || ',' || g.y || ',' || g.w || ',' || g.h FROM faces g WHERE g.person_id = p.id ORDER BY g.w * g.h DESC LIMIT 1)
             FROM persons p JOIN faces f ON f.person_id = p.id GROUP BY p.id",
        )?;
        let mut out: Vec<Person> = stmt
            .query_map([], |r| {
                let path: Option<String> = r.get(3)?;
                let bbox: Option<String> = r.get(4)?;
                let bbox = bbox.and_then(|b| {
                    let v: Vec<f32> = b.split(',').filter_map(|x| x.parse().ok()).collect();
                    <[f32; 4]>::try_from(v).ok()
                });
                Ok(Person {
                    id: r.get(0)?,
                    name: r.get::<_, Option<String>>(1)?.filter(|n| !n.trim().is_empty()),
                    count: r.get::<_, i64>(2)? as usize,
                    sample: path.map(|p| self.resolve(&p)).zip(bbox),
                })
            })?
            .collect::<rusqlite::Result<_>>()?;
        out.retain(|p| p.name.is_some() || p.count >= min_count);
        out.sort_by(|a, b| b.name.is_some().cmp(&a.name.is_some()).then(b.count.cmp(&a.count)).then(a.id.cmp(&b.id)));
        Ok(out)
    }

    /// Names a person. A person of the same name already known is merged into this one.
    /// Returns the images of the person (to update their keywords).
    pub fn name_person(&self, id: i64, name: &str) -> Result<Vec<PathBuf>> {
        let name = name.trim();
        let other: Option<i64> = self
            .conn
            .query_row("SELECT id FROM persons WHERE id <> ?1 AND name = ?2 COLLATE NOCASE", params![id, name], |r| r.get(0))
            .optional()?;
        if let Some(o) = other {
            self.conn.execute("UPDATE faces SET person_id = ?1 WHERE person_id = ?2", params![id, o])?;
            self.conn.execute("DELETE FROM persons WHERE id = ?1", [o])?;
        }
        self.conn.execute("UPDATE persons SET name = ?2 WHERE id = ?1", params![id, (!name.is_empty()).then_some(name)])?;
        self.person_images(id)
    }

    pub fn person_images(&self, id: i64) -> Result<Vec<PathBuf>> {
        let mut stmt = self.conn.prepare("SELECT DISTINCT i.path FROM faces f JOIN images i ON i.id = f.image_id WHERE f.person_id = ?1")?;
        let rows: Vec<String> = stmt.query_map([id], |r| r.get(0))?.collect::<rusqlite::Result<_>>()?;
        Ok(rows.iter().map(|p| self.resolve(p)).collect())
    }

    pub fn person_name(&self, id: i64) -> Option<String> {
        self.conn.query_row("SELECT name FROM persons WHERE id = ?1", [id], |r| r.get::<_, Option<String>>(0)).ok().flatten()
    }

    /// Favorite folders (used by sessions), sorted by name.
    pub fn favorites(&self) -> Result<Vec<PathBuf>> {
        let mut stmt = self.conn.prepare("SELECT path FROM favorites")?;
        let stored: Vec<String> = stmt.query_map([], |r| r.get(0))?.collect::<rusqlite::Result<_>>()?;
        let mut v: Vec<PathBuf> = stored.iter().map(|s| self.resolve(s)).collect();
        v.sort_by_key(|p| p.file_name().unwrap_or_default().to_string_lossy().to_lowercase());
        Ok(v)
    }

    pub fn add_favorite(&self, folder: &Path) -> Result<()> {
        self.conn.execute("INSERT OR IGNORE INTO favorites(path) VALUES (?1)", [self.key(folder)])?;
        Ok(())
    }

    pub fn remove_favorite(&self, folder: &Path) -> Result<()> {
        self.conn.execute("DELETE FROM favorites WHERE path = ?1", [self.key(folder)])?;
        Ok(())
    }

    pub fn query(&self, scope: &Scope, filter: &Filter, sort: SortKey, descending: bool) -> Result<Vec<ImageRecord>> {
        let mut sql = String::from(
            "SELECT i.id, i.path, i.name, i.capture_date, i.camera, i.lens, i.iso, i.rating, i.color_label,
                    (SELECT group_concat(keyword, char(31)) FROM keywords k WHERE k.image_id = i.id), i.stack, i.stack_top,
                    i.analyzed, i.sharpness, i.face_count, i.eyes_closed,
                    (SELECT group_concat(DISTINCT person_id) FROM faces f WHERE f.image_id = i.id AND f.person_id IS NOT NULL)
             FROM images i WHERE 1=1",
        );
        let mut args: Vec<Value> = Vec::new();
        match scope {
            Scope::All => {}
            Scope::Folder { path, recursive } => {
                let folder = self.key(path);
                let relative = self.base.as_ref().is_some_and(|b| path.starts_with(b));
                if *recursive && relative && folder.is_empty() {
                    // The whole session.
                } else if *recursive {
                    sql += " AND (i.folder = ? OR i.folder LIKE ? ESCAPE '\\')";
                    let sep = if relative { '/' } else { std::path::MAIN_SEPARATOR };
                    let pattern = subfolder_pattern(&folder, sep);
                    args.push(Value::Text(folder));
                    args.push(Value::Text(pattern));
                } else {
                    sql += " AND i.folder = ?";
                    args.push(Value::Text(folder));
                }
            }
            Scope::Album(id) => {
                let smart: Option<Option<String>> =
                    self.conn.query_row("SELECT smart FROM albums WHERE id = ?1", [id], |r| r.get(0)).optional()?;
                match smart.flatten().and_then(|s| serde_json::from_str::<Filter>(&s).ok()) {
                    // Smart album: its filter is combined with the given one.
                    Some(f) => {
                        let mut recs = self.query(&Scope::All, &f, sort, descending)?;
                        recs.retain(|r| filter.matches(r));
                        return Ok(recs);
                    }
                    None => {
                        sql += " AND i.id IN (SELECT image_id FROM album_images WHERE album_id = ?)";
                        args.push(Value::Integer(*id));
                    }
                }
            }
            Scope::LastImport => {
                sql += " AND i.import_id = (SELECT CAST(value AS INTEGER) FROM meta WHERE key = 'last_import')";
            }
            Scope::Person(id) => {
                sql += " AND EXISTS (SELECT 1 FROM faces f WHERE f.image_id = i.id AND f.person_id = ?)";
                args.push(Value::Integer(*id));
            }
        }
        match filter.culling {
            Some(CullFilter::EyesClosed) => sql += " AND i.analyzed > 0 AND i.eyes_closed > 0",
            Some(CullFilter::Blurry) => sql += &format!(" AND i.analyzed > 0 AND i.sharpness < {BLURRY}"),
            Some(CullFilter::WithFaces) => sql += " AND i.analyzed > 0 AND i.face_count > 0",
            Some(CullFilter::NotAnalyzed) => sql += " AND i.analyzed = 0",
            None => {}
        }
        if filter.min_rating > 0 {
            sql += " AND i.rating >= ?";
            args.push(Value::Integer(filter.min_rating as i64));
        }
        if let Some(l) = filter.color_label {
            sql += " AND i.color_label = ?";
            args.push(Value::Text(label_to_str(l)));
        }
        if let Some(k) = &filter.keyword {
            sql += " AND EXISTS (SELECT 1 FROM keywords k WHERE k.image_id = i.id AND k.keyword = ?)";
            args.push(Value::Text(k.clone()));
        }
        for term in filter.text.split_whitespace() {
            sql += " AND (i.name LIKE ? OR i.camera LIKE ? OR i.lens LIKE ?
                       OR EXISTS (SELECT 1 FROM keywords k WHERE k.image_id = i.id AND k.keyword LIKE ?))";
            let like = format!("%{term}%");
            for _ in 0..4 {
                args.push(Value::Text(like.clone()));
            }
        }
        let dir = if descending { "DESC" } else { "ASC" };
        sql += &match sort {
            SortKey::Name => format!(" ORDER BY i.name COLLATE NOCASE {dir}, i.path"),
            SortKey::CaptureDate => format!(" ORDER BY i.capture_date {dir}, i.name"),
            SortKey::Rating => format!(" ORDER BY i.rating {dir}, i.name"),
        };
        let mut stmt = self.conn.prepare(&sql)?;
        let rows = stmt.query_map(params_from_iter(args), |r| {
            let kw: Option<String> = r.get(9)?;
            Ok(ImageRecord {
                id: r.get(0)?,
                path: self.resolve(&r.get::<_, String>(1)?),
                name: r.get(2)?,
                capture_date: r.get(3)?,
                camera: r.get(4)?,
                lens: r.get(5)?,
                iso: r.get(6)?,
                rating: r.get::<_, i64>(7)?.clamp(0, 5) as u8,
                color_label: label_from_str(&r.get::<_, String>(8)?),
                keywords: kw.map(|s| s.split('\u{1f}').map(String::from).collect()).unwrap_or_default(),
                stack: r.get::<_, Option<String>>(10)?.map(|id| Stack { id, top: r.get::<_, bool>(11).unwrap_or(false) }),
                culling: (r.get::<_, i64>(12)? > 0).then(|| Culling {
                    sharpness: r.get::<_, Option<f64>>(13).ok().flatten().unwrap_or(0.0) as f32,
                    faces: r.get::<_, i64>(14).unwrap_or(0) as u32,
                    eyes_closed: r.get::<_, i64>(15).unwrap_or(0) as u32,
                }),
                persons: r
                    .get::<_, Option<String>>(16)?
                    .map(|s| s.split(',').filter_map(|v| v.parse().ok()).collect())
                    .unwrap_or_default(),
            })
        })?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }

    /// All indexed folders with image counts, sorted.
    pub fn folders(&self) -> Result<Vec<(PathBuf, usize)>> {
        let mut stmt = self.conn.prepare("SELECT folder, count(*) FROM images GROUP BY folder ORDER BY folder")?;
        let rows = stmt.query_map([], |r| Ok((self.resolve(&r.get::<_, String>(0)?), r.get::<_, i64>(1)? as usize)))?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }

    /// All keywords with usage counts.
    pub fn keywords(&self) -> Result<Vec<(String, usize)>> {
        let mut stmt =
            self.conn.prepare("SELECT keyword, count(*) FROM keywords GROUP BY keyword COLLATE NOCASE ORDER BY keyword COLLATE NOCASE")?;
        let rows = stmt.query_map([], |r| Ok((r.get(0)?, r.get::<_, i64>(1)? as usize)))?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }

    pub fn image_count(&self) -> Result<usize> {
        Ok(self.conn.query_row("SELECT count(*) FROM images", [], |r| r.get::<_, i64>(0))? as usize)
    }

    pub fn create_album(&self, name: &str, smart: Option<&Filter>) -> Result<i64> {
        let smart = smart.map(serde_json::to_string).transpose()?;
        self.conn.execute("INSERT INTO albums(name, smart) VALUES (?1, ?2)", params![name, smart])?;
        Ok(self.conn.last_insert_rowid())
    }

    pub fn rename_album(&self, id: i64, name: &str) -> Result<()> {
        self.conn.execute("UPDATE albums SET name = ?2 WHERE id = ?1", params![id, name])?;
        Ok(())
    }

    pub fn delete_album(&self, id: i64) -> Result<()> {
        self.conn.execute("DELETE FROM albums WHERE id = ?1", [id])?;
        Ok(())
    }

    pub fn albums(&self) -> Result<Vec<Album>> {
        let mut stmt = self.conn.prepare(
            "SELECT a.id, a.name, a.smart, (SELECT count(*) FROM album_images ai WHERE ai.album_id = a.id)
             FROM albums a ORDER BY a.name COLLATE NOCASE",
        )?;
        let rows = stmt.query_map([], |r| {
            let smart: Option<String> = r.get(2)?;
            Ok(Album {
                id: r.get(0)?,
                name: r.get(1)?,
                smart: smart.and_then(|s| serde_json::from_str(&s).ok()),
                count: r.get::<_, i64>(3)? as usize,
            })
        })?;
        let mut albums: Vec<Album> = rows.collect::<rusqlite::Result<_>>()?;
        for a in albums.iter_mut().filter(|a| a.smart.is_some()) {
            a.count = self.query(&Scope::Album(a.id), &Filter::default(), SortKey::Name, false)?.len();
        }
        Ok(albums)
    }

    pub fn add_to_album(&self, album: i64, paths: &[PathBuf]) -> Result<usize> {
        let mut n = 0;
        for p in paths {
            n += self.conn.execute(
                "INSERT OR IGNORE INTO album_images(album_id, image_id) SELECT ?1, id FROM images WHERE path = ?2",
                params![album, self.key(p)],
            )?;
        }
        Ok(n)
    }

    pub fn remove_from_album(&self, album: i64, paths: &[PathBuf]) -> Result<()> {
        for p in paths {
            self.conn.execute(
                "DELETE FROM album_images WHERE album_id = ?1 AND image_id = (SELECT id FROM images WHERE path = ?2)",
                params![album, self.key(p)],
            )?;
        }
        Ok(())
    }
}

/// Lists raw files in `folder` (optionally recursive), sorted.
pub fn scan_folder(folder: &Path, recursive: bool) -> Vec<PathBuf> {
    let mut out = Vec::new();
    let mut stack = vec![folder.to_path_buf()];
    while let Some(dir) = stack.pop() {
        let Ok(read) = std::fs::read_dir(&dir) else { continue };
        for entry in read.flatten() {
            let path = entry.path();
            let hidden = path.file_name().is_some_and(|n| n.to_string_lossy().starts_with('.'));
            if hidden {
                continue;
            }
            if path.is_dir() {
                if recursive {
                    stack.push(path);
                }
            } else if raw::is_raw_file(&path) {
                out.push(path);
            }
        }
    }
    out.sort();
    out
}

/// LIKE pattern (escape character `\`) for everything below `folder`. The separator is
/// escaped too: on Windows it is `\`, which would otherwise escape the `%`.
fn subfolder_pattern(folder: &str, sep: char) -> String {
    let escape = |s: &str| s.replace('\\', "\\\\").replace('%', "\\%").replace('_', "\\_");
    let folder = folder.strip_suffix(sep).unwrap_or(folder);
    format!("{}{}%", escape(folder), escape(&sep.to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::color::CameraColor;
    use crate::image::Orientation;
    use crate::raw::CaptureInfo;

    fn meta(path: &str, date: &str, lens: &str) -> RawMeta {
        let id = [[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]];
        RawMeta {
            path: PathBuf::from(path),
            info: CaptureInfo {
                make: "Sony".into(),
                model: "ILCE-7M3".into(),
                lens: Some(lens.into()),
                date: Some(date.into()),
                ..Default::default()
            },
            color: CameraColor::new(id),
            as_shot_wb: [1.0; 3],
            orientation: Orientation::Normal,
            hdr: false, lens: None,
        }
    }

    #[test]
    fn newer_database_is_refused_older_one_upgraded() {
        let dir = std::env::temp_dir().join(format!("sk-schema-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("c.sqlite");
        let _ = std::fs::remove_file(&path);
        // A database from before the version was recorded.
        Connection::open(&path).unwrap().execute_batch("CREATE TABLE images (id INTEGER PRIMARY KEY, path TEXT NOT NULL UNIQUE, folder TEXT NOT NULL, name TEXT NOT NULL)").unwrap();
        drop(Catalog::open(&path).unwrap());
        let version = |p: &Path| Connection::open(p).unwrap().query_row("PRAGMA user_version", [], |r| r.get::<_, u32>(0)).unwrap();
        assert_eq!(version(&path), SCHEMA_VERSION);
        Connection::open(&path).unwrap().execute_batch(&format!("PRAGMA user_version = {}", SCHEMA_VERSION + 1)).unwrap();
        let err = Catalog::open(&path).err().unwrap();
        assert!(format!("{err:#}").contains("neueren Silberkorn-Version"));
        assert_eq!(version(&path), SCHEMA_VERSION + 1);
        std::fs::remove_dir_all(dir).ok();
    }

    fn catalog() -> Catalog {
        let c = Catalog::open_in_memory().unwrap();
        let imp = c.begin_import().unwrap();
        let sc = |rating, label, kw: &[&str]| Sidecar {
            rating,
            color_label: label,
            keywords: kw.iter().map(|s| s.to_string()).collect(),
            ..Default::default()
        };
        c.upsert(&meta("/p/a/b.ARW", "2020:01:02", "FE 35mm"), &sc(3, ColorLabel::Red, &["Urlaub", "Strand"]), imp).unwrap();
        c.upsert(&meta("/p/a/a.ARW", "2021:05:01", "FE 85mm"), &sc(5, ColorLabel::None, &["Portrait"]), imp).unwrap();
        c.upsert(&meta("/p/a/sub/c.ARW", "2019:07:07", "FE 35mm"), &sc(0, ColorLabel::Red, &[]), imp).unwrap();
        c
    }

    fn names(v: Vec<ImageRecord>) -> Vec<String> {
        v.into_iter().map(|r| r.name).collect()
    }

    #[test]
    fn scopes_sorting_and_filters() {
        let c = catalog();
        let all = c.query(&Scope::All, &Filter::default(), SortKey::Name, false).unwrap();
        assert_eq!(names(all), ["a.ARW", "b.ARW", "c.ARW"]);
        let by_date = c.query(&Scope::All, &Filter::default(), SortKey::CaptureDate, true).unwrap();
        assert_eq!(names(by_date), ["a.ARW", "b.ARW", "c.ARW"]);
        let folder = Scope::Folder { path: "/p/a".into(), recursive: false };
        assert_eq!(c.query(&folder, &Filter::default(), SortKey::Name, false).unwrap().len(), 2);
        let rec = Scope::Folder { path: "/p/a".into(), recursive: true };
        assert_eq!(c.query(&rec, &Filter::default(), SortKey::Name, false).unwrap().len(), 3);

        let f = Filter { min_rating: 3, ..Default::default() };
        assert_eq!(c.query(&Scope::All, &f, SortKey::Name, false).unwrap().len(), 2);
        let f = Filter { color_label: Some(ColorLabel::Red), ..Default::default() };
        assert_eq!(c.query(&Scope::All, &f, SortKey::Name, false).unwrap().len(), 2);
        let f = Filter { keyword: Some("urlaub".into()), ..Default::default() };
        assert_eq!(names(c.query(&Scope::All, &f, SortKey::Name, false).unwrap()), ["b.ARW"]);
        let f = Filter { text: "35mm strand".into(), ..Default::default() };
        let recs = c.query(&Scope::All, &f, SortKey::Name, false).unwrap();
        assert_eq!(names(recs.clone()), ["b.ARW"]);
        assert!(recs.iter().all(|r| f.matches(r)));
        assert_eq!(c.keywords().unwrap().len(), 3);
    }

    #[test]
    fn albums_and_smart_albums() {
        let c = catalog();
        let a = c.create_album("Favoriten", None).unwrap();
        c.add_to_album(a, &[PathBuf::from("/p/a/a.ARW"), PathBuf::from("/p/a/a.ARW")]).unwrap();
        assert_eq!(c.query(&Scope::Album(a), &Filter::default(), SortKey::Name, false).unwrap().len(), 1);
        let s = c.create_album("Rot", Some(&Filter { color_label: Some(ColorLabel::Red), ..Default::default() })).unwrap();
        let albums = c.albums().unwrap();
        assert_eq!(albums.iter().find(|x| x.id == s).unwrap().count, 2);
        c.remove_from_album(a, &[PathBuf::from("/p/a/a.ARW")]).unwrap();
        assert_eq!(c.albums().unwrap().iter().find(|x| x.id == a).unwrap().count, 0);
        c.delete_album(s).unwrap();
        assert_eq!(c.albums().unwrap().len(), 1);
    }

    #[test]
    fn variants_follow_their_file() {
        let c = catalog();
        let v = crate::variant::path_for(Path::new("/p/a/a.ARW"), 2);
        c.upsert(&meta(v.to_str().unwrap(), "2024:01:01 10:00:00", "50mm"), &Sidecar::default(), 0).unwrap();
        let album = c.create_album("A", None).unwrap();
        c.add_to_album(album, std::slice::from_ref(&v)).unwrap();
        let names = |c: &Catalog| c.query(&Scope::All, &Filter::default(), SortKey::Name, false).unwrap().into_iter().map(|r| r.name).collect::<Vec<_>>();
        assert!(names(&c).contains(&"a.ARW (2)".to_string()));
        // Moving the file moves the variant, album membership included.
        c.rename_path(Path::new("/p/a/a.ARW"), Path::new("/p/sel/a.ARW")).unwrap();
        let moved = c.query(&Scope::Album(album), &Filter::default(), SortKey::Name, false).unwrap();
        assert_eq!(moved.len(), 1);
        assert_eq!(moved[0].path, Path::new("/p/sel/a.ARW#2"));
        // Removing the file removes its variants; a variant alone can be removed too.
        c.remove_path(Path::new("/p/sel/a.ARW")).unwrap();
        assert!(!names(&c).iter().any(|n| n.starts_with("a.ARW")));
    }

    #[test]
    fn index_real_sample() {
        let sample = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../samples/_DSC0009.ARW");
        if !sample.exists() {
            return;
        }
        let c = Catalog::open_in_memory().unwrap();
        assert!(c.needs_index(&sample).unwrap());
        c.index_file(&sample, 1).unwrap();
        assert!(!c.needs_index(&sample).unwrap());
        let r = c.query(&Scope::All, &Filter { text: "16-35".into(), ..Default::default() }, SortKey::Name, false).unwrap();
        assert_eq!(r.len(), 1);
        assert_eq!(r[0].camera.as_deref(), Some("Sony ILCE-7M3"));
        assert_eq!(r[0].capture_date.as_deref(), Some("2018:03:13 16:38:13"));
    }

    #[test]
    fn rename_remove_and_favorites() {
        let c = catalog();
        let a = c.create_album("A", None).unwrap();
        c.add_to_album(a, &[PathBuf::from("/p/a/a.ARW")]).unwrap();
        c.rename_path(Path::new("/p/a/a.ARW"), Path::new("/p/sel/a-2.ARW")).unwrap();
        let r = c.query(&Scope::Album(a), &Filter::default(), SortKey::Name, false).unwrap();
        assert_eq!(names(r.clone()), ["a-2.ARW"]);
        assert_eq!(r[0].keywords, ["Portrait"]);
        let sel = Scope::Folder { path: "/p/sel".into(), recursive: false };
        assert_eq!(c.query(&sel, &Filter::default(), SortKey::Name, false).unwrap().len(), 1);
        // Moving onto a stale entry replaces it.
        c.rename_path(Path::new("/p/a/b.ARW"), Path::new("/p/sel/a-2.ARW")).unwrap();
        assert_eq!(c.image_count().unwrap(), 2);
        c.remove_path(Path::new("/p/sel/a-2.ARW")).unwrap();
        assert_eq!(c.image_count().unwrap(), 1);

        c.add_favorite(Path::new("/p/z")).unwrap();
        c.add_favorite(Path::new("/p/b")).unwrap();
        c.add_favorite(Path::new("/p/b")).unwrap();
        assert_eq!(c.favorites().unwrap(), [PathBuf::from("/p/b"), PathBuf::from("/p/z")]);
        c.remove_favorite(Path::new("/p/b")).unwrap();
        assert_eq!(c.favorites().unwrap().len(), 1);
    }

    #[test]
    fn session_paths_are_portable() {
        let tmp = std::env::temp_dir().join(format!("skcat-portable-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&tmp);
        let (old, new) = (tmp.join("Alt"), tmp.join("Neu"));
        std::fs::create_dir_all(old.join("Capture")).unwrap();
        let old = old.canonicalize().unwrap();
        let db = old.join("S.sksession");
        {
            let c = Catalog::open(&db).unwrap();
            let p = old.join("Capture/a.ARW");
            c.upsert(&meta(p.to_str().unwrap(), "2020:01:01", "FE 35mm"), &Sidecar::default(), 1).unwrap();
            let album = c.create_album("Best", None).unwrap();
            c.add_to_album(album, &[p]).unwrap();
            c.add_favorite(&old.join("Kunde")).unwrap();
            c.add_favorite(Path::new("/extern/ordner")).unwrap();
            let stored: String = c.conn.query_row("SELECT path FROM images", [], |r| r.get(0)).unwrap();
            assert_eq!(stored, "Capture/a.ARW");
        }
        std::fs::rename(&old, &new).unwrap();
        let new = new.canonicalize().unwrap();
        let c = Catalog::open(&new.join("S.sksession")).unwrap();
        let r = c.query(&Scope::All, &Filter::default(), SortKey::Name, false).unwrap();
        assert_eq!(r[0].path, new.join("Capture").join("a.ARW"));
        assert_eq!(c.folders().unwrap()[0].0, new.join("Capture"));
        let album = c.albums().unwrap()[0].id;
        assert_eq!(c.query(&Scope::Album(album), &Filter::default(), SortKey::Name, false).unwrap().len(), 1);
        let rec = Scope::Folder { path: new.clone(), recursive: true };
        assert_eq!(c.query(&rec, &Filter::default(), SortKey::Name, false).unwrap().len(), 1);
        let cap = Scope::Folder { path: new.join("Capture"), recursive: true };
        assert_eq!(c.query(&cap, &Filter::default(), SortKey::Name, false).unwrap().len(), 1);
        assert_eq!(c.favorites().unwrap(), [new.join("Kunde"), PathBuf::from("/extern/ordner")]);
        std::fs::remove_dir_all(tmp).ok();
    }

    #[test]
    fn absolute_session_paths_are_migrated() {
        let tmp = std::env::temp_dir().join(format!("skcat-migrate-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&tmp);
        std::fs::create_dir_all(&tmp).unwrap();
        let tmp = tmp.canonicalize().unwrap();
        let plain = tmp.join("S.sqlite");
        {
            let c = Catalog::open(&plain).unwrap();
            let p = tmp.join("Capture").join("a.ARW");
            c.upsert(&meta(p.to_str().unwrap(), "2020:01:01", "x"), &Sidecar::default(), 1).unwrap();
            c.add_favorite(&tmp.join("Kunde")).unwrap();
        }
        std::fs::rename(&plain, tmp.join("S.sksession")).unwrap();
        let c = Catalog::open(&tmp.join("S.sksession")).unwrap();
        let (path, folder): (String, String) = c.conn.query_row("SELECT path, folder FROM images", [], |r| Ok((r.get(0)?, r.get(1)?))).unwrap();
        assert_eq!((path.as_str(), folder.as_str()), ("Capture/a.ARW", "Capture"));
        let fav: String = c.conn.query_row("SELECT path FROM favorites", [], |r| r.get(0)).unwrap();
        assert_eq!(fav, "Kunde");
        std::fs::remove_dir_all(tmp).ok();
    }

    #[test]
    fn sync_and_last_import() {
        let c = catalog();
        let sc = Sidecar { rating: 1, keywords: vec!["Neu".into()], ..Default::default() };
        c.sync_sidecar(Path::new("/p/a/a.ARW"), &sc).unwrap();
        let f = Filter { keyword: Some("Neu".into()), ..Default::default() };
        let r = c.query(&Scope::All, &f, SortKey::Name, false).unwrap();
        assert_eq!(r[0].rating, 1);
        assert_eq!(c.query(&Scope::LastImport, &Filter::default(), SortKey::Name, false).unwrap().len(), 3);
        // A variant made later belongs to the import of its file.
        let v = crate::variant::path_for(Path::new("/p/a/a.ARW"), 2);
        c.upsert(&meta(v.to_str().unwrap(), "2024:01:01 10:00:00", "50mm"), &Sidecar::default(), 0).unwrap();
        assert_eq!(c.query(&Scope::LastImport, &Filter::default(), SortKey::Name, false).unwrap().len(), 4);
    }

    #[test]
    fn faces_persons_and_culling() {
        let c = Catalog::open_in_memory().unwrap();
        for p in ["/f/a.ARW", "/f/b.ARW", "/f/c.ARW"] {
            c.upsert(&meta(p, "2024:01:01 10:00:00", "L"), &Sidecar::default(), 0).unwrap();
        }
        let unit = |k: usize| -> Vec<f32> { (0..8).map(|i| if i == k { 1.0 } else { 0.0 }).collect() };
        let near = |k: usize| -> Vec<f32> {
            let mut v = unit(k);
            v[(k + 1) % 8] = 0.2;
            let n = v.iter().map(|x| x * x).sum::<f32>().sqrt();
            v.iter().map(|x| x / n).collect()
        };
        let face = |e: Vec<f32>, closed: f32| FaceRecord { bbox: [0.4, 0.3, 0.2, 0.25], embedding: e, eyes_closed: closed };
        // a: Anna with closed eyes, b: Anna and Ben, c: blurry landscape.
        c.store_analysis(Path::new("/f/a.ARW"), 0.5, &[face(unit(0), 0.9)], 0.42).unwrap();
        c.store_analysis(Path::new("/f/b.ARW"), 0.45, &[face(near(0), 0.1), face(unit(4), 0.0)], 0.42).unwrap();
        c.store_analysis(Path::new("/f/c.ARW"), 0.1, &[], 0.42).unwrap();
        let persons = c.persons(2).unwrap();
        assert_eq!(persons.len(), 1, "{persons:?}");
        assert_eq!(persons[0].count, 2);
        assert_eq!(c.persons(1).unwrap().len(), 2);
        let anna = persons[0].id;
        let q = |scope: Scope, cull: Option<CullFilter>| -> Vec<String> {
            c.query(&scope, &Filter { culling: cull, ..Default::default() }, SortKey::Name, false).unwrap().into_iter().map(|r| r.name).collect()
        };
        assert_eq!(q(Scope::Person(anna), None), vec!["a.ARW", "b.ARW"]);
        assert_eq!(q(Scope::All, Some(CullFilter::EyesClosed)), vec!["a.ARW"]);
        assert_eq!(q(Scope::All, Some(CullFilter::Blurry)), vec!["c.ARW"]);
        assert_eq!(q(Scope::All, Some(CullFilter::WithFaces)), vec!["a.ARW", "b.ARW"]);
        // The in-memory filter agrees with the query.
        let all = c.query(&Scope::All, &Filter::default(), SortKey::Name, false).unwrap();
        let f = Filter { culling: Some(CullFilter::Blurry), ..Default::default() };
        assert_eq!(all.iter().filter(|r| f.matches(r)).count(), 1);
        assert!(all.iter().find(|r| r.name == "b.ARW").unwrap().persons.contains(&anna));
        // Naming; a second person of the same name is merged into the first.
        assert_eq!(c.name_person(anna, "Anna").unwrap().len(), 2);
        let ben = c.persons(1).unwrap().into_iter().find(|p| p.id != anna).unwrap().id;
        c.name_person(ben, "anna").unwrap();
        assert_eq!(c.persons(1).unwrap().len(), 1);
        // Analysing again replaces the faces; persons without faces disappear.
        c.store_analysis(Path::new("/f/b.ARW"), 0.45, &[], 0.42).unwrap();
        c.store_analysis(Path::new("/f/a.ARW"), 0.5, &[], 0.42).unwrap();
        assert!(c.persons(1).unwrap().is_empty());
    }

    #[test]
    fn analysing_again_keeps_the_name() {
        let c = Catalog::open_in_memory().unwrap();
        c.upsert(&meta("/f/a.ARW", "2024:01:01 10:00:00", "L"), &Sidecar::default(), 0).unwrap();
        let face = FaceRecord { bbox: [0.4, 0.3, 0.2, 0.25], embedding: vec![1.0, 0.0, 0.0], eyes_closed: 0.0 };
        c.store_analysis(Path::new("/f/a.ARW"), 0.5, std::slice::from_ref(&face), 0.42).unwrap();
        let id = c.persons(1).unwrap()[0].id;
        c.name_person(id, "Anna").unwrap();
        // The only image of Anna, analysed again.
        c.store_analysis(Path::new("/f/a.ARW"), 0.5, &[face], 0.42).unwrap();
        let persons = c.persons(1).unwrap();
        assert_eq!(persons.len(), 1);
        assert_eq!(c.person_name(persons[0].id).as_deref(), Some("Anna"));
    }

    #[test]
    fn remove_missing_keeps_unreachable_folders() {
        let dir = std::env::temp_dir().join(format!("sk-missing-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("da.ARW"), b"").unwrap();
        let c = Catalog::open_in_memory().unwrap();
        let offline = Path::new("/nicht/eingehaengt/b.ARW");
        for p in [dir.join("da.ARW"), dir.join("weg.ARW"), offline.to_path_buf()] {
            c.upsert(&meta(p.to_str().unwrap(), "2024:01:01 10:00:00", "L"), &Sidecar::default(), 0).unwrap();
        }
        // Only the file deleted from a folder that is there goes.
        assert_eq!(c.remove_missing().unwrap(), 1);
        assert!(c.image_id(offline).unwrap().is_some());
        assert!(c.image_id(&dir.join("da.ARW")).unwrap().is_some());
        std::fs::remove_dir_all(dir).ok();
    }

    #[test]
    fn subfolder_pattern_escapes_the_separator() {
        assert_eq!(subfolder_pattern("/f/a_b", '/'), "/f/a\\_b/%");
        assert_eq!(subfolder_pattern("C:\\Fotos", '\\'), "C:\\\\Fotos\\\\%");
        assert_eq!(subfolder_pattern("C:\\", '\\'), "C:\\\\%");
    }
}
