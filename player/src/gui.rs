use fltk::app::Sender;
use fltk::browser::*;
use fltk::button::*;
use fltk::enums::*;
use fltk::frame::*;
use fltk::input::*;
use fltk::prelude::*;
use fltk::valuator::*;
use fltk::window::*;

use crate::UIEvent;

pub struct UserInterface {
    pub main_win: Window,
    pub seek_bar: HorNiceSlider,
    pub status_field: Frame,
    pub users: Browser,
    pub temp_input: Input,
    pub lbl_time: Frame,
}
impl UserInterface {
    pub fn make_window(sender: Sender<UIEvent>) -> Self {
        let mut main_win = Window::new(100, 100, 400, 180, "multiplayer :3");

        let buttons_y = 120;
        let side_margin = 18;

        let mut temp_input = Input::new(150,buttons_y,120,20,"");
        main_win.add(&temp_input);

        let mut btn_play = Button::new(side_margin, buttons_y, 36, 26, "@>");
        btn_play.emit(sender, UIEvent::BtnPlay);
        main_win.add(&btn_play);

        let btn_stop = Button::new(42 + side_margin, buttons_y, 36, 26, "@-7square");
        main_win.add(&btn_stop);

        let btn_pause = Button::new(84 + side_margin, buttons_y, 36, 26, "| |");
        main_win.add(&btn_pause);

        let seek_bar = HorNiceSlider::new(side_margin, buttons_y - 35, 200, 24, "");
        main_win.add(&seek_bar);

        let mut status_field =
            Frame::new(0, main_win.height() - 21, main_win.width(), 21, "status");
        status_field.set_align(Align::Left | Align::Inside);
        status_field.set_frame(FrameType::DownBox);
        main_win.add(&status_field);

        let mut art_frame = Frame::new(side_margin, 16, 50, 50, "");
        art_frame.set_frame(FrameType::DownBox);
        main_win.add(&art_frame);

        let mut lbl_title = Frame::new(side_margin + 54, 22, 50, 16, "Title, thing");
        lbl_title.set_align(Align::Left | Align::Inside);
        main_win.add(&lbl_title);

        let mut lbl_data1 = Frame::new(side_margin + 54, 40, 50, 16, "data1");
        lbl_data1.set_align(Align::Left | Align::Inside);
        main_win.add(&lbl_data1);

        let mut users = Browser::new(main_win.width() - 85 - side_margin, 16, 85, 120, "Users");
        users.add("* you");
        main_win.add(&users);

        let mut btn_connect = Button::new(main_win.width() - 85 - 46, 16, 24, 24, "Cn");
        //btn_connect.emit(sender, crate::UIEvent::Test("wow".to_string()));
        main_win.add(&btn_connect);

        let mut btn_queue = Button::new(main_win.width() - 85 - 46, 16+24+2, 24, 24, "Qu");
        main_win.add(&btn_queue);

        let mut lbl_time = Frame::new(180,70,80,16,"00:00/00:00");
        lbl_time.set_label_font(Font::Courier);
        lbl_time.set_align(Align::Left | Align::Inside);

        main_win.add(&lbl_time);


        Self { main_win, seek_bar, status_field, users, temp_input, lbl_time }
    }
}
