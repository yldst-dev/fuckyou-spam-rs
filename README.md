# fuckyou-spam-rust

A high-performance Telegram spam detection bot written in Rust, powered by AI to automatically identify and remove spam messages from Telegram groups. This is a Rust rewrite of the original TypeScript implementation with improved performance, type safety, and reliability.

## 🚀 Features

### Core Functionality
- **AI-Powered Spam Detection**: Uses Cerebras AI (GPT-oss-120b) for intelligent spam classification
- **Priority Queue System**: Processes messages based on priority (non-members get higher priority)
- **Web Content Analysis**: Fetches and analyzes web page content using Mozilla Readability
- **SQLite Whitelist Management**: Persistent whitelist storage with SQLite database
- **Real-time Monitoring**: Comprehensive logging with Korean timezone support

### Advanced Features
- **Graceful Shutdown**: Clean termination with proper resource cleanup
- **Batch Processing**: Efficient message processing to optimize API calls
- **Automatic Restarts**: Configurable cron-based restarts for reliability
- **Admin Commands**: Full-featured admin interface for whitelist management
- **Error Recovery**: Robust error handling with automatic retries

## 📋 Requirements

- Rust 1.70+
- SQLite 3
- Telegram Bot Token
- Cerebras API key

## 🛠️ Installation

### Option 1: Docker Compose (recommended)

#### 1. Clone the Repository
```bash
git clone https://github.com/yldst-dev/fuckyou-spam-rs.git
cd fuckyou-spam-rs
```

#### 2. Configure Environment Variables
Copy `.env.example` to `.env` and configure:

```env
# Required
TELEGRAM_BOT_TOKEN=your_bot_token_here
CEREBRAS_API_KEY=your_cerebras_api_key
BOT_USERNAME=your_bot_username

# Optional
ADMIN_USER_ID=your_admin_user_id
ADMIN_GROUP_ID=your_admin_group_id
CEREBRAS_MODEL=gpt-oss-120b
LOG_LEVEL=info
WEBPAGE_FETCH_TIMEOUT=10000
MAX_URLS_PER_MESSAGE=2
RESTART_SCHEDULE=0 2 * * *
TIMEZONE=Asia/Seoul

# Auto-update (disabled by default)
AUTO_UPDATE_ENABLED=false
AUTO_UPDATE_CHECK_ON_STARTUP=true
AUTO_UPDATE_AUTO_RESTART=true
AUTO_UPDATE_REPO_OWNER=yldst-dev
AUTO_UPDATE_REPO_NAME=fuckyou-spam-rs
AUTO_UPDATE_ALLOWED_REPO_OWNERS=yldst-dev
AUTO_UPDATE_ALLOWED_REPO_NAMES=fuckyou-spam-rs
AUTO_UPDATE_ASSET_HOST_ALLOWLIST=github.com,objects.githubusercontent.com,github-releases.githubusercontent.com
AUTO_UPDATE_MAX_DOWNLOAD_BYTES=52428800
AUTO_UPDATE_ASSET_SHA256=
```

#### 3. Build and Run
```bash
docker compose up -d --build
```

#### 4. View Logs / Stop
```bash
docker compose logs -f
docker compose down
```

Persistent data (`whitelist.db`, spam cache, runtime lock) and log files live in the named
volumes `bot-data` and `bot-logs`. They survive `docker compose down` and are only removed with
`docker compose down -v`. The container runs read-only, as a non-root user, with all Linux
capabilities dropped and `no-new-privileges`; the named volumes are initialized with the
container user's ownership on first start, so no host-side permission setup is required.

### Option 1b: Dokploy (Compose)

