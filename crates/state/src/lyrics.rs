use std::collections::VecDeque;
use std::sync::Arc;
use std::time::Duration;

use gpui::{Context, Entity, Task};
use music::lyrics::LOCAL;
use music::{LyricsHit, LyricsProvider, LyricsQuery, Track, TrackKey};

use crate::sheets::{Sheets, Unsaved};
use crate::{AppSettings, Io, Playback, Queue, Session, join};

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum LyricsState {
    Idle,
    Loading,
    Ready,
    Instrumental,
    Missing,
    Failed(String),
}

const PROVIDER_TIMEOUT: Duration = Duration::from_secs(10);

const SAVE_DELAY: Duration = Duration::from_millis(800);

/// How many tracks keep their answer in memory. The rest are read back from the cache database,
/// so this only has to cover the track playing, the next one and a few just played.
const RECENT: usize = 16;

pub struct Lyrics {
    state: LyricsState,
    hits: Vec<LyricsHit>,
    chosen: usize,
    picked: bool,
    settled: bool,
    revision: u64,
    following: Option<String>,
    /// The answers looked at most recently, newest first.
    recent: VecDeque<(String, Found)>,
    store: Sheets,
    /// Answers found since the last save, written together once lookups go quiet.
    unsaved: Vec<Unsaved>,
    providers: Vec<Arc<dyn LyricsProvider>>,
    enabled_providers: Vec<String>,
    provider_priority: Vec<String>,
    prefer_local: bool,
    playback: Entity<Playback>,
    queue: Entity<Queue>,
    session: Entity<Session>,
    settings: Entity<AppSettings>,
    io: Io,
    task: Option<Task<()>>,
    ahead: Option<Task<()>>,
    ahead_of: Option<String>,
    save: Option<Task<()>>,
    /// Moves the old lyrics.json into the cache database, once.
    _migration: Task<()>,
}

impl Lyrics {
    pub fn new(
        playback: Entity<Playback>,
        queue: Entity<Queue>,
        session: Entity<Session>,
        settings: Entity<AppSettings>,
        providers: Vec<Arc<dyn LyricsProvider>>,
        io: Io,
        cx: &mut Context<Self>,
    ) -> Self {
        cx.observe(&playback, |this, _, cx| this.follow(cx))
            .detach();
        cx.observe(&queue, |this, _, cx| this.prefetch(cx)).detach();
        cx.observe(&settings, |this, settings, cx| {
            let settings = settings.read(cx);
            let enabled = settings.lyrics_providers();
            let prefer_local = settings.prefer_local_lyrics();
            let priority = settings.lyrics_provider_priority();
            if enabled == this.enabled_providers
                && prefer_local == this.prefer_local
                && priority == this.provider_priority
            {
                return;
            }
            this.enabled_providers = enabled.to_vec();
            this.prefer_local = prefer_local;
            this.provider_priority = priority;
            this.task = None;
            this.ahead = None;
            this.ahead_of = None;
            this.forget(cx);
            this.follow(cx);
        })
        .detach();
        let (enabled_providers, prefer_local, provider_priority) = {
            let settings = settings.read(cx);
            (
                settings.lyrics_providers().to_vec(),
                settings.prefer_local_lyrics(),
                settings.lyrics_provider_priority(),
            )
        };
        let store = Sheets::new(storage::Cache::standard());
        let moving = store.clone();
        let migration = cx
            .background_executor()
            .spawn(async move { moving.migrate() });
        Self {
            state: LyricsState::Idle,
            hits: Vec::new(),
            chosen: 0,
            picked: false,
            settled: false,
            revision: 0,
            following: None,
            recent: VecDeque::new(),
            store,
            unsaved: Vec::new(),
            providers,
            enabled_providers,
            provider_priority,
            prefer_local,
            playback,
            queue,
            session,
            settings,
            io,
            task: None,
            ahead: None,
            ahead_of: None,
            save: None,
            _migration: migration,
        }
    }

    pub fn state(&self) -> &LyricsState {
        &self.state
    }

