//! Bitmaps with `BITMAPINFOHEADER` as `.ico` and `.cur` files store them:
//! an XOR image of twice the height, then an AND mask of one bit per pixel, both bottom-up.
//!
//! The header is checked before any pixel is read, and the pixel loop walks the rows with iterators,
//! so it needs no index arithmetic and no bounds checks of its own.

use super::{ImageError, ImageSize, check_dimensions};
use crate::usize_from_u32;

/// The bits of a byte from the leftmost pixel on.
const BITS: [u8; 8] = [0x80, 0x40, 0x20, 0x10, 0x08, 0x04, 0x02, 0x01];

/// A checked bitmap whose pixels are not decoded yet.
pub(super) struct Bitmap<'a> {
	edge: usize,
	format: Format,
	/// `RGBQUAD` entries: blue, green, red and a reserved byte.
	palette: &'a [[u8; 4]],
	xor: &'a [u8],
	xor_stride: usize,
	mask: &'a [u8],
	mask_stride: usize,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Format {
	Indexed1,
	Indexed4,
	Indexed8,
	/// X1R5G5B5.
	Rgb555,
	Bgr,
	Bgra,
}

impl Format {
	#[must_use]
	const fn bits(self) -> usize {
		match self {
			Self::Indexed1 => 1,
			Self::Indexed4 => 4,
			Self::Indexed8 => 8,
			Self::Rgb555 => 16,
			Self::Bgr => 24,
			Self::Bgra => 32,
		}
	}

	/// Returns the largest color table the format allows, or [`None`] for formats without one.
	#[must_use]
	const fn max_colors(self) -> Option<u32> {
		match self {
			Self::Indexed1 => Some(2),
			Self::Indexed4 => Some(16),
			Self::Indexed8 => Some(256),
			Self::Rgb555 | Self::Bgr | Self::Bgra => None,
		}
	}
}

impl<'a> Bitmap<'a> {
	/// Checks the header and the size of the data, before any pixel is read.
	pub(super) fn parse(data: &'a [u8], image: usize) -> Result<(ImageSize, Self), ImageError> {
		let invalid = |reason| ImageError::InvalidBitmap { image, reason };
		let Some((header, rest)) = data.split_first_chunk::<40>() else {
			return Err(invalid("the header is truncated"));
		};
		let &[
			s0,
			s1,
			s2,
			s3,
			w0,
			w1,
			w2,
			w3,
			h0,
			h1,
			h2,
			h3,
			p0,
			p1,
			b0,
			b1,
			c0,
			c1,
			c2,
			c3,
			_,
			_,
			_,
			_,
			_,
			_,
			_,
			_,
			_,
			_,
			_,
			_,
			u0,
			u1,
			u2,
			u3,
			_,
			_,
			_,
			_,
		] = header;
		if u32::from_le_bytes([s0, s1, s2, s3]) != 40 {
			return Err(invalid("the header is not a BITMAPINFOHEADER"));
		}
		let width = u32::try_from(i32::from_le_bytes([w0, w1, w2, w3]))
			.ok()
			.filter(|&width| width > 0)
			.ok_or(invalid("the width is not positive"))?;
		let height = u32::try_from(i32::from_le_bytes([h0, h1, h2, h3]))
			.ok()
			.filter(|&height| height > 0)
			.ok_or(invalid("the height is not positive"))?;
		if u16::from_le_bytes([p0, p1]) != 1 {
			return Err(invalid("the plane count is not 1"));
		}
		let format = match u16::from_le_bytes([b0, b1]) {
			1 => Format::Indexed1,
			4 => Format::Indexed4,
			8 => Format::Indexed8,
			16 => Format::Rgb555,
			24 => Format::Bgr,
			32 => Format::Bgra,
			_ => return Err(invalid("the bit count is not 1, 4, 8, 16, 24 or 32")),
		};
		if u32::from_le_bytes([c0, c1, c2, c3]) != 0 {
			return Err(invalid("the bitmap is compressed or uses bit fields"));
		}
		let colors_used = u32::from_le_bytes([u0, u1, u2, u3]);
		let colors = match format.max_colors() {
			Some(max) if colors_used > max => return Err(invalid("the color table is larger than the bit count allows")),
			Some(max) if colors_used == 0 => max,
			Some(_) => colors_used,
			None if colors_used != 0 => return Err(invalid("the bitmap declares a color table above 8 bits per pixel")),
			None => 0,
		};
		if !height.is_multiple_of(2) {
			return Err(invalid("the height does not hold an XOR image and an AND mask of the same height"));
		}
		let size = check_dimensions(image, width, height.strict_div(2))?;

		// At most 256 pixels per row, 32 bits per pixel and 256 table entries keep every product small
		let edge = usize::from(size.get());
		let xor_stride = edge.strict_mul(format.bits()).next_multiple_of(32).strict_div(8);
		let mask_stride = edge.next_multiple_of(32).strict_div(8);
		let palette_len = usize_from_u32(colors).strict_mul(4);
		let xor_len = xor_stride.strict_mul(edge);
		let mask_len = mask_stride.strict_mul(edge);
		let too_small = || ImageError::BitmapTooSmall {
			image,
			needed: palette_len.strict_add(xor_len).strict_add(mask_len).strict_add(40),
			available: data.len(),
		};
		let (palette, rest) = rest.split_at_checked(palette_len).ok_or_else(too_small)?;
		let (xor, rest) = rest.split_at_checked(xor_len).ok_or_else(too_small)?;
		let mask = rest.get(..mask_len).ok_or_else(too_small)?;
		Ok((
			size,
			Self {
				edge,
				format,
				palette: palette.as_chunks::<4>().0,
				xor,
				xor_stride,
				mask,
				mask_stride,
			},
		))
	}

