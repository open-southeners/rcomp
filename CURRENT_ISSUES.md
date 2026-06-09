# Current issues

Discovered gaps, bugs, and spec questions routed here during orchestrated
implementation. Each entry: **Where**, **What**, suggested **Fix**.

## Open

- **Where:** `crates/rcomp-core/src/format.rs` (`Codec::short_ext`)
  **What:** The codec→short-extension mapping (`gz`, `bz2`, …) is `pub(crate)`.
  The CLI (milestone 5) will likely need it for output-name derivation when
  stripping/choosing extensions (e.g. bare-codec extraction naming, `--algo`
  handling), and the Tauri app may too.
  **Fix:** When the first external consumer appears, promote it to `pub` (or
  expose an equivalent method on `Format`) instead of duplicating the table.
