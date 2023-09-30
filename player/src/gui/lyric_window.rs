use fltk::draw;
use fltk::enums::*;
use fltk::frame::Frame;
use fltk::prelude::*;
use fltk::widget_extends;
use fltk::window::*;
use std::cell::RefCell;
use std::rc::Rc;

pub struct LyricWindow {
    pub main_win: Window,
    pub lyric_display: LyricDisplay,
}

impl LyricWindow {
    pub fn make_window() -> Self {
        let mut main_win = Window::new(100, 100, 400, 24, "lyrics");
        main_win.set_frame(FrameType::UpBox);
        main_win.set_color(Color::Black);
        main_win.set_border(false);

        let lyric_display = LyricDisplay::new(main_win.w(), main_win.h());
        main_win.add(&*lyric_display);

        main_win.end();

        Self {
            main_win,
            lyric_display,
        }
    }
    pub fn position(&mut self, main_win: &Window) {
        self.main_win
            .set_pos(main_win.x(), main_win.y() + main_win.height());
    }
}

pub struct LyricDisplay {
    inner: Frame,
    width: i32,
    tick: usize,
    speed: i32,
    musicing: bool, // dimdenism
    blinking: bool,
    centered: Rc<RefCell<bool>>,
    offset: Rc<RefCell<i32>>,
}
widget_extends!(LyricDisplay, Frame, inner);
impl LyricDisplay {
    fn new(w: i32, h: i32) -> Self {
        let mut inner = Frame::new(4, 2, w - 6, h - 4, "no lyrics");

        let offset = 0;
        let offset_h = Rc::new(RefCell::new(offset));
        let offset_h2 = offset_h.clone();

        let centered = true;
        let centered_h = Rc::new(RefCell::new(centered));
        let centered_h2 = centered_h.clone();

        inner.draw(move |i| {
            let offset = offset_h.borrow();

            draw::push_clip(i.x(), i.y(), i.w(), i.h());
            draw::draw_box(FrameType::FlatBox, i.x(), i.y(), i.w(), i.h(), Color::Black);
            draw::set_draw_color(Color::from_rgb(221, 118, 4));
            draw::set_font(Font::Helvetica, 16);
            draw::draw_text2(
                &i.label(),
                i.x() - *offset,
                i.y(),
                i.w(),
                i.h(),
                if *centered_h.borrow() {
                    Align::Inside | Align::Center
                } else {
                    Align::Inside | Align::Left
                },
            );
            draw::pop_clip();
        });
        Self {
            offset: offset_h2,
            centered: centered_h2,
            width: 0,
            tick: 0,
            speed: 4,
            musicing: false,
            blinking: false,
            inner,
        }
    }
    pub fn update(&mut self, mut line: &str, centered: bool) {
        if line == self.inner.label() {
            return;
        }
        if line.chars().next().unwrap_or_default() == '♪' {
            line = "♪ ♪ ♪";
            self.musicing = true;
            *self.centered.borrow_mut() = true;
        } else {
            self.musicing = false;
            *self.centered.borrow_mut() = centered;
        }
        *self.offset.borrow_mut() = 0;
        self.tick = 0;
        self.blinking = false;
        draw::set_font(Font::Helvetica, 16);
        self.width = draw::measure(line, false).0;
        self.speed = self.calc(0);
        self.inner.set_label(line);
        self.inner.redraw();
    }
    pub fn clear(&mut self) {
        self.update("", false);
    }
    pub fn tick(&mut self) {
        if self.blinking {
            self.tick += 1;
            if self.tick % 2 != 0 {
                // label will ALWAYS be "♪ ♪ ♪" (surely)
                let f1 = "♪ ♪ ♪";
                let f2 = "＞＞ ♪ ♪ ♪ ＜＜";
                self.inner.set_label(if self.inner.label().as_str() == f1 {
                    f2
                } else {
                    f1
                });
                self.inner.redraw();
                return; // don't need to shift
            }
        }

        if !self.needs_scroll() {
            return;
        }
        if *self.offset.borrow() >= self.width - self.inner.w() {
            return;
        }

        self.tick += 1;

        if self.tick < 15 || self.tick % 3 != 0 {
            return;
        }
        *self.offset.borrow_mut() += self.speed;
        self.inner.redraw();
    }
    pub fn blink(&mut self) {
        self.blinking = true;
    }
    fn calc(&self, ms_until_next: usize) -> i32 {
        // perfectly calculate number of pixels to move it so it's timed nicely
        // dbg!(self.width, self.inner.w(), ms_until_next / 30);
        // let magic = ((self.width - self.inner.width()) as usize).saturating_sub(ms_until_next / 30);
        // dbg!(&magic);
        // magic as i32
        // idk couldn't get it to work but this seems okay
        10
    }
    fn needs_scroll(&self) -> bool {
        self.width > self.inner.width()
    }
    pub fn musicing(&self) -> bool {
        self.musicing
    }
}
