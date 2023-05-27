use std::cell::RefCell;
use std::rc::Rc;

use fltk::app::Sender;
use fltk::browser::*;
use fltk::button::*;
use fltk::draw;
use fltk::enums::*;
use fltk::frame::*;
use fltk::group::Group;
use fltk::group::Pack;
use fltk::image;
use fltk::image::Image;
use fltk::image::PngImage;
use fltk::input::*;
use fltk::prelude::*;
use fltk::valuator::*;
use fltk::widget;
use fltk::widget_extends;
use fltk::window::*;

use crate::main;
use crate::UIEvent;

pub struct MainWindow {
    pub main_win: Window,
    pub seek_bar: HorNiceSlider,
    pub status_field: Group,
    pub status_right_display: Frame,
    pub users: Browser,
    pub lbl_time: Frame,
    pub lbl_title: Frame,
    pub lbl_data1: Frame,
    pub visualizer: Visualizer,
    pub bitrate_bar: BitrateBar,
    pub volume_slider: HorSlider,
    pub art_frame: Frame,
}
impl MainWindow {
    pub fn make_window(s: Sender<UIEvent>) -> Self {
        let mut main_win = Window::new(100, 100, 400, 190, "multiplayer :3");
        //main_win.set_border(false);
        main_win.set_frame(FrameType::UpBox);
        main_win.set_icon(Some(
            PngImage::from_data(include_bytes!("../rsrc/ryo.png")).unwrap(),
        ));

        /* let mut bar_frame = Frame::new(1,2,main_win.width()-2, 16, "");
        bar_frame.set_frame(FrameType::BorderFrame);
        bar_frame.set_color(Color::Background);
        main_win.add(&bar_frame); */

        Self::add_bar(&mut main_win, s.clone(), UIEvent::Quit, "multiplayer :3");

        // --- buttons ---
        let buttons_y = 130;
        let buttons_left = 30;
        let bp = 36 + 5;

        let mut btn_prev = Button::new(buttons_left, buttons_y, 36, 26, "");
        btn_prev.set_image(Some(
            PngImage::from_data(include_bytes!("../rsrc/btn_prev.png")).unwrap(),
        ));
        btn_prev.set_align(Align::ImageBackdrop);
        btn_prev.emit(s.clone(), UIEvent::BtnPrev);
        main_win.add(&btn_prev);

        let mut btn_play = Button::new(buttons_left + bp, buttons_y, 36, 26, "");
        btn_play.set_image(Some(
            PngImage::from_data(include_bytes!("../rsrc/btn_play.png")).unwrap(),
        ));
        btn_play.set_align(Align::ImageBackdrop);
        btn_play.emit(s.clone(), UIEvent::BtnPlay);
        main_win.add(&btn_play);

        let mut btn_pause = Button::new(buttons_left + bp * 2, buttons_y, 36, 26, "");
        btn_pause.set_image(Some(
            PngImage::from_data(include_bytes!("../rsrc/btn_pause.png")).unwrap(),
        ));
        btn_pause.set_align(Align::ImageBackdrop);
        btn_pause.emit(s.clone(), UIEvent::BtnPause);
        main_win.add(&btn_pause);

        let mut btn_stop = Button::new(buttons_left + bp * 3, buttons_y, 36, 26, "");
        btn_stop.set_image(Some(
            PngImage::from_data(include_bytes!("../rsrc/btn_stop.png")).unwrap(),
        ));
        btn_stop.set_align(Align::ImageBackdrop);
        btn_stop.emit(s.clone(), UIEvent::BtnStop);
        main_win.add(&btn_stop);

        let mut btn_next = Button::new(buttons_left + bp * 4, buttons_y, 36, 26, "");
        btn_next.set_image(Some(
            PngImage::from_data(include_bytes!("../rsrc/btn_next.png")).unwrap(),
        ));
        btn_next.set_align(Align::ImageBackdrop);
        btn_next.emit(s.clone(), UIEvent::BtnNext);
        main_win.add(&btn_next);

        // --- end buttons ---

        //let mut temp_input = Input::new(150, buttons_y, 120, 20, "");
        //main_win.add(&temp_input);

        let seek_bar = HorNiceSlider::new(14, buttons_y - 35, 270, 24, "");
        main_win.add(&seek_bar);

        let mut status_field = Group::new(
            1,
            main_win.height() - 21,
            main_win.width() - 4,
            20,
            "status",
        );
        status_field.set_align(Align::Left | Align::Inside);
        status_field.set_frame(FrameType::DownBox);

        let mut status_right_display = Frame::new(
            status_field.x(),
            status_field.y(),
            status_field.width() - 4,
            status_field.height(),
            "U.. Q..",
        );
        status_right_display.set_label_size(10);
        status_right_display.set_frame(FrameType::NoBox);
        status_right_display.set_align(Align::Right | Align::Inside);
        status_field.add(&status_right_display);

        status_field.end();
        main_win.add(&status_field);

        // --- display ---

        let mut display = Group::new(13, 31, 249, 53, "");
        display.set_image(Some(
            PngImage::from_data(include_bytes!("../rsrc/display_frame.png")).unwrap(),
        ));
        display.set_align(Align::ImageBackdrop);
        display.set_color(Color::Blue);

        let mut art_frame = Frame::new(228, 34, 31, 31, "");
        art_frame.set_frame(FrameType::DownBox);
        display.add(&art_frame);

        let mut lbl_title = Frame::new(105, 35, 150, 16, "nothing playing");
        lbl_title.set_align(Align::Left | Align::Inside);
        display.add(&lbl_title);

        let mut lbl_data1 = Frame::new(105, 50, 150, 16, "...");
        lbl_data1.set_label_size(10);
        lbl_data1.set_align(Align::Left | Align::Inside);
        display.add(&lbl_data1);

        let mut lbl_time = Frame::new(29, 35, 80, 16, "00:00");
        //lbl_time.set_label_font(Font::Courier);
        lbl_time.set_align(Align::Left | Align::Inside);
        //main_win.add(&lbl_time);

        let visualizer = Visualizer::new(19, 62);
        //visualizer.update_values([0, 1, 2, 3, 4, 5, 6, 7, 8, 7, 6, 5, 4, 3]);
        display.add(&*visualizer);

        let bitrate_bar = BitrateBar::new(67, 69);
        display.add(&*bitrate_bar);

        display.end();
        main_win.add(&display);

        // --- end of display ---

        let mut volume_slider = HorSlider::new(110, 70, 80, 13, "");
        volume_slider.set_bounds(0., 1.);
        volume_slider.set_step(0.01, 1);
        let tmp = s.clone();
        volume_slider.set_callback(move|vs| {
            tmp.send(UIEvent::VolumeSlider(Self::volume_scale(vs.value())));
        });
        main_win.add(&volume_slider);

        let mut users = Browser::new(main_win.width() - 85 - 18, 26, 85, 120, "Users");
        users.add("* not loaded :/");
        main_win.add(&users);

        let mut btn_connect = Button::new(main_win.width() - 85 - 46, 26, 24, 24, "Cn");
        //btn_connect.emit(sender, crate::UIEvent::Test("wow".to_string()));
        main_win.add(&btn_connect);

        let mut btn_queue = Button::new(main_win.width() - 85 - 46, 26 + 24 + 2, 24, 24, "Qu");
        btn_queue.emit(s.clone(), UIEvent::BtnQueue);
        main_win.add(&btn_queue);

        main_win.end();

        Self {
            main_win,
            seek_bar,
            status_field,
            status_right_display,
            users,
            lbl_time,
            lbl_title,
            lbl_data1,
            visualizer,
            bitrate_bar,
            volume_slider,
            art_frame,
        }
    }