    pub fn following(&self) -> Option<&str> {
        self.following.as_deref()
    }

    pub fn hits(&self) -> &[LyricsHit] {
        &self.hits
    }

    pub fn current(&self) -> Option<&LyricsHit> {
        self.hits.get(self.chosen)
    }

    pub fn revision(&self) -> u64 {
        self.revision
    }

    pub fn choose(&mut self, index: usize, cx: &mut Context<Self>) {
        if index >= self.hits.len() || index == self.chosen {
            return;
        }
        self.chosen = index;
        self.picked = true;
        self.revision = self.revision.wrapping_add(1);
        cx.notify();
    }

    pub fn active_line(&self, cx: &Context<Self>) -> Option<usize> {
        let music::Lyrics::Synced { lines } = &self.current()?.lyrics else {
            return None;
        };
        music::lyrics::active(lines, self.playback.read(cx).position())
    }

    fn follow(&mut self, cx: &mut Context<Self>) {
        let Some(track) = self.playback.read(cx).track() else {
            return self.forget(cx);
        };
        let Some(id) = track.id.as_deref() else {
            return self.forget(cx);
        };
        if self.following.as_deref() == Some(id) {
            return;
        }
        let (track, id) = (track.clone(), id.to_owned());
        self.following = Some(id.clone());
        self.chosen = 0;
        self.picked = false;
        self.settled = false;
        self.revision = self.revision.wrapping_add(1);

        // A file's own lyrics are never cached, so an edit to its tags shows on the next play.
        if !self.reads_file(&id, cx)
            && let Some(found) = self.recall(&id)
            && self
                .asked(&id, cx)
                .iter()
                .all(|provider| found.covers(provider))
        {
            let query = self.query(&track, &id, cx);
            self.show(&found, &query, cx);
            return;
        }
        self.load(id, track, cx);
    }

    /// Puts what `found` holds from the enabled sources on screen as the settled answer.
    fn show(&mut self, found: &Found, query: &LyricsQuery, cx: &mut Context<Self>) {
        let sources = self.asked(self.following.as_deref().unwrap_or_default(), cx);
        let hits: Vec<LyricsHit> = found
            .hits
            .iter()
            .filter(|hit| sources.iter().any(|provider| provider.name() == hit.source))
            .cloned()
            .collect();
        let instrumental = music::lyrics::instrumental(query, &hits);
        self.task = None;
        self.settled = true;
        self.hits = ordered(query, hits, &self.provider_priority);
        self.state = state_for(&self.hits, instrumental);
        cx.notify();
        self.prefetch(cx);
    }

    /// The answer held in memory for `id`, which becomes the most recently used.
    fn recall(&mut self, id: &str) -> Option<Found> {
        let index = self.recent.iter().position(|(held, _)| held == id)?;
        let entry = self.recent.remove(index)?;
        let found = entry.1.clone();
        self.recent.push_front(entry);
        Some(found)
    }

    fn recalls(&self, id: &str) -> bool {
        self.recent.iter().any(|(held, _)| held == id)
    }

    /// Holds `found` in memory as the newest answer, letting the oldest go past `RECENT`.
    fn hold(&mut self, id: String, found: Found) {
        self.recent.retain(|(held, _)| *held != id);
        self.recent.push_front((id, found));
        self.recent.truncate(RECENT);
    }

    /// Reads the answer stored for `id` off the main thread.
    fn stored(&self, id: &str, cx: &Context<Self>) -> Task<Option<Found>> {
        let key = self.key(id, cx);
        let known: Vec<&'static str> = self
            .providers
            .iter()
            .map(|provider| provider.name())
            .collect();
        let store = self.store.clone();
        cx.background_executor().spawn(async move {
            store
                .get(&key, &known)
                .map(|(hits, answered)| Found { hits, answered })
        })
    }

    /// One key per track. The stored answer covers every service that has replied, whichever
    /// are enabled now, and the selection is applied when it is shown.
    fn key(&self, id: &str, cx: &Context<Self>) -> String {
        match self.session.read(cx).slug_for(id) {
            Some(slug) => format!("{slug}:{id}:lyrics"),
            None => format!("{id}:lyrics"),
        }
    }

