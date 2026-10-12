# Windows check

Builds an executable and a DLL with icons, cursors and manifests from win-resource-builder and checks on
Windows that the loader of user32 reads every image with the pixels it stores. The main question is
whether Windows accepts every PNG form oxipng reduces images to.

```text
python tools/windows_check/check.py
python tools/windows_check/check.py --target x86_64-pc-windows-msvc --target x86_64-pc-windows-gnu
```

The first build takes a few minutes, as the build script compiles resvg and oxipng optimized. Every
target writes `report_<target>.txt` next to the script; the exit code is 1 on any failure. The
`-gnu` target needs the MinGW toolchain and checks GNU ld with the COFF object as well.

## What is checked

`build.rs` generates icons whose pixels lead oxipng to a particular PNG form: RGBA, RGB, grayscale
with 1, 2, 4 and 8 bits, grayscale with alpha, palettes with 1, 2, 4 and 8 bits, palettes with
tRNS. The case names state the intended form, the report the real one per image; small sizes often
end up in another form. An SVG icon, the icon fixtures, cursors of three forms, an SVG cursor, the
cursor fixture, an alias of each kind and the standard manifests complete the set.

Every image of the executable and of the DLL, which is loaded as data file, goes through:

- `lookup`: `LookupIconIdFromDirectoryEx` picks this image for its size from the group directory.
- `create`: `CreateIconFromResourceEx` decodes the stored image.
- `load`: `LoadImageW` finds the group and decodes the image, as `LoadIcon` and `LoadCursor` do.
- `shell`: `PrivateExtractIconsW` extracts the icon from the file the way Explorer does.

The `.ico` and `.cur` files with the PNG images as generated (`input`) and those `to_ico`/`to_cur`
wrote (`optimized`) are loaded with `LoadImageW` and `LR_LOADFROMFILE` (`file`).

Each check reads the image back with `GetIconInfo` and `GetDIBits` and compares size, hotspot and
every pixel with the PNG, decoded by the png crate. Alpha has to match exactly and so does the color
of every visible pixel, either straight or premultiplied; the report names which one Windows
returns. The executable only starts if Windows accepts its manifest; the report header shows the
active code page and the DPI awareness the manifest asks for.

## Reading the report

A failing `create` with the image form in its row means Windows cannot decode that PNG form, and
win-resource-builder has to keep oxipng from producing it. A failing `lookup` or `load` for the 256-pixel
cursor means Windows does not read the 256 that cursor groups store as 0, like `rc.exe` writes it.
