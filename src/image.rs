//! `RT_ICON`, `RT_GROUP_ICON`, `RT_CURSOR` and `RT_GROUP_CURSOR`: icons and cursors.
//!
//! Icons and cursors are loaded from `.ico` and `.cur` files, whose images may be PNG or bitmaps,
//! from one PNG per size or, with the feature `svg`, from an SVG document rendered at every size.
//! Every image is square, from 1 to 256 pixels, one per size, and the 256 pixel image is required.
//! Recommended are the sizes Windows requests at the common display scales,
//! [`ImageSize::ICON_SIZES`] for icons and [`ImageSize::CURSOR_SIZES`] for cursors.
//!
//! Every image is stored as PNG, losslessly reduced by oxipng, which keeps executables small.
//! The recommended workflow is to load the source once,
//! keep the file that [`to_ico`](Icon::to_ico) or [`to_cur`](Cursor::to_cur) returns,
//! and load that with [`PngOptimization::Retain`], so that a build neither renders nor optimizes.
//!
//! `.ico`, `.cur` and PNG input is untrusted: containers are parsed field by field,
//! every check that needs only headers runs before any pixel is decoded,
//! PNG goes through the png crate with an allocation limit and without text decompression,
//! and only one image is decoded at a time, so memory and time stay bounded for every input.
//!
//! Cursors take their hotspots from the input: the directory of a `.cur` file,
//! a `tEXt` chunk `hotspot` of a PNG with the value `x,y`,
//! or the center of the element with `id="hotspot"` of an SVG document,
//! which `visibility="hidden"` keeps out of the picture.
//! [`CursorOptions::hotspot`] replaces or completes them.
//!
//! ```no_run
//! use std::num::NonZeroU16;
//! use win_resource_builder::image::{Icon, PngOptimization};
//! use win_resource_builder::resource::{LanguageId, ResourceFile};
//!
//! fn main() -> Result<(), Box<dyn std::error::Error>> {
//! 	let icon = Icon::options().png_optimization(PngOptimization::Retain).create_from_file("app.ico")?;
//! 	let mut file = ResourceFile::new();
//! 	file.insert_icon(NonZeroU16::MIN, LanguageId::NEUTRAL, &icon)?;
//! 	Ok(())
//! }
//! ```

mod bitmap;
mod codec;
mod container;
#[cfg(feature = "svg")]
mod svg;

use self::bitmap::Bitmap;
use self::container::{Kind, Record};
use crate::resource::{
	DISCARDABLE, InsertError, LanguageId, MOVEABLE, NewEntry, PURE, RT_CURSOR, RT_GROUP_CURSOR, RT_GROUP_ICON, RT_ICON, ResourceFile,
	ResourceKey,
};
use crate::{ordinal, put_u16, put_u32, u64_from_usize};
use oxipng::Options as OxipngOptions;
use std::fmt;
use std::fs::File;
use std::io::{self, Read as _};
use std::num::{NonZeroU8, NonZeroU16};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use thiserror::Error;

/// How the images of an icon or cursor are compressed when it is loaded.
///
/// Recommended are oxipng with the default preset for regular builds,
/// [`Retain`](Self::Retain) for files that [`to_ico`](Icon::to_ico) or [`to_cur`](Cursor::to_cur) already optimized,
/// and Zopfli where the smallest executable matters more than the build time.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PngOptimization {
	/// oxipng with libdeflate: every lossless reduction, at the given search effort.
	Oxipng {
		/// Search effort; [`OxipngPreset::DEFAULT`] is recommended.
		preset: OxipngPreset,
	},
	/// oxipng with Zopfli: a few percent smaller than libdeflate, but many times slower.
	Zopfli {
		/// Search effort; [`OxipngPreset::DEFAULT`] is recommended.
		preset: OxipngPreset,
		/// Zopfli iterations per deflate stream; 15 is the usual value, more gain little.
		iterations: NonZeroU8,
	},
	/// No optimization: PNG images are checked and stored byte for byte,
	/// bitmaps and rendered images are compressed quickly.
	///
	/// For files that [`to_ico`](Icon::to_ico) or [`to_cur`](Cursor::to_cur) already optimized,
	/// so that a build does not optimize them again.
	/// The PNG images of other files keep their ancillary chunks, such as texts and color profiles.
	Retain,
	/// Like [`Retain`](PngOptimization::Retain), but every PNG image keeps only `IHDR`, `PLTE`, `tRNS`, `IDAT` and `IEND`:
	/// texts, time stamps, color profiles and gamma are dropped, and the image data stays byte for byte.
	Strip,
}

/// The edge length of a square image, from 1 to 256 pixels.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
#[repr(transparent)]
pub struct ImageSize(NonZeroU16);

impl ImageSize {
	/// 1 pixel, the smallest size.
	pub const MIN: Self = Self(NonZeroU16::MIN);
	/// 256 pixels, the size every icon and cursor contains.
	pub const MAX: Self = Self(ordinal(256));

	/// The sizes Windows requests for icons at display scales from 100 to 300 percent,
	/// recommended for every icon: 16, 20, 24, 32, 40, 48, 64, 80, 96 and 256 pixels.
	pub const ICON_SIZES: [Self; 10] = [
		Self(ordinal(16)),
		Self(ordinal(20)),
		Self(ordinal(24)),
		Self(ordinal(32)),
		Self(ordinal(40)),
		Self(ordinal(48)),
		Self(ordinal(64)),
		Self(ordinal(80)),
		Self(ordinal(96)),
		Self::MAX,
	];