    /// The providers a lookup for `id` may ask. The file's own lyrics are read only for a local
    /// file, and the services only when its metadata may go online.
    fn asked(&self, id: &str, cx: &Context<Self>) -> Vec<Arc<dyn LyricsProvider>> {
        let online = self.online(id, cx);
        let reads_file = self.reads_file(id, cx);
        let settings = self.settings.read(cx);
        let priority = settings.lyrics_provider_priority();
        providers_for(
            &self.providers,
            settings.lyrics_providers(),
            &priority,
            online,
            reads_file,
            settings.prefer_local_lyrics(),
        )
    }

    fn query(&self, track: &Track, id: &str, cx: &Context<Self>) -> LyricsQuery {
        let key = self.session.read(cx).slug_for(id).map(|provider| TrackKey {
            provider,
            id: id.to_owned(),
        });
        query_for(track, key)
    }

    /// Whether the services may be asked about this track, which a local file only allows when
    /// the user lets its metadata go online.
    fn online(&self, id: &str, cx: &Context<Self>) -> bool {
        !music::is_local_id(id) || self.settings.read(cx).lyrics_for_local_files()
    }

    fn reads_file(&self, id: &str, cx: &Context<Self>) -> bool {
        music::is_local_id(id) && self.settings.read(cx).lyrics_provider_enabled(LOCAL)
    }

    /// Ends a lookup's bookkeeping; true when it was for the track on screen.
    fn finished(&mut self, id: &str) -> bool {
        let current = self.following.as_deref() == Some(id);
        if current {
            self.task = None;
        }
        if self.ahead_of.as_deref() == Some(id) {
            self.ahead_of = None;
        }
        current
    }

    fn forget(&mut self, cx: &mut Context<Self>) {
        if self.following.is_none() {
            return;
        }
        self.task = None;
        self.following = None;
        self.hits.clear();
        self.chosen = 0;
        self.picked = false;
        self.settled = false;
        self.revision = self.revision.wrapping_add(1);
        self.state = LyricsState::Idle;
        cx.notify();
    }

    fn load(&mut self, id: String, track: Track, cx: &mut Context<Self>) {
        self.hits.clear();
        self.state = LyricsState::Loading;
        cx.notify();

        if self.ahead_of.as_deref() == Some(id.as_str()) && self.ahead.is_some() {
            self.task = self.ahead.take();
            self.ahead_of = None;
            return;
        }
        self.task = Some(self.fetch(id, track, cx));
    }

    fn prefetch(&mut self, cx: &mut Context<Self>) {
        if self.task.is_some() {
            return;
        }
        let Some(track) = self.queue.read(cx).upcoming().next() else {
            return;
        };
        let Some(id) = track.id.as_deref() else {
            return;
        };
        if self.ahead_of.as_deref() == Some(id) || self.recalls(id) {
            return;
        }
        let (track, id) = (track.clone(), id.to_owned());
        self.ahead_of = Some(id.clone());
        self.ahead = Some(self.fetch(id, track, cx));
    }

