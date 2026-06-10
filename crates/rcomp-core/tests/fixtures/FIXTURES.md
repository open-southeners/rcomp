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

---

## Tier (a) codec fixtures — `sample.txt.{gz,bz2,xz,zst}`

All four fixtures compress the same known text content.  The test file
`tests/interop.rs` embeds the exact bytes as the constant `SAMPLE_TXT`.

**Input (`sample.txt`, 137 bytes):**

```
rcomp interop test fixture
line 1: hello from the reference tool
line 2: deterministic content for byte comparison
line 3: end of sample
```

(trailing newline; UTF-8, LF line endings)

**Tool versions:**

```
gzip 1.12                         (GNU gzip)
bzip2, Version 1.0.8, 13-Jul-2019
xz (XZ Utils) 5.4.5
Zstandard CLI v1.5.5, by Yann Collet
```

---

### sample.txt.gz

**Fixture tier:** (a) — generated locally by `gzip` (GNU gzip 1.12).

**Creation command:**

```sh
gzip -9 --keep --name -c /tmp/sample.txt > tests/fixtures/sample.txt.gz
```

The `--name` flag (on by default when compressing a named file) stores the
original filename `sample.txt` in the gzip FNAME header field.  The interop
test verifies that rcomp honours this embedded name when extracting an
otherwise extension-less copy of the file.

**Format details:**

| Field                | Value             |
|----------------------|-------------------|
| Magic bytes          | `1F 8B`           |
| Flags                | `0x08` (FNAME set) |
| Embedded filename    | `sample.txt`      |
| Compression level    | 9 (maximum)       |
| Compressed size      | 131 bytes         |

---

### sample.txt.bz2

**Fixture tier:** (a) — generated locally by `bzip2` (version 1.0.8).

**Creation command:**

```sh
bzip2 -9 --keep -c /tmp/sample.txt > tests/fixtures/sample.txt.bz2
```

| Field             | Value         |
|-------------------|---------------|
| Magic bytes       | `42 5A 68`    |
| Compressed size   | 131 bytes     |

---

### sample.txt.xz

**Fixture tier:** (a) — generated locally by `xz` (XZ Utils 5.4.5).

**Creation command:**

```sh
xz -9 --keep -c /tmp/sample.txt > tests/fixtures/sample.txt.xz
```

| Field             | Value                           |
|-------------------|---------------------------------|
| Magic bytes       | `FD 37 7A 58 5A 00`             |
| Compressed size   | 184 bytes                       |

---

### sample.txt.zst

**Fixture tier:** (a) — generated locally by `zstd` (CLI v1.5.5).

**Creation command:**

```sh
zstd --ultra -22 --keep -c /tmp/sample.txt \
  -o tests/fixtures/sample.txt.zst
```

| Field             | Value                   |
|-------------------|-------------------------|
| Magic bytes       | `28 B5 2F FD`           |
| Compressed size   | 111 bytes               |

---

## Tier (a) archive fixtures — `sample.tar`, `sample.tar.gz`, `sample.zip`

All three archives contain the same two-level directory tree:

```
sample_dir/
  sample.txt          (137 bytes — same SAMPLE_TXT content as above)
  sub/
    nested.txt        (20 bytes — "nested file content\n")
```

**Tool versions:**

```
tar (GNU tar) 1.35
zip 3.0 (Info-ZIP, July 5th 2008)
```

---

### sample.tar

**Fixture tier:** (a) — generated locally by GNU tar 1.35.

**Creation commands:**

```sh
mkdir -p /tmp/tarwork/sample_dir/sub
printf '%s\n' \
  "rcomp interop test fixture" \
  "line 1: hello from the reference tool" \
  "line 2: deterministic content for byte comparison" \
  "line 3: end of sample" \
  > /tmp/tarwork/sample_dir/sample.txt
printf 'nested file content\n' > /tmp/tarwork/sample_dir/sub/nested.txt
(cd /tmp/tarwork && tar --format=gnu -cf tests/fixtures/sample.tar sample_dir/)
```

| Field             | Value          |
|-------------------|----------------|
| Format            | GNU tar        |
| Detection         | `ustar` at byte offset 257 in the first 512-byte block |
| Uncompressed size | 10240 bytes    |
| Entries           | 4 (2 dirs + 2 files) |

**Archive contents:**

| Path                          | Size | Type |
|-------------------------------|-----:|------|
| `sample_dir/`                 |    0 | dir  |
| `sample_dir/sub/`             |    0 | dir  |
| `sample_dir/sub/nested.txt`   |   20 | file |
| `sample_dir/sample.txt`       |  137 | file |

---

### sample.tar.gz

**Fixture tier:** (a) — generated locally by GNU tar 1.35 with gzip compression.

**Creation command:**

```sh
(cd /tmp/tarwork && tar --format=gnu -czf tests/fixtures/sample.tar.gz sample_dir/)
```

