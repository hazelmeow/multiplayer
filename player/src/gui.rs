use std::collections::HashMap;
use std::collections::VecDeque;
use std::path::PathBuf;

use fltk::app;
use fltk::app::App;
use fltk::app::Receiver;
use fltk::app::Sender;
use fltk::button::*;
use fltk::draw;
use fltk::enums::*;
use fltk::frame::*;
use fltk::group::Group;
use fltk::image;
use fltk::image::JpegImage;
use fltk::prelude::*;
use fltk::window::*;

use protocol::GetInfo;
use protocol::Track;
use protocol::TrackArt;

pub mod bitrate_bar;
pub mod main_window;
pub mod marquee_label;
pub mod queue_window;
pub mod visualizer;

use self::main_window::*;
use self::queue_window::QueueWindow;

// only temporary
#[derive(Debug, Clone)]
pub enum UIEvent {
    BtnPlay, // bleh
    BtnStop,
    BtnPause,
    BtnNext,
    BtnPrev,
    BtnQueue,
    BtnConnect,
    VolumeSlider(f32),
    VolumeUp,
    VolumeDown,
    Play(PathBuf),
    Stop,
    Pause,
    Test(String),
    GetInfo(GetInfo),
    HideQueue,
    Quit,

    Update(UIUpdateEvent),
}

#[derive(Debug, Clone)]
pub enum UIUpdateEvent {
    SetTime(usize, usize),
    UpdateUserList(HashMap<String, String>),
    UpdateQueue(Option<Track>, VecDeque<Track>),
    Status(String),
    Visualizer([u8; 14]),
    Buffer(u8),
    Bitrate(usize),
    VolumeUp,
    VolumeDown,
    Periodic,
}

#[derive(Default)]
struct DragState {
    x: i32,
    y: i32,
    dnd: bool,
    released: bool,
}

pub struct UIThreadHandle {
    sender: Sender<UIEvent>,
    pub ui_rx: UiRx,
}

impl UIThreadHandle {
    pub fn update(&mut self, update: UIUpdateEvent) {
        self.sender.send(UIEvent::Update(update))
    }
}

type UiTx = tokio::sync::mpsc::UnboundedSender<UIEvent>;
type UiRx = tokio::sync::mpsc::UnboundedReceiver<UIEvent>;
pub struct UIThread {
    app: App,
    sender: Sender<UIEvent>,
    receiver: Receiver<UIEvent>,
    tx: UiTx,

    my_id: String,

    gui: MainWindow,
    queue_gui: QueueWindow,
}

impl UIThread {
    // TODO: put the id somewhere nicer
    pub fn spawn(my_id: String) -> UIThreadHandle {
        // from anywhere to ui (including ui to ui..?)
        let (sender, receiver) = fltk::app::channel::<UIEvent>();

        // from main thread logic to ui
        let (ui_tx, ui_rx) = tokio::sync::mpsc::unbounded_channel::<UIEvent>();

        let thread_sender = sender.to_owned();
        std::thread::spawn(move || {
            let mut t = UIThread::new(thread_sender, receiver, ui_tx, my_id);
            t.run();
        });

        UIThreadHandle { sender, ui_rx }
    }

    fn new(sender: Sender<UIEvent>, receiver: Receiver<UIEvent>, tx: UiTx, my_id: String) -> Self {
        let app = fltk::app::App::default();
        let widgets = fltk_theme::WidgetTheme::new(fltk_theme::ThemeType::Classic);
        widgets.apply();

        let theme = fltk_theme::ColorTheme::new(fltk_theme::color_themes::GRAY_THEME);
        theme.apply();

        let mut gui = MainWindow::make_window(sender.clone());
        let mut queue_gui = QueueWindow::make_window(sender.clone());

        // stuff for cool custom titlebars
        gui.main_win.set_border(false);
        gui.main_win.show();
        gui.fix_taskbar_after_show();

        gui.main_win.emit(sender.clone(), UIEvent::Quit);

        // TODO: we'd probably have to communicate this somehow
        //       for now just pretend
        gui.bitrate_bar.update_bitrate(256);

        // blehhhh this is all such a mess
        gui.volume_slider.set_value(0.5);
        tx.send(UIEvent::VolumeSlider(MainWindow::volume_scale(0.5)))
            .unwrap();

        gui.lbl_title.set_text(&"hi, welcome >.<".to_string());

        UIThread {
            app,
            sender,
            receiver,
            tx,

            my_id,

            gui,
            queue_gui,
        }
    }

