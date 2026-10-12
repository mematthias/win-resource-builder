//! `RT_MANIFEST`: the application manifest, which Windows reads before a process starts.
//!
//! The manifest selects the version of the common controls, the privileges the process requests,
//! the Windows versions it was tested on and settings such as DPI awareness and the code page.
//! Windows refuses to start a program with a broken manifest, before its first instruction.
//!
//! Two levels of checks apply:
//! - The typed settings of the standard manifest are fully checked and always write values Windows accepts:
//!   enumerations, versions, assembly names, public key tokens and escaped text in normalization form C.
//! - Everything else, a manifest from XML text, custom windows settings and raw XML,
//!   is checked to be well-formed XML 1.0 in UTF-8 without DTD; its content carries no guarantee.
//!
//! Every manifest as a whole must be in Unicode normalization form C,
//! which also catches parts that are normalized each, but compose where they meet.
//!
//! Recommended are the standard manifest for executables, changed only where a setting below applies,
//! and the DLL manifest for DLLs that show dialogs or controls.
//!
//! ```no_run
//! use win_resource_builder::manifest::Manifest;
//! use win_resource_builder::resource::ResourceFile;
//!
//! fn main() -> Result<(), Box<dyn std::error::Error>> {
//! 	let mut file = ResourceFile::new();
//! 	file.insert_manifest(Manifest::standard().create()?)?;
//! 	Ok(())
//! }
//! ```

use crate::resource::{InsertError, LanguageId, MOVEABLE, NewEntry, PURE, RT_MANIFEST, ResourceFile, ResourceKey};
use crate::version_info::Version;
use crate::{nfc_violation, ordinal};
use roxmltree::{Document, Error as XmlError, ParsingOptions};
use std::borrow::Cow;
use std::iter;
use std::num::NonZeroU16;
use thiserror::Error;

/// The resource name of a manifest, which decides when Windows reads it.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub enum ManifestKind {
	/// Name 1, read when an executable starts; for executables.
	Process,
	/// Name 2, for DLLs: Windows activates it for the static imports of the DLL,
	/// and isolation-aware code activates it at run time.
	IsolationAware,
	/// Name 3, for DLLs whose isolation-aware code activates it at run time only.
	IsolationAwareNoStaticImport,
}

impl ManifestKind {
	/// Returns the resource name of the kind: 1, 2 or 3.
	#[must_use]
	pub const fn resource_id(self) -> NonZeroU16 {
		match self {
			Self::Process => ordinal(1),
			Self::IsolationAware => ordinal(2),
			Self::IsolationAwareNoStaticImport => ordinal(3),
		}
	}
}

/// The processor architecture of an assembly identity; `Any` suits most dependencies.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub enum Architecture {
	/// Any processor, written as `*`.
	Any,
	/// 32-bit x86.
	X86,
	/// x86-64.
	Amd64,
	/// 32-bit ARM.
	Arm,
	/// 64-bit ARM.
	Arm64,
}

impl Architecture {
	#[must_use]
	const fn as_str(self) -> &'static str {
		match self {
			Self::Any => "*",
			Self::X86 => "x86",
			Self::Amd64 => "amd64",
			Self::Arm => "arm",
			Self::Arm64 => "arm64",
		}
	}
}

/// The look of the common controls.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub enum Theming {
	/// The controls of Windows 2000, without visual styles and without task dialogs.
	Classic,
	/// Common controls 6 with visual styles; the default and the recommended look.
	Themed,
	/// Common controls 6 with visual styles turned off, for programs that draw every control themselves.
	Unthemed,
}

/// The privileges a process requests when it starts.
///
/// Every executable should state them:
/// without, Windows takes programs whose names contain words like setup or install for installers,
/// and asks for administrator rights.
/// `ui_access` lets accessibility tools control the windows of elevated programs;
/// it only works for signed executables in protected folders such as Program Files.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub enum ExecutionLevel {
	/// The privileges of the user who starts the program, without a prompt; recommended.
	AsInvoker { ui_access: bool },
	/// Administrator rights after a prompt if the user has them, the user's privileges otherwise.
	HighestAvailable { ui_access: bool },
	/// Administrator rights after a prompt; the program does not start without them.
	RequireAdministrator { ui_access: bool },
}

/// A Windows version the program was tested on.
///
/// Windows applies compatibility behavior for programs that do not name the running version,
/// for example version functions that report Windows 8.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub enum SupportedOs {
	/// Windows Vista and Windows Server 2008.
	WindowsVista,
	/// Windows 7 and Windows Server 2008 R2.
	Windows7,
	/// Windows 8 and Windows Server 2012.
	Windows8,
	/// Windows 8.1 and Windows Server 2012 R2.
	Windows81,
	/// Windows 10 and 11; the recommended entry.
	Windows10,
}

impl SupportedOs {
	#[must_use]
	const fn id(self) -> &'static str {
		match self {
			Self::WindowsVista => "{e2011457-1546-43c5-a5fe-008deee3d3f0}",
			Self::Windows7 => "{35138b9a-5d96-4fbd-8e2d-a2440225f93a}",
			Self::Windows8 => "{4a2f28e3-53b9-4441-ba9c-d69d4a4a6e38}",
			Self::Windows81 => "{1f676c76-80e1-4239-95bb-83d0f6d0da78}",
			Self::Windows10 => "{8e0f7a12-bfb3-4fe8-b9a5-48fd50a15a9a}",
		}
	}
}

/// How the process handles display scaling.
///
/// Windows draws programs that are not aware at 96 DPI and stretches them, which blurs them on high-DPI displays.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub enum DpiAwareness {
	/// Drawn at 96 DPI and stretched; `gdi_scaling` lets GDI draw text and shapes sharply instead.
	Unaware { gdi_scaling: bool },
	/// Scaled for the primary display at sign-in and stretched on the others;
	/// `gdi_scaling` lets GDI draw text and shapes sharply where stretched.
	System { gdi_scaling: bool },
	/// Scaled per display, without the scaling of dialogs and the non-client area of later versions.
	PerMonitor,
	/// Scaled per display, dialogs and the non-client area included; recommended for programs with windows.
	/// Windows before 10 version 1703 falls back to per-monitor awareness.
	PerMonitorV2,
}

/// Scrolling with precision touchpads in steps finer than one wheel notch.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub enum ScrollingAwareness {
	/// `highResolutionScrollingAware`.
	HighResolution,
	/// `ultraHighResolutionScrollingAware`, the finer of the two settings.
	UltraHighResolution,
}

