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
use fltk::input::*;
use fltk::prelude::*;
use fltk::valuator::*;
use fltk::window::*;

use crate::main;
use crate::UIEvent;

pub struct MainWindow {
    pub main_win: Window,
    pub seek_bar: HorNiceSlider,
    pub status_field: Frame,
    pub users: Browser,
    pub temp_input: Input,
    pub lbl_time: Frame,
}
impl MainWindow {
    pub fn make_window(s: Sender<UIEvent>) -> Self {
        let mut main_win = Window::new(100, 100, 400, 190, "multiplayer :3");
        main_win.set_border(false);
        main_win.set_frame(FrameType::UpBox);

        /* let mut bar_frame = Frame::new(1,2,main_win.width()-2, 16, "");
        bar_frame.set_frame(FrameType::BorderFrame);
        bar_frame.set_color(Color::Background);
        main_win.add(&bar_frame); */

        let mut bar = Group::new(4,4,main_win.width()-8, 17, "");
        let mut bar_bg = create_horizontal_gradient_frame(
            4,
            4,
            main_win.width() - 8,
            17,
            Color::from_rgb(56, 85, 145),
            Color::from_rgb(166, 202, 240),
        );
        let mut bar_title = Frame::new(8, 4, 100, 17, "multiplayer :3");
        bar_title.set_align(Align::Left | Align::Inside);
        bar_title.set_label_font(Font::HelveticaBold);
        bar_title.set_label_size(12);
        bar_title.set_label_color(Color::White);
        bar.add(&bar_title);

        bar_bg.set_frame(FrameType::FlatBox);

        main_win.add(&bar_bg);
        //bar.set_color(Color::from_rgb(56, 85, 145));

        let mut bar_btn_close = Button::new(main_win.width() - 4 - 17, 6, 15, 13, "");
        let ico_x = image::BmpImage::from_data(include_bytes!("close.bmp")).unwrap();
        bar_btn_close.set_image(Some(ico_x));
        bar_btn_close.set_align(Align::Center | Align::ImageBackdrop);
        //bar_btn_close.set_color(Color::White);
        //bar_btn_close.set_frame(FrameType::BorderBox);
        bar_btn_close.emit(s.clone(), UIEvent::Quit);
        bar.add(&bar_btn_close);

        bar.end();
        main_win.add(&bar);

        let buttons_y = 130;
        let side_margin = 18;

        let mut temp_input = Input::new(150, buttons_y, 120, 20, "");
        main_win.add(&temp_input);

        let mut btn_play = Button::new(side_margin, buttons_y, 36, 26, "@>");
        btn_play.emit(s.clone(), UIEvent::BtnPlay);
        main_win.add(&btn_play);

        let mut btn_stop = Button::new(42 + side_margin, buttons_y, 36, 26, "@-7square");
        btn_stop.emit(s.clone(), UIEvent::BtnStop);
        main_win.add(&btn_stop);

        let mut btn_pause = Button::new(84 + side_margin, buttons_y, 36, 26, "| |");
        btn_pause.emit(s.clone(), UIEvent::BtnPause);
        main_win.add(&btn_pause);

        let seek_bar = HorNiceSlider::new(side_margin, buttons_y - 35, 200, 24, "");
        main_win.add(&seek_bar);

        let mut status_field = Frame::new(
            1,
            main_win.height() - 21,
            main_win.width() - 4,
            20,
            "status",
        );
        status_field.set_align(Align::Left | Align::Inside);
        status_field.set_frame(FrameType::DownBox);
        main_win.add(&status_field);

        let mut art_frame = Frame::new(side_margin, 30, 50, 50, "");
        art_frame.set_frame(FrameType::DownBox);
        main_win.add(&art_frame);

        let mut lbl_title = Frame::new(side_margin + 54, 38, 50, 16, "Title, thing");
        lbl_title.set_align(Align::Left | Align::Inside);
        main_win.add(&lbl_title);

        let mut lbl_data1 = Frame::new(side_margin + 54, 54, 50, 16, "data1");
        lbl_data1.set_align(Align::Left | Align::Inside);
        main_win.add(&lbl_data1);

        let mut users = Browser::new(main_win.width() - 85 - side_margin, 26, 85, 120, "Users");
        users.add("* you");
        main_win.add(&users);

        let mut btn_connect = Button::new(main_win.width() - 85 - 46, 26, 24, 24, "Cn");
        //btn_connect.emit(sender, crate::UIEvent::Test("wow".to_string()));
        main_win.add(&btn_connect);

        let mut btn_queue = Button::new(main_win.width() - 85 - 46, 26 + 24 + 2, 24, 24, "Qu");
        btn_queue.emit(s.clone(), UIEvent::BtnQueue);
        main_win.add(&btn_queue);

        let mut lbl_time = Frame::new(180, 70, 80, 16, "00:00/00:00");
        lbl_time.set_label_font(Font::Courier);
        lbl_time.set_align(Align::Left | Align::Inside);
        main_win.add(&lbl_time);

        main_win.end();

        Self {
            main_win,
            seek_bar,
            status_field,
            users,
            temp_input,
            lbl_time,
        }
    }
}

pub struct QueueWindow {
    pub main_win: Window,
    pub queue_browser: SelectBrowser,
}

impl QueueWindow {
    pub fn make_window(s: Sender<UIEvent>) -> Self {
        let mut main_win = Window::new(100, 100, 300, 400, "queue");
        //main_win.make_resizable(true);
        /* main_win.set_border(false);
        main_win.set_color(Color::White);
        main_win.set_frame(FrameType::BorderBox);

        let mut bar = Group::new(1,1,main_win.width()-2, 20, "");
        bar.set_frame(FrameType::FlatBox);
        bar.set_color(Color::White);

        let mut bar_btn_close = Button::new(bar.width()-20, 4, 16, 16, "X");
        bar_btn_close.set_color(Color::White);
        bar_btn_close.set_frame(FrameType::BorderBox);
        bar_btn_close.emit(s.clone(), UIEvent::HideQueue);
        bar.add(&bar_btn_close);

        bar.end();
        main_win.add(&bar); */

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
