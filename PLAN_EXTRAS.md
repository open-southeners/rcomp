# rcomp — Plan extras

Open questions, suggestions, and out-of-scope ideas. Nothing here is in the
plan until you answer a question or accept a suggestion; it then graduates
into `PLAN.md`.

## Open questions

None currently.

## Out-of-scope ideas (V2+)

- The Tauri desktop app itself (V1's `Progress` callback + `CancelToken` are
  designed to be forwarded as Tauri events from a worker thread).
- Async/`Stream` wrapper crate (`rcomp-async`) if Tauri integration wants it.
- Smarter codec discovery for mystery files (beyond magic bytes + `--algo`),
  e.g. content heuristics — revisit if users hit the `--algo` fallback often.
- stdin/stdout streaming mode (`cat x | rcomp - out.zst`), pipe-friendliness.
- Multiple inputs / batch mode (`rcomp a b c out.zip`), glob support.
- Password-protected archives (7z and zip encryption; rar decryption).
- Custom parallel codec implementations (pigz-style chunked gzip) — distinct
  from the codec-native multithreading already in V1.
- Compression-ratio pre-check / "already compressed" warning for media files.
- Partial extraction (extract a single entry by name/glob).
- Preserving extended attributes, ACLs, and high-fidelity permissions.
- Benchmarks (`criterion`) comparing codecs at each unified level.
- Distribution: cargo-binstall metadata, Homebrew tap, prebuilt release
  binaries.
- Self-test command (`rcomp verify <archive>`) for integrity checking.

## Resolved (graduated or rejected)

- Formats, silent-tar rule, inference + flags, sync core — locked 2026-06-09.
- `rcomp ls`, shell completions + man page, GitHub Actions CI, `--unwrap`
  wrapping behavior, `-y` confirmations, lz4 C bindings, `--edge` =
  max-ratio-features, gzip-header naming for mystery files — graduated into
  `PLAN.md` 2026-06-09.
- `--smart-name` auto-rename — **rejected**: keep the user's name, warn +
  confirm instead.
- License: dual MIT OR Apache-2.0; publishing: release-driven via CI, no
  placeholder reservation — graduated into `PLAN.md` 2026-06-09.
