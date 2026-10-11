//! SQLite catalog: an index over raw files for searching, filtering, sorting and albums.
//!
//! Sidecars stay the source of truth for adjustments, ratings, labels and keywords;
//! the catalog mirrors them so that queries across folders are fast.

use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result};
use rusqlite::{Connection, OptionalExtension, params, params_from_iter, types::Value};
use serde::{Deserialize, Serialize};

use crate::raw::{self, RawMeta};
use crate::sidecar::{self, ColorLabel, Sidecar};

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
}

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
}

#[derive(Clone, Debug, PartialEq)]
pub struct Album {
    pub id: i64,
    pub name: String,
    /// `Some` for smart albums.
    pub smart: Option<Filter>,
    pub count: usize,
}

pub struct Catalog {
    conn: Connection,
    pub path: PathBuf,
}

pub fn default_catalog_path() -> PathBuf {
    let base = std::env::var_os("XDG_DATA_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".local/share")))
        .unwrap_or_else(|| PathBuf::from("."));
    crate::paths::app_dir(&base).join("catalog.sqlite")
}

fn now() -> i64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.as_secs() as i64)
}

fn file_stamp(path: &Path) -> (i64, i64) {
    std::fs::metadata(path)
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
        Self::init(conn, path.to_path_buf())
    }

    pub fn open_in_memory() -> Result<Self> {
        Self::init(Connection::open_in_memory()?, PathBuf::from(":memory:"))
    }

    fn init(conn: Connection, path: PathBuf) -> Result<Self> {
        conn.busy_timeout(std::time::Duration::from_secs(5))?;
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
             CREATE TABLE IF NOT EXISTS meta (key TEXT PRIMARY KEY, value TEXT);",
        )?;
        Ok(Self { conn, path })
    }

    /// True if the file is unknown or changed since it was indexed.
    pub fn needs_index(&self, path: &Path) -> Result<bool> {
        let (mtime, size) = file_stamp(path);
        let row: Option<(i64, i64)> = self
            .conn
            .query_row(
                "SELECT file_mtime, file_size FROM images WHERE path = ?1",
                [path.to_string_lossy()],
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
        let folder = path.parent().unwrap_or(Path::new("")).to_string_lossy().to_string();
        let name = path.file_name().unwrap_or_default().to_string_lossy().to_string();
        let info = &meta.info;
        let camera = format!("{} {}", info.make, info.model).trim().to_string();
        // Keep the original import id when re-indexing a changed file.
        let id: i64 = self.conn.query_row(
            "INSERT INTO images (path, folder, name, capture_date, camera, lens, iso, rating, color_label, file_mtime, file_size, import_id)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12)
             ON CONFLICT(path) DO UPDATE SET
                 folder=excluded.folder, name=excluded.name, capture_date=excluded.capture_date,
                 camera=excluded.camera, lens=excluded.lens, iso=excluded.iso, rating=excluded.rating,
                 color_label=excluded.color_label, file_mtime=excluded.file_mtime, file_size=excluded.file_size
             RETURNING id",
            params![
                path.to_string_lossy(),
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
                import_id
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
            self.conn.query_row("SELECT id FROM images WHERE path = ?1", [path.to_string_lossy()], |r| r.get(0)).optional()?;
        if let Some(id) = id {
            self.conn.execute(
                "UPDATE images SET rating = ?2, color_label = ?3 WHERE id = ?1",
                params![id, sc.rating, label_to_str(sc.color_label)],
            )?;
            self.write_keywords(id, &sc.keywords)?;
        }
        Ok(())
    }

    /// Removes entries whose files no longer exist. Returns the number removed.
    pub fn remove_missing(&self) -> Result<usize> {
        let paths: Vec<(i64, String)> = self
            .conn
            .prepare("SELECT id, path FROM images")?
            .query_map([], |r| Ok((r.get(0)?, r.get(1)?)))?
            .collect::<rusqlite::Result<_>>()?;
        let mut n = 0;
        for (id, p) in paths {
            if !Path::new(&p).exists() {
                self.conn.execute("DELETE FROM images WHERE id = ?1", [id])?;
                n += 1;
            }
        }
        Ok(n)
    }

    pub fn query(&self, scope: &Scope, filter: &Filter, sort: SortKey, descending: bool) -> Result<Vec<ImageRecord>> {
        let mut sql = String::from(
            "SELECT i.id, i.path, i.name, i.capture_date, i.camera, i.lens, i.iso, i.rating, i.color_label,
                    (SELECT group_concat(keyword, char(31)) FROM keywords k WHERE k.image_id = i.id)
             FROM images i WHERE 1=1",
        );
        let mut args: Vec<Value> = Vec::new();
        match scope {
            Scope::All => {}
            Scope::Folder { path, recursive } => {
                let folder = path.to_string_lossy().to_string();
                if *recursive {
                    sql += " AND (i.folder = ? OR i.folder LIKE ? ESCAPE '\\')";
                    let escaped = folder.replace('\\', "\\\\").replace('%', "\\%").replace('_', "\\_");
                    args.push(Value::Text(folder.clone()));
                    args.push(Value::Text(format!("{escaped}{}%", std::path::MAIN_SEPARATOR)));
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
                path: PathBuf::from(r.get::<_, String>(1)?),
                name: r.get(2)?,
                capture_date: r.get(3)?,
                camera: r.get(4)?,
                lens: r.get(5)?,
                iso: r.get(6)?,
                rating: r.get::<_, i64>(7)?.clamp(0, 5) as u8,
                color_label: label_from_str(&r.get::<_, String>(8)?),
                keywords: kw.map(|s| s.split('\u{1f}').map(String::from).collect()).unwrap_or_default(),
            })
        })?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }

    /// All indexed folders with image counts, sorted.
    pub fn folders(&self) -> Result<Vec<(PathBuf, usize)>> {
        let mut stmt = self.conn.prepare("SELECT folder, count(*) FROM images GROUP BY folder ORDER BY folder")?;
        let rows = stmt.query_map([], |r| Ok((PathBuf::from(r.get::<_, String>(0)?), r.get::<_, i64>(1)? as usize)))?;
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
                params![album, p.to_string_lossy()],
            )?;
        }
        Ok(n)
    }

    pub fn remove_from_album(&self, album: i64, paths: &[PathBuf]) -> Result<()> {
        for p in paths {
            self.conn.execute(
                "DELETE FROM album_images WHERE album_id = ?1 AND image_id = (SELECT id FROM images WHERE path = ?2)",
                params![album, p.to_string_lossy()],
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
        }
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
    fn sync_and_last_import() {
        let c = catalog();
        let sc = Sidecar { rating: 1, keywords: vec!["Neu".into()], ..Default::default() };
        c.sync_sidecar(Path::new("/p/a/a.ARW"), &sc).unwrap();
        let f = Filter { keyword: Some("Neu".into()), ..Default::default() };
        let r = c.query(&Scope::All, &f, SortKey::Name, false).unwrap();
        assert_eq!(r[0].rating, 1);
        assert_eq!(c.query(&Scope::LastImport, &Filter::default(), SortKey::Name, false).unwrap().len(), 3);
    }
}