/// The code page of the `A` functions of Windows and of the C runtime in the process.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub enum ActiveCodePage {
	/// UTF-8, on Windows 10 version 1903 and later; the default.
	Utf8,
	/// The code page of the system locale, as without the setting.
	Legacy,
}

/// The heap implementation of the process, written as `heapType`.
///
/// The segment heap can lower the memory use of programs with many allocations,
/// but changes the cost of every allocation, those of the global allocator of Rust included.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub enum HeapType {
	/// The NT heap, as without the setting; the default and the recommended heap until measurements show otherwise.
	Nt,
	/// The segment heap, `SegmentHeap`, on Windows 10 version 2004 and later; earlier versions ignore it.
	Segment,
}

/// A checked manifest, ready to insert.
#[derive(Debug, Eq, PartialEq)]
pub struct Manifest {
	kind: ManifestKind,
	language: LanguageId,
	text: String,
}

impl Manifest {
	/// Returns options for a manifest from XML text, as executable manifest in neutral language.
	#[must_use]
	pub const fn options() -> ManifestOptions {
		ManifestOptions {
			kind: ManifestKind::Process,
			language: LanguageId::NEUTRAL,
		}
	}

	/// Returns options for the manifest of an executable, generated from typed settings.
	///
	/// The defaults are what most programs need:
	/// - Common controls 6 with visual styles, so controls look like the rest of Windows.
	/// - `asInvoker`, so Windows starts the program without a prompt and never takes it for an installer.
	/// - Windows 10 and 11 as tested versions, so version functions report the real version.
	/// - Per-monitor DPI awareness, so windows stay sharp on every display.
	/// - The UTF-8 code page.
	///   It applies to the whole process, linked C and C++ code included:
	///   `GetACP`, the `A` functions of Windows and the C runtime then use UTF-8,
	///   not the code page of the system locale, which `active_code_page(None)` keeps for code that expects it.
	///
	/// The heap stays the NT heap unless measurements show that the program benefits from another one.
	#[must_use]
	pub fn standard() -> StandardManifestOptions {
		StandardManifestOptions {
			kind: ManifestKind::Process,
			language: LanguageId::NEUTRAL,
			identity: None,
			description: None,
			theming: Theming::Themed,
			dependencies: Vec::new(),
			execution_level: Some(ExecutionLevel::AsInvoker { ui_access: false }),
			supported_os: vec![SupportedOs::Windows10],
			max_version_tested: None,
			dpi_awareness: Some(DpiAwareness::PerMonitorV2),
			scrolling_awareness: None,
			disable_window_filtering: None,
			printer_driver_isolation: None,
			long_path_aware: None,
			active_code_page: Some(ActiveCodePage::Utf8),
			heap_type: HeapType::Nt,
			windows_settings: Vec::new(),
			prefix: String::new(),
			postfix: String::new(),
		}
	}

	/// Returns options for the manifest of a DLL: common controls 6 with visual styles, as resource name 2.
	///
	/// A DLL that shows dialogs or controls needs it to look like the executable;
	/// the process-wide settings are left out, as Windows only reads them from the manifest of the executable.
	#[must_use]
	pub fn standard_dll() -> StandardManifestOptions {
		StandardManifestOptions {
			kind: ManifestKind::IsolationAware,
			execution_level: None,
			supported_os: Vec::new(),
			dpi_awareness: None,
			active_code_page: None,
			..Self::standard()
		}
	}

	/// Returns the kind, which gives the resource name.
	#[must_use]
	pub const fn kind(&self) -> ManifestKind {
		self.kind
	}

	/// Returns the language the manifest is inserted in.
	#[must_use]
	pub const fn language(&self) -> LanguageId {
		self.language
	}

	/// Returns the checked text, as it is stored.
	#[must_use]
	pub fn text(&self) -> &str {
		&self.text
	}
}

/// Options for a manifest from XML text, for content the typed settings cannot express.
#[derive(Clone, Debug)]
pub struct ManifestOptions {
	kind: ManifestKind,
	language: LanguageId,
}

impl ManifestOptions {
	/// Checks `text` and returns the manifest.
	///
	/// The text only has to be well-formed XML 1.0 in UTF-8 without DTD and in normalization form C;
	/// whether Windows accepts its content is not checked.
	pub fn create(self, text: impl Into<String>) -> Result<Manifest, ManifestError> {
		let text = text.into();
		check(&text)?;
		Ok(Manifest {
			kind: self.kind,
			language: self.language,
			text,
		})
	}

	/// Sets the resource name: `Process` for executables, the default, or a name for DLLs.
	#[must_use]
	pub const fn kind(mut self, value: ManifestKind) -> Self {
		self.kind = value;
		self
	}

	/// Sets the language, neutral by default, which is recommended.
	#[must_use]
	pub const fn language(mut self, value: LanguageId) -> Self {
		self.language = value;
		self
	}
}

/// Options for a manifest generated from typed settings.
///
/// Every setting can be changed or left out;
/// dependencies, further windows settings and raw XML before and after the generated elements extend it.
/// The typed settings are checked in full, the rest only as XML.
#[derive(Clone, Debug)]
pub struct StandardManifestOptions {
	kind: ManifestKind,
	language: LanguageId,
	identity: Option<Identity>,
	description: Option<String>,
	theming: Theming,
	dependencies: Vec<Identity>,
	execution_level: Option<ExecutionLevel>,
	supported_os: Vec<SupportedOs>,
	max_version_tested: Option<Version>,
	dpi_awareness: Option<DpiAwareness>,
	scrolling_awareness: Option<ScrollingAwareness>,
	disable_window_filtering: Option<bool>,
	printer_driver_isolation: Option<bool>,
	long_path_aware: Option<bool>,
	active_code_page: Option<ActiveCodePage>,
	heap_type: HeapType,
	windows_settings: Vec<(String, String, String)>,
	prefix: String,
	postfix: String,
}

impl StandardManifestOptions {
	/// Checks the typed settings, generates the manifest and checks it as XML.
	pub fn create(self) -> Result<Manifest, ManifestError> {
		if let Some(identity) = &self.identity {
			identity.check()?;
		}
		for dependency in &self.dependencies {
			dependency.check()?;
		}
		if let Some(description) = &self.description {
			check_text("description", description)?;
		}
		let text = self.generate();
		check(&text)?;
		Ok(Manifest {
			kind: self.kind,
			language: self.language,
			text,
		})
	}