	/// The sizes Windows requests for cursors at display scales from 100 to 300 percent and for large pointers,
	/// recommended for every cursor: 32, 40, 48, 64, 80, 96 and 256 pixels.
	pub const CURSOR_SIZES: [Self; 7] = [
		Self(ordinal(32)),
		Self(ordinal(40)),
		Self(ordinal(48)),
		Self(ordinal(64)),
		Self(ordinal(80)),
		Self(ordinal(96)),
		Self::MAX,
	];

	/// Returns the size, or [`None`] outside of 1 to 256 pixels.
	#[must_use]
	pub const fn new(pixels: u32) -> Option<Self> {
		if pixels > 256 {
			return None;
		}
		let [low, high, _, _] = pixels.to_le_bytes();
		match NonZeroU16::new(u16::from_le_bytes([low, high])) {
			Some(edge) => Some(Self(edge)),
			None => None,
		}
	}

	/// Returns the edge length in pixels.
	#[must_use]
	pub const fn get(self) -> u16 {
		self.0.get()
	}

	/// Returns the edge length in pixels as [`u32`], the type of PNG dimensions.
	#[must_use]
	pub const fn pixels(self) -> u32 {
		let [low, high] = self.0.get().to_le_bytes();
		u32::from_le_bytes([low, high, 0, 0])
	}

	/// Returns the size as a directory byte stores it, where 0 stands for 256.
	#[must_use]
	const fn dimension(self) -> u8 {
		match self.0.get().to_le_bytes() {
			[low, 0] => low,
			_ => 0,
		}
	}
}

/// The point of a cursor that clicks, in pixels from the top left corner of its image.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct Hotspot {
	x: u16,
	y: u16,
}

impl Hotspot {
	/// Returns the hotspot at column `x` and row `y`.
	#[must_use]
	pub const fn new(x: u16, y: u16) -> Self {
		Self { x, y }
	}

	/// Returns the column, counted from the left edge.
	#[must_use]
	pub const fn x(self) -> u16 {
		self.x
	}

	/// Returns the row, counted from the top edge.
	#[must_use]
	pub const fn y(self) -> u16 {
		self.y
	}

	/// Returns the `LOCALHEADER` of a cursor image: x and y as WORDs.
	#[must_use]
	const fn to_le_bytes(self) -> [u8; 4] {
		let [x0, x1] = self.x.to_le_bytes();
		let [y0, y1] = self.y.to_le_bytes();
		[x0, x1, y0, y1]
	}
}

/// The search effort of oxipng, from 0 to 6: higher presets try more filters and strategies.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
#[repr(transparent)]
pub struct OxipngPreset(u8);

impl OxipngPreset {
	/// Preset 0, for quick debug builds.
	pub const FASTEST: Self = Self(0);
	/// Preset 2, the recommended balance of size and build time.
	pub const DEFAULT: Self = Self(2);
	/// Preset 6, for release builds that want the smallest images.
	pub const MAXIMUM: Self = Self(6);

	/// Returns preset `level`, or [`None`] above 6.
	#[must_use]
	pub const fn new(level: u8) -> Option<Self> {
		if level <= Self::MAXIMUM.0 { Some(Self(level)) } else { None }
	}

	/// Returns the preset level, from 0 to 6.
	#[must_use]
	pub const fn level(self) -> u8 {
		self.0
	}
}

/// An icon with its images stored as PNG, ascending by size, ready to insert.
#[derive(Debug, Eq, PartialEq)]
pub struct Icon {
	images: Vec<Image>,
}

impl Icon {
	/// Returns options with the default oxipng preset, no minimum size and cursor files rejected.
	#[must_use]
	pub const fn options() -> IconOptions {
		IconOptions {
			allow_cursor_files: false,
			min_size: ImageSize::MIN,
			png_optimization: DEFAULT_OPTIMIZATION,
		}
	}

	/// Returns the sizes of the images, ascending.
	pub fn sizes(&self) -> impl ExactSizeIterator<Item = ImageSize> {
		self.images.iter().map(|image| image.size)
	}

	/// Returns the icon as `.ico` file, which loads again to the same icon.
	///
	/// Keeping this file and loading it with [`PngOptimization::Retain`] spares later builds the optimization.
	#[must_use]
	pub fn to_ico(&self) -> Vec<u8> {
		// Planes 1 and BitCount 32: Windows decodes every PNG to 32 bits
		container::write(Kind::Icon, self.images.iter().map(|image| (image.size, [1, 32], &*image.png)))
	}

	fn new(images: Vec<Image>) -> Result<Self, ImageError> {
		file_fits(images.iter().map(|image| image.png.len()))?;
		Ok(Self { images })
	}
}

/// Options for loading an icon.
#[derive(Clone, Debug)]
pub struct IconOptions {
	allow_cursor_files: bool,
	min_size: ImageSize,
	png_optimization: PngOptimization,
}

impl IconOptions {
	/// Loads an icon from an `.ico` file of at most 16 MiB.
	pub fn create_from_file(self, path: impl AsRef<Path>) -> Result<Icon, ImageError> {
		let data = read_file(path.as_ref())?;
		self.create_from_ico(&data)
	}

