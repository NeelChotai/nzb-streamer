# NZB Streamer

Stream video files directly from Usenet NZBs without downloading the entire file first. Inspired by NZBDrive's virtual filesystem approach.

## Features

- 🎬 Stream video while downloading
- ⏩ Full seeking support  
- 📦 RAR transparency (no extraction needed)
- 🚀 Minimal bandwidth usage
- 🔧 Future Stremio addon support

## How It Works

1. Upload an NZB file or provide one via API
2. The system parses RAR headers to find video files
3. Video files become immediately streamable via HTTP
4. Only downloads the parts needed for playback
5. Intelligent prefetching for smooth streaming

## Requirements

- Rust 1.75+
- Usenet account with provider
- NZBs with video in uncompressed RARs (store mode)

## Quick Start

```bash
# Clone the repository
git clone https://github.com/yourusername/nzb-streamer
cd nzb-streamer

# Copy environment config
cp .env.example .env
# Edit .env with your Usenet credentials

# Build and run
cargo run

# Upload test NZB
curl -X POST http://localhost:3000/test/upload \
  -F "nzb=@movie.nzb"

# Response includes streaming URL
# Open in VLC: http://localhost:3000/stream/{session_id}/movie.mp4
```

## Development

See `CLAUDE.md` for architecture details and `PROGRESS.md` for current status.

## Implementation Status

Currently implementing Phase 1 (core streaming). Check `PROGRESS.md` for details.

## Contributing

1. Read `PROMPT` for implementation guidelines
2. Check `PROGRESS.md` for current tasks
3. Make small, focused commits
4. Update `PROGRESS.md` with your changes
5. Write tests for new functionality

## License

MIT
