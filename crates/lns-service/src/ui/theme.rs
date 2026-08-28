use eframe::egui::Vec2;

/// Button metrics matching macOS standard (AppKit / SwiftUI bordered) controls; egui draws a circular corner, the closest it can get to macOS's continuous-curve corner.
pub const BUTTON_CORNER_RADIUS: u8 = 6;
pub const BUTTON_FONT_SIZE: f32 = 13.0;
pub const BUTTON_PADDING: Vec2 = Vec2::new(14.0, 7.0);
pub const BUTTON_MIN_HEIGHT: f32 = 28.0;

pub const FONT_EYEBROW: f32 = 11.0;
pub const FONT_EYEBROW_ICON: f32 = 13.0;
pub const FONT_BADGE: f32 = 11.0;

pub const BADGE_CORNER_RADIUS: u8 = 6;
pub const BADGE_PAD_X: i8 = 7;
pub const BADGE_PAD_Y: i8 = 2;
pub const BADGE_GAP: f32 = 6.0;

/// A chip is a control, so it takes a button's touch target rather than a badge's tight label box.
pub const CHIP_PAD_X: i8 = 10;
pub const CHIP_PAD_Y: i8 = 5;
pub const FONT_TITLE: f32 = 16.0;
pub const FONT_BODY: f32 = 13.0;
pub const FONT_CAPTION: f32 = 12.0;

pub const CARD_CORNER_RADIUS: u8 = 18;
pub const CARD_PADDING: i8 = 16;
pub const CARD_GAP: f32 = 12.0;
pub const STACK_MARGIN: i8 = 18;
/// Card fill opacity (0-255); below 255 the desktop shows through for a frosted-glass-ish look without a real backdrop blur.
pub const CARD_FILL_ALPHA: u8 = 250;
