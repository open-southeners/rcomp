# rcomp

One tool for the most popular compression and archive formats.

**`rcomp-core`** is a reusable Rust library (the product). **`rcomp`** is its
first consumer: a command-line tool. A Tauri desktop app is the planned second
consumer.

## Install

```
cargo install rcomp
```

Note: not yet published to crates.io. The first release will publish both
crates automatically.

## Usage

```
rcomp /path/to/folder compressed.bz2     # picks bzip2 from the extension, tars the folder transparently
rcomp big.iso big.iso.zst --edge         # max zstd compression
rcomp archive.7z                         # detects 7z by magic bytes, extracts to cwd
rcomp archive.7z ~/restored              # ...or into an explicit destination folder
rcomp ls archive.zip                     # list entries without extracting
rcomp weird-file -a bzip2                # force an algorithm explicitly
```

### Subcommands

```
rcomp <INPUT> [OUTPUT] [OPTIONS]    # compress or extract (inferred from arguments)
rcomp ls <ARCHIVE>                  # list entries without extracting
rcomp completions <SHELL>           # print shell completion script to stdout
rcomp man                           # print a troff man page to stdout
```

### Options

```
  -a, --algo <ALGO>      Force algorithm/format (bzip2, zstd, 7z, tar.xz, ...)
      --fast             Fastest compression
      --best             Balanced (default)
      --edge             Maximum compression ratio, hardware expensive
  -c, --compress         Force compress mode (for re-compressing a .gz, etc.)
  -x, --extract          Force extract mode
      --unwrap           Extract entries directly into the destination
                         (skip the auto-wrap folder)
  -y, --yes              Auto-accept all confirmation prompts
  -f, --force            Overwrite existing output
  -q, --quiet            No progress output
```

### Inference rules

Compress vs. extract is inferred from the arguments in this order:

1. `--compress` or `--extract` given — obey.
2. `OUTPUT` given with a recognizable compression/archive extension — compress.
3. `INPUT` is a readable file recognized as compressed/archive via magic bytes —
   extract. `OUTPUT`, when given, is the destination directory (created if
   missing).
4. Otherwise — error listing both interpretations and the flag to pick one.

### Silent-tar folder rule

When the output format is a codec (e.g. `.bz2`) and the input is a folder,
rcomp automatically wraps the folder in a tar stream first. The output file
keeps the exact name you gave (`out.bz2` contains tar-then-bzip2 data).
rcomp warns you about this and asks for confirmation before proceeding.
Pass `-y` to auto-accept.

### Extraction auto-wrap

When an archive contains multiple loose top-level entries, rcomp wraps them in
a new folder named after the archive stem (`photos.tar.gz` extracts into
`./photos/`). A single top-level folder, a single file, or a bare codec stream
extracts directly without extra nesting. `--unwrap` forces direct extraction
regardless.

### Exit codes

| Code | Meaning |
|------|---------|
| 0 | Success |
| 1 | Operation error (I/O, already-exists, unsupported format, cancelled, ...) |
| 2 | Usage/ambiguity error (bad `--algo` value, inference ambiguity) |

Ctrl-C cancels cleanly: partial output is removed and rcomp exits 1.

## Supported formats

### Codecs (single-stream)

| Codec | Extension | Notes |
|-------|-----------|-------|
| gzip | `.gz` | |
| bzip2 | `.bz2` | |
| xz | `.xz` | |
| zstd | `.zst` | |
| lz4 | `.lz4` | |
| brotli | `.br` | No magic bytes — see Limitations |

### Archives

| Format | Extension | Notes |
|--------|-----------|-------|
| tar | `.tar`, `.tar.*`, `.tgz`, `.tbz2`, `.txz`, ... | |
| zip | `.zip` | |
| 7z | `.7z` | |
| rar | `.rar` | Extract-only — see License note |

tar can be combined with any codec: `.tar.gz`, `.tar.bz2`, `.tar.xz`,
`.tar.zst`, `.tar.lz4`, `.tar.br`.

## Compression levels

The three levels describe the **output ratio**, never the hardware cost.
`--edge` turns on every ratio-improving feature a codec offers.
Codec-native multithreading (zstd workers, xz threads) is used at every
level because it does not change the compressed result.

| Codec | `--fast` | `--best` (default) | `--edge` |
|-------|----------|--------------------|----------|
| gzip | 1 | 6 | 9 |
| bzip2 | 1 | 6 | 9 |
| xz | 1 | 6 | 9 + extreme |
| zstd | 1 | 3 | 22 + long-distance matching |
| brotli | 2 | 6 | 11 + large window |
| lz4 | 1 | 6 | 12 (HC) |
| zip (deflate) | 1 | 6 | 9 |
| 7z (LZMA2) | 1 | 5 | 9 |

## Limitations

- **7z: no unix permissions or symlinks.** The `sevenz-rust2` crate does not
  expose unix mode bits or symlink entries. Executable scripts come back as
  `0644`, and symlinks are stored as regular files (dereferenced). This is an
  upstream crate limitation.

- **Brotli: no magic bytes, no content checksum.** Brotli-compressed files
  cannot be detected by content alone — an extensionless `.br` file requires
  `--algo brotli`. Additionally, brotli has no framing checksum, so a
  corrupted stream can decode "successfully" into wrong bytes rather than
  returning an error.

- **Zip extraction progress has no percentage.** The zip format is
  random-access, so rcomp reports entry count and bytes written rather than
  a percentage of the input file consumed.

- **Ctrl-C and multithreaded xz.** Ctrl-C cancels cleanly for all formats,
  but with the multithreaded xz encoder the interrupt may only take effect
  once encoding finishes its queued blocks (input is already queued to worker
  threads before the cancel signal is checked).

## rar license note

The default `rar` cargo feature links the freeware `unrar` library, which is
**not OSI-approved**. If your project requires only OSI-approved dependencies,
build without it:

```
cargo install rcomp --no-default-features
```

RAR extraction will not be available in that build. All other formats are
unaffected.

## License

Licensed under either of:

- [MIT License](LICENSE-MIT)
- [Apache License, Version 2.0](LICENSE-APACHE)

at your option.

Unless you explicitly state otherwise, any contribution intentionally submitted
for inclusion in this project by you shall be dual-licensed as above, without
any additional terms or conditions.
