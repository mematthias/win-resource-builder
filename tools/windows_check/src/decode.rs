//! The parts of the check that need no Windows: PNG decoding and the directories of groups and files.
//!
//! Every directory is checked against its length before an entry is read.

use png::{BitDepth, ColorType, Decoder, Transformations};
use std::io::Cursor;

/// A stored PNG decoded to RGBA, with the form it is stored in.
pub struct Decoded {
	pub size: u32,
	pub form: String,
	pub pixels: Vec<u8>,
}

/// One image of an `.ico` or `.cur` file.
pub struct FileImage<'a> {
	/// The two record fields that hold the hotspot of a cursor.
	pub hotspot: (u32, u32),
	pub data: &'a [u8],
}

/// Decodes a square PNG to 8-bit RGBA and names its stored form, such as `Indexed 4 +tRNS`.
pub fn decode_png(data: &[u8]) -> Result<Decoded, String> {
	let mut decoder = Decoder::new(Cursor::new(data));
	decoder.set_transformations(Transformations::EXPAND | Transformations::ALPHA | Transformations::STRIP_16);
	let mut reader = decoder.read_info().map_err(|err| err.to_string())?;
	let info = reader.info();
	let trns = if info.trns.is_some() { " +tRNS" } else { "" };
	let form = format!("{:?} {}{trns}", info.color_type, bit_depth(info.bit_depth));
	let (width, height) = (info.width, info.height);
	if width != height {
		return Err(format!("{width}x{height} is not square"));
	}
	let len = reader.output_buffer_size().ok_or_else(|| "the PNG is too large".to_owned())?;
	let mut buffer = vec![0; len];
	let frame = reader.next_frame(&mut buffer).map_err(|err| err.to_string())?;
	buffer.truncate(frame.buffer_size());
	let pixels = match frame.color_type {
		ColorType::Rgba => buffer,
		ColorType::GrayscaleAlpha => buffer
			.as_chunks::<2>()
			.0
			.iter()
			.flat_map(|&[gray, alpha]| [gray, gray, gray, alpha])
			.collect(),
		other => return Err(format!("the decoder delivered {other:?}")),
	};
	Ok(Decoded { size: width, form, pixels })
}

/// Returns the image names a group directory (`GRPICONDIR`) lists, in its order.
pub fn group_image_ids(directory: &[u8]) -> Result<Vec<u16>, String> {
	let (&[_, _, _, _, c0, c1], entries) = directory
		.split_first_chunk::<6>()
		.ok_or_else(|| "the group directory is truncated".to_owned())?;
	let count = usize::from(u16::from_le_bytes([c0, c1]));
	let records = entries
		.as_chunks::<14>()
		.0
		.get(..count)
		.ok_or_else(|| format!("the group directory lists {count} images, but holds fewer"))?;
	Ok(records.iter().map(|&[.., i0, i1]| u16::from_le_bytes([i0, i1])).collect())
}

/// Returns every image of an `.ico` or `.cur` file, checked to lie inside the file.
pub fn file_images(file: &[u8]) -> Result<Vec<FileImage<'_>>, String> {
	let (&[_, _, _, _, c0, c1], rest) = file.split_first_chunk::<6>().ok_or_else(|| "the file is truncated".to_owned())?;
	let count = usize::from(u16::from_le_bytes([c0, c1]));
	let records = rest
		.as_chunks::<16>()
		.0
		.get(..count)
		.ok_or_else(|| format!("the directory lists {count} images, but holds fewer"))?;
	records
		.iter()
		.enumerate()
		.map(|(index, &[_, _, _, _, x0, x1, y0, y1, l0, l1, l2, l3, o0, o1, o2, o3])| {
			let outside = || format!("image {index} lies outside of the file");
			let len = usize::try_from(u32::from_le_bytes([l0, l1, l2, l3])).map_err(|_| outside())?;
			let start = usize::try_from(u32::from_le_bytes([o0, o1, o2, o3])).map_err(|_| outside())?;
			let data = start.checked_add(len).and_then(|end| file.get(start..end)).ok_or_else(outside)?;
			Ok(FileImage {
				hotspot: (u32::from(u16::from_le_bytes([x0, x1])), u32::from(u16::from_le_bytes([y0, y1]))),
				data,
			})
		})
		.collect()
}

