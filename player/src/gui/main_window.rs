use crate::state::{dispatch, dispatch_update, Action, StateUpdate};
use std::sync::Arc;

use fltk::browser::*;
use fltk::button::*;
use fltk::enums::*;
use fltk::frame::*;
use fltk::group::Group;
use fltk::image::PngImage;
use fltk::prelude::*;
use fltk::valuator::*;
use fltk::window::*;
use tokio::sync::Mutex;

use super::bitrate_bar::*;
use super::marquee_label::*;
use super::play_status::*;
use super::visualizer::*;

use super::add_bar;
use super::sender;
use super::UIEvent;

pub struct MainWindow {
    pub main_win: Window,
    pub seek_bar: HorNiceSlider,
    pub status_field: Group,
    pub status_right_display: Frame,
    pub users: Browser,
    pub lbl_time: Frame,
    pub lbl_title: MarqueeLabel,
    pub lbl_artist: Frame,
    pub visualizer: Visualizer,
    pub bitrate_bar: BitrateBar,
    pub play_status: PlayStatus,
    pub volume_slider: HorSlider,
    pub art_frame: Frame,

    pub seek_bar_dragging: Arc<Mutex<bool>>,
    pub volume_slider_dragging: Arc<Mutex<bool>>,
}
impl MainWindow {
    pub fn make_window() -> Self {
        let mut main_win = Window::new(100, 100, 400, 185, "multiplayer :3");
        //main_win.set_border(false);
        main_win.set_frame(FrameType::UpBox);
        main_win.set_icon(Some(
            PngImage::from_data(include_bytes!("../../rsrc/ryo.png")).unwrap(),
        ));

        /* let mut bar_frame = Frame::new(1,2,main_win.width()-2, 16, "");
        bar_frame.set_frame(FrameType::BorderFrame);
        bar_frame.set_color(Color::Background);
        main_win.add(&bar_frame); */

        add_bar(&mut main_win, UIEvent::Quit, "multiplayer :3", true);

        // --- buttons ---
        let buttons_y = 128;
        let buttons_left = 13;
        let bp = 36 + 5;

        let mut btn_prev = Button::new(buttons_left, buttons_y, 36, 26, "");
        btn_prev.set_image(Some(
            PngImage::from_data(include_bytes!("../../rsrc/btn_prev.png")).unwrap(),
        ));
        btn_prev.set_align(Align::ImageBackdrop);
        btn_prev.emit(sender!(), UIEvent::BtnPrev);
        main_win.add(&btn_prev);

        let mut btn_play = Button::new(buttons_left + bp, buttons_y, 36, 26, "");
        btn_play.set_image(Some(
            PngImage::from_data(include_bytes!("../../rsrc/btn_play.png")).unwrap(),
        ));
        btn_play.set_align(Align::ImageBackdrop);
        btn_play.emit(sender!(), UIEvent::BtnPlay);
        main_win.add(&btn_play);

        let mut btn_pause = Button::new(buttons_left + bp * 2, buttons_y, 36, 26, "");
        btn_pause.set_image(Some(
            PngImage::from_data(include_bytes!("../../rsrc/btn_pause.png")).unwrap(),
        ));
        btn_pause.set_align(Align::ImageBackdrop);
        btn_pause.emit(sender!(), UIEvent::BtnPause);
        main_win.add(&btn_pause);

        let mut btn_stop = Button::new(buttons_left + bp * 3, buttons_y, 36, 26, "");
        btn_stop.set_image(Some(
            PngImage::from_data(include_bytes!("../../rsrc/btn_stop.png")).unwrap(),
        ));
        btn_stop.set_align(Align::ImageBackdrop);
        btn_stop.emit(sender!(), UIEvent::BtnStop);
        main_win.add(&btn_stop);

        let mut btn_next = Button::new(buttons_left + bp * 4, buttons_y, 36, 26, "");
        btn_next.set_image(Some(
            PngImage::from_data(include_bytes!("../../rsrc/btn_next.png")).unwrap(),
        ));
        btn_next.set_align(Align::ImageBackdrop);
        btn_next.emit(sender!(), UIEvent::BtnNext);
        main_win.add(&btn_next);

        // --- end buttons ---

        //let mut temp_input = Input::new(150, buttons_y, 120, 20, "");
        //main_win.add(&temp_input);

        let seek_bar_dragging = Arc::new(Mutex::new(false));

        let mut seek_bar = HorNiceSlider::new(buttons_left, buttons_y - 35, 250, 24, "");
        seek_bar.handle({
            let seek_bar_dragging = seek_bar_dragging.clone();
            move |sb, ev| match ev {
                fltk::enums::Event::Push => {
                    let mut d = seek_bar_dragging.blocking_lock();
                    *d = true;

                    dispatch!(Action::DragSeekBar(sb.value() as f32));

                    true
                }
                fltk::enums::Event::Drag => {
                    dispatch!(Action::DragSeekBar(sb.value() as f32));

                    true
                }
                fltk::enums::Event::Released => {
                    let mut d = seek_bar_dragging.blocking_lock();
                    *d = false;

                    dispatch!(Action::Seek(sb.value() as f32));

                    true
                }
                _ => false,
            }
        });
        main_win.add(&seek_bar);

        let mut status_field = Group::new(
            3,
            main_win.height() - 20,
            main_win.width() - 6,
            16,
            "status",
        );
        status_field.set_align(Align::Left | Align::Inside);
        status_field.set_frame(FrameType::ThinDownBox);

        let mut status_right_display = Frame::new(
            status_field.x(),
            status_field.y(),
            status_field.width() - 4,
            status_field.height(),
            "U00 Q00",
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
            PngImage::from_data(include_bytes!("../../rsrc/display_frame.png")).unwrap(),
        ));
        display.set_align(Align::ImageBackdrop);
        display.set_color(Color::Blue);