	/// Sets the resource name: `Process` for executables or a name for DLLs.
	#[must_use]
	pub const fn kind(mut self, value: ManifestKind) -> Self {
		self.kind = value;
		self
	}

	/// Sets the language, neutral by default, which is recommended.
	#[must_use]
	pub const fn language(mut self, value: LanguageId) -> Self {
		self.language = value;
		self
	}

	/// Sets the identity of the program itself, which only side-by-side assemblies need;
	/// programs are best left without.
	///
	/// The name consists of ASCII letters, digits, `.`, `-` and `_`, such as `Example.Tools.Viewer`.
	#[must_use]
	pub fn assembly_identity(mut self, name: impl Into<String>, version: Version, architecture: Architecture) -> Self {
		self.identity = Some(Identity {
			name: name.into(),
			version,
			architecture,
			public_key_token: None,
		});
		self
	}

	/// Sets `description`, a text Windows does not show; it is escaped as needed,
	/// and must consist of XML characters in normalization form C.
	#[must_use]
	pub fn description(mut self, value: impl Into<String>) -> Self {
		self.description = Some(value.into());
		self
	}

	/// Sets the look of the common controls, common controls 6 with visual styles by default.
	#[must_use]
	pub const fn theming(mut self, value: Theming) -> Self {
		self.theming = value;
		self
	}

	/// Adds a dependent assembly, such as a private side-by-side assembly shipped with the program.
	///
	/// The name follows the rules of `assembly_identity`;
	/// `public_key_token` is the 16 hexadecimal digits of a signed assembly, `None` for unsigned ones.
	/// Windows refuses to start the program if the assembly is missing.
	#[must_use]
	pub fn add_dependency(
		mut self,
		name: impl Into<String>,
		version: Version,
		public_key_token: Option<&str>,
		architecture: Architecture,
	) -> Self {
		self.dependencies.push(Identity {
			name: name.into(),
			version,
			architecture,
			public_key_token: public_key_token.map(str::to_owned),
		});
		self
	}

	/// Sets the privileges the process requests, `asInvoker` by default.
	///
	/// `None` leaves them out, which lets Windows take the program for an installer by its name.
	#[must_use]
	pub const fn execution_level(mut self, value: Option<ExecutionLevel>) -> Self {
		self.execution_level = value;
		self
	}

	/// Sets the Windows versions the program was tested on, Windows 10 and 11 by default,
	/// and the newest Windows build it was tested on.
	///
	/// Most programs leave `max_version_tested` unset;
	/// XAML Islands and other features that Windows only enables for programs tested on a given build need it.
	#[must_use]
	pub fn compatibility(mut self, supported_os: &[SupportedOs], max_version_tested: Option<Version>) -> Self {
		self.supported_os = supported_os.to_vec();
		self.supported_os.sort_unstable();
		self.supported_os.dedup();
		self.max_version_tested = max_version_tested;
		self
	}

	/// Sets the DPI awareness, per-monitor V2 by default.
	///
	/// `None` leaves it out, which makes the process unaware; console programs do not need it.
	#[must_use]
	pub const fn dpi_awareness(mut self, value: Option<DpiAwareness>) -> Self {
		self.dpi_awareness = value;
		self
	}

	/// Sets high-resolution scrolling, off by default.
	#[must_use]
	pub const fn scrolling_awareness(mut self, value: Option<ScrollingAwareness>) -> Self {
		self.scrolling_awareness = value;
		self
	}

	/// Sets `disableWindowFiltering`, which lets window enumeration find the windows of Store apps;
	/// unset by default.
	#[must_use]
	pub const fn disable_window_filtering(mut self, value: Option<bool>) -> Self {
		self.disable_window_filtering = value;
		self
	}

	/// Sets `printerDriverIsolation`, which loads printer drivers in a process of their own;
	/// unset by default.
	#[must_use]
	pub const fn printer_driver_isolation(mut self, value: Option<bool>) -> Self {
		self.printer_driver_isolation = value;
		self
	}

	/// Sets `longPathAware`, which lets file functions take paths beyond 260 characters,
	/// where the system enables long paths; unset by default.
	///
	/// The standard library of Rust handles long paths without it, so it matters for linked C and C++ code.
	#[must_use]
	pub const fn long_path_aware(mut self, value: Option<bool>) -> Self {
		self.long_path_aware = value;
		self
	}

	/// Sets the code page of the process, UTF-8 by default; `None` leaves the setting out.
	///
	/// It applies to the whole process, linked C and C++ code included:
	/// `GetACP`, the `A` functions of Windows and the C runtime use it,
	/// so keep the code page of the system locale for code that expects it.
	/// Windows before 10 version 1903 ignores the setting.
	#[must_use]
	pub const fn active_code_page(mut self, value: Option<ActiveCodePage>) -> Self {
		self.active_code_page = value;
		self
	}

	/// Sets the heap implementation of the process, the NT heap by default.
	///
	/// `HeapType::Segment` changes the cost of every allocation, so choose it only after measuring.
	#[must_use]
	pub const fn heap_type(mut self, value: HeapType) -> Self {
		self.heap_type = value;
		self
	}

	/// Adds a windows setting the typed ones do not cover, written as `<name xmlns="namespace">value</name>`.
	///
	/// Namespace and value are escaped; beyond well-formed XML nothing is checked,
	/// so Windows may reject the setting or the manifest.
	#[must_use]
	pub fn add_windows_setting(mut self, namespace: impl Into<String>, name: impl Into<String>, value: impl Into<String>) -> Self {
		self.windows_settings.push((namespace.into(), name.into(), value.into()));
		self
	}

	/// Inserts raw XML after the description, before the generated elements; only checked as XML.
	#[must_use]
	pub fn prefix(mut self, xml: impl Into<String>) -> Self {
		self.prefix = xml.into();
		self
	}

	/// Inserts raw XML after the generated elements, at the end of `assembly`; only checked as XML.
	#[must_use]
	pub fn postfix(mut self, xml: impl Into<String>) -> Self {
		self.postfix = xml.into();
		self
	}

