use std::sync::Arc;

use anyhow::{Context as _, Result};
use music::{Album, Playlist, PlaylistDetail, SavedArtist, Track};
use rusqlite::{Connection, OptionalExtension as _, params};
use storage::Database;

/// The id prefix every playlist a signed-out listener makes carries, so it can be told apart
/// from both a provider playlist and a local-file playlist without asking a server.
pub const PLAYLIST_PREFIX: &str = "guest:playlist:";

/// Whether `id` names a playlist kept on this machine for a signed-out listener.
pub fn is_guest_playlist(id: &str) -> bool {
    id.starts_with(PLAYLIST_PREFIX)
}

/// Which kind of favorite a row in `guest_favorites` holds.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Track,
    Album,
    Artist,
}

impl Kind {
    fn as_str(self) -> &'static str {
        match self {
            Self::Track => "track",
            Self::Album => "album",
            Self::Artist => "artist",
        }
    }
}

/// Playlists and favorites a listener can keep while signed out. Every row holds a full
/// snapshot of its track, album or artist, so a saved YouTube song still shows correctly when
/// nothing is signed in and no provider is asked.
#[derive(Clone)]
pub struct GuestStore {
    database: Database,
}

impl GuestStore {
    pub fn new(database: Database) -> Arc<Self> {
        Arc::new(Self { database })
    }

    fn open(&self) -> Result<Connection> {
        self.database
            .open()
            .context("cannot open the signed-out library")
    }

    pub fn playlists(&self) -> Result<Vec<Playlist>> {
        let connection = self.open()?;
        let mut query = connection
            .prepare("SELECT id, name, modified_at FROM guest_playlists ORDER BY modified_at DESC")
            .context("cannot read signed-out playlists")?;
        let rows = query
            .query_map([], |row| {
                let id: String = row.get(0)?;
                let name: String = row.get(1)?;
                let modified_at: i64 = row.get(2)?;
                Ok((id, name, modified_at))
            })
            .context("cannot read signed-out playlists")?;

        let mut playlists = Vec::new();
        for row in rows {
            let (id, name, modified_at) = row.context("cannot read a signed-out playlist")?;
            let tracks = self.tracks_in(&connection, &id)?;
            playlists.push(playlist_from(id, name, modified_at, &tracks));
        }
        Ok(playlists)
    }