        let mut art_frame = Frame::new(228, 34, 31, 31, "");
        art_frame.set_align(Align::ImageBackdrop);
        art_frame.set_frame(FrameType::DownFrame);

        display.add(&art_frame);

        let lbl_title = MarqueeLabel::new(105, 35, 120);
        display.add(&*lbl_title);

        let mut lbl_artist = Frame::new(103, 50, 120, 16, "...");
        lbl_artist.set_label_size(10);
        lbl_artist.set_align(Align::Left | Align::Inside);
        display.add(&lbl_artist);

        let mut lbl_time = Frame::new(29, 35, 80, 16, "00:00");
        lbl_time.set_align(Align::Left | Align::Inside);
        display.add(&lbl_time);

        let visualizer = Visualizer::new(19, 62);
        display.add(&*visualizer);

        let bitrate_bar = BitrateBar::new(67, 69);
        display.add(&*bitrate_bar);

        let play_status = PlayStatus::new(18, 38);
        display.add(&*play_status);

        display.end();
        main_win.add(&display);

        // --- end of display ---

        let volume_slider_dragging = Arc::new(Mutex::new(false));

        let mut volume_slider = HorSlider::new(110, 70, 80, 13, "");
        volume_slider.set_bounds(0., 1.);
        volume_slider.set_step(0.01, 1);
        volume_slider.handle({
            let volume_slider_dragging = volume_slider_dragging.clone();
            move |vs: &mut HorSlider, ev| match ev {
                fltk::enums::Event::Push => {
                    let mut d = volume_slider_dragging.blocking_lock();
                    *d = true;

                    dispatch_update!(StateUpdate::SetVolume(vs.value() as f32));

                    true
                }
                fltk::enums::Event::Drag => {
                    dispatch_update!(StateUpdate::SetVolume(vs.value() as f32));

                    true
                }
                fltk::enums::Event::Released => {
                    let mut d = volume_slider_dragging.blocking_lock();
                    *d = false;

                    dispatch_update!(StateUpdate::SetVolume(vs.value() as f32));

                    true
                }
                _ => false,
            }
        });
        main_win.add(&volume_slider);

