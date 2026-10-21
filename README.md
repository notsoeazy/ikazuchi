<div align="center">

# 雷 IKAZUCHI

**Lightweight, 100% Rust & Dioxus Multi-Platform Manga & Manhwa Reader**

[![Rust](https://img.shields.io/badge/Language-Rust_2024-f46623?style=flat-square&logo=rust)](https://www.rust-lang.org)
[![Dioxus](https://img.shields.io/badge/UI-Dioxus_0.6-00d2ff?style=flat-square)](https://dioxuslabs.com)
[![Platforms](https://img.shields.io/badge/Platforms-Linux_|_Windows_|_Android-8a2be2?style=flat-square)](https://github.com)
[![License](https://img.shields.io/badge/License-MPL--2.0-blue?style=flat-square)](LICENSE)

*Inspired by [Mihon](https://github.com/mihonapp/mihon) & [Yōkai](https://github.com/null2264/yokai). Native performance, zero JVM or Android runtime dependencies, and serverless multi-device synchronization via your private GitHub repository.*

---

</div>

## ⚡ Highlights

* **Pure-Rust Extension Engine (`ikazuchi-dex`):** Reads Mihon `.apk` packages and inspects compiled `classes.dex` bytecode directly in Rust to discover and run multisrc scrapers (MangaThemesia, Madara, MangaDex, LocalSource) without Android runtime overhead.
* **The 5 Realms Architecture:**
  * ⚡ **Sanctuary:** Fast library grid with unread lightning badges, categories, and Yōkai dynamic grouping (*By Category, Status, Source, Genre, Author*).
  * 🌩 **Storm:** Real-time feed of newly dropped chapters and your continue-reading timeline.
  * 🧭 **Horizon:** Universal extension catalog browser supporting Keiyoushi v2 repos (`index.json` & `index.pb`), 1-click installer, and saved searches.
  * 📜 **Chronicle:** Detailed manga synopsis, custom metadata editing (`custom_manga_info`), scanlator group exclusion, and lossless source migration.
  * ⛩ **Vault:** Application settings, library statistics, and serverless synchronization.
* **⚡ Raijin Reader Engine:** Gapless high-framerate **Webtoon Vertical Strip** and **Paged RTL/LTR** modes with background page pre-fetching, zoom clamping, and a minimalist obsidian HUD.
* **☁ Serverless Multi-Device Sync (`ikazuchi-sync`):** Synchronize reading progress, bookmarks, and library state across Linux, Windows, and Android using your own **Private GitHub Repository** with deterministic CRDT sharding—no server to host or maintain.
* **🔄 Mihon & Yōkai Compatibility:**
  * Import & export standard `.tachibk` (`.proto.gz`) backups with full preservation of categories, custom metadata (tags `602, 800..805`), history, and trackers.
  * Downloads saved as standard `.cbz` archives embedding Anansi `ComicInfo.xml` v2.0 metadata.
  * Anti-ban source-grouped rate limiting and single-flight lock for manual & scheduled library updates.

---

## 🏗 Workspace Architecture

```
ikazuchi/
├── app/                  # Dioxus multi-platform application shell, UI views & Raijin reader
├── crates/
│   ├── ikazuchi-core/    # Domain models, MD5 63-bit source IDs, and config.json settings
│   ├── ikazuchi-dex/     # Pure-Rust APK/DEX parser, repo index parser & native scrapers
│   ├── ikazuchi-db/      # SQLite storage, .tachibk Protobuf backup engine & migration logic
│   ├── ikazuchi-sync/    # Serverless GitHub sync engine & deterministic CRDT merger
│   └── ikazuchi-archive/ # CBZ archive streaming, ComicInfo.xml v2.0 reader & atomic downloads
```

---

## 🚀 Quick Start

### Prerequisites
* [Rust](https://rustup.rs/) (1.80+ or latest stable)

### Run Tests
```bash
cargo test --all
```

### Launch Application
```bash
cargo run --bin ikazuchi
```

---

## 📜 License

Licensed under the [Mozilla Public License 2.0 (MPL-2.0)](LICENSE).