	/// Loads an icon from the content of an `.ico` file of at most 16 MiB.
	pub fn create_from_ico(self, data: &[u8]) -> Result<Icon, ImageError> {
		let (kind, records) = container::parse(data)?;
		if kind == Kind::Cursor && !self.allow_cursor_files {
			return Err(ImageError::WrongFileKind {
				expected: Kind::Icon.name(),
				found: kind.name(),
			});
		}
		self.finish(pending_from_records(kind, records, false)?)
	}

	/// Builds an icon from one PNG image per size, at most 16 MiB together.
	///
	/// For artwork drawn at every size, which stays sharper than one image that Windows scales.
	pub fn create_from_pngs<T>(self, pngs: &[T]) -> Result<Icon, ImageError>
	where
		T: AsRef<[u8]>,
	{
		self.finish(pending_from_pngs(pngs, false)?)
	}

	/// Renders an SVG document of at most 1 MiB at every size of `sizes`.
	///
	/// The sizes are checked before the document is parsed.
	/// The document must not have a DTD or more than 100000 nodes; images and text are not drawn.
	/// Its rendering time is not bounded, so render once and keep the file [`to_ico`](Icon::to_ico) returns.
	#[cfg(feature = "svg")]
	pub fn create_from_svg(self, svg: &[u8], sizes: &[ImageSize]) -> Result<Icon, ImageError> {
		check_sizes(sizes.iter().copied().enumerate(), self.min_size)?;
		let document = svg::Document::parse(svg, false)?;
		self.finish(pending_from_svg(&document, sizes))
	}

	/// Accepts `.cur` files as icons and ignores their hotspots; off by default.
	#[must_use]
	pub const fn allow_cursor_files(mut self, value: bool) -> Self {
		self.allow_cursor_files = value;
		self
	}

	/// Rejects images smaller than `value`, 1 pixel by default.
	#[must_use]
	pub const fn min_size(mut self, value: ImageSize) -> Self {
		self.min_size = value;
		self
	}

	/// Sets how the images are compressed, oxipng with the default preset by default.
	#[must_use]
	pub const fn png_optimization(mut self, value: PngOptimization) -> Self {
		self.png_optimization = value;
		self
	}

	fn finish(self, pending: Vec<Pending<'_>>) -> Result<Icon, ImageError> {
		let pending = arrange(pending, self.min_size)?;
		let storage = Storage::new(self.png_optimization);
		let mut images = Vec::with_capacity(pending.len());
		for item in &pending {
			images.push(Image {
				size: item.size,
				png: storage.store(item, &[])?,
			});
		}
		Icon::new(images)
	}
}

/// A cursor with its images stored as PNG and a hotspot per image, ascending by size, ready to insert.
#[derive(Debug, Eq, PartialEq)]
pub struct Cursor {
	images: Vec<CursorImage>,
}

impl Cursor {
	/// Returns options with the default oxipng preset, no minimum size and the hotspots of the input.
	#[must_use]
	pub const fn options<'a>() -> CursorOptions<'a> {
		CursorOptions {
			min_size: ImageSize::MIN,
			png_optimization: DEFAULT_OPTIMIZATION,
			hotspot: None,
		}
	}

	/// Returns the size and the hotspot of every image, ascending by size.
	pub fn images(&self) -> impl ExactSizeIterator<Item = (ImageSize, Hotspot)> {
		self.images.iter().map(|image| (image.size, image.hotspot))
	}

	/// Returns the cursor as `.cur` file, which loads again to the same cursor.
	///
	/// Keeping this file and loading it with [`PngOptimization::Retain`] spares later builds the optimization.
	#[must_use]
	pub fn to_cur(&self) -> Vec<u8> {
		// A .cur file stores the hotspot where an .ico file stores planes and bit count
		let entries = self
			.images
			.iter()
			.map(|image| (image.size, [image.hotspot.x, image.hotspot.y], image.png()));
		container::write(Kind::Cursor, entries)
	}

	fn new(images: Vec<CursorImage>) -> Result<Self, ImageError> {
		file_fits(images.iter().map(|image| image.data.len()))?;
		Ok(Self { images })
	}
}

/// Gives the hotspot of every image from its size and the hotspot the input declares, if any.
type HotspotFn<'a> = dyn FnMut(ImageSize, Option<Hotspot>) -> Hotspot + 'a;

/// Options for loading a cursor.
pub struct CursorOptions<'a> {
	min_size: ImageSize,
	png_optimization: PngOptimization,
	hotspot: Option<Box<HotspotFn<'a>>>,
}

impl<'a> CursorOptions<'a> {
	/// Loads a cursor from a `.cur` file of at most 16 MiB.
	pub fn create_from_file(self, path: impl AsRef<Path>) -> Result<Cursor, ImageError> {
		let data = read_file(path.as_ref())?;
		self.create_from_cur(&data)
	}

	/// Loads a cursor from the content of a `.cur` file of at most 16 MiB.
	pub fn create_from_cur(self, data: &[u8]) -> Result<Cursor, ImageError> {
		let (kind, records) = container::parse(data)?;
		if kind != Kind::Cursor {
			return Err(ImageError::WrongFileKind {
				expected: Kind::Cursor.name(),
				found: kind.name(),
			});
		}
		self.finish(pending_from_records(kind, records, true)?)
	}

