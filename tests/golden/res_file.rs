#[derive(Debug, Eq, PartialEq)]
pub struct Entry {
	pub resource_type: u16,
	pub name: u16,
	pub language: u16,
	pub memory_flags: u16,
	pub data_version: u32,
	pub version: u32,
	pub characteristics: u32,
	pub data: Vec<u8>,
}

impl Entry {
	#[must_use]
	pub fn label(&self) -> String {
		format!("type {} name {} language 0x{:04X}", self.resource_type, self.name, self.language)
	}
}

#[must_use]
fn word(file: &[u8], offset: usize) -> u16 {
	let end = offset.checked_add(2).expect("Small offset");
	u16::from_le_bytes(
		file.get(offset..end)
			.and_then(|word| word.try_into().ok())
			.expect("WORD inside the file"),
	)
}

#[must_use]
fn dword(file: &[u8], offset: usize) -> u32 {
	let end = offset.checked_add(4).expect("Small offset");
	u32::from_le_bytes(
		file.get(offset..end)
			.and_then(|dword| dword.try_into().ok())
			.expect("DWORD inside the file"),
	)
}

#[must_use]
fn at(start: usize, offset: usize) -> usize {
	start.checked_add(offset).expect("Small offset")
}

#[must_use]
pub fn read_entries(file: &[u8]) -> Vec<Entry> {
	let null_header: [u8; 32] = [
		0x00, 0x00, 0x00, 0x00, 0x20, 0x00, 0x00, 0x00, 0xFF, 0xFF, 0x00, 0x00, 0xFF, 0xFF, 0x00, 0x00, //
		0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, //
	];
	assert_eq!(file.get(..32), Some(&null_header[..]), "File does not start with the null header");

	let mut entries = Vec::new();
	let mut start = 32_usize;
	while start < file.len() {
		let data_size = usize::try_from(dword(file, start)).expect("Small data");
		assert_eq!(dword(file, at(start, 4)), 32, "Header with string type or name is not supported");
		assert_eq!(
			(word(file, at(start, 8)), word(file, at(start, 12))),
			(0xFFFF, 0xFFFF),
			"TYPE and NAME are ordinals"
		);

		let data_start = at(start, 32);
		let data_end = at(data_start, data_size);
		entries.push(Entry {
			resource_type: word(file, at(start, 10)),
			name: word(file, at(start, 14)),
			data_version: dword(file, at(start, 16)),
			memory_flags: word(file, at(start, 20)),
			language: word(file, at(start, 22)),
			version: dword(file, at(start, 24)),
			characteristics: dword(file, at(start, 28)),
			data: file.get(data_start..data_end).expect("Data inside the file").to_vec(),
		});

		let next = data_end.next_multiple_of(4);
		let padding = file.get(data_end..next).expect("Padding inside the file");
		assert!(padding.iter().all(|&byte| byte == 0), "Padding is not zero");
		start = next;
	}
	assert_eq!(start, file.len(), "File ends inside an entry");
	entries
}