    pub fn playlist(&self, id: &str) -> Result<PlaylistDetail> {
        let connection = self.open()?;
        let (name, modified_at) = connection
            .query_row(
                "SELECT name, modified_at FROM guest_playlists WHERE id = ?",
                params![id],
                |row| Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)?)),
            )
            .with_context(|| format!("cannot find the signed-out playlist {id}"))?;
        let tracks = self.tracks_in(&connection, id)?;
        Ok(PlaylistDetail {
            playlist: playlist_from(id.to_owned(), name, modified_at, &tracks),
            tracks,
            continuation: None,
        })
    }

    pub fn tracks(&self, id: &str) -> Result<Vec<Track>> {
        self.tracks_in(&self.open()?, id)
    }

    /// Finds a track in this machine's saved music, for the catalog to answer without a server.
    pub fn track(&self, id: &str) -> Result<Track> {
        let connection = self.open()?;
        let favorite: Option<String> = connection
            .query_row(
                "SELECT item_json FROM guest_favorites WHERE kind = 'track' AND item_id = ?",
                params![id],
                |row| row.get(0),
            )
            .optional()
            .context("cannot read a signed-out favorite")?;
        if let Some(track) = favorite.and_then(|json| serde_json::from_str::<Track>(&json).ok()) {
            return Ok(track);
        }

        let mut query = connection
            .prepare("SELECT track_json FROM guest_playlist_tracks ORDER BY position")
            .context("cannot read a signed-out playlist")?;
        let rows = query
            .query_map([], |row| row.get::<_, String>(0))
            .context("cannot read a signed-out playlist")?;
        for row in rows {
            let json = row.context("cannot read a signed-out playlist")?;
            if let Ok(track) = serde_json::from_str::<Track>(&json)
                && track.id.as_deref() == Some(id)
            {
                return Ok(track);
            }
        }
        anyhow::bail!("no signed-out track {id}")
    }

    fn tracks_in(&self, connection: &Connection, id: &str) -> Result<Vec<Track>> {
        let mut query = connection
            .prepare(
                "SELECT track_json FROM guest_playlist_tracks
                 WHERE playlist_id = ? ORDER BY position",
            )
            .context("cannot read a signed-out playlist")?;
        let rows = query
            .query_map(params![id], |row| row.get::<_, String>(0))
            .context("cannot read a signed-out playlist")?;

        let mut tracks = Vec::new();
        for row in rows {
            let json = row.context("cannot read a signed-out playlist")?;
            match serde_json::from_str::<Track>(&json) {
                Ok(track) => tracks.push(track),
                Err(error) => log::warn!("guest: cannot read a saved track: {error:#}"),
            }
        }
        Ok(tracks)
    }

    pub fn create_playlist(&self, name: &str, tracks: &[Track]) -> Result<Playlist> {
        let id = format!("{PLAYLIST_PREFIX}{}-{}", stamp(), minted());
        let connection = self.open()?;
        connection
            .execute(
                "INSERT INTO guest_playlists (id, name, modified_at) VALUES (?, ?, ?)",
                params![id, name, stamp()],
            )
            .context("cannot create a signed-out playlist")?;
        write_tracks(&connection, &id, tracks)?;
        let tracks = self.tracks_in(&connection, &id)?;
        Ok(playlist_from(id, name.to_owned(), stamp(), &tracks))
    }

    pub fn rename_playlist(&self, id: &str, name: &str) -> Result<()> {
        self.open()?
            .execute(
                "UPDATE guest_playlists SET name = ?, modified_at = ? WHERE id = ?",
                params![name, stamp(), id],
            )
            .context("cannot rename a signed-out playlist")?;
        Ok(())
    }

    pub fn delete_playlist(&self, id: &str) -> Result<()> {
        let connection = self.open()?;
        connection
            .execute(
                "DELETE FROM guest_playlist_tracks WHERE playlist_id = ?",
                params![id],
            )
            .context("cannot empty a signed-out playlist")?;
        connection
            .execute("DELETE FROM guest_playlists WHERE id = ?", params![id])
            .context("cannot delete a signed-out playlist")?;
        Ok(())
    }

    /// Appends the tracks not already in the playlist, in order.
    pub fn add_tracks(&self, id: &str, tracks: &[Track]) -> Result<()> {
        let connection = self.open()?;
        let known: Vec<String> = self
            .tracks_in(&connection, id)?
            .iter()
            .filter_map(|track| track.id.clone())
            .collect();
        let mut position: i64 = connection
            .query_row(
                "SELECT COALESCE(MAX(position), -1) + 1 FROM guest_playlist_tracks
                 WHERE playlist_id = ?",
                params![id],
                |row| row.get(0),
            )
            .context("cannot place a track in a signed-out playlist")?;
        for track in tracks {
            if track
                .id
                .as_deref()
                .is_some_and(|id| known.contains(&id.to_owned()))
            {
                continue;
            }
            let json = serde_json::to_string(track).context("cannot store a track")?;
            connection
                .execute(
                    "INSERT OR IGNORE INTO guest_playlist_tracks (playlist_id, position, track_json)
                     VALUES (?, ?, ?)",
                    params![id, position, json],
                )
                .context("cannot add a track to a signed-out playlist")?;
            position += 1;
        }
        touch(&connection, id)
    }

    pub fn remove_tracks(&self, id: &str, track_ids: &[String]) -> Result<()> {
        let connection = self.open()?;
        for track_id in track_ids {
            let mut query = connection
                .prepare(
                    "SELECT position, track_json FROM guest_playlist_tracks
                     WHERE playlist_id = ? ORDER BY position",
                )
                .context("cannot read a signed-out playlist")?;
            let mut positions = Vec::new();
            let rows = query
                .query_map(params![id], |row| {
                    Ok((row.get::<_, i64>(0)?, row.get::<_, String>(1)?))
                })
                .context("cannot read a signed-out playlist")?;
            for row in rows {
                let (position, json) = row.context("cannot read a signed-out playlist")?;
                let matches = serde_json::from_str::<Track>(&json)
                    .ok()
                    .and_then(|track| track.id)
                    .is_some_and(|id| &id == track_id);
                if matches {
                    positions.push(position);
                }
            }
            for position in positions {
                connection
                    .execute(
                        "DELETE FROM guest_playlist_tracks WHERE playlist_id = ? AND position = ?",
                        params![id, position],
                    )
                    .context("cannot remove a track from a signed-out playlist")?;
            }
        }
        touch(&connection, id)
    }

    pub fn favorites(&self, kind: Kind) -> Result<Vec<(i64, String)>> {
        let connection = self.open()?;
        let mut query = connection
            .prepare(
                "SELECT added_at, item_json FROM guest_favorites
                 WHERE kind = ? ORDER BY added_at DESC",
            )
            .context("cannot read signed-out favorites")?;
        let rows = query
            .query_map(params![kind.as_str()], |row| {
                Ok((row.get::<_, i64>(0)?, row.get::<_, String>(1)?))
            })
            .context("cannot read signed-out favorites")?;

        let mut favorites = Vec::new();
        for row in rows {
            let (added_at, json) = row.context("cannot read a signed-out favorite")?;
            favorites.push((added_at, json));
        }
        Ok(favorites)
    }

    pub fn saved_tracks(&self) -> Result<Vec<Track>> {
        Ok(self
            .favorites(Kind::Track)?
            .into_iter()
            .filter_map(|(added_at, json)| {
                let mut track = serde_json::from_str::<Track>(&json).ok()?;
                track.added_at.get_or_insert(added_at);
                Some(track)
            })
            .collect())
    }

    pub fn saved_albums(&self) -> Result<Vec<Album>> {
        Ok(self
            .favorites(Kind::Album)?
            .into_iter()
            .filter_map(|(added_at, json)| {
                let mut album = serde_json::from_str::<Album>(&json).ok()?;
                album.added_at.get_or_insert(added_at);
                Some(album)
            })
            .collect())
    }

    pub fn saved_artists(&self) -> Result<Vec<SavedArtist>> {
        Ok(self
            .favorites(Kind::Artist)?
            .into_iter()
            .filter_map(|(_, json)| serde_json::from_str::<SavedArtist>(&json).ok())
            .collect())
    }

    pub fn star<T: serde::Serialize>(&self, kind: Kind, id: &str, item: &T) -> Result<()> {
        let json = serde_json::to_string(item).context("cannot store a favorite")?;
        self.open()?
            .execute(
                "INSERT OR REPLACE INTO guest_favorites (kind, item_id, added_at, item_json)
                 VALUES (?, ?, ?, ?)",
                params![kind.as_str(), id, stamp(), json],
            )
            .context("cannot save a favorite")?;
        Ok(())
    }

    pub fn unstar(&self, kind: Kind, id: &str) -> Result<()> {
        self.open()?
            .execute(
                "DELETE FROM guest_favorites WHERE kind = ? AND item_id = ?",
                params![kind.as_str(), id],
            )
            .context("cannot remove a favorite")?;
        Ok(())
    }
}

