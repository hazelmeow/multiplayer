use std::cell::RefCell;
use std::rc::Rc;

use fltk::draw;
use fltk::enums::*;
use fltk::prelude::*;
use fltk::widget;
use fltk::widget_extends;

pub struct Visualizer {
    inner: widget::Widget,
    values: Rc<RefCell<[u8; 14]>>, // 14 bars (height 0 to 8)
    prev_vals: [u8; 14],
    timing: bool,
}

widget_extends!(Visualizer, widget::Widget, inner);

impl Visualizer {
    pub fn new(x: i32, y: i32) -> Self {
        let mut inner = widget::Widget::default().with_size(41, 20).with_pos(x, y);
        inner.set_frame(FrameType::FlatBox);

        let values = [5; 14];
        let values = Rc::from(RefCell::from(values));
        let v = values.clone();

        inner.draw(move |i| {
            draw::draw_box(i.frame(), i.x(), i.y(), i.w(), i.h(), i.color());
            draw::set_draw_color(Color::Black);
            draw::draw_xyline(i.x(), i.y() + 16, i.x() + (14 * 3) - 2);
            for bar in 0..14 {
                let value = v.borrow()[bar] as i32;
                for val in 0..value {
                    let x = i.x() + bar as i32 * 3;
                    let y = i.y() + (7 - val) * 2;
                    draw::draw_xyline(x, y, x + 1);
                }
            }
        });

        Self {
            inner,
            values,
            timing: false,
            prev_vals: [0; 14],
        }
    }
    pub fn update_values(&mut self, values: [u8; 14]) {
        {
            let prev = self.values.borrow().clone();
            let mut sv = self.values.borrow_mut();
            let temp = sv.clone();
            for i in 0..14 {
                if values[i] > prev[i] {
                    sv[i] = ((prev[i] as usize + self.prev_vals[i] as usize + values[i] as usize)
                        / 3 as usize) as u8;
                } else if sv[i] > 1 {
                    if self.timing {
                        sv[i] = sv[i] - 1;
                    }
                } else {
                    sv[i] = 0;
                }
            }
            self.timing = !self.timing;
            self.prev_vals = temp;
        }
        self.redraw();
    }
}
