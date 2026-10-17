//! Small Unicode TrueType renderer. No GUI toolkit, compositor or GPU needed.
use crate::config::{Config, Result, Theme};
use ab_glyph::{point, Font, FontArc, PxScale, ScaleFont};
use x11rb::{connection::Connection, protocol::xproto::*, rust_connection::RustConnection};
pub struct Renderer {
    font: FontArc,
    size: f32,
    pub advance: f32,
    msb: bool,
    depth: u8,
}
impl Renderer {
    pub fn new(c: &RustConnection, root: Window, cfg: &Config) -> Result<Self> {
        let path =
            if let Some(p) = &cfg.font_path {
                std::path::PathBuf::from(p)
            } else {
                let found = std::process::Command::new("fc-match")
                    .args([
                        "-f",
                        "%{file}",
                        "JetBrainsMono Nerd Font,DejaVu Sans Mono:style=Regular",
                    ])
                    .output()
                    .ok()
                    .filter(|o| o.status.success())
                    .map(|o| String::from_utf8_lossy(&o.stdout).to_string());
                std::path::PathBuf::from(found.unwrap_or_else(|| {
                    "/usr/share/fonts/truetype/dejavu/DejaVuSansMono.ttf".into()
                }))
            };
        let font =
            FontArc::try_from_vec(std::fs::read(path)?).map_err(|_| "Cannot load TrueType font")?;
        let depth = c.get_geometry(root)?.reply()?.depth;
        if ![24, 32].contains(&depth)
            || !c
                .setup()
                .pixmap_formats
                .iter()
                .any(|f| f.depth == depth && f.bits_per_pixel == 32)
        {
            return Err("nirubar currently requires a 24/32-bit TrueColor X screen".into());
        }
        let advance = font
            .as_scaled(PxScale::from(cfg.font_size))
            .h_advance(font.glyph_id('M'));
        Ok(Self {
            font,
            size: cfg.font_size,
            advance,
            msb: c.setup().image_byte_order == ImageOrder::MSB_FIRST,
            depth,
        })
    }
    pub fn width(&self, text: &str) -> i16 {
        text.chars()
            .map(|ch| {
                self.font
                    .as_scaled(self.size)
                    .h_advance(self.font.glyph_id(ch))
            })
            .sum::<f32>()
            .ceil() as i16
    }
    // X11 drawing target, geometry and paint are explicit to keep this renderer stateless.
    #[allow(clippy::too_many_arguments)]
    pub fn draw(
        &self,
        c: &RustConnection,
        win: Window,
        gc: Gcontext,
        x: i16,
        height: u16,
        available: u16,
        text: &str,
        fg: &str,
        bg: &str,
    ) -> Result<()> {
        if available == 0 {
            return Ok(());
        }
        let width = (self.width(text).max(0) as u16).min(available);
        if width == 0 {
            return Ok(());
        }
        let background = Theme::pixel(bg)?;
        let foreground = Theme::pixel(fg)?;
        let mut pixels = vec![background; usize::from(width) * usize::from(height)];
        let scaled = self.font.as_scaled(self.size);
        let baseline = (f32::from(height) - scaled.height()) / 2.0 + scaled.ascent();
        let mut cursor = 0.0;
        for ch in text.chars().filter(|ch| !ch.is_control()) {
            let id = self.font.glyph_id(ch);
            let mut glyph = id.with_scale(self.size);
            glyph.position = point(cursor, baseline);
            if let Some(outline) = self.font.outline_glyph(glyph) {
                let bounds = outline.px_bounds();
                outline.draw(|gx, gy, alpha| {
                    let px = gx as i32 + bounds.min.x as i32;
                    let py = gy as i32 + bounds.min.y as i32;
                    if px >= 0 && py >= 0 && px < i32::from(width) && py < i32::from(height) {
                        let blend = |shift: u32| -> u32 {
                            let b = (background >> shift) & 255;
                            let f = (foreground >> shift) & 255;
                            (b as f32 * (1.0 - alpha) + f as f32 * alpha).round() as u32
                        };
                        pixels[py as usize * usize::from(width) + px as usize] =
                            (blend(16) << 16) | (blend(8) << 8) | blend(0);
                    }
                });
            }
            cursor += scaled.h_advance(id);
            if cursor >= f32::from(width) {
                break;
            }
        }
        let bytes = pixels
            .iter()
            .flat_map(|p| {
                if self.msb {
                    p.to_be_bytes()
                } else {
                    p.to_le_bytes()
                }
            })
            .collect::<Vec<_>>();
        let rows = (60000 / (usize::from(width) * 4)).max(1);
        for (i, chunk) in bytes.chunks(rows * usize::from(width) * 4).enumerate() {
            c.put_image(
                ImageFormat::Z_PIXMAP,
                win,
                gc,
                width,
                (chunk.len() / (usize::from(width) * 4)) as u16,
                x,
                (i * rows) as i16,
                0,
                self.depth,
                chunk,
            )?;
        }
        Ok(())
    }
}