    fn run(&mut self) {
        let mut drag_state_main = DragState::default();
        let sender1 = self.sender.clone();
        self.gui.main_win.handle(move |w, ev| {
            Self::handle_window_event(&mut drag_state_main, sender1.clone(), w, ev)
        });

        let mut drag_state_queue = DragState::default();
        let sender2 = self.sender.clone();
        self.queue_gui.main_win.handle(move |w, ev| {
            Self::handle_window_event(&mut drag_state_queue, sender2.clone(), w, ev)
        });

        while self.app.wait() {
            if let Some(msg) = self.receiver.recv() {
                // forward event to logic thread for handling
                // TODO: don't send stuff it doesn't need? like UIUpdateEvent?
                self.tx.send(msg.clone()).unwrap();

                // only deal with ui-relevant stuff here
                match msg {
                    UIEvent::Update(evt) => match evt {
                        UIUpdateEvent::SetTime(elapsed, total) => {
                            // TODO: switch between elapsed and remaining on there
                            self.gui.lbl_time.set_label(&min_secs(elapsed));
                            let progress = elapsed as f64 / total as f64;
                            self.gui.seek_bar.set_value(progress);
                        }
                        UIUpdateEvent::Visualizer(bars) => {
                            self.gui.visualizer.update_values(bars);
                        }
                        UIUpdateEvent::Buffer(val) => {
                            self.gui.bitrate_bar.update_buffer_level(val);
                        }
                        UIUpdateEvent::Bitrate(val) => {
                            self.gui.bitrate_bar.update_bitrate(val);
                        }
                        UIUpdateEvent::Status(val) => {
                            self.gui.status_field.set_label(&val);
                        }
                        UIUpdateEvent::Periodic => {
                            if self.gui.lbl_title.waited_long_enough() {
                                self.gui.lbl_title.nudge(-4);
                            }
                        }
                        UIUpdateEvent::VolumeUp => {
                            self.gui
                                .volume_slider
                                .set_value((self.gui.volume_slider.value() + 0.02).min(1.));
                            self.tx
                                .send(UIEvent::VolumeSlider(MainWindow::volume_scale(
                                    self.gui.volume_slider.value(),
                                )))
                                .unwrap();
                        }
                        UIUpdateEvent::VolumeDown => {
                            self.gui
                                .volume_slider
                                .set_value((self.gui.volume_slider.value() - 0.02).max(0.));
                            self.tx
                                .send(UIEvent::VolumeSlider(MainWindow::volume_scale(
                                    self.gui.volume_slider.value(),
                                )))
                                .unwrap();
                        }
                        UIUpdateEvent::UpdateUserList(val) => {
                            self.gui.users.clear();
                            for (id, name) in val.iter() {
                                let line = if id == &self.my_id {
                                    format!("@b* you")
                                } else {
                                    format!("* {} ({})", name, id)
                                };
                                self.gui.users.add(&line);
                            }
                            self.gui.status_right_display.set_label(&format!(
                                "U{:0>2} Q{:0>2}",
                                self.gui.users.size(),
                                self.queue_gui.queue_browser.size(),
                            ));
                        }
                        UIUpdateEvent::UpdateQueue(current, queue) => {
                            self.queue_gui.queue_browser.clear();
                            // TODO: metadata stuff here is TOO LONG AND ANNOYING
                            if let Some(track) = current {
                                let line = format!(
                                    "@b{}\t[{}] {}",
                                    track.owner,
                                    min_secs(track.metadata.duration),
                                    track
                                        .metadata
                                        .title
                                        .as_ref()
                                        .unwrap_or(&"[no title]".to_string())
                                );
                                self.queue_gui.queue_browser.add(&line);

                                // also update display
                                self.gui.lbl_title.set_text(
                                    track
                                        .metadata
                                        .title
                                        .as_ref()
                                        .unwrap_or(&"[no title]".to_string()),
                                );
                                self.gui.lbl_title.zero();

                                self.gui.lbl_data1.set_label(
                                    track
                                        .metadata
                                        .artist
                                        .as_ref()
                                        .unwrap_or(&"[unknown artist]".to_string()),
                                );

                                if let Some(art) = track.metadata.art {
                                    if let Err(err) = match art {
                                        TrackArt::Jpeg(data) => JpegImage::from_data(&data)
                                            .and_then(|img| {
                                                self.gui.art_frame.set_image(Some(img));
                                                self.gui.art_frame.redraw();
                                                Ok(())
                                            }),
                                    } {
                                        eprintln!("failed to load image: {:?}", err);
                                    }
                                } else {
                                    self.gui.art_frame.set_image(None::<JpegImage>); // hm that's silly
                                    self.gui.art_frame.redraw();
                                }
                            }
                            for track in queue {
                                let line = format!(
                                    "{}\t[{}] {}",
                                    track.owner,
                                    min_secs(track.metadata.duration),
                                    track.metadata.title.unwrap_or("[no title]".to_string())
                                );
                                self.queue_gui.queue_browser.add(&line);
                            }
                            self.gui.status_right_display.set_label(&format!(
                                "U{:0>2} Q{:0>2}",
                                self.gui.users.size(),
                                self.queue_gui.queue_browser.size(),
                            ));
                        }
                        _ => {
                            dbg!(evt);
                        }
                    },
                    UIEvent::BtnPlay => {
                        // play button behaviour:
                        // if stopped and queue is empty, ask for file
                        // if stopped and queue has songs, tell it to play
                        // if already playing, do nothing
                    }
                    UIEvent::BtnStop => {
                        self.send(UIEvent::Stop);
                    }
                    UIEvent::BtnPause => {}
                    UIEvent::BtnNext => {}
                    UIEvent::BtnPrev => {}

                    UIEvent::BtnQueue => {
                        self.queue_gui.main_win.show();
                        //ui_tx.send(UIEvent::Update(UIUpdateEvent::UpdateQueue(self.queue.clone())));
                    }
                    UIEvent::HideQueue => {
                        self.queue_gui.main_win.hide();
                    }
                    UIEvent::Quit => break,
                    UIEvent::Test(s) => {
                        //dbg!(gui.temp_input.value());
                    }
                    _ => {}
                }
            }
        }
    }