/// Splits a cursor resource into its hotspot (`LOCALHEADER`) and the PNG behind it.
#[must_use]
pub fn split_cursor(data: &[u8]) -> Option<((u32, u32), &[u8])> {
	let (&[x0, x1, y0, y1], png) = data.split_first_chunk::<4>()?;
	Some((
		(u32::from(u16::from_le_bytes([x0, x1])), u32::from(u16::from_le_bytes([y0, y1]))),
		png,
	))
}

#[must_use]
const fn bit_depth(depth: BitDepth) -> u8 {
	match depth {
		BitDepth::One => 1,
		BitDepth::Two => 2,
		BitDepth::Four => 4,
		BitDepth::Eight => 8,
		BitDepth::Sixteen => 16,
	}
}

#[cfg(test)]
mod tests {
	use super::*;

	#[must_use]
	fn png(width: u32, height: u32, color: ColorType, pixels: &[u8]) -> Vec<u8> {
		let mut data = Vec::new();
		let mut encoder = png::Encoder::new(&mut data, width, height);
		encoder.set_color(color);
		encoder.set_depth(BitDepth::Eight);
		let mut writer = encoder.write_header().unwrap();
		writer.write_image_data(pixels).unwrap();
		writer.finish().unwrap();
		data
	}

	#[test]
	fn pngs_decode_to_rgba_with_their_form() {
		let gray = decode_png(&png(1, 1, ColorType::GrayscaleAlpha, &[7, 9])).unwrap();
		assert_eq!(
			(gray.size, gray.form.as_str(), gray.pixels),
			(1, "GrayscaleAlpha 8", vec![7, 7, 7, 9])
		);
		let rgba = decode_png(&png(1, 1, ColorType::Rgba, &[1, 2, 3, 4])).unwrap();
		assert_eq!(rgba.pixels, [1, 2, 3, 4]);
		assert_eq!(
			decode_png(&png(2, 1, ColorType::Rgba, &[0; 8])).err().as_deref(),
			Some("2x1 is not square")
		);
		assert!(decode_png(b"not a PNG").is_err());
	}

	#[test]
	fn directories_are_checked_before_they_are_read() {
		// Two records announced, one stored
		let mut group = vec![0, 0, 1, 0, 2, 0, 16, 16, 0, 0, 1, 0, 32, 0, 9, 0, 0, 0, 7, 0];
		assert_eq!(
			group_image_ids(&group).err().as_deref(),
			Some("the group directory lists 2 images, but holds fewer")
		);
		group.extend_from_slice(&[32, 32, 0, 0, 1, 0, 32, 0, 9, 0, 0, 0, 8, 0]);
		assert_eq!(
			group_image_ids(&group),
			Ok(vec![7, 8]),
			"Each record has 14 bytes and ends with the name"
		);
		assert_eq!(group_image_ids(&[0; 5]).err().as_deref(), Some("the group directory is truncated"));

		let mut file = vec![0, 0, 2, 0, 1, 0, 0, 0, 0, 0, 3, 0, 4, 0, 2, 0, 0, 0, 22, 0, 0, 0];
		file.extend_from_slice(&[0xAB, 0xCD]);
		let images = file_images(&file).unwrap();
		assert_eq!(images.len(), 1);
		assert_eq!((images[0].hotspot, images[0].data), ((3, 4), &[0xAB, 0xCD][..]));
		file[14] = 3;
		assert_eq!(file_images(&file).err().as_deref(), Some("image 0 lies outside of the file"));
		assert_eq!(
			split_cursor(&[1, 0, 2, 0, 0x89]),
			Some(((1, 2), &[0x89][..])),
			"The hotspot comes first"
		);
		assert_eq!(split_cursor(&[1, 0, 2]), None);
	}
}