	/// Decodes the pixels to RGBA, top-down, the way Windows draws them.
	///
	/// A 32-bit bitmap with any alpha value is drawn with its alpha channel, every other one with its mask.
	/// Black under a set mask bit is transparent; any other color would invert the screen,
	/// which an image with alpha channel cannot express, so it is rejected.
	/// Transparent pixels lose their color, so that equal images compress equally.
	pub(super) fn decode(&self, image: usize) -> Result<Vec<u8>, ImageError> {
		let use_alpha = self.format == Format::Bgra && self.xor.as_chunks::<4>().0.iter().any(|&[.., alpha]| alpha != 0);
		let mut out = Vec::with_capacity(self.edge.strict_mul(self.edge).strict_mul(4));
		let rows = self
			.xor
			.chunks_exact(self.xor_stride)
			.rev()
			.zip(self.mask.chunks_exact(self.mask_stride).rev());
		for (y, (colors, mask)) in (0_u16..).zip(rows) {
			let mut row = Row {
				out: &mut out,
				image,
				y,
				edge: self.edge,
				use_alpha,
			};
			let mask = mask.iter().flat_map(|&byte| BITS.map(|bit| byte & bit != 0));
			match self.format {
				Format::Indexed1 => {
					let indices = colors.iter().flat_map(|&byte| BITS.map(|bit| u8::from(byte & bit != 0)));
					row.push(indices.map(|index| self.color(index)), mask)
				},
				Format::Indexed4 => {
					let indices = colors.iter().flat_map(|&byte| [byte.strict_shr(4), byte & 0x0F]);
					row.push(indices.map(|index| self.color(index)), mask)
				},
				Format::Indexed8 => row.push(colors.iter().map(|&index| self.color(index)), mask),
				Format::Rgb555 => {
					let pixels = colors.as_chunks::<2>().0.iter().map(|&pair| Some(rgb555(u16::from_le_bytes(pair))));
					row.push(pixels, mask)
				},
				Format::Bgr => row.push(colors.as_chunks::<3>().0.iter().map(|&[b, g, r]| Some([r, g, b, 0])), mask),
				Format::Bgra => row.push(colors.as_chunks::<4>().0.iter().map(|&[b, g, r, a]| Some([r, g, b, a])), mask),
			}?;
		}
		Ok(out)
	}

	/// Returns the color of a table entry, or [`None`] behind the table.
	#[must_use]
	fn color(&self, index: u8) -> Option<[u8; 4]> {
		self.palette.get(usize::from(index)).map(|&[b, g, r, _]| [r, g, b, 0])
	}
}

/// The output of one row, top-down.
struct Row<'o> {
	out: &'o mut Vec<u8>,
	image: usize,
	y: u16,
	edge: usize,
	use_alpha: bool,
}

impl Row<'_> {
	/// Appends `edge` pixels; every color holds red, green, blue and the stored alpha.
	fn push(&mut self, colors: impl Iterator<Item = Option<[u8; 4]>>, mask: impl Iterator<Item = bool>) -> Result<(), ImageError> {
		for ((x, color), masked) in (0_u16..).zip(colors.take(self.edge)).zip(mask) {
			let [red, green, blue, alpha] = color.ok_or(ImageError::InvalidBitmap {
				image: self.image,
				reason: "a pixel refers to a color behind the color table",
			})?;
			let pixel = if self.use_alpha {
				[red, green, blue, alpha]
			} else if !masked {
				[red, green, blue, 0xFF]
			} else if [red, green, blue] == [0; 3] {
				[0; 4]
			} else {
				return Err(ImageError::InvertingPixels {
					image: self.image,
					x,
					y: self.y,
				});
			};
			let [.., visible] = pixel;
			self.out.extend_from_slice(if visible == 0 { &[0; 4] } else { &pixel });
		}
		Ok(())
	}
}

/// Widens X1R5G5B5 to 8 bits per channel by bit replication.
#[must_use]
fn rgb555(value: u16) -> [u8; 4] {
	let channel = |shift: u32| {
		let [five, _] = (value.strict_shr(shift) & 0x1F).to_le_bytes();
		five.strict_shl(3) | five.strict_shr(2)
	};
	[channel(10), channel(5), channel(0), 0]
}
