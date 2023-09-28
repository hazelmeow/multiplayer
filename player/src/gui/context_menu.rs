// meowful fltk ctx menu that doesn't block the main thread
// made with <3

use crate::gui::{ui_send, UIEvent};
use fltk::draw::measure;
use fltk::enums::*;
use fltk::frame;
use fltk::frame::*;
use fltk::prelude::*;
use fltk::widget_extends;
use fltk::window::*;

use winapi::shared::windef::HWND;
use winapi::um::winuser::SetActiveWindow;

pub struct ContextMenu {
    pub win: Window,
    pub content: Option<ContextMenuContent>,
}

impl ContextMenu {
    pub fn new() -> Self {
        let mut win = Window::default();
        win.set_border(false);
        win.set_frame(FrameType::UpBox);
        win.set_override();

        win.handle(move |w, e| match e {
            Event::Unfocus => {
                ui_send!(UIEvent::ContextMenuUnfocus);
                true
            }
            _ => false,
        });
        Self { win, content: None }
    }

    pub fn pop(&mut self, coords: (i32, i32), content: ContextMenuContent) {
        self.win.set_pos(coords.0, coords.1);
        self.content = Some(content);
        self.render();
        self.win.show();
        unsafe {
            Self::windows_focus_hack(&self.win);
        }
    }

    fn render(&mut self) {
        assert!(self.content.is_some());
        let mut width = 60;
        let height = 20; // cuz idk
        let sep_height = 5;
        let mut sep_count = 0;
        let content = self.content.as_ref().unwrap();
        let mut items: Vec<ContextMenuItem> = vec![];
        for (i, (text, action, sep_next)) in content.items.iter().enumerate() {
            // render
            // also figure out width for later
            let w = measure(text, true).0;
            if w > width {
                width = w;
            }

            let mut item =
                ContextMenuItem::new(height * i as i32 + sep_height * sep_count, 2, height, text);
            let action = action.clone();
            item.handle(move |f, e| match e {
                Event::Push => {
                    ui_send!(UIEvent::ContextMenuUnfocus);
                    ui_send!(action.clone()); // epic double clone
                    true
                }
                Event::Enter => {
                    f.set_color(Color::from_rgb(10, 36, 106)); // dark blue
                    f.set_label_color(Color::White);
                    f.redraw();
                    true
                }
                Event::Leave => {
                    f.set_color(Color::FrameDefault);
                    f.set_label_color(Color::Black);
                    f.redraw();
                    true
                }
                _ => false,
            });
            self.win.add(&*item);
            items.push(item);
            // and height
            if *sep_next {
                sep_count += 1;
            }
        }
        width += 6;

        self.win.resize(
            self.win.x(),
            self.win.y(),
            width + 5,
            height * content.items.len() as i32 + sep_height * sep_count + 5,
        );

        for item in &mut items {
            let h = item.height();
            item.set_size(width, h);
        }
    }

    pub fn hide(&mut self) {
        self.win.hide();
    }

    pub fn invalidate(&mut self) {
        self.content = None;
    }

    unsafe fn windows_focus_hack(w: &DoubleWindow) {
        // yay i know we all love these things
        // needs to be run when the window is shown
        let hwnd = w.raw_handle() as HWND;
        SetActiveWindow(hwnd);
    }
}

pub struct ContextMenuContent {
    pub(crate) items: Vec<(String, UIEvent, bool)>,
}

pub struct ContextMenuItem {
    pub inner: Frame,
}
widget_extends!(ContextMenuItem, frame::Frame, inner);
impl ContextMenuItem {
    fn new(y: i32, pad: i32, height: i32, text: &str) -> Self {
        let mut inner = Frame::default()
            .with_pos(pad, pad + y)
            .with_label(text)
            .with_size(1000, height)
            .with_align(Align::Left | Align::Inside);

        inner.set_frame(FrameType::FlatBox);

        Self { inner }
    }
}
