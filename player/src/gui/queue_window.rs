use fltk::browser::*;
use fltk::button::*;
use fltk::enums::*;
use fltk::group::Group;
use fltk::prelude::*;
use fltk::window::*;

use super::add_bar;
use super::UIEvent;

pub struct QueueWindow {
    pub main_win: Window,
    pub queue_browser: SelectBrowser,
}

impl QueueWindow {
    pub fn make_window() -> Self {
        let mut main_win = Window::new(100, 100, 300, 400, "queue");
        main_win.set_frame(FrameType::UpBox);

        add_bar(&mut main_win, UIEvent::HideQueue, "Queue");
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
