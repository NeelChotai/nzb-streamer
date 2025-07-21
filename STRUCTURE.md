# Project Structure

```
nzb-streamer/
├── Cargo.toml              # Project dependencies
├── CLAUDE.md               # Project requirements (this file)
├── PROMPT                  # Instructions for implementers  
├── PROGRESS.md             # Implementation progress tracking
├── README.md               # User-facing documentation
├── .env.example            # Example environment variables
├── src/
│   ├── main.rs            # Application entry point & server setup
│   ├── lib.rs             # Library root
│   ├── error.rs           # Error types
│   ├── config.rs          # Configuration handling
│   ├── nzb/
│   │   ├── mod.rs         # Module exports
│   │   ├── parser.rs      # NZB XML parsing
│   │   └── types.rs       # NZB data structures
│   ├── nntp/
│   │   ├── mod.rs         # Module exports
│   │   ├── client.rs      # NNTP protocol client
│   │   ├── connection.rs  # Individual connection handling
│   │   ├── pool.rs        # Connection pooling
│   │   ├── yenc.rs        # yEnc encoding/decoding
│   │   └── types.rs       # NNTP data structures
│   ├── rar/
│   │   ├── mod.rs         # Module exports
│   │   ├── parser.rs      # RAR header parsing
│   │   ├── types.rs       # RAR data structures
│   │   └── stream.rs      # RAR streaming logic
│   ├── vfs/               # Virtual filesystem
│   │   ├── mod.rs         # Module exports
│   │   ├── mapper.rs      # Byte range mapping logic
│   │   ├── cache.rs       # LRU cache for articles
│   │   ├── prefetch.rs    # Prefetching strategy
│   │   └── session.rs     # Streaming session management
│   └── http/
│       ├── mod.rs         # Module exports
│       ├── server.rs      # Axum server setup
│       ├── routes.rs      # HTTP route handlers
│       ├── range.rs       # Range request parsing
│       └── response.rs    # Response builders
├── tests/
│   ├── fixtures/          # Test NZB/RAR files
│   │   ├── simple.nzb     # Single file test
│   │   ├── multi.nzb      # Multi-file test
│   │   └── README.md      # How to generate test files
│   ├── unit/              # Unit tests (if needed beyond src/)
│   └── integration/       # Integration tests
│       ├── nzb_parsing.rs
│       ├── nntp_client.rs
│       ├── rar_parsing.rs
│       └── streaming.rs
└── examples/
    ├── parse_nzb.rs       # Example: Parse and display NZB
    ├── test_nntp.rs       # Example: Test NNTP connection
    └── stream_file.rs     # Example: Stream a specific file
```

## Module Responsibilities

### `nzb/` - NZB File Handling
- Parse NZB XML files
- Extract segment information (message IDs, sizes)
- Handle multi-file NZBs

### `nntp/` - Usenet Server Communication  
- Establish secure connections
- Authenticate with server
- Fetch articles by message ID
- Decode yEnc data
- Manage connection pool

### `rar/` - RAR Archive Handling
- Parse RAR headers without full download
- Map files to positions in archive
- Handle multi-volume archives
- Only support store mode (0x30)

### `vfs/` - Virtual Filesystem Layer
- Map video byte ranges → RAR segments → NNTP articles
- Cache decoded articles
- Implement prefetching
- Manage streaming sessions

### `http/` - Web Server
- Handle file uploads (testing)
- Serve streaming endpoints
- Process range requests
- Return proper HTTP 206 responses

## Key Files to Start With

1. **`src/error.rs`** - Define error types used throughout
2. **`src/nzb/types.rs`** - Core data structures
3. **`src/nzb/parser.rs`** - First implementation task
4. **`Cargo.toml`** - Project dependencies