	#[must_use]
	fn generate(&self) -> String {
		const ASM_V3: &str = "urn:schemas-microsoft-com:asm.v3";

		let mut xml = Xml(String::with_capacity(1536));
		xml.push(r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>"#);
		xml.line(0, &[r#"<assembly xmlns=""#, ASM_V1, r#"" manifestVersion="1.0">"#]);
		if let Some(identity) = &self.identity {
			xml.identity(1, identity);
		}
		if let Some(description) = &self.description {
			xml.line(1, &["<description>", &escaped(description), "</description>"]);
		}
		xml.raw(&self.prefix);

		let common_controls = (self.theming != Theming::Classic).then(|| Identity {
			name: "Microsoft.Windows.Common-Controls".to_owned(),
			version: Version::new(6, 0, 0, 0),
			architecture: Architecture::Any,
			public_key_token: Some("6595b64144ccf1df".to_owned()),
		});
		for identity in common_controls.iter().chain(&self.dependencies) {
			xml.line(1, &["<dependency>"]);
			xml.line(2, &["<dependentAssembly>"]);
			xml.identity(3, identity);
			xml.line(2, &["</dependentAssembly>"]);
			xml.line(1, &["</dependency>"]);
		}

		if let Some(level) = self.execution_level {
			let (level, ui_access) = match level {
				ExecutionLevel::AsInvoker { ui_access } => ("asInvoker", ui_access),
				ExecutionLevel::HighestAvailable { ui_access } => ("highestAvailable", ui_access),
				ExecutionLevel::RequireAdministrator { ui_access } => ("requireAdministrator", ui_access),
			};
			xml.line(1, &[r#"<trustInfo xmlns=""#, ASM_V3, r#"">"#]);
			xml.line(2, &["<security>"]);
			xml.line(3, &["<requestedPrivileges>"]);
			xml.line(
				4,
				&[
					r#"<requestedExecutionLevel level=""#,
					level,
					r#"" uiAccess=""#,
					boolean(ui_access),
					r#""/>"#,
				],
			);
			xml.line(3, &["</requestedPrivileges>"]);
			xml.line(2, &["</security>"]);
			xml.line(1, &["</trustInfo>"]);
		}

		if !self.supported_os.is_empty() || self.max_version_tested.is_some() {
			xml.line(1, &[r#"<compatibility xmlns="urn:schemas-microsoft-com:compatibility.v1">"#]);
			xml.line(2, &["<application>"]);
			if let Some(version) = self.max_version_tested {
				xml.line(3, &[r#"<maxversiontested Id=""#, &version.to_string(), r#""/>"#]);
			}
			for os in &self.supported_os {
				xml.line(3, &[r#"<supportedOS Id=""#, os.id(), r#""/>"#]);
			}
			xml.line(2, &["</application>"]);
			xml.line(1, &["</compatibility>"]);
		}

		let typed = self.typed_settings();
		if !typed.is_empty() || !self.windows_settings.is_empty() {
			xml.line(1, &[r#"<application xmlns=""#, ASM_V3, r#"">"#]);
			xml.line(2, &["<windowsSettings>"]);
			for (namespace, name, value) in typed {
				xml.line(3, &["<", name, r#" xmlns=""#, namespace, r#"">"#, value, "</", name, ">"]);
			}
			for (namespace, name, value) in &self.windows_settings {
				let (namespace, value) = (escaped(namespace), escaped(value));
				xml.line(3, &["<", name, r#" xmlns=""#, &namespace, r#"">"#, &value, "</", name, ">"]);
			}
			xml.line(2, &["</windowsSettings>"]);
			xml.line(1, &["</application>"]);
		}

		xml.raw(&self.postfix);
		xml.line(0, &["</assembly>"]);
		xml.push("\n");
		xml.0
	}

	/// Returns namespace, name and value of every typed windows setting; none of them needs escaping.
	#[must_use]
	fn typed_settings(&self) -> Vec<(&'static str, &'static str, &'static str)> {
		const SMI_2005: &str = "http://schemas.microsoft.com/SMI/2005/WindowsSettings";
		const SMI_2011: &str = "http://schemas.microsoft.com/SMI/2011/WindowsSettings";
		const SMI_2013: &str = "http://schemas.microsoft.com/SMI/2013/WindowsSettings";
		const SMI_2016: &str = "http://schemas.microsoft.com/SMI/2016/WindowsSettings";
		const SMI_2017: &str = "http://schemas.microsoft.com/SMI/2017/WindowsSettings";
		const SMI_2019: &str = "http://schemas.microsoft.com/SMI/2019/WindowsSettings";
		const SMI_2020: &str = "http://schemas.microsoft.com/SMI/2020/WindowsSettings";

		let mut settings = Vec::new();
		if self.theming == Theming::Unthemed {
			settings.push((SMI_2005, "disableTheming", "true"));
		}
		if let Some(value) = self.disable_window_filtering {
			settings.push((SMI_2011, "disableWindowFiltering", boolean(value)));
		}
		if let Some(awareness) = self.dpi_awareness {
			let (legacy, current, gdi_scaling) = match awareness {
				DpiAwareness::Unaware { gdi_scaling } => ("false", "Unaware", gdi_scaling),
				DpiAwareness::System { gdi_scaling } => ("true", "System", gdi_scaling),
				DpiAwareness::PerMonitor => ("true/pm", "PerMonitor", false),
				DpiAwareness::PerMonitorV2 => ("true/pm", "PerMonitorV2, PerMonitor", false),
			};
			settings.push((SMI_2005, "dpiAware", legacy));
			settings.push((SMI_2016, "dpiAwareness", current));
			if gdi_scaling {
				settings.push((SMI_2017, "gdiScaling", "true"));
			}
		}
		if let Some(awareness) = self.scrolling_awareness {
			let name = match awareness {
				ScrollingAwareness::HighResolution => "highResolutionScrollingAware",
				ScrollingAwareness::UltraHighResolution => "ultraHighResolutionScrollingAware",
			};
			settings.push((SMI_2013, name, "true"));
		}
		if let Some(value) = self.printer_driver_isolation {
			settings.push((SMI_2011, "printerDriverIsolation", boolean(value)));
		}
		if let Some(value) = self.long_path_aware {
			settings.push((SMI_2016, "longPathAware", boolean(value)));
		}
		if let Some(code_page) = self.active_code_page {
			let value = match code_page {
				ActiveCodePage::Utf8 => "UTF-8",
				ActiveCodePage::Legacy => "Legacy",
			};
			settings.push((SMI_2019, "activeCodePage", value));
		}
		match self.heap_type {
			HeapType::Nt => {},
			HeapType::Segment => settings.push((SMI_2020, "heapType", "SegmentHeap")),
		}
		settings
	}
}

impl ResourceFile {
	/// Inserts a manifest as `RT_MANIFEST` resource under the name of its kind, in its language.
	///
	/// The text moves into the file without a copy.
	pub fn insert_manifest(&mut self, manifest: Manifest) -> Result<(), InsertError> {
		let key = ResourceKey::new(RT_MANIFEST, manifest.kind.resource_id(), manifest.language);
		let data = manifest.text.into_bytes().into_boxed_slice();
		self.insert_entries(vec![NewEntry::new(key, MOVEABLE | PURE, data)])
	}
}

/// Why a manifest could not be created.
#[derive(Clone, Debug, Eq, Error, PartialEq)]
pub enum ManifestError {
	/// The text contains a document type declaration.
	#[error("Manifest contains a document type declaration")]
	DocumentType,

	/// The text is not well-formed XML; `message` names line and column.
	#[error("Manifest is not well-formed XML: {message}")]
	Malformed { message: String },

	/// The XML declaration names another version than 1.0 or another encoding than UTF-8.
	#[error("XML declaration is invalid: {reason}")]
	InvalidDeclaration { reason: &'static str },

	/// An assembly name contains other characters than ASCII letters, digits, `.`, `-` and `_`.
	#[error("Assembly name {name:?} is not ASCII letters, digits, '.', '-' and '_' without a '.' at either end")]
	InvalidAssemblyName { name: String },

	/// A public key token is not 16 hexadecimal digits.
	#[error("Public key token {token:?} is not 16 hexadecimal digits")]
	InvalidPublicKeyToken { token: String },

	/// A typed text contains a character XML 1.0 does not allow.
	#[error("The {field} contains {character:?} at byte {offset}, which XML 1.0 does not allow")]
	InvalidCharacter {
		field: &'static str,
		offset: usize,
		character: char,
	},

	/// A text is not in Unicode normalization form C; `offset` is the first byte that differs.
	#[error("The {field} is not in Unicode normalization form C from byte {offset} on")]
	NotNormalized { field: &'static str, offset: usize },
}

const ASM_V1: &str = "urn:schemas-microsoft-com:asm.v1";

/// An assembly identity, the program's own or that of a dependency.
#[derive(Clone, Debug)]
struct Identity {
	name: String,
	version: Version,
	architecture: Architecture,
	public_key_token: Option<String>,
}

impl Identity {
	/// Checks name and token, which are ASCII and therefore in normalization form C.
	fn check(&self) -> Result<(), ManifestError> {
		let name = &self.name;
		let valid_name = !name.is_empty()
			&& !name.starts_with('.')
			&& !name.ends_with('.')
			&& name
				.bytes()
				.all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'-' | b'_'));
		if !valid_name {
			return Err(ManifestError::InvalidAssemblyName {
				name: <String>::clone(name),
			});
		}
		if let Some(token) = &self.public_key_token
			&& !(token.len() == 16 && token.bytes().all(|byte| byte.is_ascii_hexdigit()))
		{
			return Err(ManifestError::InvalidPublicKeyToken {
				token: <String>::clone(token),
			});
		}
		Ok(())
	}
}

/// The manifest text under construction, one element per line.
struct Xml(String);

impl Xml {
	fn push(&mut self, text: &str) {
		self.0.push_str(text);
	}

	fn line(&mut self, depth: usize, parts: &[&str]) {
		self.0.push('\n');
		self.0.extend(iter::repeat_n('\t', depth));
		for part in parts {
			self.0.push_str(part);
		}
	}

	/// Writes a checked identity, whose name and token need no escaping.
	fn identity(&mut self, depth: usize, identity: &Identity) {
		let version = identity.version.to_string();
		self.line(
			depth,
			&[
				r#"<assemblyIdentity type="win32" name=""#,
				&identity.name,
				r#"" version=""#,
				&version,
				r#"" processorArchitecture=""#,
				identity.architecture.as_str(),
				"\"",
			],
		);
		if let Some(token) = &identity.public_key_token {
			self.push(r#" publicKeyToken=""#);
			self.push(token);
			self.push(r#"" language="*""#);
		}
		self.push("/>");
	}

	fn raw(&mut self, xml: &str) {
		let xml = xml.trim_end_matches('\n');
		if !xml.is_empty() {
			self.0.push('\n');
			self.0.push_str(xml);
		}
	}
}

#[must_use]
const fn boolean(value: bool) -> &'static str {
	if value { "true" } else { "false" }
}

/// Escapes the characters with a meaning in XML text and attribute values.
#[must_use]
fn escaped(text: &str) -> Cow<'_, str> {
	if !text.contains(['&', '<', '>', '\'', '"']) {
		return Cow::Borrowed(text);
	}
	let mut out = String::with_capacity(text.len().saturating_add(16));
	for character in text.chars() {
		match character {
			'&' => out.push_str("&amp;"),
			'<' => out.push_str("&lt;"),
			'>' => out.push_str("&gt;"),
			'\'' => out.push_str("&apos;"),
			'"' => out.push_str("&quot;"),
			other => out.push(other),
		}
	}
	Cow::Owned(out)
}

/// Checks that a typed text consists of XML characters in normalization form C.
fn check_text(field: &'static str, text: &str) -> Result<(), ManifestError> {
	if let Some((offset, character)) = text.char_indices().find(|&(_, character)| !is_xml_char(character)) {
		return Err(ManifestError::InvalidCharacter { field, offset, character });
	}
	match nfc_violation(text) {
		Some(offset) => Err(ManifestError::NotNormalized { field, offset }),
		None => Ok(()),
	}
}

/// Checks that `text` is well-formed XML 1.0 in UTF-8 without DTD and in normalization form C.
fn check(text: &str) -> Result<(), ManifestError> {
	let options = ParsingOptions {
		allow_dtd: false,
		..ParsingOptions::default()
	};
	Document::parse_with_options(text, options).map_err(|err| match err {
		XmlError::DtdDetected => ManifestError::DocumentType,
		other => ManifestError::Malformed {
			message: other.to_string(),
		},
	})?;
	check_declaration(text)?;
	match nfc_violation(text) {
		Some(offset) => Err(ManifestError::NotNormalized { field: "manifest", offset }),
		None => Ok(()),
	}
}

/// Checks version and encoding of the XML declaration, which roxmltree only checks for its syntax.
fn check_declaration(text: &str) -> Result<(), ManifestError> {
	let text = text.strip_prefix('\u{FEFF}').unwrap_or(text);
	let Some((declaration, _)) = text
		.strip_prefix("<?xml")
		.filter(|rest| rest.starts_with(is_xml_whitespace))
		.and_then(|rest| rest.split_once("?>"))
	else {
		return Ok(());
	};
	// The syntax is checked, so no name and no value contains whitespace
	let compact: String = declaration.chars().filter(|&character| !is_xml_whitespace(character)).collect();
	let mut rest = compact.as_str();
	while let Some((name, tail)) = rest.split_once('=') {
		let Some((value, after)) = tail
			.strip_prefix('"')
			.and_then(|value| value.split_once('"'))
			.or_else(|| tail.strip_prefix('\'').and_then(|value| value.split_once('\'')))
		else {
			break;
		};
		match name {
			"version" if value != "1.0" => {
				return Err(ManifestError::InvalidDeclaration {
					reason: "version must be 1.0",
				});
			},
			"encoding" if !value.eq_ignore_ascii_case("UTF-8") => {
				return Err(ManifestError::InvalidDeclaration {
					reason: "encoding must be UTF-8, the encoding the text is stored in",
				});
			},
			_ => {},
		}
		rest = after;
	}
	Ok(())
}

#[must_use]
const fn is_xml_char(character: char) -> bool {
	matches!(character, '\u{9}' | '\u{A}' | '\u{D}' | '\u{20}'..='\u{D7FF}' | '\u{E000}'..='\u{FFFD}' | '\u{10000}'..='\u{10FFFF}')
}

#[must_use]
const fn is_xml_whitespace(character: char) -> bool {
	matches!(character, ' ' | '\t' | '\r' | '\n')
}

#[cfg(test)]
mod tests {
	use super::*;
	use crate::resource::CoffMachine;

	const DECLARATION: &str = r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>"#;
	const MINIMAL: &str = concat!(
		r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>"#,
		r#"<assembly xmlns="urn:schemas-microsoft-com:asm.v1" manifestVersion="1.0"/>"#
	);

	#[must_use]
	fn with_declaration(declaration: &str) -> String {
		format!(r#"{declaration}<assembly xmlns="urn:schemas-microsoft-com:asm.v1" manifestVersion="1.0"/>"#)
	}

	#[must_use]
	fn malformed(text: &str) -> bool {
		matches!(check(text), Err(ManifestError::Malformed { .. }))
	}

	#[test]
	fn a_typical_manifest_is_accepted() {
		let manifest = r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<!-- Comments and whitespace may surround the root element -->
<assembly xmlns="urn:schemas-microsoft-com:asm.v1" manifestVersion="1.0" xmlns:asmv3="urn:schemas-microsoft-com:asm.v3">
  <assemblyIdentity type="win32" name="Example.App" version="1.2.3.0" processorArchitecture="*"/>
  <description>Example &amp; friends &#x263A;</description>
  <asmv3:application>
    <asmv3:windowsSettings xmlns:ws="http://schemas.microsoft.com/SMI/2016/WindowsSettings">
      <ws:dpiAwareness>PerMonitorV2</ws:dpiAwareness>
    </asmv3:windowsSettings>
  </asmv3:application>
  <![CDATA[ignored <data>]]>
</assembly>
"#;
		assert_eq!(check(manifest), Ok(()));
		let prefixed = r#"<asmv1:assembly xmlns:asmv1="urn:schemas-microsoft-com:asm.v1" manifestVersion="1.0"/>"#;
		assert_eq!(check(&format!("{DECLARATION}{prefixed}")), Ok(()));
		assert_eq!(check(&format!("\u{FEFF}{MINIMAL}")), Ok(()), "A byte order mark may precede it");
	}

	#[test]
	fn the_declaration_is_optional_and_follows_xml_1_0() {
		assert!(malformed(""), "A document needs a root element");
		assert!(malformed(&format!(" {MINIMAL}")), "The declaration comes first");
		for declaration in [
			"",
			"<?xml-stylesheet href='a'?>",
			r#"<?xml version="1.0"?>"#,
			r#"<?xml version="1.0" standalone="no"?>"#,
			r#"<?xml version="1.0" encoding="UTF-8"?>"#,
			"<?xml version='1.0' encoding='utf-8' standalone='yes' ?>",
		] {
			assert_eq!(check(&with_declaration(declaration)), Ok(()), "{declaration}");
		}

		let reason = |declaration: &str| match check(&with_declaration(declaration)) {
			Err(ManifestError::InvalidDeclaration { reason }) => Some(reason),
			_ => None,
		};
		assert_eq!(reason(r#"<?xml version="1.1"?>"#), Some("version must be 1.0"));
		assert_eq!(
			reason(r#"<?xml version = '1.0' encoding="UTF-16"?>"#),
			Some("encoding must be UTF-8, the encoding the text is stored in")
		);
		for broken in [
			"<?xml version=1.0?>",
			r#"<?xml encoding="UTF-8" version="1.0"?>"#,
			r#"<?xml version="1.0" standalone="yes" encoding="UTF-8"?>"#,
			r#"<?xml version="1.0" extra="1"?>"#,
		] {
			assert!(malformed(&with_declaration(broken)), "{broken}");
		}
	}

	#[test]
	fn document_type_declarations_are_rejected() {
		let subset =
			format!(r#"{DECLARATION}<!DOCTYPE assembly [<!ENTITY e "x">]><assembly xmlns="{ASM_V1}" manifestVersion="1.0">&e;</assembly>"#);
		assert_eq!(check(&subset), Err(ManifestError::DocumentType));
		let bare = format!(r#"{DECLARATION}<!-- <!DOCTYPE in a comment --><!DOCTYPE assembly><assembly xmlns="{ASM_V1}"/>"#);
		assert_eq!(check(&bare), Err(ManifestError::DocumentType));
	}

	#[test]
	fn documents_that_are_not_well_formed_are_rejected() {
		#[must_use]
		fn with_body(body: &str) -> String {
			format!(r#"{DECLARATION}<assembly xmlns="urn:schemas-microsoft-com:asm.v1" manifestVersion="1.0">{body}</assembly>"#)
		}

		for body in [
			"<a></b>",
			"<a>",
			"<a x='1' x='2'/>",
			"<a x=1/>",
			"<a x='<'/>",
			"&undefined;",
			"&#0;",
			"text ]]> text",
			"<1a/>",
			"<p:a/>",
			"<a p:x='1'/>",
			"<!-- a -- b -->",
			"\u{1}",
			"\u{FFFE}",
		] {
			assert!(malformed(&with_body(body)), "{body:?}");
		}
		assert!(malformed(&format!("{MINIMAL}<assembly/>")), "Second root element");
		assert!(malformed(&format!("{MINIMAL}text")), "Text behind the root element");
		assert!(malformed(&format!("{MINIMAL}{DECLARATION}")), "Second declaration");
		assert_eq!(check(&format!("{MINIMAL}\r\n<!-- end --><?pi data?>\n")), Ok(()));
		assert_eq!(
			check(&format!("{DECLARATION}\n<a>\n  <b></c>")),
			Err(ManifestError::Malformed {
				message: "expected 'b' tag, not 'c' at 3:6".to_owned(),
			}),
			"The message names line and column"
		);
	}

	#[test]
	fn any_root_element_is_well_formed_xml() {
		for root in [
			r#"<assembly manifestVersion="2.0"/>"#,
			"<other/>",
			r#"<assembly xmlns="urn:schemas-microsoft-com:asm.v1"/>"#,
		] {
			assert_eq!(check(&format!("{DECLARATION}{root}")), Ok(()), "{root}");
		}
		assert!(malformed(&format!("{DECLARATION}<!-- only a comment -->")));
	}

	#[test]
	fn manifests_are_stored_byte_for_byte_under_the_name_of_their_kind() {
		assert_eq!(ManifestKind::Process.resource_id().get(), 1);
		assert_eq!(ManifestKind::IsolationAware.resource_id().get(), 2);
		assert_eq!(ManifestKind::IsolationAwareNoStaticImport.resource_id().get(), 3);

		let german = LanguageId::GERMAN;
		let manifest = || {
			Manifest::options()
				.kind(ManifestKind::IsolationAware)
				.language(german)
				.create(MINIMAL)
				.unwrap()
		};
		let mut file = ResourceFile::new();
		file.insert_manifest(manifest()).unwrap();
		let key = ResourceKey::new(RT_MANIFEST, NonZeroU16::new(2).unwrap(), german);
		assert_eq!(file.entry(key), Some((MOVEABLE | PURE, MINIMAL.as_bytes())));
		assert_eq!(
			file.insert_manifest(manifest()),
			Err(InsertError::DuplicateResource {
				resource_type: 24,
				name: 2,
				language: 0x0407
			})
		);
		assert_ne!(file.to_coff(CoffMachine::X64), []);
	}

	const STANDARD: &str = concat!(
		"<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?>\n",
		"<assembly xmlns=\"urn:schemas-microsoft-com:asm.v1\" manifestVersion=\"1.0\">\n",
		"\t<dependency>\n",
		"\t\t<dependentAssembly>\n",
		"\t\t\t<assemblyIdentity type=\"win32\" name=\"Microsoft.Windows.Common-Controls\"",
		" version=\"6.0.0.0\" processorArchitecture=\"*\" publicKeyToken=\"6595b64144ccf1df\"",
		" language=\"*\"/>\n",
		"\t\t</dependentAssembly>\n",
		"\t</dependency>\n",
		"\t<trustInfo xmlns=\"urn:schemas-microsoft-com:asm.v3\">\n",
		"\t\t<security>\n",
		"\t\t\t<requestedPrivileges>\n",
		"\t\t\t\t<requestedExecutionLevel level=\"asInvoker\" uiAccess=\"false\"/>\n",
		"\t\t\t</requestedPrivileges>\n",
		"\t\t</security>\n",
		"\t</trustInfo>\n",
		"\t<compatibility xmlns=\"urn:schemas-microsoft-com:compatibility.v1\">\n",
		"\t\t<application>\n",
		"\t\t\t<supportedOS Id=\"{8e0f7a12-bfb3-4fe8-b9a5-48fd50a15a9a}\"/>\n",
		"\t\t</application>\n",
		"\t</compatibility>\n",
		"\t<application xmlns=\"urn:schemas-microsoft-com:asm.v3\">\n",
		"\t\t<windowsSettings>\n",
		"\t\t\t<dpiAware xmlns=\"http://schemas.microsoft.com/SMI/2005/WindowsSettings\">true/pm</dpiAware>\n",
		"\t\t\t<dpiAwareness xmlns=\"http://schemas.microsoft.com/SMI/2016/WindowsSettings\">PerMonitorV2,",
		" PerMonitor</dpiAwareness>\n",
		"\t\t\t<activeCodePage",
		" xmlns=\"http://schemas.microsoft.com/SMI/2019/WindowsSettings\">UTF-8</activeCodePage>\n",
		"\t\t</windowsSettings>\n",
		"\t</application>\n",
		"</assembly>\n",
	);

	#[must_use]
	fn text(options: StandardManifestOptions) -> String {
		options.create().unwrap().text().to_owned()
	}

	#[test]
	fn the_default_standard_manifest_is_the_common_one_with_the_nt_heap() {
		assert_eq!(text(Manifest::standard()), STANDARD);
		assert_eq!(
			text(Manifest::standard().heap_type(HeapType::Nt)),
			STANDARD,
			"The NT heap needs no setting"
		);
		let heap = text(Manifest::standard().heap_type(HeapType::Segment));
		assert!(heap.contains(
			"\t\t\t<heapType xmlns=\"http://schemas.microsoft.com/SMI/2020/WindowsSettings\">SegmentHeap</heapType>\n\t\t</windowsSettings>"
		));
	}

	#[test]
	fn standard_parts_can_be_changed_and_are_escaped() {
		let text = text(
			Manifest::standard()
				.assembly_identity("Tom.Jerry-App_2", Version::new(1, 2, 3, 4), Architecture::Amd64)
				.description("Tom & Jerry's <app>")
				.add_dependency("A.B", Version::new(2, 0, 0, 0), Some("0123456789abcdef"), Architecture::Arm64)
				.execution_level(Some(ExecutionLevel::RequireAdministrator { ui_access: true }))
				.compatibility(&[SupportedOs::Windows7, SupportedOs::Windows7], Some(Version::new(10, 0, 22621, 0)))
				.dpi_awareness(Some(DpiAwareness::Unaware { gdi_scaling: true }))
				.theming(Theming::Unthemed)
				.add_windows_setting("urn:a\"b", "note", "x & 'y'"),
		);
		for expected in [
			r#"<assemblyIdentity type="win32" name="Tom.Jerry-App_2" version="1.2.3.4" processorArchitecture="amd64"/>"#,
			"<description>Tom &amp; Jerry&apos;s &lt;app&gt;</description>",
			r#"name="A.B" version="2.0.0.0" processorArchitecture="arm64" publicKeyToken="0123456789abcdef" language="*"/>"#,
			r#"<requestedExecutionLevel level="requireAdministrator" uiAccess="true"/>"#,
			concat!(
				"<maxversiontested Id=\"10.0.22621.0\"/>\n",
				"\t\t\t<supportedOS Id=\"{35138b9a-5d96-4fbd-8e2d-a2440225f93a}\"/>\n\t\t</application>"
			),
			">false</dpiAware>",
			">Unaware</dpiAwareness>",
			">true</gdiScaling>",
			">true</disableTheming>",
			r#"<note xmlns="urn:a&quot;b">x &amp; &apos;y&apos;</note>"#,
		] {
			assert!(text.contains(expected), "{expected} missing in\n{text}");
		}
	}

	#[test]
	fn typed_settings_are_checked_in_full() {
		let create = |options: StandardManifestOptions| options.create().map(|_| ());
		for name in ["", ".a", "a.", "a b", "Tom \"Jerry\"", "\u{E9}"] {
			assert_eq!(
				create(Manifest::standard().assembly_identity(name, Version::default(), Architecture::Any)),
				Err(ManifestError::InvalidAssemblyName { name: name.to_owned() })
			);
		}
		for token in ["", "0123456789abcde", "0123456789abcdefa", "<token>0123456"] {
			assert_eq!(
				create(Manifest::standard().add_dependency("A", Version::default(), Some(token), Architecture::Any)),
				Err(ManifestError::InvalidPublicKeyToken { token: token.to_owned() })
			);
		}
		assert_eq!(
			create(Manifest::standard().description("a\u{1}")),
			Err(ManifestError::InvalidCharacter {
				field: "description",
				offset: 1,
				character: '\u{1}'
			})
		);
	}

	#[test]
	fn every_manifest_is_in_normalization_form_c() {
		assert!(Manifest::standard().description("Caf\u{E9}").create().is_ok());
		assert_eq!(
			Manifest::standard().description("Cafe\u{301}").create().map(|_| ()),
			Err(ManifestError::NotNormalized {
				field: "description",
				offset: 3
			})
		);
		// U+0338 alone is in NFC, but composes with the '>' of the tag before it to U+226F
		let tag_end = text(Manifest::standard().description("x")).find("<description>").unwrap() + 12;
		assert_eq!(
			Manifest::standard().description("\u{338}").create().map(|_| ()),
			Err(ManifestError::NotNormalized {
				field: "manifest",
				offset: tag_end
			})
		);
		let value_at = text(Manifest::standard().add_windows_setting("urn:a", "b", "x"))
			.find(">x</b>")
			.unwrap() + 1;
		assert_eq!(
			Manifest::standard()
				.add_windows_setting("urn:a", "b", "e\u{301}")
				.create()
				.map(|_| ()),
			Err(ManifestError::NotNormalized {
				field: "manifest",
				offset: value_at
			}),
			"Custom settings and raw XML are checked as part of the text"
		);
		let raw = format!("{DECLARATION}<assembly>\u{212B}</assembly>");
		assert_eq!(
			Manifest::options().create(raw).map(|_| ()),
			Err(ManifestError::NotNormalized {
				field: "manifest",
				offset: DECLARATION.len() + 10
			})
		);
	}

	#[test]
	fn the_dll_manifest_has_visual_styles_only() {
		const DLL: &str = concat!(
			"<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?>\n",
			"<assembly xmlns=\"urn:schemas-microsoft-com:asm.v1\" manifestVersion=\"1.0\">\n",
			"\t<dependency>\n",
			"\t\t<dependentAssembly>\n",
			"\t\t\t<assemblyIdentity type=\"win32\" name=\"Microsoft.Windows.Common-Controls\"",
			" version=\"6.0.0.0\" processorArchitecture=\"*\" publicKeyToken=\"6595b64144ccf1df\"",
			" language=\"*\"/>\n",
			"\t\t</dependentAssembly>\n",
			"\t</dependency>\n",
			"</assembly>\n",
		);
		let manifest = Manifest::standard_dll().create().unwrap();
		assert_eq!((manifest.kind(), manifest.text()), (ManifestKind::IsolationAware, DLL));
	}

	#[test]
	fn custom_windows_settings_are_only_checked_as_xml() {
		let setting = |namespace: &str, name: &str| Manifest::standard().add_windows_setting(namespace, name, "1").create();
		for name in ["not a name", "ws:prefixed", ""] {
			assert!(
				matches!(setting("urn:example", name), Err(ManifestError::Malformed { .. })),
				"{name}"
			);
		}
		let smi_2016 = "http://schemas.microsoft.com/SMI/2016/WindowsSettings";
		assert!(setting(smi_2016, "dpiAwareness").is_ok(), "A repeated setting is no XML error");
		assert!(matches!(setting("urn:\u{1}", "a"), Err(ManifestError::Malformed { .. })));
	}

	#[test]
	fn standard_parts_can_be_left_out_and_extended() {
		let empty = text(
			Manifest::standard()
				.theming(Theming::Classic)
				.execution_level(None)
				.compatibility(&[], None)
				.disable_window_filtering(None)
				.dpi_awareness(None)
				.long_path_aware(None)
				.active_code_page(None),
		);
		assert_eq!(
			empty,
			format!("{DECLARATION}\n<assembly xmlns=\"{ASM_V1}\" manifestVersion=\"1.0\">\n</assembly>\n")
		);

		let extended = text(
			Manifest::standard()
				.prefix("\t<file name=\"helper.dll\"/>")
				.postfix("\t<!-- generated -->\n"),
		);
		assert!(extended.find("<file name") < extended.find("<dependency>"));
		assert!(extended.ends_with("\t<!-- generated -->\n</assembly>\n"));

		let malformed = |options: StandardManifestOptions| matches!(options.create(), Err(ManifestError::Malformed { .. }));
		assert!(malformed(Manifest::standard().prefix("<file>")));
		assert!(malformed(Manifest::standard().postfix("</assembly><assembly>")));
		assert!(malformed(Manifest::standard().prefix("&undefined;")));
	}
}