    pub fn volume_scale(val: f64) -> f32 {
        val.powf(3.) as f32
    }

    pub fn fix_taskbar_after_show(&mut self) {
        // TODO: implement on other platforms? lmao
        unsafe {
            shitty_windows_only_hack(&mut self.main_win);
        }
    }
    pub fn add_bar(
        win: &mut DoubleWindow,
        s: Sender<UIEvent>,
        close_message: UIEvent,
        title: &str,
    ) {
        let mut bar = Group::new(4, 4, win.width() - 8, 17, "");
        let mut bar_bg = create_horizontal_gradient_frame(
            4,
            4,
            win.width() - 8,
            17,
            Color::from_rgb(56, 85, 145),
            Color::from_rgb(166, 202, 240),
        );
        let t = title.to_owned();
        let mut bar_title = Frame::new(8, 4, 100, 17, "").with_label(&t);
        bar_title.set_align(Align::Left | Align::Inside);
        bar_title.set_label_font(Font::HelveticaBold);
        bar_title.set_label_size(12);
        bar_title.set_label_color(Color::White);
        bar.add(&bar_title);

        bar_bg.set_frame(FrameType::FlatBox);

        win.add(&bar_bg);
        //bar.set_color(Color::from_rgb(56, 85, 145));

        let mut bar_btn_close = Button::new(win.width() - 4 - 18, 6, 16, 14, "");
        let ico_x = image::BmpImage::from_data(include_bytes!("../rsrc/close.bmp")).unwrap();
        bar_btn_close.set_image(Some(ico_x));
        bar_btn_close.set_align(Align::Center | Align::ImageBackdrop);
        //bar_btn_close.set_color(Color::White);
        //bar_btn_close.set_frame(FrameType::BorderBox);
        bar_btn_close.emit(s.clone(), close_message);
        bar.add(&bar_btn_close);

        bar.end();
        win.add(&bar);
    }
}