The bot runs as a **Compose** service in [Dokploy](https://dokploy.com/). No code changes are
needed — `compose.yaml` is already Dokploy-compatible (named volumes, no host bind mounts, no
`HOST_UID` handling).

1. **Create service**: In your Dokploy project, add a new service of type **Compose** and point
   it at this Git repository. Set the Compose Path to `compose.yaml`.
2. **Environment**: Open the service **Environment** tab and paste the contents of `.env.example`,
   filling in the real `TELEGRAM_BOT_TOKEN`, `CEREBRAS_API_KEY`, `BOT_USERNAME`, and admin IDs.
   Dokploy writes these to the `.env` file that `compose.yaml` reads via `env_file`.
3. **Deploy**: Click **Deploy**. Dokploy builds the image from the `Dockerfile` and starts the
   `spam-bot` service. The `bot-data` / `bot-logs` volumes persist across redeploys.
4. **Updates**: Push to the tracked branch and redeploy from Dokploy. Keep
   `AUTO_UPDATE_ENABLED=false` — in-container self-updates are blocked by the read-only root
   filesystem, and updates should go through a rebuild/redeploy instead.

Notes:
- There is no exposed port. This is a Telegram long-polling bot, so no domain/Traefik route is
  required. Leave the ports/domains section empty.
- Application logs are visible in Dokploy; the app also writes rotating files into the `bot-logs`
  volume.

### Option 2: Build from Source

#### 1. Clone the Repository
```bash
git clone https://github.com/yldst-dev/fuckyou-spam-rs.git
cd fuckyou-spam-rs
```

#### 2. Install Dependencies
```bash
cargo build --release
```

### 3. Configure Environment Variables
Copy `.env.example` to `.env` and configure (same as Docker Compose).

> **Auto Update**
> When `AUTO_UPDATE_ENABLED=true`, the bot checks the latest GitHub release for the configured
> repository on startup (Linux/macOS targets). If a newer binary exists, it is downloaded into
> `data/updates/`, swapped into the current install directory, and optionally re-exec'd immediately
> when `AUTO_UPDATE_AUTO_RESTART=true`. If `ADMIN_GROUP_ID` is set, the bot sends a status message
> (old/new versions + restart plan) to that group after the binary swap. Disable the flag when
> running from `cargo run` or inside container builds where self-updates are undesired. The updater
> only accepts configured repository owner/name values that also appear in the allowlists, only
> downloads release assets from exact HTTPS hosts in `AUTO_UPDATE_ASSET_HOST_ALLOWLIST`, rejects
> assets larger than `AUTO_UPDATE_MAX_DOWNLOAD_BYTES`, and verifies SHA-256 before unpacking. Set
> `AUTO_UPDATE_ASSET_SHA256` to pin a specific asset hash, or leave it empty and publish a
> `<asset>.sha256` release asset next to each binary archive.

### 4. Run the Bot

#### Using Cargo
```bash
cargo run --release
```

## 📖 Usage

### Commands

#### General Commands
- `/start` - Start the bot and see welcome message
- `/help` - View all available commands
- `/status` - Check bot status and queue information
- `/chatid` - Get current chat/group ID
- `/ping` - Test bot response time

#### Admin Commands
- `/whitelist_add` - Add current chat to whitelist
- `/whitelist_remove` - Remove current chat from whitelist
- `/whitelist_list` - List all whitelisted chats
- `/sync_commands` - Update bot commands in Telegram

### How It Works

1. **Message Reception**: Bot receives messages from Telegram
2. **Whitelist Check**: Verifies if the chat is whitelisted
3. **Priority Assignment**:
   - High priority: Non-members, messages with URLs
   - Normal priority: Regular members
4. **Batch Processing**: Processes messages in batches for efficiency
5. **AI Analysis**: Sends messages to Cerebras AI for spam detection
6. **Action Taken**: Deletes spam messages and notifies admins

## 🏗️ Architecture

### Project Structure
```
src/
├── main.rs              # Application entry point
├── app.rs               # Core application logic
├── config/              # Configuration management
│   ├── env.rs          # Environment variables
│   ├── loader.rs       # Configuration loader
│   └── mod.rs
├── ai/                  # AI integration
│   ├── client.rs       # Cerebras API client
│   ├── inference.rs    # Spam inference logic
│   └── mod.rs
├── telegram/            # Telegram bot integration
│   ├── handler.rs      # Message and command handlers
│   ├── types.rs        # Telegram-specific types
│   ├── utils.rs        # Utility functions
│   └── mod.rs
├── db/                  # Database layer
│   ├── whitelist.rs    # Whitelist operations
│   ├── mod.rs
├── tasks/               # Message processing
│   ├── processor.rs    # Message processor
│   ├── queue.rs        # Priority queue
│   ├── scheduler.rs    # Cron scheduler
│   └── mod.rs
├── infrastructure/       # Core infrastructure
│   ├── directories.rs  # Directory management
│   ├── logging.rs      # Logging setup
│   └── shutdown.rs     # Graceful shutdown
├── web_content/         # Web content analysis
│   └── fetcher.rs      # URL content fetcher
└── domain/              # Domain models
    └── mod.rs
```

### Key Components

#### 1. Configuration System
- Environment-based configuration using `dotenvy`
- Structured types with validation
- Support for all bot settings

#### 2. AI Integration
- HTTP client wrapper for Cerebras API
- Structured JSON responses
- Configurable model selection
- Comprehensive error handling

#### 3. Database Layer
- SQLite with `sqlx` for async operations
- Automatic migrations
- Connection pooling
- Type-safe queries

#### 4. Message Processing
- Priority-based queue system
- Batch processing for efficiency
- Thread-safe operations
- Graceful shutdown support

## 🔧 Configuration

### Environment Variables

| Variable | Required | Default | Description |
|----------|----------|---------|-------------|
| `TELEGRAM_BOT_TOKEN` | Yes | - | Bot token from @BotFather |
| `CEREBRAS_API_KEY` | Yes | - | API key for Cerebras AI |
| `BOT_USERNAME` | Yes | - | Bot's username (without @) |
| `ADMIN_USER_ID` | No | - | Admin user ID for management |
| `ADMIN_GROUP_ID` | No | - | Admin group ID for notifications |
| `CEREBRAS_MODEL` | No | gpt-oss-120b | AI model to use |
| `LOG_LEVEL` | No | info | Logging level (trace, debug, info, warn, error) |
| `WEBPAGE_FETCH_TIMEOUT` | No | 10000 | Timeout for URL analysis (ms) |
| `MAX_URLS_PER_MESSAGE` | No | 2 | Max URLs to analyze per message |
| `WEBPAGE_RESPONSE_MAX_BYTES` | No | 1048576 | Maximum external webpage response bytes to read |
| `QUEUE_MAX_MESSAGES` | No | 5000 | Maximum queued messages across priorities |
| `QUEUE_HIGH_PRIORITY_MAX` | No | 1000 | Maximum high-priority queued messages |
| `QUEUE_NORMAL_PRIORITY_MAX` | No | 4000 | Maximum normal-priority queued messages |
| `SPAM_CACHE_SIMILARITY_THRESHOLD` | No | 0.92 | Similarity threshold for deleting previously seen spam without LLM |
| `SPAM_CACHE_SCAN_LIMIT` | No | 1000 | Recent spam cache rows to compare for similarity |
| `SPAM_CACHE_MIN_NORMALIZED_CHARS` | No | 8 | Minimum normalized message length for spam cache matching |
| `RESTART_SCHEDULE` | No | 0 2 * * * | Cron schedule for restarts |
| `TIMEZONE` | No | Asia/Seoul | Timezone for logging |
| `AUTO_UPDATE_ENABLED` | No | false | Enable startup auto-update |
| `AUTO_UPDATE_CHECK_ON_STARTUP` | No | true | Check latest release on process startup |
| `AUTO_UPDATE_AUTO_RESTART` | No | true | Re-exec after installing a new binary |
| `AUTO_UPDATE_REPO_OWNER` | No | yldst-dev | GitHub release repository owner |
| `AUTO_UPDATE_REPO_NAME` | No | fuckyou-spam-rs | GitHub release repository name |
| `AUTO_UPDATE_ALLOWED_REPO_OWNERS` | No | yldst-dev | Comma-separated allowed release repository owners |
| `AUTO_UPDATE_ALLOWED_REPO_NAMES` | No | fuckyou-spam-rs | Comma-separated allowed release repository names |
| `AUTO_UPDATE_ASSET_HOST_ALLOWLIST` | No | github.com,objects.githubusercontent.com,github-releases.githubusercontent.com | Comma-separated exact HTTPS hosts allowed for release asset downloads |
| `AUTO_UPDATE_MAX_DOWNLOAD_BYTES` | No | 52428800 | Maximum release asset download size in bytes |
| `AUTO_UPDATE_ASSET_SHA256` | No | - | Optional pinned SHA-256 for the selected asset; otherwise `<asset>.sha256` release asset is required |

### Database Schema

```sql
CREATE TABLE whitelist (
  chat_id INTEGER PRIMARY KEY,
  chat_title TEXT,
  chat_type TEXT,
  added_at DATETIME DEFAULT CURRENT_TIMESTAMP,
  added_by INTEGER
);
```

## 📊 Logging

The bot provides comprehensive logging with multiple levels:

- **Combined Logs**: `logs/combined.log` - All activity
- **Error Logs**: `logs/error.log` - Error-only messages
- **Spam Actions**: `logs/spam-actions.log` - Spam deletion records

All logs are formatted in Korean timezone (Asia/Seoul) by default.

## 🚀 Deployment

### Running as a Service (systemd)

Create a service file at `/etc/systemd/system/fuckyou-spam.service`:

```ini
[Unit]
Description=FuckYou Spam Rust Bot
After=network.target

[Service]
Type=simple
User=botuser
WorkingDirectory=/path/to/fuckyou-spam-rust
Environment="RUST_LOG=info"
ExecStart=/path/to/fuckyou-spam-rust/target/release/fuckyou-spam-rust
Restart=on-failure
RestartSec=5

[Install]
WantedBy=multi-user.target
```

Enable and start the service:
```bash
sudo systemctl enable fuckyou-spam
sudo systemctl start fuckyou-spam
```

### Docker Deployment

A `Dockerfile` is included for containerized builds, but the recommended path is Docker Compose
for local and server deployments.

## 🤝 Contributing

1. Fork the repository
2. Create a feature branch (`git checkout -b feature/amazing-feature`)
3. Commit your changes (`git commit -m 'Add some amazing feature'`)
4. Push to the branch (`git push origin feature/amazing-feature`)
5. Open a Pull Request

## 📝 Development

### Running in Development Mode

```bash
# Install development dependencies
cargo install cargo-watch

# Run with auto-reload
cargo watch -x run

# Run tests
cargo test

# Check code formatting
cargo fmt --check

# Run clippy for linting
cargo clippy -- -D warnings
```

### Project Dependencies

Key dependencies include:
- `teloxide` - Telegram bot framework
- `sqlx` - Async SQL toolkit
- `reqwest` - HTTP client
- `tokio` - Async runtime
- `serde` - Serialization
- `tracing` - Logging framework
- `chrono` - Date/time handling

## 📄 License

This project is licensed under the MIT License - see the [LICENSE](LICENSE) file for details.

## 🙏 Acknowledgments

- [Cerebras AI](https://cerebras.ai/) - For providing the AI API
- [Teloxide](https://github.com/teloxide/teloxide) - Excellent Telegram bot framework for Rust
- [SQLx](https://github.com/launchbadge/sqlx) - Async SQL toolkit
- Original [TypeScript implementation](https://github.com/yldst-dev/fuckyou-spam) - The foundation and inspiration

## 📋 TODO

- [ ] 해당 메시지가 차단된 이유(reason)가 관리자 그룹에 표시되도록 하는 기능 추가

## 📞 Support

If you encounter any issues or have questions:
1. Check the [Issues](https://github.com/yldst-dev/fuckyou-spam-rs/issues) page
2. Create a new issue with detailed information
3. Join the Telegram group for community support

## ⭐ Star History

[![Star History Chart](https://api.star-history.com/svg?repos=yldst-dev/fuckyou-spam-rs&type=Date)](https://star-history.com/#yldst-dev/fuckyou-spam-rs&Date)

---

Made with ❤️ and Rust 🦀