        let mut users = Browser::new(main_win.width() - 85 - 18, 31, 90, 125, "");
        users.set_text_size(11);
        main_win.add(&users);

        let mut btn_connect = Button::new(main_win.width() - 85 - 46, 31, 24, 24, "Cn");
        btn_connect.emit(sender!(), UIEvent::BtnOpenConnectionDialog);
        main_win.add(&btn_connect);

        let mut btn_queue = Button::new(main_win.width() - 85 - 46, 31 + 24 + 2, 24, 24, "Qu");
        btn_queue.emit(sender!(), UIEvent::BtnQueue);
        main_win.add(&btn_queue);

        let mut btn_queue = Button::new(
            main_win.width() - 85 - 46,
            31 + 24 + 2 + 24 + 2,
            24,
            24,
            "Me",
        );
        main_win.add(&btn_queue);

        let mut btn_queue = Button::new(
            main_win.width() - 85 - 46,
            31 + 24 + 2 + 24 + 2 + 24 + 2,
            24,
            24,
            "ow",
        );
        main_win.add(&btn_queue);

        main_win.end();

        let mut s = Self {
            main_win,
            seek_bar,
            status_field,
            status_right_display,
            users,
            lbl_time,
            lbl_title,
            lbl_artist,
            visualizer,
            bitrate_bar,
            play_status,
            volume_slider,
            art_frame,

            seek_bar_dragging,
            volume_slider_dragging,
        };
        s.reset();
        s
    }

    pub fn reset(&mut self) {
        self.lbl_title.set_text(&"hi, welcome >.<".to_string());
        self.lbl_artist.set_label(&"...".to_string());
        self.lbl_time.set_label(&"00:00".to_string());
        self.art_frame.set_image(None::<fltk::image::JpegImage>);
        self.art_frame.redraw();

        self.bitrate_bar.reset();
        self.visualizer.reset();
    }

    #[cfg(not(target_os = "windows"))]
    pub fn fix_taskbar_after_show(&mut self) {
        // TODO: implement on other platforms? lmao
    }
    #[cfg(target_os = "windows")]
    pub fn fix_taskbar_after_show(&mut self) {
        unsafe {
            shitty_windows_only_hack(&mut self.main_win);
        }
    }
}

#[cfg(target_os = "windows")]
use winapi::shared::windef::HWND;

#[cfg(target_os = "windows")]
use winapi::um::winuser::{
    GetWindowLongPtrW, SetWindowLongPtrW, ShowWindow, GWL_EXSTYLE, GWL_STYLE, SW_HIDE, SW_SHOW,
};

// this is fucking cursed
#[cfg(target_os = "windows")]
unsafe fn shitty_windows_only_hack(w: &mut DoubleWindow) {
    let handle: HWND = std::mem::transmute(w.raw_handle());
    ShowWindow(handle, SW_HIDE);

    let mut exstyle = GetWindowLongPtrW(handle, GWL_EXSTYLE);
    exstyle |= 0x00040000; // WS_EX_APPWINDOW - appear in taskbar
    exstyle |= 0x00000010; // WS_EX_ACCEPTFILES - idk what this does but sounds ok
    SetWindowLongPtrW(handle, GWL_EXSTYLE, exstyle);

    let mut style = GetWindowLongPtrW(handle, GWL_STYLE);
    style |= 0x00080000; // WS_SYSMENU - adds system menu to taskbar, alt+space etc
                         // (winamp has a custom menu there.. i wonder how to do that)
    style |= 0x00020000; // WS_MINIMIZEBOX - allow minimizing

    // this flag makes the main window not snap perfectly so we cant use it
    // style |= 0x00800000; // WS_BORDER - adds nice 2px border (it did one time and now it doesn't..)

    SetWindowLongPtrW(handle, GWL_STYLE, style);

    ShowWindow(handle, SW_SHOW);
}
