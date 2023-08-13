use fltk::app;
use fltk::draw;
use fltk::draw::measure;
use fltk::enums::*;
use fltk::group::Group;
use fltk::prelude::*;
use fltk::widget_extends;

pub struct GroupBox {
    inner: Group,
}

widget_extends!(GroupBox, Group, inner);

impl GroupBox {
    pub fn new(x: i32, y: i32, w: i32, h: i32) -> Self {
        let mut inner = Group::default().with_pos(x, y).with_size(w, h);
        inner.set_label_type(LabelType::None);

        inner.draw(|g| {
            draw::set_draw_color(Color::Black);
            draw::draw_box(
                FrameType::BorderFrame,
                g.x(),
                g.y(),
                g.w(),
                g.h(),
                Color::from_rgb(100, 100, 100),
            );

            let text_w = measure(&g.label(), true);

            draw::draw_box(
                FrameType::FlatBox,
                g.x() + 8,
                g.y() - text_w.1 / 2,
                text_w.0 + 4,
                text_w.1,
                g.color(),
            );
            draw::set_draw_color(Color::Black);
            draw::set_font(Font::Helvetica, app::font_size());
            draw::draw_text(&g.label(), g.x() + 10, g.y() + text_w.1 / 2 - 3);
            g.draw_children();
        });
        Self { inner }
    }
}