	/// Builds a cursor from one PNG image per size, at most 16 MiB together.
	///
	/// Every PNG may declare its hotspot in a `tEXt` chunk `hotspot` with the value `x,y` or `x y`.
	pub fn create_from_pngs<T>(self, pngs: &[T]) -> Result<Cursor, ImageError>
	where
		T: AsRef<[u8]>,
	{
		self.finish(pending_from_pngs(pngs, true)?)
	}

	/// Renders an SVG document of at most 1 MiB at every size of `sizes`.
	///
	/// The center of the bounding box of the element with `id="hotspot"` is the hotspot at every size;
	/// `visibility="hidden"` keeps the element out of the picture.
	/// Neither `display="none"` nor `<defs>` work, as usvg drops such elements before the search.
	///
	/// With a `viewBox` of one unit per pixel of a base size, a 1x1 rectangle marks a pixel;
	/// here pixel (4, 7) at 32 pixels, which scales to (9, 15) at 64 and (36, 60) at 256 pixels:
	///
	/// ```xml
	/// <svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 32 32">
	/// 	<rect id="hotspot" x="4" y="7" width="1" height="1" visibility="hidden"/>
	/// 	<path d="M4 7 L4 28 L9 23 L13 31 L16 30 L12 22 L19 22 Z" fill="#000" stroke="#fff"/>
	/// </svg>
	/// ```
	#[cfg(feature = "svg")]
	pub fn create_from_svg(self, svg: &[u8], sizes: &[ImageSize]) -> Result<Cursor, ImageError> {
		check_sizes(sizes.iter().copied().enumerate(), self.min_size)?;
		let document = svg::Document::parse(svg, true)?;
		self.finish(pending_from_svg(&document, sizes))
	}

	/// Gives the hotspot of every image from its size and the hotspot the input declares;
	/// without, an image without hotspot is an error.
	#[must_use]
	pub fn hotspot(mut self, value: impl FnMut(ImageSize, Option<Hotspot>) -> Hotspot + 'a) -> Self {
		self.hotspot = Some(Box::new(value));
		self
	}

	/// Rejects images smaller than `value`, 1 pixel by default.
	#[must_use]
	pub const fn min_size(mut self, value: ImageSize) -> Self {
		self.min_size = value;
		self
	}

	/// Sets how the images are compressed, oxipng with the default preset by default.
	#[must_use]
	pub const fn png_optimization(mut self, value: PngOptimization) -> Self {
		self.png_optimization = value;
		self
	}

	fn finish(mut self, pending: Vec<Pending<'_>>) -> Result<Cursor, ImageError> {
		let pending = arrange(pending, self.min_size)?;
		// The hotspots need only the sizes, so a wrong one fails before any image is decoded
		let mut hotspots = Vec::with_capacity(pending.len());
		for item in &pending {
			let hotspot = match self.hotspot.as_mut() {
				Some(resolve) => resolve(item.size, item.hotspot),
				None => item.hotspot.ok_or(ImageError::MissingHotspot { image: item.index })?,
			};
			check_hotspot(item.index, item.size, hotspot)?;
			hotspots.push(hotspot);
		}
		let storage = Storage::new(self.png_optimization);
		let mut images = Vec::with_capacity(pending.len());
		for (item, hotspot) in pending.iter().zip(hotspots) {
			images.push(CursorImage {
				size: item.size,
				hotspot,
				data: storage.store(item, &hotspot.to_le_bytes())?,
			});
		}
		Cursor::new(images)
	}
}

impl fmt::Debug for CursorOptions<'_> {
	#[inline]
	fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
		f.debug_struct("CursorOptions")
			.field("min_size", &self.min_size)
			.field("png_optimization", &self.png_optimization)
			.field("hotspot", &self.hotspot.is_some())
			.finish()
	}
}

impl ResourceFile {
	/// Inserts an icon as `RT_GROUP_ICON` resource `id`, with its images as `RT_ICON` resources.
	///
	/// The images take the names after the highest image name in the file.
	/// Explorer shows the icon group with the lowest name for the file, so the main icon is usually 1.
	/// The resource file shares the data with the icon, so no byte is copied;
	/// the icon stays usable, after a failed insertion as well, and can go into further files.
	/// To use an icon under several names of one file, insert an alias.
	pub fn insert_icon(&mut self, id: NonZeroU16, language: LanguageId, icon: &Icon) -> Result<(), InsertError> {
		let images = icon.images.iter().map(|image| {
			let dimension = image.size.dimension();
			// Planes 1 and BitCount 32: Windows decodes every PNG to 32 bits
			([dimension, dimension, 0, 0, 1, 0, 32, 0], <Arc<[u8]>>::clone(&image.png))
		});
		let entries = self.group_entries(Kind::Icon, ResourceKey::new(RT_GROUP_ICON, id, language), images)?;
		self.insert_entries(entries)
	}