    /// Looks a track up: in memory, then in the cache database, then with the providers. Only
    /// the services that have not answered for this track yet are asked, and a file's own lyrics
    /// are read again on every play. What the cache holds is shown alongside the new answers.
    fn fetch(&mut self, id: String, track: Track, cx: &mut Context<Self>) -> Task<()> {
        let priority = self.settings.read(cx).lyrics_provider_priority();
        let asked = self.asked(&id, cx);
        let remembered = self.recall(&id);
        let stored = remembered.is_none().then(|| self.stored(&id, cx));
        let query = self.query(&track, &id, cx);
        let io = self.io.clone();
        cx.spawn(async move |this, cx| {
            let cached = match stored {
                Some(stored) => stored.await,
                None => remembered,
            };
            if let Some(found) = &cached {
                let held = found.clone();
                let id = id.clone();
                this.update(cx, |this, _| this.hold(id, held)).ok();
            }
            let cached = cached.unwrap_or_default();
            let providers = asked;
            let (sender, mut incoming) = tokio::sync::mpsc::unbounded_channel();
            let held = cached.clone();
            let ranking = query.clone();
            let worker = io.spawn(async move { gather(providers, query, held, sender).await });
            let mut hits = Vec::new();
            while let Some(mut found) = incoming.recv().await {
                hits.append(&mut found);
            }

            let answered = join(worker).await;

            this.update(cx, |this, cx| {
                let current = this.finished(&id);
                match answered {
                    Ok(answered) => {
                        crate::Network::reached(cx);
                        this.keep(id, cached, &hits, answered, cx);
                        if current {
                            let instrumental = music::lyrics::instrumental(&ranking, &hits);
                            let ranked = ordered(&ranking, hits, &priority);
                            this.settle(ranked, instrumental, cx);
                        }
                    }
                    Err(error) => {
                        log::warn!("lyrics: cannot look up {}: {error:#}", track.name);
                        let reason = crate::blamed(&error, cx);
                        if current {
                            this.state = LyricsState::Failed(reason);
                            cx.notify();
                        }
                    }
                }
            })
            .ok();
        })
    }

    /// Adds what the services said in this lookup to the track's answer and saves it when one of
    /// them replied for the first time. A service that failed stays out of `answered`, so the
    /// next lookup asks it again.
    fn keep(
        &mut self,
        id: String,
        mut found: Found,
        hits: &[LyricsHit],
        answered: Vec<&'static str>,
        cx: &mut Context<Self>,
    ) {
        // A file's own lyrics are read again on every play, so they are never kept.
        if !self.online(&id, cx) {
            return;
        }
        let fresh: Vec<&'static str> = answered
            .into_iter()
            .filter(|name| *name != LOCAL && !found.answered.contains(name))
            .collect();
        if !fresh.is_empty() {
            found.hits.extend(
                hits.iter()
                    .filter(|hit| fresh.contains(&hit.source))
                    .cloned(),
            );
            found.answered.extend(fresh);
            if !found.hits.is_empty() {
                let key = self.key(&id, cx);
                self.unsaved
                    .push(Sheets::unsaved(key, &found.hits, &found.answered));
                self.schedule_save(cx);
            }
        }
        self.hold(id, found);
    }

    /// Shows the preferred usable answer after lookup and starts prefetching the next track.
    fn settle(&mut self, ranked: Vec<LyricsHit>, instrumental: bool, cx: &mut Context<Self>) {
        if !self.settled {
            log::debug!("lyrics: settling on the best answer");
            self.settled = true;
            self.pin(ranked, None);
            self.state = state_for(&self.hits, instrumental);
        }
        cx.notify();
        self.prefetch(cx);
    }

    fn pin(&mut self, ranked: Vec<LyricsHit>, displayed: Option<&LyricsHit>) {
        let anchor = match self.picked {
            true => self.hits.get(self.chosen).cloned(),
            false => displayed.cloned(),
        };
        let before = self.current().map(|hit| hit.lyrics.clone());
        let mut hits = ranked;
        let held = keep_displayed_first(&mut hits, anchor.as_ref());
        self.hits = hits;
        self.chosen = 0;
        self.picked &= held;
        if before.as_ref() != self.current().map(|hit| &hit.lyrics) {
            self.revision = self.revision.wrapping_add(1);
        }
    }

    /// Writes the unsaved answers once lookups have gone quiet for `SAVE_DELAY`. A write that
    /// has begun always finishes, even when another answer reschedules the save.
    fn schedule_save(&mut self, cx: &mut Context<Self>) {
        self.save = Some(cx.spawn(async move |this, cx| {
            cx.background_executor().timer(SAVE_DELAY).await;
            let Ok((store, unsaved)) = this.update(cx, |this, _| {
                (this.store.clone(), std::mem::take(&mut this.unsaved))
            }) else {
                return;
            };
            cx.background_executor()
                .spawn(async move { store.save(unsaved) })
                .detach();
        }));
    }
}

