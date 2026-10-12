//! SVG documents rendered with resvg, behind basic safeguards that use what the parsers offer:
//! at most 1 MiB of UTF-8, no DTD, at most 100000 XML nodes and no images or other external resources.
//! Text is not drawn, as resvg is built without fonts.
//! The rendering time of a document is not bounded; filters and patterns can make it render for long.

use super::codec::clear_transparent;
use super::{Hotspot, ImageError, ImageSize, pixel_under};
use resvg::tiny_skia::{Pixmap, Transform};
use resvg::usvg::{ImageHrefResolver, Options, Tree};
use roxmltree::{Document as XmlDocument, ParsingOptions};

pub(super) const MAX_SVG_SIZE: usize = 0x0010_0000;
const MAX_NODES: u32 = 100_000;
/// The `id` of the element whose bounding box centers on the hotspot of a cursor.
pub(super) const HOTSPOT_ID: &str = "hotspot";

/// A parsed square document.
pub(super) struct Document {
	tree: Tree,
	/// The edge length in user units of the canvas.
	edge: f32,
	/// The center of the hotspot element in canvas coordinates.
	hotspot: Option<(f32, f32)>,
}

impl Document {
	/// Parses and checks a document; `read_hotspot` looks for the hotspot element, which only cursors need.
	pub(super) fn parse(svg: &[u8], read_hotspot: bool) -> Result<Self, ImageError> {
		let invalid = |message: String| ImageError::InvalidSvg { message };
		if svg.len() > MAX_SVG_SIZE {
			return Err(ImageError::InputTooLarge {
				len: svg.len(),
				limit: MAX_SVG_SIZE,
			});
		}
		let text = str::from_utf8(svg).map_err(|_| invalid("the document is not UTF-8".to_owned()))?;
		let parsing = ParsingOptions {
			allow_dtd: false,
			nodes_limit: MAX_NODES,
			..ParsingOptions::default()
		};
		let xml = XmlDocument::parse_with_options(text, parsing).map_err(|err| invalid(err.to_string()))?;
		let options = Options {
			image_href_resolver: ImageHrefResolver {
				resolve_data: Box::new(|_, _, _| None),
				resolve_string: Box::new(|_, _| None),
			},
			..Options::default()
		};
		let tree = Tree::from_xmltree(&xml, &options).map_err(|err| invalid(err.to_string()))?;
		drop(xml);

		let (width, height) = (tree.size().width(), tree.size().height());
		if (width - height).abs() > width.max(height) * 1e-6 {
			return Err(invalid(format!(
				"the document is {width}x{height}, but icons and cursors must be square"
			)));
		}
		// usvg resolves viewBox, preserveAspectRatio and every transform into the absolute bounding box
		let hotspot = tree.node_by_id(HOTSPOT_ID).filter(|_| read_hotspot).map(|node| {
			let bounds = node.abs_bounding_box();
			(
				f32::midpoint(bounds.left(), bounds.right()),
				f32::midpoint(bounds.top(), bounds.bottom()),
			)
		});
		if let Some((x, y)) = hotspot
			&& !((0.0..width).contains(&x) && (0.0..height).contains(&y))
		{
			return Err(ImageError::InvalidSvgHotspot {
				reason: "the center of the element lies outside of the document",
			});
		}
		Ok(Self {
			tree,
			edge: width,
			hotspot,
		})
	}

	/// Returns the pixel under the hotspot element in the image of `size`, if the document has one.
	#[must_use]
	pub(super) fn hotspot(&self, size: ImageSize) -> Option<Hotspot> {
		let (x, y) = self.hotspot?;
		let scale = f64::from(size.get()) / f64::from(self.edge);
		Some(Hotspot::new(
			pixel_under(f64::from(x) * scale, size),
			pixel_under(f64::from(y) * scale, size),
		))
	}

	/// Renders the document to straight RGBA and clears the color of transparent pixels.
	pub(super) fn render(&self, size: ImageSize, image: usize) -> Result<Vec<u8>, ImageError> {
		let mut pixmap = Pixmap::new(size.pixels(), size.pixels()).ok_or(ImageError::Decode {
			image,
			message: "the pixmap cannot be allocated".to_owned(),
		})?;
		let scale = f32::from(size.get()) / self.edge;
		resvg::render(&self.tree, Transform::from_scale(scale, scale), &mut pixmap.as_mut());
		let mut pixels = pixmap.take_demultiplied();
		clear_transparent(&mut pixels);
		Ok(pixels)
	}
}
