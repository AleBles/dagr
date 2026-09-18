//! Color helpers: parse/format `#rrggbb` and draw the little priority dot.

use adw::prelude::*;
use gtk::gdk;

pub use crate::task::PALETTE;

pub fn parse(color: &str) -> gdk::RGBA {
    gdk::RGBA::parse(color).unwrap_or_else(|_| gdk::RGBA::new(0.6, 0.6, 0.6, 1.0))
}

pub fn to_hex(rgba: &gdk::RGBA) -> String {
    let channel = |v: f32| (v.clamp(0.0, 1.0) * 255.0).round() as u8;
    format!(
        "#{:02x}{:02x}{:02x}",
        channel(rgba.red()),
        channel(rgba.green()),
        channel(rgba.blue())
    )
}

/// A filled circle in the given color, used on task rows and in pickers.
pub fn dot(color: &str) -> gtk::DrawingArea {
    let rgba = parse(color);
    let area = gtk::DrawingArea::builder()
        .content_width(12)
        .content_height(12)
        .valign(gtk::Align::Center)
        .halign(gtk::Align::Center)
        .build();
    area.set_draw_func(move |_, cr, width, height| {
        let (w, h) = (f64::from(width), f64::from(height));
        cr.set_source_rgba(
            f64::from(rgba.red()),
            f64::from(rgba.green()),
            f64::from(rgba.blue()),
            f64::from(rgba.alpha()),
        );
        cr.arc(w / 2.0, h / 2.0, w.min(h) / 2.0, 0.0, std::f64::consts::TAU);
        let _ = cr.fill();
    });
    area
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hex_round_trips() {
        for hex in ["#e01b24", "#000000", "#ffffff", "#3584e4"] {
            assert_eq!(to_hex(&parse(hex)), hex);
        }
    }
}