    // send to our own loop which also sends to main thread
    fn send(&self, ev: UIEvent) {
        self.sender.send(ev);
    }

    fn handle_window_event(
        state: &mut DragState,
        sender: Sender<UIEvent>,
        w: &mut DoubleWindow,
        ev: Event,
    ) -> bool {
        match ev {
            fltk::enums::Event::Push => {
                let coords = app::event_coords();
                state.x = coords.0;
                state.y = coords.1;
                true
            }
            fltk::enums::Event::Drag => {
                //if y < 20 {
                w.set_pos(app::event_x_root() - state.x, app::event_y_root() - state.y);
                true
                //} else {
                //    false
                //}
            }
            fltk::enums::Event::DndEnter => {
                println!("blah");
                state.dnd = true;
                true
            }
            fltk::enums::Event::DndDrag => true,
            fltk::enums::Event::DndRelease => {
                state.released = true;
                true
            }
            fltk::enums::Event::Paste => {
                if state.dnd && state.released {
                    for path in app::event_text().split("\n") {
                        let path = path.trim().replace("file://", "");
                        let path = std::path::PathBuf::from(&path);

                        if path.exists() {
                            sender.send(UIEvent::Play(path.clone()));
                        }
                    }
                    state.dnd = false;
                    state.released = false;
                    true
                } else {
                    println!("paste");
                    false
                }
            }
            fltk::enums::Event::DndLeave => {
                state.dnd = false;
                state.released = false;
                true
            }
            fltk::enums::Event::MouseWheel => {
                let dy = app::event_dy();
                match dy {
                    // uhhhhhhhh these are the opposite of what you'd think (???)
                    app::MouseWheel::Up => {
                        sender.send(UIEvent::VolumeDown);
                        true
                    }
                    app::MouseWheel::Down => {
                        sender.send(UIEvent::VolumeUp);
                        true
                    }
                    _ => false,
                }
            }
            _ => false,
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

fn add_bar(win: &mut DoubleWindow, s: Sender<UIEvent>, close_message: UIEvent, title: &str) {
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

fn min_secs(secs: usize) -> String {
    format!("{:02}:{:02}", secs / 60, secs % 60)
}