	/// Inserts a cursor as `RT_GROUP_CURSOR` resource `id`, with its images as `RT_CURSOR` resources.
	///
	/// The images take the names after the highest image name in the file.
	/// The resource file shares the data with the cursor, so no byte is copied;
	/// the cursor stays usable, after a failed insertion as well, and can go into further files.
	/// To use a cursor under several names of one file, insert an alias.
	pub fn insert_cursor(&mut self, id: NonZeroU16, language: LanguageId, cursor: &Cursor) -> Result<(), InsertError> {
		let images = cursor.images.iter().map(|image| {
			// Like rc.exe, WORDs of the directory size byte, where 0 stands for 256, and of its double,
			// then planes 1 and BitCount 32; the data already starts with the hotspot (LOCALHEADER)
			let dimension = u16::from(image.size.dimension());
			let [width_low, width_high] = dimension.to_le_bytes();
			let [height_low, height_high] = dimension.strict_mul(2).to_le_bytes();
			(
				[width_low, width_high, height_low, height_high, 1, 0, 32, 0],
				<Arc<[u8]>>::clone(&image.data),
			)
		});
		let entries = self.group_entries(Kind::Cursor, ResourceKey::new(RT_GROUP_CURSOR, id, language), images)?;
		self.insert_entries(entries)
	}

	/// Inserts icon `target` once more as icon `id`, sharing its images.
	///
	/// For programs and frameworks that load the same icon under several names, without storing the images twice.
	pub fn insert_icon_alias(&mut self, id: NonZeroU16, language: LanguageId, target: NonZeroU16) -> Result<(), InsertError> {
		self.insert_alias(RT_GROUP_ICON, id, language, target)
	}

	/// Inserts cursor `target` once more as cursor `id`, sharing its images.
	///
	/// For programs and frameworks that load the same cursor under several names, without storing the images twice.
	pub fn insert_cursor_alias(&mut self, id: NonZeroU16, language: LanguageId, target: NonZeroU16) -> Result<(), InsertError> {
		self.insert_alias(RT_GROUP_CURSOR, id, language, target)
	}

	/// Returns the image entries and, last, the group directory that lists them.
	fn group_entries(
		&self,
		kind: Kind,
		group: ResourceKey,
		images: impl ExactSizeIterator<Item = ([u8; 8], Arc<[u8]>)>,
	) -> Result<Vec<NewEntry>, InsertError> {
		let image_type = match kind {
			Kind::Icon => RT_ICON,
			Kind::Cursor => RT_CURSOR,
		};
		let count = images.len();
		let out_of_ids = || InsertError::OutOfImageIds {
			resource_type: image_type.get(),
			count,
		};
		let first = match self.max_name(image_type) {
			Some(highest) => highest.checked_add(1).ok_or_else(out_of_ids)?,
			None => NonZeroU16::MIN,
		};
		let count_word = u16::try_from(count).map_err(|_| out_of_ids())?;
		first.checked_add(count_word.saturating_sub(1)).ok_or_else(out_of_ids)?;

		// An icon or cursor has at most 256 images, one per size
		let mut directory = Vec::with_capacity(count.strict_mul(14).strict_add(6));
		put_u16(&mut directory, 0); // idReserved
		put_u16(&mut directory, kind.id()); // idType
		put_u16(&mut directory, count_word); // idCount
		let mut entries = Vec::with_capacity(count.strict_add(1));
		for ((head, data), image_id) in images.zip((first.get()..=u16::MAX).filter_map(NonZeroU16::new)) {
			directory.extend_from_slice(&head);
			put_u32(&mut directory, u32::try_from(data.len()).map_err(|_| InsertError::FileTooLarge)?);
			put_u16(&mut directory, image_id.get());
			let key = ResourceKey::new(image_type, image_id, group.language);
			entries.push(NewEntry::new(key, MOVEABLE | DISCARDABLE, data));
		}
		entries.push(NewEntry::new(group, MOVEABLE | PURE | DISCARDABLE, Arc::from(directory)));
		Ok(entries)
	}

	fn insert_alias(
		&mut self,
		group_type: NonZeroU16,
		id: NonZeroU16,
		language: LanguageId,
		target: NonZeroU16,
	) -> Result<(), InsertError> {
		let (flags, directory) = self
			.shared_entry(ResourceKey::new(group_type, target, language))
			.ok_or(InsertError::MissingGroup {
				resource_type: group_type.get(),
				name: target.get(),
				language: language.raw(),
			})?;
		let entry = NewEntry::new(ResourceKey::new(group_type, id, language), flags, directory);
		self.insert_entries(vec![entry])
	}
}

/// Why an icon or cursor could not be created.
///
/// A field `image` is the position of the image in its input:
/// the directory record of a file, the PNG in the slice or the size in the list of sizes.
#[derive(Debug, Error)]
pub enum ImageError {
	/// The file could not be read.
	#[error("Cannot read '{}'", .path.display())]
	Io {
		path: PathBuf,
		#[source]
		source: io::Error,
	},

	/// The input exceeds its size limit.
	#[error("Input of {len} bytes exceeds the limit of {limit} bytes")]
	InputTooLarge { len: usize, limit: usize },

	/// An `.ico` file was given where a `.cur` file was expected, or the reverse.
	#[error("Expected an {expected} file, but got a {found} file")]
	WrongFileKind { expected: &'static str, found: &'static str },

	/// The file ends inside its header or directory.
	#[error("Icon file is truncated: {needed} bytes needed, {available} available")]
	Truncated { needed: usize, available: usize },

	/// The input contains no image.
	#[error("Input contains no images")]
	NoImages,

