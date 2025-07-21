# CLAUDE.md - NZB Streamer Project

## Project Overview
Build a Rust-based NZB streaming service that allows real-time streaming of video files from Usenet without full download, similar to NZBDrive's approach. The system will eventually integrate with Stremio as an addon.

## Core Architecture Principles
1. **Virtual filesystem approach** - Files appear available immediately, downloaded on-demand
2. **RAR transparency** - Video files inside RARs are accessible without extraction
3. **Range request support** - Enable seeking in video players
4. **Minimal download** - Only fetch segments actually needed for playback
5. **Store mode only** - Assume RARs use store mode (no compression) for video files

## Technical Stack
- **Language**: Rust
- **Web Framework**: Axum
- **Async Runtime**: Tokio
- **Key Libraries**: 
  - `quick-xml` for NZB parsing
  - `tokio-native-tls` for NNTP SSL
  - `bytes` for zero-copy operations
  - `lru` for segment caching

## Implementation Phases

### Phase 1: Core Streaming (CHECKPOINT)
**Goal**: Upload an NZB file and stream video from it via HTTP

1. **NZB Parser** - Parse NZB XML to extract file segments and metadata
2. **NNTP Client** - Connect to Usenet server, fetch articles, decode yEnc
3. **RAR Parser** - Parse RAR headers to locate video files without full extraction
4. **Virtual File System** - Map byte ranges to RAR segments to NNTP articles
5. **HTTP Range Server** - Serve video with proper range request support

**Checkpoint Success Criteria**:
- Can upload an NZB file via HTTP POST
- Receive a streaming URL in response
- Open URL in VLC/mpv and play video
- Seeking works properly

### Phase 2: Stremio Integration (POST-CHECKPOINT)
- Implement Stremio addon protocol
- Add NZB indexer search capabilities
- Map IMDB IDs to NZB searches

## Key Technical Challenges

### RAR Streaming Logic
```
HTTP Range Request (bytes 1000-2000 of video.mp4)
    ↓
Map to RAR archive (video.mp4 at offset 5000 in archive)  
    ↓
Calculate RAR parts needed (part2.rar, segments 10-15)
    ↓
Fetch NNTP articles (message IDs from NZB)
    ↓
Decode yEnc → Extract range → Return bytes
```

### Critical Implementation Details
1. **RAR Headers**: Must parse without downloading entire archive
2. **Caching**: LRU cache decoded articles to avoid refetching
3. **Prefetching**: Anticipate next ranges for smooth playback
4. **Connection Pooling**: Reuse NNTP connections (expensive to establish)
5. **Error Handling**: Gracefully handle missing/corrupt articles

## Git Workflow
- Main branch for stable code
- Individual commits for each component
- Update PROGRESS.md with each significant change
- Commit messages: `feat:`, `fix:`, `chore:`, `docs:` prefixes

## Testing Requirements
1. Unit tests for each module
2. Integration test with real NZB file
3. Performance test for streaming large files
4. Error handling test for incomplete NZBs

## Environment Variables
```
NNTP_HOST=news.usenetserver.com
NNTP_PORT=563
NNTP_USER=username
NNTP_PASS=password
```

## Success Metrics
- Can stream 1080p video smoothly
- Seeking latency < 2 seconds
- Memory usage stable during long streams
- Handles incomplete NZBs gracefully
