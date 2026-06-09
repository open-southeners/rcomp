# Test Fixtures

## sample.rar

**Fixture tier:** (b) — copied from the `unrar` crate's bundled test data.
`rar` was not found on PATH, so the archive was not generated locally.

**Origin:**
`~/.cargo/registry/src/.../unrar-0.5.8/data/version.rar`

**Archive contents:**

| Path    | Size (uncompressed) | Type |
|---------|--------------------:|------|
| VERSION | 11 bytes            | file |

The `VERSION` file contains the UTF-8 string `unrar-0.4.0` (no trailing
newline).  This was verified by running `rar::list()` against the file:

```
entries[0] = Entry { path: "VERSION", size: 11, is_dir: false }
```

**Format:** RAR4 (unrar 0.5.8 can read both RAR4 and RAR5; this archive is RAR4)

**Encoding:** no password, no encryption, no multi-volume splitting, no solid archive flag.
