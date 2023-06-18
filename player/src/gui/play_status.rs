use std::cell::RefCell;
use std::rc::Rc;

use fltk::draw;
use fltk::enums::*;
use fltk::image::PngImage;
use fltk::prelude::*;
use fltk::widget;
use fltk::widget_extends;

#[derive(Clone, Copy)]
pub enum PlayStatusIcon {
    PlayTx,
    PlayRx,
    Pause,
    Stop
}

pub struct PlayStatus {
    inner: widget::Widget,
    icon: Rc<RefCell<PlayStatusIcon>>,
}

widget_extends!(PlayStatus, widget::Widget, inner);

impl PlayStatus {
    pub fn new(x: i32, y: i32) -> Self {
        let mut inner = widget::Widget::default()
            .with_size(14, 14)
            .with_pos(x, y);
        inner.set_frame(FrameType::FlatBox);

        let mut img_data = PngImage::from_data(include_bytes!("../../rsrc/play_status.png")).unwrap();

        let icon = Rc::new(RefCell::new(PlayStatusIcon::Stop));
        let icon_h = icon.clone();

        inner.draw(move |i| {
            draw::draw_box(i.frame(), i.x(), i.y(), i.w(), i.h(), i.color());
            let icon = icon_h.borrow();
            let cx = Self::icon_offset(*icon);
            img_data.draw_ext(x, y, 14, 14, cx, 0);
        });

        Self {
            inner,
            icon
        }
    }

    fn icon_offset(icon: PlayStatusIcon) -> i32 {
        match icon {
            PlayStatusIcon::PlayTx => 0,
            PlayStatusIcon::PlayRx => 16,
            PlayStatusIcon::Pause => 32,
            PlayStatusIcon::Stop => 48,
        }
    }

    pub fn update(&mut self, icon: PlayStatusIcon) {
        self.icon.replace(icon);
        self.redraw();
    }
}