fn keep_displayed_first(hits: &mut Vec<LyricsHit>, displayed: Option<&LyricsHit>) -> bool {
    let Some(index) =
        displayed.and_then(|displayed| hits.iter().position(|hit| same(hit, displayed)))
    else {
        return false;
    };
    let displayed = hits.remove(index);
    hits.insert(0, displayed);
    true
}

fn same(left: &LyricsHit, right: &LyricsHit) -> bool {
    left.source == right.source
        && left.title == right.title
        && left.artist == right.artist
        && left.album == right.album
        && left.duration == right.duration
        && left.lyrics.worded() == right.lyrics.worded()
}

/// Applies enablement, local-file privacy and preference order before any provider is queried.
fn providers_for(
    providers: &[Arc<dyn LyricsProvider>],
    enabled: &[String],
    priority: &[String],
    online: bool,
    reads_file: bool,
    prefer_local: bool,
) -> Vec<Arc<dyn LyricsProvider>> {
    let mut providers: Vec<_> = providers
        .iter()
        .filter(|provider| enabled.iter().any(|name| name == provider.name()))
        .filter(|provider| match provider.name() == LOCAL {
            true => reads_file,
            false => online,
        })
        .cloned()
        .collect();
    providers.sort_by_key(|provider| match prefer_local && provider.name() == LOCAL {
        true => 0,
        false => {
            priority
                .iter()
                .position(|name| name == provider.name())
                .unwrap_or(priority.len())
                + 1
        }
    });
    providers
}

fn ordered(query: &LyricsQuery, hits: Vec<LyricsHit>, priority: &[String]) -> Vec<LyricsHit> {
    let mut ranked = music::lyrics::rank_with_priority(query, hits, priority);
    music::lyrics::reshape(&mut ranked);
    ranked
}

/// What the services have said about one track: every hit matched to it, unranked and from every
/// source, and the services that replied. The selection is applied when it is shown.
#[derive(Clone, Default)]
struct Found {
    hits: Vec<LyricsHit>,
    answered: Vec<&'static str>,
}

impl Found {
    /// Whether `provider` has nothing left to say. A file's own lyrics are never covered, since
    /// they are read again on every play.
    fn covers(&self, provider: &Arc<dyn LyricsProvider>) -> bool {
        provider.name() != LOCAL && self.answered.contains(&provider.name())
    }
}

fn state_for(hits: &[LyricsHit], instrumental: bool) -> LyricsState {
    match (hits.is_empty(), instrumental) {
        (false, _) => LyricsState::Ready,
        (true, true) => LyricsState::Instrumental,
        (true, false) => LyricsState::Missing,
    }
}

fn query_for(track: &Track, key: Option<TrackKey>) -> LyricsQuery {
    LyricsQuery {
        title: track.name.clone(),
        artist: track.artists.clone(),
        album: (!track.album.is_empty()).then(|| track.album.clone()),
        duration: track.duration,
        track: key,
    }
}

