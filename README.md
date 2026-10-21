<div align="center">

<img src="./.github/readme-icon.svg" alt="Tempo Desktop" width="96" height="96">

<h1>Tempo Desktop</h1>

### A native music streaming client, built with Rust and GPUI

Stream from YouTube Music and play your local files — all in one **native** app.
</div>

> [!NOTE]
> **Tempo Desktop is a personal, unofficial fork of [Sonora](https://github.com/sonorahq/sonora),
> rebranded and narrowed to YouTube Music.** It keeps the original GPL-3.0 license and credits the
> upstream project for the vast majority of the application. If you want Spotify, Apple Music,
> Deezer or Subsonic support, use upstream Sonora.

> [!IMPORTANT]
> **Tempo Desktop is not a piracy tool.**
>
> It is not a platform for obtaining or sharing copyrighted material, it does not export decrypted
> streams, DRM licenses or content keys, and it does not circumvent subscriptions or other
> restrictions put in place by music streaming platforms. If a service requires a valid
> subscription to play a track, so does Tempo Desktop.

## Features

- **YouTube Music** playback, signed in or as a guest, plus **local file** playback
- Gapless playback, audio normalization, shuffle, sleep timer
- Synced/karaoke lyrics, background vocals, and romanization
- Scrobbling with LastFM, ListenBrainz, LibreFM, and Maloja
- Themes, fonts, icons, transparency, blur, and window styling
- Discord Rich Presence, native file opening
- macOS, Windows, and Linux support

## Install

### Debian / Ubuntu (preset, no setup)

Grab the `.deb` from [Releases](https://github.com/4jeel-cloud/tempo-desktop/releases) and install
it. Everything the app needs is pulled in as a dependency:

```sh
sudo apt install ./tempo-desktop_*_amd64.deb
```

Launch **Tempo Desktop** from your app menu, or run `sonora` in a terminal. On first launch it opens
straight into YouTube Music as a guest — no account needed. Signing in is optional, in Settings.

### Build from source

You need a Rust toolchain and, for the native window, the usual GPU and audio development
libraries. The project is a Cargo workspace.

```sh
git clone https://github.com/4jeel-cloud/tempo-desktop.git
cd tempo-desktop
nix develop            # recommended: brings every build dependency; or set up your own
cargo run --locked --package sonora
```

To produce and run an optimized binary on any Linux distro:

```sh
cargo build --release --package sonora
./target/release/sonora
```

`.cargo/config.toml` links with `mold` on `x86_64-unknown-linux-gnu`, so install `mold` or build
with `RUSTFLAGS=""`. `crates/widevine` compiles C++, so a C++ compiler is required as well.

#### Nix

```nix
inputs.tempo.url = "github:4jeel-cloud/tempo-desktop";
```

```text
inputs.tempo.packages.${system}.default     (prebuilt where available, else from source)
inputs.tempo.packages.${system}.sonora      (same as default)
inputs.tempo.packages.${system}.sonora-bin  (prebuilt, x86_64-linux, aarch64-linux, aarch64-darwin)
inputs.tempo.packages.${system}.sonora-src  (always built from source)
```

## Share a packaged build

Friends install Tempo Desktop by downloading a `.deb` from the
[Releases](https://github.com/4jeel-cloud/tempo-desktop/releases) page and running one command —
no toolchain, no setup:

```sh
sudo apt install ./tempo-desktop_*_amd64.deb
```

Then they launch **Tempo Desktop** from the app menu. On first launch it opens straight into
YouTube Music as a guest.

### Publishing a release

Push a version tag and GitHub Actions builds the `.deb` and attaches it to the Release:

```sh
git tag tempo-v0.43.0
git push tempo tempo-v0.43.0
```

The [`deb` workflow](.github/workflows/deb.yml) then publishes `tempo-desktop_<version>_amd64.deb`
and a `SHA256SUMS` file on the tag's Release. It can also be run by hand from the Actions tab. To
build the same package locally:

```sh
./scripts/package-deb.sh     # writes dist/tempo-desktop_<version>_amd64.deb
```

## Translations

<!-- i18n:start -->

| Language | Translated | Coverage |
| --- | --- | --- |
| English (`en-US`) | 755/755 | 100% |
| Deutsch (`de`) | 628/755 | 83% |
| Español (`es`) | 690/755 | 91% |
| Français (`fr`) | 749/755 | 99% |
| Italiano (`it`) | 605/755 | 80% |
| Bahasa Indonesia (`id`) | 605/755 | 80% |
| 日本語 (`ja`) | 605/755 | 80% |
| Русский (`ru`) | 711/755 | 94% |
| Українська (`uk`) | 711/755 | 94% |
| Polski (`pl`) | 711/755 | 94% |
| Čeština (`cs`) | 743/755 | 98% |
| Português (Brasil) (`pt-BR`) | 605/755 | 80% |
| 简体中文 (`zh-CN`) | 605/755 | 80% |
| Türkçe (`tr`) | 605/755 | 80% |
| Shqip (`sq`) | 714/755 | 95% |

<!-- i18n:end -->

## Credits

Tempo Desktop exists because of [Sonora](https://github.com/sonorahq/sonora), the upstream project
this fork is built on. It is built with the help of some incredible open-source projects, including:

- [Zed](https://github.com/zed-industries/zed) — provides `gpui`, the native Rust rendering stack.
- [librespot](https://github.com/librespot-org/librespot) — Spotify playback and library integration.
- [yt-dlp](https://github.com/yt-dlp/yt-dlp) — certain YouTube ideas implemented in
  [ytmusic-rs](https://github.com/sonorahq/ytmusic-rs).

## License

Copyright (C) 2026 Tempo Desktop Contributors.

Tempo Desktop is free software, released under the [GNU General Public License version
3 or later](COPYING). As a fork of Sonora, it is distributed under the same license and with the
same upstream credits.

Tempo Desktop is an unofficial client and is not affiliated with, endorsed by, or sponsored by
Spotify AB or YouTube.

The binary also embeds the [Inter](https://github.com/rsms/inter) typeface (SIL
Open Font License 1.1) and four interchangeable icon sets:
[Lucide](https://lucide.dev) (ISC), [Iconoir](https://iconoir.com) (MIT),
[Remix Icon](https://remixicon.com) 4.8.0 (Apache 2.0) and the
[Solar](https://www.figma.com/community/file/1166831539721848736) Linear set
(CC BY 4.0, by 480 Design). Each pack keeps its licence beside its files in
`assets/icons`. `THIRD-PARTY.md` lists every bundled dependency.
