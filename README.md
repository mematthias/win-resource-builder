# win-resource-builder

[![Crates.io](https://img.shields.io/crates/v/win-resource-builder.svg)](https://crates.io/crates/win-resource-builder)
[![Documentation](https://docs.rs/win-resource-builder/badge.svg)](https://docs.rs/win-resource-builder)
[![License: MIT](https://img.shields.io/badge/License-MIT-blue.svg)](https://opensource.org/licenses/MIT)
[![License: Apache 2.0](https://img.shields.io/badge/License-Apache_2.0-blue.svg)](https://opensource.org/licenses/Apache-2.0)

Windows resources built directly in Rust, without `rc.exe`, `windres`, `llvm-rc`, `cvtres` or any other
resource tool.

It writes Win32 resource files (`.res`) and COFF resource objects on every platform. A typed API takes
manifests, version information, icons and cursors; the result goes to `link.exe` or `lld-link` as
`.res`, or to every linker as COFF object. There is no `.rc` text, so no input can change the
structure of the output.

The crate is meant for build scripts:

```toml
[build-dependencies]
win-resource-builder = "0.1"
```

```rust
let mut file = ResourceFile::new();
file.insert_manifest(Manifest::standard().create()?)?;
file.insert_version_info(VersionInfo::options().create("Example", "app.exe", Version::new(1, 2, 3, 4))?)?;
file.insert_icon(NonZeroU16::MIN, LanguageId::NEUTRAL, Icon::options().create_from_file("app.ico")?)?;
file.write_coff(File::create("app.obj")?, CoffMachine::cargo_target()?)?;
```

## Features

- `images` (default): icons and cursors, with png and oxipng, which brings libdeflate (C). Without
  it, the crate builds manifests and version information only and needs none of them.
- `parallel`: oxipng optimizes on every core with rayon. rayon ignores the jobserver of Cargo and
  competes with the other build jobs, so it is off by default.
- `svg`: icons and cursors rendered from SVG by resvg, with basic safeguards only (see below).

## Icons and cursors

`Icon::options()` and `Cursor::options()` load from one of these sources:

- `create_from_file` / `create_from_ico` / `create_from_cur`:
  an `.ico` or `.cur` file of at most 16 MiB, whose images may be PNG or bitmaps.
- `create_from_pngs`: one PNG per size, at most 16 MiB together.
- `create_from_svg` (feature `svg`): one document rendered at every requested size.

Every image is stored as PNG, losslessly reduced by oxipng. `to_ico` and `to_cur` write the
optimized file; loaded with `PngOptimization::Retain` it is kept byte for byte, so a persisted file
spares later builds the rendering and the optimization, and the build script needs no `svg` feature:

```rust
let icon = Icon::options().create_from_svg(&svg, &ImageSize::ICON_SIZES)?;
std::fs::write("app.ico", icon.to_ico())?;
// Later builds
let icon = Icon::options().png_optimization(PngOptimization::Retain).create_from_file("app.ico")?;
```

`PngOptimization::Strip` keeps the image data byte for byte as well, but drops every ancillary chunk
except `tRNS`. Images are decoded one at a time, after every check that needs only the headers.

Cursors take their hotspots from the input: the directory of a `.cur` file, a `tEXt` chunk `hotspot`
with the value `x,y` in a PNG, or the center of the element with `id="hotspot"` in an SVG document,
which `visibility="hidden"` keeps out of the picture; usvg resolves viewBox and transforms for it.
`CursorOptions::hotspot` receives size and declared hotspot of every image and returns the one to use;
without it, an image without hotspot is an error.

With a `viewBox` of one unit per pixel of a base size, a 1x1 rectangle marks a pixel. Here it is
pixel (4, 7) at 32 pixels, which scales to (9, 15) at 64 and (36, 60) at 256 pixels. The element
must neither have `display="none"` nor sit in `<defs>`, as usvg drops such elements:

```xml
<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 32 32">
	<rect id="hotspot" x="4" y="7" width="1" height="1" visibility="hidden"/>
	<path d="M4 7 L4 28 L9 23 L13 31 L16 30 L12 22 L19 22 Z" fill="#000" stroke="#fff"/>
</svg>
```

`insert_icon_alias` and `insert_cursor_alias` give an icon or cursor a further name that shares its
images. Every group counts for positive `ExtractIcon` indices; negative ids such as `app.exe,-1`
stay independent of aliases.

## Rules

Every text the resources store must be in Unicode normalization form C (NFC). The crate checks it
with `unicode-normalization` and reports the byte where the text first differs from its NFC form;
it never normalizes on its own, as a normalized `OriginalFilename` could name another file.

Version information, icons and cursors accept a strict subset of what Windows loads:

- Version information requires product name, file name and version; strings are non-empty,
  in NFC and free of control characters and, unless `allow_bidi_controls(true)`, of the text
  direction controls that make Explorer show a name in another order than it is stored.
- Icons and cursors are square, every size occurs once, the 256-pixel image is required, and
  cursor hotspots lie inside their image. Bitmaps whose mask would invert the screen are rejected.

Manifests have two levels of checks:

- The typed settings of `Manifest::standard()` are fully checked and always write values Windows
  accepts: enumerations, versions, assembly names (ASCII letters, digits, `.`, `-`, `_`), public key
  tokens (16 hexadecimal digits) and escaped text. `Manifest::standard()` generates the common
  manifest of an executable: visual styles, `asInvoker`, Windows 10 and 11, per-monitor DPI
  awareness and the UTF-8 code page. The UTF-8 code page applies to the whole process, linked C and
  C++ code included; `active_code_page(None)` leaves it out. `Manifest::standard_dll()` generates the
  manifest of a DLL with visual styles only.
- A manifest from text, custom windows settings and raw XML are checked to be well-formed XML 1.0
  in UTF-8 without DTD, with roxmltree, and the XML declaration to name version 1.0 and UTF-8.
  Their content carries no further guarantee.
- Every manifest is also checked as a whole to be in NFC, as two normalized parts can compose
  where they meet: a description that starts with U+0338 turns the `>` before it into U+226F.

## Safety

`.ico`, `.cur` and PNG input is untrusted. Containers are parsed field by field without indexing,
images must neither overlap nor point into the directory, PNG goes through the png crate with an
allocation limit and without decompressing text or color profile chunks, and dimensions are checked
before any pixel is decoded, so memory and time stay bounded for every input.

SVG input gets basic protection from what the parsers offer: at most 1 MiB of UTF-8, no DTD, at most
100000 XML nodes, no images or other external resources, and no text, as resvg is built without fonts.
The rendering time is not bounded; filters can make a document render for long.

## Testing

```text
cargo test --no-default-features
cargo test
cargo test --all-features
cargo clippy --all-targets --no-default-features -- -D warnings
cargo clippy --all-targets -- -D warnings
cargo clippy --all-targets --all-features -- -D warnings
cargo fmt --check
```

The golden tests compare the `.res` form entry by entry and the COFF form byte by byte. The COFF
references come from `tools/generate_coff_golden.py` with `llvm-cvtres`. The `.res` references
come from `tools/generate_golden.py`; they are currently generated with `--llvm-rc` and should be
regenerated with `rc.exe` on Windows. The icon and cursor fixtures next to `source/` are files that
`to_ico` and `to_cur` wrote from the files in `source/`; after a change of the encoder or of oxipng,
write them anew the same way, then the references.

`tools/windows_check/check.py` builds an executable and a DLL with every PNG form oxipng produces
and checks on Windows with `LoadImageW` and related functions that every image loads with its
pixels; see its README.

Open: the `rc.exe` references, the results of the Windows check, fuzzing of the `.ico`/`.cur` and
manifest parsers.

## License

Licensed under either of the Apache License, Version 2.0 (`LICENSE-APACHE`) or the MIT license
(`LICENSE-MIT`), at your option.

Unless you explicitly state otherwise, any contribution intentionally submitted for inclusion in
this crate by you, as defined in the Apache-2.0 license, shall be dual licensed as above, without any
additional terms or conditions.