/// Tries sources in order, reusing cached answers and stopping at the first usable lyrics.
/// Only successful requests enter `answered`; failures and skipped sources remain retryable.
async fn gather(
    providers: Vec<Arc<dyn LyricsProvider>>,
    query: LyricsQuery,
    cached: Found,
    sender: tokio::sync::mpsc::UnboundedSender<Vec<LyricsHit>>,
) -> anyhow::Result<Vec<&'static str>> {
    let mut answered = Vec::new();
    for provider in providers {
        let name = provider.name();
        let mut found = if cached.covers(&provider) {
            cached
                .hits
                .iter()
                .filter(|hit| hit.source == name)
                .cloned()
                .collect()
        } else {
            let result = tokio::select! {
                _ = sender.closed() => break,
                result = tokio::time::timeout(PROVIDER_TIMEOUT, provider.search(&query)) => result,
            };
            match result {
                Ok(Ok(hits)) => {
                    answered.push(name);
                    hits
                }
                Ok(Err(error)) => {
                    log::warn!("lyrics: {name} did not answer: {error:#}");
                    continue;
                }
                Err(_) => {
                    log::warn!("lyrics: {name} did not answer before the timeout");
                    continue;
                }
            }
        };
        found.retain(|hit| music::lyrics::matched(&query, hit));
        let usable = found.iter().any(|hit| music::lyrics::eligible(&query, hit));
        if sender.send(found).is_err() || usable {
            break;
        }
    }
    Ok(answered)
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use music::{Lyrics as Sheet, LyricsHit, LyricsLine, LyricsWord, Voice};

    use super::keep_displayed_first;

    fn hit(source: &'static str, lyrics: Sheet) -> LyricsHit {
        LyricsHit {
            source,
            trust: 0,
            lyrics,
            instrumental: false,
            title: "title".to_owned(),
            artist: "artist".to_owned(),
            album: None,
            duration: None,
            writers: Vec::new(),
        }
    }

    struct Provider {
        name: &'static str,
        hits: Vec<LyricsHit>,
        fail: bool,
        calls: std::sync::Arc<std::sync::Mutex<Vec<&'static str>>>,
    }

    #[async_trait::async_trait]
    impl music::LyricsProvider for Provider {
        fn name(&self) -> &'static str {
            self.name
        }
        async fn search(&self, _: &music::LyricsQuery) -> anyhow::Result<Vec<LyricsHit>> {
            self.calls.lock().unwrap().push(self.name);
            anyhow::ensure!(!self.fail, "provider failed");
            Ok(self.hits.clone())
        }
    }

    fn query() -> music::LyricsQuery {
        music::LyricsQuery {
            title: "title".into(),
            artist: "artist".into(),
            album: None,
            duration: Duration::ZERO,
            track: None,
        }
    }

    fn providers(
        cases: Vec<(&'static str, Vec<LyricsHit>, bool)>,
        calls: &std::sync::Arc<std::sync::Mutex<Vec<&'static str>>>,
    ) -> Vec<std::sync::Arc<dyn music::LyricsProvider>> {
        cases
            .into_iter()
            .map(|(name, hits, fail)| {
                std::sync::Arc::new(Provider {
                    name,
                    hits,
                    fail,
                    calls: calls.clone(),
                }) as _
            })
            .collect()
    }

    #[tokio::test]
    async fn plain_lyrics_stop_before_a_lower_priority_provider() {
        let calls = Default::default();
        let sources = providers(
            vec![
                ("first", vec![hit("first", Sheet::plain("line"))], false),
                ("second", vec![hit("second", Sheet::plain("other"))], false),
            ],
            &calls,
        );
        let (sender, mut receiver) = tokio::sync::mpsc::unbounded_channel();
        let answered = super::gather(sources, query(), super::Found::default(), sender)
            .await
            .unwrap();
        assert_eq!(answered, vec!["first"]);
        assert_eq!(*calls.lock().unwrap(), vec!["first"]);
        assert_eq!(receiver.recv().await.unwrap()[0].source, "first");
        assert!(receiver.recv().await.is_none());
    }

    #[tokio::test]
    async fn errors_empty_and_wrong_track_answers_fall_back_without_marking_failures() {
        let calls = Default::default();
        let mut wrong = hit("wrong", Sheet::plain("line"));
        wrong.artist = "another artist".into();
        let sources = providers(
            vec![
                ("error", vec![], true),
                ("empty", vec![], false),
                ("wrong", vec![wrong], false),
                ("last", vec![hit("last", Sheet::plain("line"))], false),
                ("skipped", vec![], false),
            ],
            &calls,
        );
        let (sender, mut receiver) = tokio::sync::mpsc::unbounded_channel();
        let answered = super::gather(sources, query(), super::Found::default(), sender)
            .await
            .unwrap();
        assert_eq!(answered, vec!["empty", "wrong", "last"]);
        assert_eq!(
            *calls.lock().unwrap(),
            vec!["error", "empty", "wrong", "last"]
        );
        assert!(receiver.recv().await.unwrap().is_empty());
        assert!(receiver.recv().await.unwrap().is_empty());
        assert_eq!(receiver.recv().await.unwrap()[0].source, "last");
    }

    #[tokio::test]
    async fn cached_lower_priority_hits_do_not_skip_an_unanswered_preferred_source() {
        let calls = Default::default();
        let sources = providers(
            vec![
                (
                    "first",
                    vec![hit("first", Sheet::plain("preferred"))],
                    false,
                ),
                ("second", vec![], false),
            ],
            &calls,
        );
        let cached = super::Found {
            hits: vec![hit("second", Sheet::plain("cached"))],
            answered: vec!["second"],
        };
        let (sender, mut receiver) = tokio::sync::mpsc::unbounded_channel();
        super::gather(sources, query(), cached, sender)
            .await
            .unwrap();
        assert_eq!(*calls.lock().unwrap(), vec!["first"]);
        assert_eq!(receiver.recv().await.unwrap()[0].source, "first");
    }

    #[tokio::test]
    async fn cached_empty_answers_fall_back_to_cached_lyrics_without_network_requests() {
        let calls = Default::default();
        let sources = providers(
            vec![("first", vec![], false), ("second", vec![], false)],
            &calls,
        );
        let cached = super::Found {
            hits: vec![hit("second", Sheet::plain("cached"))],
            answered: vec!["first", "second"],
        };
        let (sender, mut receiver) = tokio::sync::mpsc::unbounded_channel();
        let answered = super::gather(sources, query(), cached, sender)
            .await
            .unwrap();
        assert!(answered.is_empty());
        assert!(calls.lock().unwrap().is_empty());
        assert!(receiver.recv().await.unwrap().is_empty());
        assert_eq!(receiver.recv().await.unwrap()[0].source, "second");
    }

    #[tokio::test]
    async fn local_lyrics_are_read_again_even_when_the_cache_claims_an_answer() {
        let calls = Default::default();
        let sources = providers(
            vec![(
                music::lyrics::LOCAL,
                vec![hit(music::lyrics::LOCAL, Sheet::plain("edited"))],
                false,
            )],
            &calls,
        );
        let cached = super::Found {
            hits: vec![],
            answered: vec![music::lyrics::LOCAL],
        };
        let (sender, mut receiver) = tokio::sync::mpsc::unbounded_channel();
        super::gather(sources, query(), cached, sender)
            .await
            .unwrap();
        assert_eq!(*calls.lock().unwrap(), vec![music::lyrics::LOCAL]);
        assert_eq!(
            receiver.recv().await.unwrap()[0].lyrics,
            Sheet::plain("edited")
        );
    }

    struct Waiting {
        started: std::sync::Arc<tokio::sync::Notify>,
        stopped: std::sync::Arc<std::sync::atomic::AtomicBool>,
    }

    struct Stopped(std::sync::Arc<std::sync::atomic::AtomicBool>);

    impl Drop for Stopped {
        fn drop(&mut self) {
            self.0.store(true, std::sync::atomic::Ordering::SeqCst);
        }
    }

    #[async_trait::async_trait]
    impl music::LyricsProvider for Waiting {
        fn name(&self) -> &'static str {
            "waiting"
        }
        async fn search(&self, _: &music::LyricsQuery) -> anyhow::Result<Vec<LyricsHit>> {
            let _stopped = Stopped(self.stopped.clone());
            self.started.notify_one();
            std::future::pending().await
        }
    }

    #[tokio::test]
    async fn dropping_the_receiver_cancels_an_in_flight_provider() {
        let started = std::sync::Arc::new(tokio::sync::Notify::new());
        let stopped = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let provider = std::sync::Arc::new(Waiting {
            started: started.clone(),
            stopped: stopped.clone(),
        });
        let (sender, receiver) = tokio::sync::mpsc::unbounded_channel();
        let worker = tokio::spawn(super::gather(
            vec![provider],
            query(),
            super::Found::default(),
            sender,
        ));
        tokio::time::timeout(Duration::from_secs(1), started.notified())
            .await
            .unwrap();
        drop(receiver);
        let answered = tokio::time::timeout(Duration::from_secs(1), worker)
            .await
            .unwrap()
            .unwrap()
            .unwrap();
        assert!(answered.is_empty());
        assert!(stopped.load(std::sync::atomic::Ordering::SeqCst));
    }

    #[tokio::test]
    async fn a_stalled_source_times_out_and_remains_retryable() {
        let started = std::sync::Arc::new(tokio::sync::Notify::new());
        let stopped = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let calls = Default::default();
        let mut sources: Vec<std::sync::Arc<dyn music::LyricsProvider>> =
            vec![std::sync::Arc::new(Waiting {
                started,
                stopped: stopped.clone(),
            })];
        sources.extend(providers(
            vec![("next", vec![hit("next", Sheet::plain("line"))], false)],
            &calls,
        ));
        let (sender, mut receiver) = tokio::sync::mpsc::unbounded_channel();
        let answered = tokio::time::timeout(
            super::PROVIDER_TIMEOUT + Duration::from_secs(2),
            super::gather(sources, query(), super::Found::default(), sender),
        )
        .await
        .unwrap()
        .unwrap();
        assert_eq!(answered, vec!["next"]);
        assert_eq!(receiver.recv().await.unwrap()[0].source, "next");
        assert!(stopped.load(std::sync::atomic::Ordering::SeqCst));
    }

    #[test]
    fn lookup_order_respects_priority_enablement_and_local_privacy() {
        let calls = Default::default();
        let sources = providers(
            vec![
                ("Spotify", vec![], false),
                ("LrcLib", vec![], false),
                (music::lyrics::LOCAL, vec![], false),
            ],
            &calls,
        );
        let priority = vec![
            "LrcLib".into(),
            "Spotify".into(),
            music::lyrics::LOCAL.into(),
        ];
        let names = |online, reads_file, prefer_local, enabled: &[String]| {
            super::providers_for(
                &sources,
                enabled,
                &priority,
                online,
                reads_file,
                prefer_local,
            )
            .iter()
            .map(|source| source.name())
            .collect::<Vec<_>>()
        };
        assert_eq!(
            names(true, false, false, &priority),
            vec!["LrcLib", "Spotify"]
        );
        assert_eq!(
            names(true, false, false, &["Spotify".into()]),
            vec!["Spotify"]
        );
        assert_eq!(
            names(false, true, false, &priority),
            vec![music::lyrics::LOCAL]
        );
        assert!(names(false, false, false, &priority).is_empty());
        assert_eq!(
            names(true, true, true, &priority),
            vec![music::lyrics::LOCAL, "LrcLib", "Spotify"]
        );
    }

    #[test]
    fn cached_provider_priority_beats_lyrics_quality_and_duplicate_text() {
        let first = hit("first", Sheet::plain("line"));
        let mut second = first.clone();
        second.source = "second";
        second.trust = 100;
        let ranked = super::ordered(
            &query(),
            vec![second, first],
            &["first".into(), "second".into()],
        );
        assert_eq!(ranked[0].source, "first");
        assert_eq!(ranked.len(), 1);
    }

    #[test]
    fn a_displayed_karaoke_result_stays_selected_after_final_ranking() {
        let plain = hit("plain", Sheet::plain("line"));
        let displayed = hit(
            "karaoke",
            Sheet::Synced {
                lines: vec![LyricsLine {
                    start: Duration::ZERO,
                    end: Some(Duration::from_secs(1)),
                    text: "line".to_owned(),
                    romanized: None,
                    words: Some(vec![LyricsWord {
                        start: Duration::ZERO,
                        end: Duration::from_secs(1),
                        text: "line".to_owned(),
                    }]),
                    secondary: Vec::new(),
                    voice: Voice::Lead,
                }]
                .into(),
            },
        );
        let mut final_ranking = vec![plain, displayed.clone()];

        keep_displayed_first(&mut final_ranking, Some(&displayed));

        assert_eq!(final_ranking.first(), Some(&displayed));
    }
}