	/// The header is neither that of an `.ico` nor that of a `.cur` file.
	#[error("Icon file header is invalid (reserved {reserved}, type {file_type})")]
	InvalidHeader { reserved: u16, file_type: u16 },

	/// The directory lists more images than sizes exist.
	#[error("Icon file lists {count} images, but at most 256 sizes exist")]
	TooManyImages { count: usize },

	/// A directory record has a length of 0.
	#[error("Image {image} is empty")]
	EmptyImage { image: usize },

	/// A directory record points outside of the file or into the directory.
	#[error("Image {image} lies outside of the image data")]
	ImageOutOfBounds { image: usize },

	/// Two directory records share bytes.
	#[error("Images {first} and {second} overlap")]
	OverlappingImages { first: usize, second: usize },

	/// An image is neither PNG nor a bitmap with `BITMAPINFOHEADER`.
	#[error("Image {image} is neither PNG nor a bitmap with BITMAPINFOHEADER")]
	UnknownImageFormat { image: usize },

	/// The size of an image differs from the size its directory record states.
	#[error("Image {image} is {size}x{size}, but the directory says {width}x{height}")]
	DimensionMismatch { image: usize, width: u32, height: u32, size: u32 },

	/// A bitmap header or pixel is invalid; `reason` says how.
	#[error("Bitmap of image {image} is invalid: {reason}")]
	InvalidBitmap { image: usize, reason: &'static str },

	/// A bitmap is shorter than its header requires.
	#[error("Bitmap of image {image} needs {needed} bytes, but has {available}")]
	BitmapTooSmall { image: usize, needed: usize, available: usize },

	/// The PNG decoder rejected an image.
	#[error("Image {image} cannot be decoded: {message}")]
	Decode { image: usize, message: String },

	/// A PNG is animated, which icons and cursors cannot show.
	#[error("PNG of image {image} is animated")]
	AnimatedPng { image: usize },

	/// A bitmap inverts the screen beneath a pixel, which PNG cannot express.
	#[error("Image {image} changes the screen beneath pixel ({x}, {y}), which an image with alpha channel cannot express")]
	InvertingPixels { image: usize, x: u16, y: u16 },

	/// An image is not square.
	#[error("Image {image} is {width}x{height}, but icons and cursors must be square")]
	NotSquare { image: usize, width: u32, height: u32 },

	/// An image is larger than 256 pixels or smaller than the minimum size.
	#[error("Image {image} is {size} pixels wide, outside of {min_size} to 256")]
	SizeOutOfRange { image: usize, size: u32, min_size: u32 },

	/// Two images have the same size.
	#[error("Images {first} and {second} are both {size} pixels wide")]
	DuplicateSize { first: usize, second: usize, size: u32 },

	/// No image has 256 pixels.
	#[error("The required 256x256 image is missing")]
	Missing256,

	/// A cursor image declares no hotspot, and no hotspot function gives one.
	#[error("Image {image} declares no hotspot; CursorOptions::hotspot can give one")]
	MissingHotspot { image: usize },

	/// A hotspot lies outside of its image.
	#[error("Hotspot ({x}, {y}) of image {image} lies outside of its {size}x{size} pixels")]
	HotspotOutsideImage { image: usize, x: u16, y: u16, size: u32 },

	/// The `tEXt` chunk `hotspot` of a PNG is invalid; `reason` says how.
	#[error("The tEXt chunk \"hotspot\" of PNG {image} holds {value:?}, which {reason}")]
	InvalidPngHotspot { image: usize, value: String, reason: &'static str },

	/// The hotspot element of an SVG document is invalid; `reason` says how.
	#[error("The SVG hotspot is invalid: {reason}")]
	InvalidSvgHotspot { reason: &'static str },

	/// An SVG document was rejected; `message` says why.
	#[error("SVG is invalid: {message}")]
	InvalidSvg { message: String },

	/// Encoding an image as PNG failed.
	#[error("Encoding the {size}-pixel image failed: {message}")]
	Encode { size: u32, message: String },

	/// The images exceed the 4 GiB an `.ico` or `.cur` file can address.
	#[error("The images exceed the 4 GiB of an icon or cursor file")]
	TooLarge,
}

/// The limit of `.ico` and `.cur` files and of the PNG images of one icon or cursor together.
const MAX_INPUT_SIZE: usize = 0x0100_0000;

const DEFAULT_OPTIMIZATION: PngOptimization = PngOptimization::Oxipng {
	preset: OxipngPreset::DEFAULT,
};

#[derive(Clone, Eq, PartialEq)]
struct Image {
	size: ImageSize,
	png: Arc<[u8]>,
}

/// Shows the size and the length of the PNG, not its bytes.
impl fmt::Debug for Image {
	fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
		f.debug_struct("Image")
			.field("size", &self.size.get())
			.field("len", &self.png.len())
			.finish_non_exhaustive()
	}
}

/// A cursor image as the resource stores it: the hotspot (`LOCALHEADER`) followed by the PNG.
#[derive(Clone, Eq, PartialEq)]
struct CursorImage {
	size: ImageSize,
	hotspot: Hotspot,
	data: Arc<[u8]>,
}

/// Shows size, hotspot and the length of the PNG, not its bytes.
impl fmt::Debug for CursorImage {
	fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
		f.debug_struct("CursorImage")
			.field("size", &self.size.get())
			.field("hotspot", &self.hotspot)
			.field("len", &self.png().len())
			.finish_non_exhaustive()
	}
}

impl CursorImage {
	#[must_use]
	fn png(&self) -> &[u8] {
		self.data.get(4..).unwrap_or_default()
	}
}

/// One image whose headers are checked and whose pixels are not decoded yet.
struct Pending<'a> {
	/// The position in the input, for errors.
	index: usize,
	size: ImageSize,
	hotspot: Option<Hotspot>,
	source: Source<'a>,
}