| Field             | Value          |
|-------------------|----------------|
| Format            | tar + gzip     |
| Magic bytes       | `1F 8B` (gzip) |
| Compressed size   | 308 bytes      |
| Entries           | same as sample.tar |

---

### sample.zip

**Fixture tier:** (a) — generated locally by Info-ZIP 3.0.

**Creation command:**

```sh
(cd /tmp/tarwork && zip -9 -r tests/fixtures/sample.zip sample_dir/)
```

| Field             | Value                    |
|-------------------|--------------------------|
| Format            | ZIP (local file header)  |
| Magic bytes       | `50 4B 03 04`            |
| Compressed size   | 800 bytes                |
| Entries           | 4 (2 dirs + 2 files)     |

**Archive contents:**

| Path                          | Size | Type |
|-------------------------------|-----:|------|
| `sample_dir/`                 |    0 | dir  |
| `sample_dir/sub/`             |    0 | dir  |
| `sample_dir/sub/nested.txt`   |   20 | file |
| `sample_dir/sample.txt`       |  137 | file |

---

## Tier (b) archive fixtures — `sample.7z`

### sample.7z

**Fixture tier:** (b) — copied from `sevenz-rust2-0.21.0` bundled test data.
No `p7zip` / `7z` tool was found on PATH, so the archive was not generated
locally.

**Origin:**
`~/.cargo/registry/src/.../sevenz-rust2-0.21.0/tests/resources/single_file_with_content_lzma.7z`

The `sevenz-rust2` crate's own test suite documents this archive's content:

```rust
// From sevenz-rust2-0.21.0/tests/decompression_tests.rs:
assert_eq!(read_to_string(file1_path).unwrap(), "this is a file\n");
```

**Archive contents:**

| Path       | Size | Type |
|------------|-----:|------|
| `file.txt` |   15 | file |

`file.txt` contains the UTF-8 string `"this is a file\n"` (15 bytes, LF
terminated, no trailing newline after the final `\n`).

**Format details:**

| Field           | Value                     |
|-----------------|---------------------------|
| Magic bytes     | `37 7A BC AF 27 1C`       |
| Compression     | LZMA (single-file solid)  |
| Compressed size | 160 bytes                 |

**Note on unix permissions:** The 7z format stores unix file-mode bits in the
`windows_attributes` field (upper 16 bits), but this mapping is only populated
by p7zip on Linux and is not present in archives created by the Windows 7-Zip
GUI.  This fixture was created by the 7-Zip library (no p7zip unix-attributes
extension) so it does **not** contain unix permission bits.  See `CURRENT_ISSUES.md`
for the blocked investigation into the `windows_attributes` mode-bit mapping
— that investigation requires a fixture created by `p7zip` with known unix
permissions, which cannot be produced without the `p7zip` tool on PATH.

---

## Tier (b) codec fixture — `ipsum.br`

**Fixture tier:** (b) — copied from `brotli-decompressor-5.0.1` bundled test data.
No `brotli` CLI tool was found on PATH, so the file was not generated locally.

**Origin:**
`~/.cargo/registry/src/.../brotli-decompressor-5.0.1/src/bin/ipsum.brotli`

The file was created by the reference C Brotli encoder (not by rcomp).  The
corresponding plaintext is
`~/.cargo/registry/src/.../brotli-decompressor-5.0.1/src/bin/ipsum.raw`.

The file was renamed from `ipsum.brotli` → `ipsum.br` to use the canonical
rcomp/RFC extension for Brotli-compressed files.

**Fixture note — no magic bytes:** Brotli has no standardised magic bytes.
Therefore the interop test **does not** include an extensionless magic-byte
detection test for this fixture (unlike all other codec fixtures).  With the
`.br` extension absent, rcomp cannot detect the format from file content alone
and would require an explicit `--algo brotli` flag.

**Decompressed content:**

| Field                     | Value              |
|---------------------------|--------------------|
| First 50 bytes            | `Lorem ipsum dolor sit amet, consectetur adipiscing` |
| Decompressed size         | 5018 bytes         |
| Compressed (fixture) size | 1752 bytes         |

---

## Gaps — formats with no usable upstream fixture

### lz4

No `.lz4` frame-format fixture was found in the `lz4-1.28.1` or
`lz4-sys-1.11.1+lz4-1.10.0` crate sources bundled under
`~/.cargo/registry/src/`.  The `lz4-sys` package bundles a single binary file
`liblz4/tests/goldenSamples/skip.bin` which is a raw block-format sample, not
an LZ4 frame, and cannot be used as an interop fixture for the lz4 frame
decoder that rcomp uses.

`lz4` is not on PATH on the build host, so no tier-(a) fixture can be
generated.

**Recommendation:** add a tier-(a) `sample.txt.lz4` fixture once `lz4` is
available on PATH, and add the corresponding interop detection + extraction
test.
