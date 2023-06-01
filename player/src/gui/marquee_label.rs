use std::cell::RefCell;
use std::rc::Rc;

use fltk::draw;
use fltk::enums::*;
use fltk::prelude::*;
use fltk::widget;
use fltk::widget_extends;

pub struct MarqueeLabel {
    inner: widget::Widget,
    offset: Rc<RefCell<i32>>,
    text: Rc<RefCell<String>>,
    start_timer: usize,
    each_timer: usize,
}

widget_extends!(MarqueeLabel, widget::Widget, inner);

impl MarqueeLabel {
    pub fn new(x: i32, y: i32, width: i32) -> Self {
        let mut inner = widget::Widget::default()
            .with_size(width, 16)
            .with_pos(x, y);
        inner.set_frame(FrameType::FlatBox);

        let offset = 0;
        let offset_h = Rc::new(RefCell::new(offset));
        let offset_h2 = offset_h.clone();

        let text = String::from("");
        let text_h = Rc::new(RefCell::new(text));
        let text_h2 = text_h.clone();

        inner.draw(move |i| {
            let mut offset = offset_h.borrow_mut();
            let label = text_h.borrow();
            let (text_width, _) = draw::measure(&label, false);

            let os_two = if text_width >= i.w() {
                text_width + 2
            } else {
                // we reallllllly don't care if the width is less
                *offset = 0;
                0
            };

            draw::push_clip(i.x(), i.y(), i.w(), i.h());
            draw::draw_box(i.frame(), i.x(), i.y(), i.w(), i.h(), i.color());
            draw::set_draw_color(Color::Black);
            draw::set_font(Font::Helvetica, 14);
            draw::draw_text(&*label, i.x() + *offset, i.y() + 12);

            draw::draw_text(&*label, i.x() + *offset + i.width() + os_two, i.y() + 12);
            draw::pop_clip();

            if *offset + i.width() + os_two <= 0 {
                *offset = 0;
            }
        });

        Self {
            inner,
            offset: offset_h2,
            text: text_h2,
            start_timer: 1,
            each_timer: 0,
        }
    }
    pub fn nudge(&mut self, offset: i32) {
        *self.offset.borrow_mut() += offset;
        self.redraw();
    }
    pub fn zero(&mut self) {
        *self.offset.borrow_mut() = 0;
        self.start_timer = 0;
        self.redraw();
    }
    pub fn set_text(&mut self, text: &String) {
        *self.text.borrow_mut() = text.to_owned();
        self.redraw();
    }
    pub fn waited_long_enough(&mut self) -> bool {
        const EACH_DELAY: usize = 4;
        const START_DELAY: usize = 25;

        let offset = *self.offset.borrow();

        if self.start_timer > 0 {
            if self.start_timer > START_DELAY {
                self.start_timer = 0;
                true
            } else {
                self.start_timer += 1;
                false
            }
        } else {
            if self.each_timer > EACH_DELAY {
                if offset == 0 {
                    self.start_timer = 1;
                    self.each_timer = 0;
                    false
                } else {
                    self.each_timer = 0;
                    true
                }
            } else {
                self.each_timer += 1;
                false
            }
        }
    }
}
