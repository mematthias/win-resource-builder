//! The `.ico` and `.cur` container: a header, a directory of 16-byte records and the image data.
//!
//! The parser trusts no field: every record is checked against the file before its data is used,
//! and images must neither overlap nor point into the directory.

use super::{ImageError, ImageSize, MAX_INPUT_SIZE};
use crate::{put_u16, put_u32, usize_from_u32};

/// The two kinds of container, which differ in the type field and in two fields of every record.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum Kind {
	/// `.ico`: the record fields hold planes and bit count.
	Icon,
	/// `.cur`: the record fields hold the hotspot.
	Cursor,
}

impl Kind {
	#[must_use]
	pub(super) const fn id(self) -> u16 {
		match self {
			Self::Icon => 1,
			Self::Cursor => 2,
		}
	}

	#[must_use]
	pub(super) const fn name(self) -> &'static str {
		match self {
			Self::Icon => ".ico",
			Self::Cursor => ".cur",
		}
	}
}

/// One directory record with the data it points to.
pub(super) struct Record<'a> {
	pub(super) index: usize,
	pub(super) width: u8,
	pub(super) height: u8,
	/// Planes of an icon or the horizontal hotspot of a cursor.
	pub(super) first: u16,
	/// Bit count of an icon or the vertical hotspot of a cursor.
	pub(super) second: u16,
	pub(super) data: &'a [u8],
}

impl Record<'_> {
	/// Returns the edge length the record states, where 0 stands for 256.
	#[must_use]
	pub(super) fn directory_size(&self) -> (u32, u32) {
		let edge = |value: u8| if value == 0 { 256 } else { u32::from(value) };
		(edge(self.width), edge(self.height))
	}
}

/// Parses the header and the directory and returns every record with its data.
pub(super) fn parse(file: &[u8]) -> Result<(Kind, Vec<Record<'_>>), ImageError> {
	if file.len() > MAX_INPUT_SIZE {
		return Err(ImageError::InputTooLarge {
			len: file.len(),
			limit: MAX_INPUT_SIZE,
		});
	}
	let Some(&[r0, r1, t0, t1, c0, c1]) = file.first_chunk::<6>() else {
		return Err(ImageError::Truncated {
			needed: 6,
			available: file.len(),
		});
	};
	let (reserved, file_type, count) = (
		u16::from_le_bytes([r0, r1]),
		u16::from_le_bytes([t0, t1]),
		usize::from(u16::from_le_bytes([c0, c1])),
	);
	let kind = match (reserved, file_type) {
		(0, 1) => Kind::Icon,
		(0, 2) => Kind::Cursor,
		_ => return Err(ImageError::InvalidHeader { reserved, file_type }),
	};
	if count == 0 {
		return Err(ImageError::NoImages);
	}
	if count > 256 {
		return Err(ImageError::TooManyImages { count });
	}
	// At most 256 records of 16 bytes
	let directory_end = count.strict_mul(16).strict_add(6);
	let directory = file.get(6..directory_end).ok_or(ImageError::Truncated {
		needed: directory_end,
		available: file.len(),
	})?;

	let (records, _) = directory.as_chunks::<16>();
	let mut parsed = Vec::with_capacity(count);
	let mut ranges = Vec::with_capacity(count);
	for (index, record) in records.iter().enumerate() {
		let &[width, height, _colors, _reserved, f0, f1, s0, s1, l0, l1, l2, l3, o0, o1, o2, o3] = record;
		let len = usize_from_u32(u32::from_le_bytes([l0, l1, l2, l3]));
		let start = usize_from_u32(u32::from_le_bytes([o0, o1, o2, o3]));
		if len == 0 {
			return Err(ImageError::EmptyImage { image: index });
		}
		let end = start
			.checked_add(len)
			.filter(|_| start >= directory_end)
			.ok_or(ImageError::ImageOutOfBounds { image: index })?;
		let data = file.get(start..end).ok_or(ImageError::ImageOutOfBounds { image: index })?;
		ranges.push((start, end, index));
		parsed.push(Record {
			index,
			width,
			height,
			first: u16::from_le_bytes([f0, f1]),
			second: u16::from_le_bytes([s0, s1]),
			data,
		});
	}

	ranges.sort_unstable();
	for pair in ranges.windows(2) {
		if let &[(_, first_end, first), (second_start, _, second)] = pair
			&& second_start < first_end
		{
			return Err(ImageError::OverlappingImages { first, second });
		}
	}
	Ok((kind, parsed))
}

/// Writes a container; every entry holds the size, both record fields and the PNG of one image.
///
/// The callers checked that the whole file fits 32 bits, so no offset overflows.
pub(super) fn write<'a, I>(kind: Kind, entries: I) -> Vec<u8>
where
	I: ExactSizeIterator<Item = (ImageSize, [u16; 2], &'a [u8])> + Clone,
{
	let count = entries.len();
	let directory_end = count.strict_mul(16).strict_add(6);
	// Only a capacity hint, so an overflow leaves the vector to grow instead
	let capacity = <I>::clone(&entries)
		.try_fold(directory_end, |total, (_, _, png)| total.checked_add(png.len()))
		.unwrap_or(0);
	let mut out = Vec::with_capacity(capacity);
	put_u16(&mut out, 0); // idReserved
	put_u16(&mut out, kind.id()); // idType
	put_u16(&mut out, u16::try_from(count).unwrap_or(u16::MAX)); // idCount, at most 256 sizes
	let mut offset = u32::try_from(directory_end).unwrap_or(u32::MAX);
	for (size, [first, second], png) in <I>::clone(&entries) {
		let dimension = size.dimension();
		out.extend_from_slice(&[dimension, dimension, 0, 0]); // bWidth, bHeight, bColorCount, bReserved
		put_u16(&mut out, first);
		put_u16(&mut out, second);
		let len = len32(png);
		put_u32(&mut out, len); // dwBytesInRes
		put_u32(&mut out, offset); // dwImageOffset
		offset = offset.strict_add(len);
	}
	for (_, _, png) in entries {
		out.extend_from_slice(png);
	}
	out
}

/// Returns the length of data the callers checked to fit 32 bits.
#[must_use]
pub(super) fn len32(data: &[u8]) -> u32 {
	u32::try_from(data.len()).unwrap_or(u32::MAX)
}