fn playlist_from(id: String, name: String, modified_at: i64, tracks: &[Track]) -> Playlist {
    Playlist {
        id,
        name,
        owner: String::new(),
        owner_id: String::new(),
        owned: true,
        collaborative: false,
        blend: false,
        public: false,
        cover: tracks.iter().find_map(|track| track.cover.clone()),
        track_count: tracks.len() as u32,
        modified_at: Some(modified_at / 1_000),
    }
}

fn write_tracks(connection: &Connection, id: &str, tracks: &[Track]) -> Result<()> {
    let mut seen = std::collections::HashSet::new();
    let mut position = 0i64;
    for track in tracks {
        if let Some(track_id) = track.id.as_deref()
            && !seen.insert(track_id.to_owned())
        {
            continue;
        }
        let json = serde_json::to_string(track).context("cannot store a track")?;
        connection
            .execute(
                "INSERT OR IGNORE INTO guest_playlist_tracks (playlist_id, position, track_json)
                 VALUES (?, ?, ?)",
                params![id, position, json],
            )
            .context("cannot fill a signed-out playlist")?;
        position += 1;
    }
    Ok(())
}

fn touch(connection: &Connection, id: &str) -> Result<()> {
    connection
        .execute(
            "UPDATE guest_playlists SET modified_at = ? WHERE id = ?",
            params![stamp(), id],
        )
        .context("cannot stamp a signed-out playlist")?;
    Ok(())
}

fn minted() -> u64 {
    static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
}

fn stamp() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
        .min(i64::MAX as u128) as i64
}