pub struct QueueWindow {
    pub main_win: Window,
    pub queue_browser: SelectBrowser,
}

impl QueueWindow {
    pub fn make_window(s: Sender<UIEvent>) -> Self {
        let mut main_win = Window::new(100, 100, 300, 400, "queue");
        main_win.set_frame(FrameType::UpBox);

        MainWindow::add_bar(&mut main_win, s.clone(), UIEvent::HideQueue, "Queue");
        main_win.set_border(false);

        let mut main_grp = Group::new(0, 24, main_win.width(), main_win.height() - 21, "");

        let mut queue_browser =
            SelectBrowser::new(10, 24, main_grp.width() - 20, main_grp.height() - 40, "");

        main_grp.add(&queue_browser);

        let mut btns_grp = Group::new(10, main_grp.height() - 15, 10, 10, "");

        let mut btn_add = Button::new(10, main_grp.height() - 15, 40, 20, "ADD");
        btns_grp.add(&btn_add);

        let mut btn_rem =
            Button::new(10, main_grp.height() - 15, 40, 20, "REM").right_of(&btn_add, 8);
        btns_grp.add(&btn_rem);

        btns_grp.end();
        main_grp.add(&btns_grp);

        main_grp.end();
        main_win.end();

        Self {
            main_win,
            queue_browser,
        }
    }
}

fn create_horizontal_gradient_frame(
    x: i32,
    y: i32,
    w: i32,
    h: i32,
    col1: Color,
    col2: Color,
) -> Frame {
    let mut frame = Frame::new(x, y, w, h, "multiplayer :3");
    frame.draw(move |f| {
        let imax = f.w();
        let d = if imax > 0 { imax } else { 1 };
        for i in 0..=imax {
            let w = 1.0 - i as f32 / d as f32;
            draw::set_draw_color(Color::color_average(col1, col2, w));
            draw::draw_yxline(f.x() + i, f.y(), f.y() + f.h());
        }
    });
    frame
}

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
            prev_vals: [0;14],
        }
    }
    pub fn update_values(&mut self, values: [u8; 14]) {
        {
            let prev = self.values.borrow().clone();
            let mut sv = self.values.borrow_mut();
            let temp = sv.clone();
            for i in 0..14 {
                if values[i] > prev[i] {
                    sv[i] = ((prev[i] as usize + self.prev_vals[i] as usize + values[i] as usize) / 3 as usize) as u8;
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

pub struct BitrateBar {
    inner: widget::Widget,
    bitrate: Rc<RefCell<usize>>,
    buffer_level: Rc<RefCell<u8>>, // 0 to 6
}

widget_extends!(BitrateBar, widget::Widget, inner);

impl BitrateBar {
    pub fn new(x: i32, y: i32) -> Self {
        let mut inner = widget::Widget::default().with_size(35, 14).with_pos(x, y);
        inner.set_frame(FrameType::FlatBox);

        let bitrate = 123;
        let bitrate_c = Rc::from(RefCell::from(bitrate));
        let bitrate_c_c = bitrate_c.clone();

        let buffer_level = 3;
        let buffer_level_c = Rc::from(RefCell::from(buffer_level));
        let buffer_level_c_c = buffer_level_c.clone();

        let mut font = PngImage::from_data(include_bytes!("../rsrc/font.png")).unwrap();

        let mut buf_bar = PngImage::from_data(include_bytes!("../rsrc/bufbar.png")).unwrap();

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
    pub fn update_bitrate(&mut self, bitrate: usize) {
        self.bitrate.replace(bitrate);
        self.redraw();
    }
    pub fn update_buffer_level(&mut self, level: u8) {
        self.buffer_level.replace(level);
        self.redraw();
    }
}

use winapi::shared::windef::HWND;

use winapi::um::winuser::{
    GetWindowLongPtrW, SetWindowLongPtrW, ShowWindow, GWL_EXSTYLE, SW_HIDE, SW_SHOW,
};

// this is fucking cursed
unsafe fn shitty_windows_only_hack(w: &mut DoubleWindow) {
    let handle: HWND = std::mem::transmute(w.raw_handle());
    ShowWindow(handle, SW_HIDE);
    let mut style = GetWindowLongPtrW(handle, GWL_EXSTYLE);
    style |= 0x00040000; // WS_EX_APPWINDOW
    SetWindowLongPtrW(handle, GWL_EXSTYLE, style);
    ShowWindow(handle, SW_SHOW);
}
