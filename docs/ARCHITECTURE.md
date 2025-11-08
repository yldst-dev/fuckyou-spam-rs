# fuckyou-spam-rust Architecture

## Objectives
- Feature-parity rewrite of the existing Node.js spam-detection bot (`fuckyou-spam`) in idiomatic Rust.
- Preserve operational guarantees (Telegram command set, Cerebras-powered classification, whitelist enforcement, logging, and restart cron jobs).
- Improve modularity via well-defined service boundaries so we can evolve individual subsystems (e.g., switching AI providers or storage engines) without cross-cutting rewrites.

## Module Overview
| Module | Responsibility |
| --- | --- |
| `config` | Deterministic environment parsing, sane defaults, and strongly-typed sub-configs (directories, web fetch limits, scheduler crons, etc.). |
| `directories` | Filesystem bootstrap, permission normalization, and write checks for `logs/` + `data/`. |
| `logging` | `tracing`-based fan-out to console + rotating file sinks, ensuring consistent metadata and timezone-aware timestamps. |
| `db` | Async SQLite (`sqlx`) pool init plus a repository dedicated to whitelist management. |
| `web_content` | HTTP fetching (`reqwest`), lightweight readability heuristics (`scraper` + `html2text`), URL extraction, and per-message limits. |
| `queue` | Thread-safe dual-queue abstraction for high/normal priority flows; later wired into Telegram update handlers. |
| `ai` | Cerebras Chat Completions client with deterministic system prompts and JSON parsing for classification maps. |
| `telegram` | `teloxide` dispatcher, command handling, and (future) message ingestion hooks. |
| `scheduler` | Cron-based restart callbacks using `tokio-cron-scheduler`. |
| `app` | Composition root tying together config, infra, services, and lifecycle orchestration. |

## Technology Decisions
- **Rust 2024 Edition** gives us `let ... else` and `async fn` improvements while aligning with the Rust 1.88 release cadence announced on 2025-10-24, which formalized the edition hand-off and stabilizations such as trait solver tweaks and let-else match ergonomics.
- **`teloxide` 0.17.1** (released 2025-04-07) is the latest Telegram bot framework with Dispatcher v0.7 semantics, improved command macros, and native `ctrlc` handling that mirrors Node's polling logic.
- **`reqwest` 0.12.9`** (2025-09-08) ships HTTP/2 fixes and connection pooling improvements we need for Cerebras + web scraping workloads.
- **`sqlx` 0.8.3** (2025-09-16) delivers WAL-friendly SQLite performance fixes and compile-time query checking for the whitelist repository.
- **`tokio-cron-scheduler` 0.10.0** adds async job builders and timezone-aware scheduling, letting us re-create the midnight/noon restart semantics without relying on external process managers.
- **`dom_smoothie` 0.13** mirrors Mozilla’s readability.js in pure Rust, so we get deterministic article extraction plus Markdown/HTML text modes without pulling in JS tooling; it handles URL normalization, scoring, and cleaning via the built-in `Config`/`Readability` APIs. citeturn0search0

## Next Implementation Steps
1. Wire the `queue` module into `telegram::on_plain_message`, persisting metadata (membership, URLs, priority) per incoming message.
2. Introduce an `analyzer` module that batches queued items, fetches web context via `web_content`, and invokes `ai::CerebrasClient`.
3. Expand admin command handlers (add/remove/list whitelist + `/sync_commands`) backed by `WhitelistRepository` and shared templates for Markdown logging.
4. Finish restart callbacks by gracefully draining the queue, closing the SQLite pool, and exiting the process so systemd/Docker can restart the bot.