enum Source<'a> {
	Png(&'a [u8]),
	Bitmap(Bitmap<'a>),
	#[cfg(feature = "svg")]
	Svg(&'a svg::Document),
}

/// Stores images as PNG the way a [`PngOptimization`] asks for.
struct Storage {
	optimizer: Option<OxipngOptions>,
	strip: bool,
}

impl Storage {
	#[must_use]
	fn new(mode: PngOptimization) -> Self {
		match mode {
			PngOptimization::Oxipng { preset } => Self {
				optimizer: Some(codec::oxipng_options(preset, None)),
				strip: false,
			},
			PngOptimization::Zopfli { preset, iterations } => Self {
				optimizer: Some(codec::oxipng_options(preset, Some(iterations))),
				strip: false,
			},
			PngOptimization::Retain | PngOptimization::Strip => Self {
				optimizer: None,
				strip: mode == PngOptimization::Strip,
			},
		}
	}

	/// Decodes or checks one image and returns `prefix` followed by its PNG,
	/// in one allocation of the exact size that the resource file later shares.
	fn store(&self, item: &Pending<'_>, prefix: &[u8]) -> Result<Arc<[u8]>, ImageError> {
		let &Pending { index, size, .. } = item;
		if self.optimizer.is_none()
			&& let Source::Png(png) = item.source
		{
			codec::validate(png, index)?;
			if !self.strip {
				// The length is known in advance, so collecting allocates once
				return Ok(prefix.iter().chain(png).copied().collect());
			}
			let mut out = Vec::with_capacity(prefix.len().saturating_add(png.len()));
			out.extend_from_slice(prefix);
			codec::strip_into(png, &mut out);
			return Ok(Arc::from(out));
		}

		let pixels = match &item.source {
			Source::Png(png) => codec::decode_rgba(png, index, size)?,
			Source::Bitmap(bitmap) => bitmap.decode(index)?,
			#[cfg(feature = "svg")]
			Source::Svg(document) => document.render(size, index)?,
		};
		let Some(options) = &self.optimizer else {
			let mut out = prefix.to_vec();
			codec::encode_fast(&pixels, size, &mut out)?;
			return Ok(Arc::from(out));
		};
		let png = codec::optimize(pixels, size, options)?;
		Ok(prefix.iter().chain(&png).copied().collect())
	}
}

/// Reads a file of at most [`MAX_INPUT_SIZE`] bytes, also if it grows while it is read.
fn read_file(path: &Path) -> Result<Vec<u8>, ImageError> {
	let io_error = |source| ImageError::Io {
		path: path.to_owned(),
		source,
	};
	let too_large = |len| ImageError::InputTooLarge {
		len,
		limit: MAX_INPUT_SIZE,
	};
	let file = File::open(path).map_err(io_error)?;
	let len = file.metadata().map_err(io_error)?.len();
	let limit = u64_from_usize(MAX_INPUT_SIZE);
	if len > limit {
		return Err(too_large(usize::try_from(len).unwrap_or(usize::MAX)));
	}
	let mut data = Vec::with_capacity(usize::try_from(len).unwrap_or(0));
	file.take(limit.strict_add(1)).read_to_end(&mut data).map_err(io_error)?;
	if data.len() > MAX_INPUT_SIZE {
		return Err(too_large(data.len()));
	}
	Ok(data)
}

/// Checks the header of every image of a container against its directory record.
fn pending_from_records(kind: Kind, records: Vec<Record<'_>>, read_hotspots: bool) -> Result<Vec<Pending<'_>>, ImageError> {
	let mut pending = Vec::with_capacity(records.len());
	for record in records {
		let index = record.index;
		let (size, source) = if record.data.starts_with(&codec::SIGNATURE) {
			(codec::size(record.data, index)?, Source::Png(record.data))
		} else if record.data.first_chunk::<4>() == Some(&40_u32.to_le_bytes()) {
			let (size, bitmap) = Bitmap::parse(record.data, index)?;
			(size, Source::Bitmap(bitmap))
		} else {
			return Err(ImageError::UnknownImageFormat { image: index });
		};
		let (width, height) = record.directory_size();
		if (width, height) != (size.pixels(), size.pixels()) {
			return Err(ImageError::DimensionMismatch {
				image: index,
				width,
				height,
				size: size.pixels(),
			});
		}
		pending.push(Pending {
			index,
			size,
			hotspot: (read_hotspots && kind == Kind::Cursor).then_some(Hotspot::new(record.first, record.second)),
			source,
		});
	}
	Ok(pending)
}

/// Checks the header and reads the declared hotspot of every PNG.
fn pending_from_pngs<T>(pngs: &[T], read_hotspots: bool) -> Result<Vec<Pending<'_>>, ImageError>
where
	T: AsRef<[u8]>,
{
	let total = pngs.iter().try_fold(0_usize, |total, png| total.checked_add(png.as_ref().len()));
	if total.is_none_or(|total| total > MAX_INPUT_SIZE) {
		return Err(ImageError::InputTooLarge {
			len: total.unwrap_or(usize::MAX),
			limit: MAX_INPUT_SIZE,
		});
	}
	let mut pending = Vec::with_capacity(pngs.len());
	for (index, png) in pngs.iter().enumerate() {
		let data = png.as_ref();
		if !data.starts_with(&codec::SIGNATURE) {
			return Err(ImageError::UnknownImageFormat { image: index });
		}
		let size = codec::size(data, index)?;
		let hotspot = if read_hotspots {
			codec::text_hotspot(data, index, size)?
		} else {
			None
		};
		pending.push(Pending {
			index,
			size,
			hotspot,
			source: Source::Png(data),
		});
	}
	Ok(pending)
}

#[cfg(feature = "svg")]
#[must_use]
fn pending_from_svg<'a>(document: &'a svg::Document, sizes: &[ImageSize]) -> Vec<Pending<'a>> {
	sizes
		.iter()
		.enumerate()
		.map(|(index, &size)| Pending {
			index,
			size,
			hotspot: document.hotspot(size),
			source: Source::Svg(document),
		})
		.collect()
}

/// Checks the sizes and sorts the images by size, before anything is decoded.
fn arrange(mut pending: Vec<Pending<'_>>, min_size: ImageSize) -> Result<Vec<Pending<'_>>, ImageError> {
	check_sizes(pending.iter().map(|item| (item.index, item.size)), min_size)?;
	pending.sort_unstable_by_key(|item| item.size);
	Ok(pending)
}

/// Checks that every size is at least `min_size`, occurs once and that 256 pixels are among them.
fn check_sizes(sizes: impl Iterator<Item = (usize, ImageSize)>, min_size: ImageSize) -> Result<(), ImageError> {
	// The input position of every size seen so far, indexed by size
	let mut seen = [None::<usize>; 257];
	let mut empty = true;
	for (index, size) in sizes {
		empty = false;
		if size < min_size {
			return Err(ImageError::SizeOutOfRange {
				image: index,
				size: size.pixels(),
				min_size: min_size.pixels(),
			});
		}
		if let Some(slot) = seen.get_mut(usize::from(size.get())) {
			if let Some(first) = *slot {
				return Err(ImageError::DuplicateSize {
					first,
					second: index,
					size: size.pixels(),
				});
			}
			*slot = Some(index);
		}
	}
	if empty {
		return Err(ImageError::NoImages);
	}
	if seen.last().copied().flatten().is_none() {
		return Err(ImageError::Missing256);
	}
	Ok(())
}

/// Checks that an `.ico` or `.cur` file with images of these lengths fits the 32-bit offsets.
fn file_fits(lengths: impl ExactSizeIterator<Item = usize>) -> Result<(), ImageError> {
	let directory = lengths.len().checked_mul(16).and_then(|len| len.checked_add(6));
	let total = lengths.fold(directory, |total, len| total?.checked_add(len));
	match total.map(u32::try_from) {
		Some(Ok(_)) => Ok(()),
		_ => Err(ImageError::TooLarge),
	}
}

fn check_dimensions(image: usize, width: u32, height: u32) -> Result<ImageSize, ImageError> {
	if width != height {
		return Err(ImageError::NotSquare { image, width, height });
	}
	ImageSize::new(width).ok_or(ImageError::SizeOutOfRange {
		image,
		size: width,
		min_size: 1,
	})
}

fn check_hotspot(image: usize, size: ImageSize, hotspot: Hotspot) -> Result<(), ImageError> {
	if hotspot.x >= size.get() || hotspot.y >= size.get() {
		return Err(ImageError::HotspotOutsideImage {
			image,
			x: hotspot.x,
			y: hotspot.y,
			size: size.pixels(),
		});
	}
	Ok(())
}

/// Parses two finite numbers separated by whitespace or a comma.
fn parse_pair(text: &str) -> Result<(f64, f64), &'static str> {
	const FORMAT: &str = "is not two numbers separated by whitespace or a comma";
	let trimmed = text.trim();
	let (x, y) = trimmed
		.split_once(',')
		.or_else(|| trimmed.split_once(|character: char| character.is_ascii_whitespace()))
		.ok_or(FORMAT)?;
	let (x, y) = (
		x.trim().parse::<f64>().map_err(|_| FORMAT)?,
		y.trim().parse::<f64>().map_err(|_| FORMAT)?,
	);
	if x.is_finite() && y.is_finite() {
		Ok((x, y))
	} else {
		Err("contains a number that is not finite")
	}
}

/// Returns the pixel of an image of `size` that contains `position`, clamped to the image.
///
/// A position within a billionth below a pixel border counts as the next pixel,
/// so that scaled coordinates such as 4.999999999 land where they were meant.
#[must_use]
fn pixel_under(position: f64, size: ImageSize) -> u16 {
	let last = size.get().saturating_sub(1);
	(0..=last).rev().find(|&pixel| f64::from(pixel) <= position + 1e-9).unwrap_or(0)
}

#[cfg(test)]
mod tests;
