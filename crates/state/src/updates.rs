use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{Context as _, Result};
use gpui::{Context, Entity, Task};
use sha2::{Digest as _, Sha256};

use crate::{AppSettings, Io, join};

const REPO: &str = "https://github.com/4jeel-cloud/tempo-desktop";
const LATEST: &str = "https://github.com/4jeel-cloud/tempo-desktop/releases/latest";
const INSTALLER: &str = "Tempo-Desktop-Setup.exe";
const SUMS: &str = "SHA256SUMS-windows";
const UNINSTALLER: &str = "unins000.exe";
const RUNNING: &str = env!("CARGO_PKG_VERSION");
const INSTALLABLE: bool = cfg!(target_os = "windows");
const AGENT: &str = concat!(
    "tempo-desktop/",
    env!("CARGO_PKG_VERSION"),
    " (https://github.com/4jeel-cloud/tempo-desktop)"
);

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Release {
    pub version: String,
    pub page: String,
    installer: Option<String>,
    sums: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum UpdateState {
    Quiet,
    Current,
    Offered(Release),
    Fetching,
    Failed,
}

pub struct Updates {
    state: UpdateState,
    settings: Entity<AppSettings>,
    http: reqwest::Client,
    io: Io,
    task: Option<Task<()>>,
}

impl Updates {
    pub fn new(settings: Entity<AppSettings>, io: Io, cx: &mut Context<Self>) -> Self {
        let mut updates = Self {
            state: UpdateState::Quiet,
            settings,
            http: reqwest::Client::new(),
            io,
            task: None,
        };
        updates.look(cx);
        updates
    }

    pub fn state(&self) -> &UpdateState {
        &self.state
    }

    pub fn offered(&self) -> Option<&Release> {
        match &self.state {
            UpdateState::Offered(release) => Some(release),
            _ => None,
        }
    }

    /// Whether a release lookup is in flight. The page shows its checking state from this,
    /// because a lookup keeps `Quiet` until it has an answer.
    pub fn checking(&self) -> bool {
        self.task.is_some()
    }

    /// Runs a release lookup now, rather than waiting for the next launch.
    pub fn check(&mut self, cx: &mut Context<Self>) {
        if self.task.is_some() {
            return;
        }
        self.look(cx);
    }

    pub fn installable(&self) -> bool {
        INSTALLABLE
            && installed()
            && self
                .offered()
                .is_some_and(|release| release.installer.is_some() && release.sums.is_some())
    }

    pub fn dismiss(&mut self, cx: &mut Context<Self>) {
        self.task = None;
        self.state = UpdateState::Quiet;
        cx.notify();
    }

    pub fn install(&mut self, cx: &mut Context<Self>) {
        let Some(release) = self.offered().cloned() else {
            return;
        };
        let (Some(installer), Some(sums)) = (release.installer, release.sums) else {
            return;
        };
        self.state = UpdateState::Fetching;
        cx.notify();

        let http = self.http.clone();
        let io = self.io.clone();
        let version = release.version;
        self.task = Some(cx.spawn(async move |this, cx| {
            let fetched =
                join(io.spawn(async move { fetch(&http, &installer, &sums, &version).await }))
                    .await;
            let started = fetched.and_then(|installer| launch(&installer));
            match started {
                Ok(()) => {
                    cx.update(|cx| cx.quit());
                }
                Err(error) => {
                    log::warn!("updates: cannot install the new version: {error:#}");
                    this.update(cx, |this, cx| {
                        this.task = None;
                        this.state = UpdateState::Failed;
                        cx.notify();
                    })
                    .ok();
                }
            }
        }));
    }

    fn look(&mut self, cx: &mut Context<Self>) {
        if !self.settings.read(cx).check_updates() {
            return;
        }
        let http = self.http.clone();
        let io = self.io.clone();
        self.task = Some(cx.spawn(async move |this, cx| {
            let found = join(io.spawn(async move { latest(&http).await })).await;
            this.update(cx, |this, cx| {
                this.task = None;
                match found {
                    Ok(Some(release)) => {
                        this.state = UpdateState::Offered(release);
                        cx.notify();
                    }
                    Ok(None) => {
                        this.state = UpdateState::Current;
                        cx.notify();
                    }
                    Err(error) => log::warn!("updates: cannot ask github: {error:#}"),
                }
            })
            .ok();
        }));
        cx.notify();
    }
}

async fn latest(http: &reqwest::Client) -> Result<Option<Release>> {
    // `/releases/latest` redirects to the newest release's tag page. Reading that redirect
    // avoids the API's small unauthenticated rate limit, which otherwise leaves the app with
    // no notice when it runs out.
    let response = http
        .get(LATEST)
        .header("User-Agent", AGENT)
        .send()
        .await
        .context("cannot reach github")?
        .error_for_status()
        .context("github refused the release request")?;

    let url = response.url().as_str().to_owned();
    let Some((_, tag)) = url.split_once("/releases/tag/") else {
        return Ok(None);
    };
    let tag = tag.trim_end_matches('/');
    let version = tag.trim_start_matches("tempo-").trim_start_matches('v');
    if !newer(version, RUNNING) {
        return Ok(None);
    }

    Ok(Some(Release {
        version: version.to_owned(),
        page: format!("{REPO}/releases/tag/{tag}"),
        installer: Some(format!("{REPO}/releases/download/{tag}/{INSTALLER}")),
        sums: Some(format!("{REPO}/releases/download/{tag}/{SUMS}")),
    }))
}

async fn fetch(
    http: &reqwest::Client,
    installer: &str,
    sums: &str,
    version: &str,
) -> Result<PathBuf> {
    let listed = http
        .get(sums)
        .header("User-Agent", AGENT)
        .send()
        .await
        .context("cannot reach the checksums")?
        .error_for_status()
        .context("the checksums are missing")?
        .text()
        .await
        .context("cannot read the checksums")?;
    let wanted = listed
        .lines()
        .filter_map(|line| line.split_once(char::is_whitespace))
        .find(|(_, name)| {
            name.trim()
                .trim_start_matches('*')
                .rsplit(['/', '\\'])
                .next()
                == Some(INSTALLER)
        })
        .map(|(sum, _)| sum.trim().to_ascii_lowercase())
        .context("the release lists no checksum for the installer")?;

    let bytes = http
        .get(installer)
        .header("User-Agent", AGENT)
        .send()
        .await
        .context("cannot reach the installer")?
        .error_for_status()
        .context("the installer is missing")?
        .bytes()
        .await
        .context("cannot read the installer")?;

    let sum = format!("{:x}", Sha256::digest(&bytes));
    if sum != wanted {
        anyhow::bail!("the installer does not match its checksum");
    }

    let path = std::env::temp_dir().join(format!("Tempo-Desktop-Setup-{version}.exe"));
    std::fs::write(&path, &bytes).context("cannot keep the installer")?;
    Ok(path)
}

fn launch(installer: &Path) -> Result<()> {
    Command::new(installer)
        .args(["/SILENT", "/SUPPRESSMSGBOXES", "/NORESTART", "/relaunch=1"])
        .spawn()
        .context("cannot start the installer")?;
    Ok(())
}

fn installed() -> bool {
    std::env::current_exe()
        .ok()
        .and_then(|exe| Some(exe.parent()?.join(UNINSTALLER)))
        .is_some_and(|uninstaller| uninstaller.exists())
}

fn newer(offered: &str, running: &str) -> bool {
    match (numbered(offered), numbered(running)) {
        (Some(offered), Some(running)) => offered > running,
        _ => false,
    }
}

fn numbered(version: &str) -> Option<(u64, u64, u64)> {
    let core = version.trim().split(['-', '+']).next()?;
    let mut parts = core.split('.').map(|part| part.parse::<u64>().ok());
    Some((parts.next()??, parts.next()??, parts.next()??))
}
