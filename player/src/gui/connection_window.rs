use fltk::app::Sender;
use fltk::browser::*;
use fltk::button::*;
use fltk::enums::*;
use fltk::group::Group;
use fltk::input::Input;
use fltk::prelude::*;
use fltk::window::*;

use super::add_bar;
use super::UIEvent;

pub struct ConnectionWindow {
    pub main_win: Window,
}

impl ConnectionWindow {
    pub fn make_window(s: Sender<UIEvent>) -> Self {
        let mut main_win = Window::new(100, 100, 300, 200, "queue");
        main_win.set_frame(FrameType::UpBox);

        add_bar(&mut main_win, s.clone(), UIEvent::HideConnectionWindow, "Connection Dialog");
        main_win.set_border(false);

        // very temp
        let mut addr_bar = Input::new(10,40,150,25,"Address").with_align(Align::TopLeft);
        let tmp = s.clone();
        addr_bar.set_trigger(CallbackTrigger::EnterKey);
        addr_bar.set_callback(move |bar| {
            tmp.send(UIEvent::Connect(bar.value()));
        });

        Self {
            main_win
        }
    }
}