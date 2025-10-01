# NZB Streamer

Stream video files directly from Usenet NZBs without downloading the entire file first.

This is still VERY much a work in progress. Prototype is working and can stream
video, but there's a decent chunk of refactoring to do.

## Features

- 🎬 Stream video while downloading
- ⏩ Full seeking support  
- 📦 RAR transparency (no extraction needed)
- 🚀 Minimal bandwidth usage
- 🔧 Future Stremio addon support

## How It Works

1. Upload an NZB file or provide one via API
1. The system parses par2 file to deobfuscate file names
1. Download the first segment, background download the rest
1. Video files become immediately streamable via HTTP
1. Only downloads the parts needed for playback
1. Intelligent prefetching for smooth streaming

## Requirements

- Rust 1.75+
- Usenet account with provider
- NZBs with video in uncompressed RARs (store mode)

## TODO

- migrate fully over to sparse files
- tweak batch generator, iterated over the architecture a few times but batching
  still seems like the right choice despite minor worker waste
- CI failing because upstream fix for `rek2_nntp` hasn't been accepted yet,
  switch over to fork?
