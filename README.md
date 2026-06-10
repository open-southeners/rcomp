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
      --checksum         Write a sha256sum-format sidecar <OUTPUT>.sha256
                         and show the digest in the summary (compress only;
                         on extraction a sidecar is auto-verified when found)
      --all              Ignore .gitignore rules: include every file in the
                         input folder, .git included (compress only)
      --exclude <GLOB>   Exclude paths matching a gitignore-style glob,
                         relative to the input folder; repeatable; works
                         with or without --all (compress only)
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

### Checksums

Pass `--checksum` when compressing to compute a SHA-256 digest of the output
and write a sidecar file next to it:

```
rcomp folder/ archive.tar.gz --checksum
```

The sidecar is named `<output>.sha256` (e.g. `archive.tar.gz.sha256`).  Its
format is compatible with `sha256sum -c`:

```
# content-sha256: 3b4c2a1d...e8f9        ← only for codec/tar outputs
a1b2c3d4...f0  archive.tar.gz
```

The `# content-sha256:` comment line is present for formats that have a single
pre-compression byte stream (codec-only, tar, and tar+codec combinations).  It
is absent for zip and 7z, which compress entries individually.

**Artifact digest** — SHA-256 of the compressed file written to disk (transport
integrity).

**Content digest** — SHA-256 of the pre-compression stream:

- codec-only, file input: digest of the raw input file bytes
- tar + codec: digest of the uncompressed tar byte stream
- plain tar: same bytes as the artifact digest (both are always equal)
- zip / 7z: not produced (`None`)

The content digest is brotli's only integrity check — see [Limitations](#limitations).

The artifact digest is also printed to stdout after the summary line:

```
archive.tar.gz  1.2 MiB → 380 KiB (31.7%)  in 0.4s
sha256: a1b2c3d4...f0
```

`-q` suppresses all stdout output but still writes the sidecar file.

**Verifying manually:**

```
sha256sum -c archive.tar.gz.sha256
```

`sha256sum` verifies the artifact line and ignores the `#` comment.

**Auto-verify on extraction:**

When extracting an archive that has a sibling `<archive>.sha256` file, rcomp
reads and verifies both digests automatically — no flag is needed:

```
rcomp archive.tar.gz                  # sidecar found → verifies before unpacking
```

The artifact digest is checked before any files are written to the destination.
The content digest is checked during extraction as the stream is read.  A
mismatch on either causes an error (exit 1) with a clear message; no output
files are left behind when the artifact check fails.

If the sidecar is present but cannot be parsed, extraction fails (exit 1) —
a present sidecar is treated as a promise.  If no sidecar exists, extraction
proceeds exactly as before with no verification.

`--checksum` on an extract operation is a usage error (exit 2).

### .gitignore awareness

When compressing a folder, rcomp follows `.gitignore` rules by default.  Full
git semantics are applied: the origin folder's own `.gitignore` and any nested
`.gitignore` files in subdirectories are honoured.  The `.git` directory itself
is always excluded.  Hidden dotfiles (e.g. `.editorconfig`, `.gitignore`) are
included — only ignore rules and `.git` exclude things.

A folder with no `.gitignore` anywhere is archived identically to before.

**`--all`** disables all `.gitignore` filtering.  Every file is included,
including the `.git` directory:

```
rcomp folder/ archive.tar.gz --all
```

**`--exclude <GLOB>`** excludes paths matching a gitignore-style glob, matched
relative to the input folder.  The flag may be repeated and works with or
without `--all`:

```
rcomp folder/ archive.tar.gz --exclude 'target/' --exclude '*.log'
rcomp folder/ archive.tar.gz --all --exclude 'target/'
```

An invalid glob is a usage error (exit 2).

When any paths are excluded, rcomp prints a note to stderr naming which sources
were active:

```
excluded 47 paths via .gitignore (use --all to include)
excluded 12 paths via --exclude
excluded 59 paths via .gitignore and --exclude
```

This note goes to stderr and is suppressed by `-q`, like other non-error output.

**Reproducibility:** rcomp deliberately does not consult parent-directory
`.gitignore` files, the global gitignore (`~/.config/git/ignore`), or
`.git/info/exclude`.  Archives are reproducible from the folder alone,
independent of machine-local git configuration.

`--all` and `--exclude` on an extract operation are usage errors (exit 2).

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

- **Brotli: no magic bytes, no internal checksum.** Brotli-compressed files
  cannot be detected by content alone — an extensionless `.br` file requires
  `--algo brotli`.  Brotli has no framing checksum, so a corrupted stream can
  decode "successfully" into wrong bytes rather than returning an error.  Use
  `--checksum` when compressing brotli outputs; the content digest in the
  resulting sidecar is the only integrity protection available for `.br` files.

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
