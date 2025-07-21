# PROGRESS.md - NZB Streamer Project

## Project Status: Not Started

### Overview
Building an NZB streaming service that enables real-time video streaming from Usenet without full downloads. Using Rust with Axum framework.

### Phase 1: Core Streaming (Current Phase)

#### Components Status:
1. **NZB Parser** ✅
   - [x] Define data structures (Nzb, NzbFile, NzbSegment)
   - [x] XML parsing implementation with quick-xml
   - [x] Multi-file NZB support
   - [x] Path normalization and filename extraction
   - [x] Unit tests and integration tests

2. **NNTP Client** 🚧
   - [x] Basic data structures and types
   - [x] yEnc decoder with escape handling
   - [ ] TLS connection support
   - [ ] Authentication
   - [ ] Article fetching by message ID
   - [ ] Connection pooling
   - [ ] Error handling
   - [ ] Unit tests

3. **RAR Parser** 📋
   - [x] Basic placeholder structure
   - [ ] RAR header structures
   - [ ] Header parsing (store mode only)
   - [ ] Multi-volume support
   - [ ] File offset calculation
   - [ ] Unit tests

4. **Virtual File System** 📋
   - [x] Basic placeholder structure
   - [ ] Byte range to RAR segment mapping
   - [ ] RAR segment to NNTP article mapping
   - [ ] LRU cache implementation
   - [ ] Prefetching logic
   - [ ] Unit tests

5. **HTTP Server** 🚧
   - [x] Basic Axum server setup with upload endpoint
   - [x] NZB upload and parsing endpoint
   - [ ] Range request handling
   - [ ] Content-Range headers
   - [ ] Video streaming endpoints
   - [ ] Session management
   - [ ] Integration tests

### Phase 2: Stremio Integration (Future)
- Not started

### Current Status
**Status**: Phase 1 Core Components Started
**Next Step**: Implement NNTP connection logic and complete streaming pipeline

### Blockers
- None

### Important Decisions
- Using store mode RAR assumption (no compression for video files)
- Virtual filesystem approach over full download
- Axum chosen for web framework
- Phase 1 focuses on core streaming before Stremio integration

### Testing Checklist
- [ ] Unit tests for each module
- [ ] Integration test: Upload NZB → Get streaming URL
- [ ] End-to-end test: Stream video in VLC/mpv
- [ ] Seeking functionality test
- [ ] Error handling for missing articles

---

## Implementation Log

### 2024-12-19 - Foundation and NZB Parser Implementation

#### Completed
- [x] Project structure setup with proper module organization
- [x] Comprehensive error handling with `thiserror` and custom error types
- [x] Complete NZB parser implementation:
  - XML parsing with `quick-xml`
  - Support for multi-file NZBs
  - Proper filename extraction from subjects
  - Path normalization (backslash to forward slash)
  - Validation of parsed NZB structure
- [x] NZB data structures with helper methods:
  - File type detection (video/RAR files)
  - Segment validation and sorting
  - Size calculations
- [x] yEnc decoder with proper escape character handling
- [x] NNTP client foundation with data structures
- [x] Basic HTTP server with NZB upload endpoint
- [x] Unit tests for core NZB parsing functionality
- [x] Integration tests with sample NZB files

#### In Progress
- [ ] NNTP connection implementation with TLS support

#### Notes
- Used `quick-xml` for efficient streaming XML parsing
- yEnc decoder handles both normal and escaped characters correctly
- NZB parser extracts filenames from complex Usenet subject formats
- Error handling is comprehensive with specific error types for each component
- Project structure follows modular design for maintainability

#### Key Technical Decisions
- Chose `anyhow` for application errors and `thiserror` for library errors
- Used streaming XML parser to handle large NZB files efficiently
- Implemented filename extraction heuristics for various Usenet posting formats
- Created comprehensive error types for better debugging

#### Next Steps
1. Implement NNTP TLS connection and authentication
2. Complete article fetching and yEnc decoding pipeline
3. Implement RAR header parsing for store mode files
4. Create virtual filesystem mapping logic

#### Blockers
- None currently, development proceeding as planned
