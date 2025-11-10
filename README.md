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

### Option 1: Download Pre-built Binary

You can download pre-built binaries from the [Releases](https://github.com/yldst-dev/fuckyou-spam-rs/releases) page. The following platforms are supported:

- **Linux**: x86_64 (GNU and MUSL), ARM64 (AArch64)
- **macOS**: x86_64 (Intel), Apple Silicon (M1/M2)
- **Windows**: x86_64

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
```

### 4. Run the Bot

#### Using Cargo
```bash
cargo run --release
```

#### Using Binary
```bash
# Linux/macOS
./fuckyou-spam-rust

# Windows
./fuckyou-spam-rust.exe
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
| `RESTART_SCHEDULE` | No | 0 2 * * * | Cron schedule for restarts |
| `TIMEZONE` | No | Asia/Seoul | Timezone for logging |

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

A `Dockerfile` is included in the repository for containerized deployment.

#### Using Pre-built Docker Image
```bash
docker run -d --name spam-bot \
  -v $(pwd)/data:/app/data \
  -v $(pwd)/logs:/app/logs \
  --env-file .env \
  yldstdev/fuckyou-spam-rust:latest
```

#### Build from Source
```bash
docker build -t fuckyou-spam-rust .
docker run -d --name spam-bot \
  -v $(pwd)/data:/app/data \
  -v $(pwd)/logs:/app/logs \
  --env-file .env \
  fuckyou-spam-rust
```

### GitHub Actions

The project includes comprehensive CI/CD pipelines:

#### CI Pipeline (`.github/workflows/ci.yml`)
- **Multi-version Testing**: Tests against stable, beta, and nightly Rust
- **Code Quality**: Rustfmt formatting check and Clippy linting
- **Security Audit**: Automated security vulnerability scanning
- **Code Coverage**: Integration with Codecov for coverage reporting
- **Caching**: Optimized build caching for faster CI

#### Release Pipeline (`.github/workflows/release.yml`)
- **Cross-platform Builds**: Supports Linux (x86_64, ARM64), macOS (Intel, Apple Silicon), and Windows
- **Binary Releases**: Automatic GitHub releases with pre-built binaries
- **Docker Images**: Multi-architecture Docker images (AMD64, ARM64)
- **Artifact Management**: Proper binary stripping and compression

#### Release Process
To create a new release:
1. Tag your commit with a version number: `git tag v1.0.0`
2. Push the tag: `git push origin v1.0.0`
3. GitHub Actions will automatically:
   - Build binaries for all platforms
   - Create a GitHub release
   - Build and push Docker images
   - Attach binaries to the release

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