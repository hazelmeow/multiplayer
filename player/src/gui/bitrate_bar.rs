use std::cell::RefCell;
use std::rc::Rc;

use fltk::draw;
use fltk::enums::*;
use fltk::image::PngImage;
use fltk::prelude::*;
use fltk::widget;
use fltk::widget_extends;

pub struct BitrateBar {
    inner: widget::Widget,
    bitrate: Rc<RefCell<u32>>,
    buffer_level: Rc<RefCell<u8>>, // 0 to 6
}

widget_extends!(BitrateBar, widget::Widget, inner);

impl BitrateBar {
    pub fn new(x: i32, y: i32) -> Self {
        let mut inner = widget::Widget::default().with_size(35, 14).with_pos(x, y);
        inner.set_frame(FrameType::FlatBox);

        let bitrate = 0;
        let bitrate_c = Rc::from(RefCell::from(bitrate));
        let bitrate_c_c = bitrate_c.clone();

        let buffer_level = 0;
        let buffer_level_c = Rc::from(RefCell::from(buffer_level));
        let buffer_level_c_c = buffer_level_c.clone();

        let mut font = PngImage::from_data(include_bytes!("../../rsrc/font.png")).unwrap();

        let mut buf_bar = PngImage::from_data(include_bytes!("../../rsrc/bufbar.png")).unwrap();

        inner.draw(move |i| {
            draw::draw_box(i.frame(), i.x(), i.y(), i.w(), i.h(), i.color());
            draw::set_draw_color(Color::Black);
            let br = bitrate_c_c.borrow();
            let br_chars = format!("{}k", br);
            let mut x = 0;
            for c in br_chars.chars().rev() {
                let (cx, w) = Self::char_offset(c);
                font.draw_ext(i.x() + i.width() - x - w, i.y(), w - 1, 5, cx, 0);
                x += w - 1;
            }

            let bl = buffer_level_c_c.borrow();
            buf_bar.draw(i.x(), i.y() + 6, 34, 4);
            if *bl > 0 {
                let w = (*bl as i32 * 5).min(30);
                draw::draw_rect_fill(i.x() + 1, i.y() + 7, w + 1, 2, Color::Black);
            }
        });

        Self {
            inner,
            bitrate: bitrate_c,
            buffer_level: buffer_level_c,
        }
    }
    fn char_offset(c: char) -> (i32, i32) {
        match c {
            // x-pos, width
            '0' => (0, 7),
            '1' => (7, 4),
            '2' => (12, 7),
            '3' => (19, 7),
            '4' => (26, 7),
            '5' => (33, 6),
            '6' => (40, 6),
            '7' => (47, 6),
            '8' => (54, 6),
            '9' => (61, 6),
            'k' => (68, 19), // "kbps"
            _ => (0, 0),
        }
    }
    pub fn update_bitrate(&mut self, bitrate: u32) {
        self.bitrate.replace(bitrate);
        self.redraw();
    }
    pub fn update_buffer_level(&mut self, level: u8) {
        self.buffer_level.replace(level);
        self.redraw();
    }
}
