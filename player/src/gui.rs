use std::sync::Arc;

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
use tokio::sync::mpsc;
use tokio::sync::RwLock;

use protocol::TrackArt;

pub mod connection_window;
pub mod main_window;
pub mod preferences_window;
pub mod queue_window;

pub mod bitrate_bar;
pub mod group_box;
pub mod marquee_label;
pub mod play_status;
pub mod visualizer;

use crate::preferences::Server;
use crate::State;

use self::connection_window::ConnectionWindow;
use self::main_window::*;
use self::play_status::PlayStatusIcon;
use self::preferences_window::PrefsWindow;
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
    BtnOpenConnectionDialog,
    Connect(Server),
    JoinRoom(u32),
    VolumeSlider(f32),
    VolumeUp,
    VolumeDown,
    DroppedFiles(String),
    Stop,
    Pause,
    HideQueue,
    HideConnectionWindow,
    HidePrefsWindow,
    PleaseSavePreferencesWithThisData,
    SavePreferences { name: String },
    Quit,

    Update(UIUpdateEvent),
    ConnectionDlg(ConnectionDlgEvent),
}

#[derive(Debug, Clone)]
pub enum UIUpdateEvent {
    SetTime(usize, usize),
    UserListChanged,
    RoomChanged,
    QueueChanged,
    UpdateConnectionTree(Vec<self::connection_window::ServerStatus>),
    UpdateConnectionTreePartial(self::connection_window::ServerStatus),
    ConnectionChanged,
    Status,
    Visualizer([u8; 14]),
    Buffer(u8),
    Bitrate(u32),
    Volume(f32),
    Periodic,
}

#[derive(Debug, Clone)]
pub enum ConnectionDlgEvent {
    BtnConnect,
    BtnNewRoom,
    BtnRefresh,
    AddServer(String),
}

#[derive(Default)]
struct DragState {
    x: i32,
    y: i32,
    dnd: bool,
    released: bool,
}

#[derive(Clone)]
pub struct UIThreadHandle {
    sender: Sender<UIEvent>,
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

    state: Arc<RwLock<State>>,

    gui: MainWindow,
    queue_gui: QueueWindow,
    connection_gui: ConnectionWindow,
    prefs_gui: PrefsWindow,
}

impl UIThread {
    // TODO: put the id somewhere nicer
    pub fn spawn(state: Arc<RwLock<State>>) -> (UIThreadHandle, mpsc::UnboundedReceiver<UIEvent>) {
        // from anywhere to ui (including ui to ui..?)
        let (sender, receiver) = fltk::app::channel::<UIEvent>();

        // from main thread logic to ui
        let (ui_tx, ui_rx) = mpsc::unbounded_channel::<UIEvent>();

        let thread_sender = sender.to_owned();
        std::thread::spawn(move || {
            let mut t = UIThread::new(thread_sender, receiver, ui_tx, state);
            t.run();
        });

        // ui_rx can't be part of the handle because the handle needs to be clone
        // only one thing can own and consume from the receiver
        (UIThreadHandle { sender }, ui_rx)
    }

    fn new(
        sender: Sender<UIEvent>,
        receiver: Receiver<UIEvent>,
        tx: UiTx,
        state: Arc<RwLock<State>>,
    ) -> Self {
        let app = fltk::app::App::default();
        let widgets = fltk_theme::WidgetTheme::new(fltk_theme::ThemeType::Classic);
        widgets.apply();

        let theme = fltk_theme::ColorTheme::new(fltk_theme::color_themes::GRAY_THEME);
        theme.apply();

        let mut gui = MainWindow::make_window(sender.clone());
        let queue_gui = QueueWindow::make_window(sender.clone());
        let connection_gui = ConnectionWindow::make_window(sender.clone(), state.clone());
        let mut prefs_gui = PrefsWindow::make_window(sender.clone());
        prefs_gui.main_win.show();
        prefs_gui.load_state(&*state.blocking_read().preferences);

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

        gui.lbl_title.set_text(&"hi, welcome >.<".to_string());

        UIThread {
            app,
            sender,
            receiver,
            tx,

            state,

            gui,
            queue_gui,
            connection_gui,
            prefs_gui,
        }
    }

    fn run(&mut self) {
        let mut ds = DragState::default();
        let sender1 = self.sender.clone();
        self.gui
            .main_win
            .handle(move |w, ev| Self::handle_window_event(&mut ds, sender1.clone(), w, ev));

        let mut ds = DragState::default();
        let sender2 = self.sender.clone();
        self.queue_gui
            .main_win
            .handle(move |w, ev| Self::handle_window_event(&mut ds, sender2.clone(), w, ev));

        let mut ds = DragState::default();
        let sender2 = self.sender.clone();
        self.connection_gui
            .main_win
            .handle(move |w, ev| Self::handle_window_event(&mut ds, sender2.clone(), w, ev));

        let mut ds = DragState::default();
        let sender2 = self.sender.clone();
        self.prefs_gui
            .main_win
            .handle(move |w, ev| Self::handle_window_event(&mut ds, sender2.clone(), w, ev));

        while self.app.wait() {
            if let Some(msg) = self.receiver.recv() {
                // forward event to logic thread for handling
                // TODO: don't send stuff it doesn't need? like UIUpdateEvent?
                self.tx.send(msg.clone()).unwrap();

                // only deal with ui-relevant stuff here
                match msg {
                    UIEvent::Update(evt) => self.handle_update(evt),
                    UIEvent::ConnectionDlg(evt) => match evt {
                        ConnectionDlgEvent::BtnConnect => {
                            if let Some(item) = self.connection_gui.tree.first_selected_item() {
                                let (maybe_room_id, server): (Option<u32>, Server) =
                                    match item.depth() {
                                        1 => unsafe {
                                            // server selected
                                            (None, item.user_data().unwrap())
                                        },
                                        2 => unsafe {
                                            // room selected
                                            (
                                                item.user_data(),
                                                item.parent().unwrap().user_data().unwrap(),
                                            )
                                        },
                                        _ => unreachable!(),
                                    };

                                self.tx.send(UIEvent::Connect(server)).unwrap();
                                if let Some(room_id) = maybe_room_id {
                                    self.tx.send(UIEvent::JoinRoom(room_id)).unwrap();
                                }
                            }
                        }
                        ConnectionDlgEvent::AddServer(_) => todo!(),
                        _ => {}
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

                    UIEvent::BtnOpenConnectionDialog => {
                        self.connection_gui.main_win.show();
                        self.tx
                            .send(UIEvent::ConnectionDlg(ConnectionDlgEvent::BtnRefresh))
                            .unwrap();
                    }
                    UIEvent::BtnQueue => {
                        self.queue_gui.main_win.show();
                        //ui_tx.send(UIEvent::Update(UIUpdateEvent::UpdateQueue(self.queue.clone())));
                    }
                    UIEvent::HideQueue => {
                        self.queue_gui.main_win.hide();
                    }
                    UIEvent::HideConnectionWindow => {
                        self.connection_gui.main_win.hide();
                    }
                    UIEvent::HidePrefsWindow => {
                        self.prefs_gui.main_win.hide();
                    }
                    UIEvent::PleaseSavePreferencesWithThisData => {
                        self.sender.send(UIEvent::SavePreferences {
                            name: self.prefs_gui.fld_name.value(),
                        });
                    }
                    UIEvent::Quit => break,
                    _ => {}
                }
            }
        }
    }

    fn handle_update(&mut self, evt: UIUpdateEvent) {
        match evt {
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
            UIUpdateEvent::Status => {
                self.update_status();
            }
            UIUpdateEvent::Volume(val) => {
                self.gui.volume_slider.set_value(val.into());
            }
            UIUpdateEvent::Periodic => {
                if self.gui.lbl_title.waited_long_enough() {
                    self.gui.lbl_title.nudge(-4);
                }
            }
            UIUpdateEvent::UserListChanged => {
                self.update_right_status();

                let s = self.state.blocking_read();

                self.gui.users.clear();

                if let Some(c) = &s.connection {
                    if let Some(r) = &c.room {
                        self.gui.users.add(&format!("[{}]", r.name));

                        for (id, name) in r.connected_users.iter() {
                            let line = if id == &s.my_id {
                                format!("@b* you")
                            } else {
                                format!("* {} ({})", name, id)
                            };
                            self.gui.users.add(&line);
                        }
                    } else {
                        self.gui.users.add("no room".into());
                    }
                } else {
                    // not connected
                }
            }
            UIUpdateEvent::RoomChanged => {}
            UIUpdateEvent::QueueChanged => {
                self.update_right_status();

                let s = self.state.blocking_read();

                self.queue_gui.queue_browser.clear();

                if let Some(c) = &s.connection {
                    if let Some(r) = &c.room {
                        // TODO: metadata stuff here is TOO LONG AND ANNOYING
                        if let Some(track) = &r.playing {
                            let name = r.connected_users.get(&track.owner);

                            let line = format!(
                                "@b{}\t[{}] {}",
                                name.unwrap_or(&"?".into()),
                                min_secs(track.metadata.duration as usize),
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

                            self.gui.lbl_artist.set_label(
                                track
                                    .metadata
                                    .album_artist
                                    .as_ref()
                                    .unwrap_or(&"[unknown artist]".to_string()),
                            );

                            if let Some(art) = &track.metadata.art {
                                if let Err(err) = match art {
                                    TrackArt::Jpeg(data) => {
                                        JpegImage::from_data(&data).and_then(|img| {
                                            self.gui.art_frame.set_image(Some(img));
                                            self.gui.art_frame.redraw();
                                            Ok(())
                                        })
                                    }
                                } {
                                    eprintln!("failed to load image: {:?}", err);
                                }
                            } else {
                                self.gui.art_frame.set_image(None::<JpegImage>); // hm that's silly
                                self.gui.art_frame.redraw();
                            }
                        }
                        for track in &r.queue {
                            let name = r.connected_users.get(&track.owner);

                            let line = format!(
                                "{}\t[{}] {}",
                                name.unwrap_or(&"?".into()),
                                min_secs(track.metadata.duration as usize),
                                track
                                    .metadata
                                    .title
                                    .clone()
                                    .unwrap_or("[no title]".to_string())
                            );
                            self.queue_gui.queue_browser.add(&line);
                        }
                    }
                }

                self.gui.status_right_display.set_label(&format!(
                    "U{:0>2} Q{:0>2}",
                    self.gui.users.size(),
                    self.queue_gui.queue_browser.size(),
                ));
            }
            UIUpdateEvent::UpdateConnectionTree(list) => {
                self.connection_gui.populate(list);
            }
            UIUpdateEvent::UpdateConnectionTreePartial(server) => {
                self.connection_gui.update_just_one_server(server);
            }
            UIUpdateEvent::ConnectionChanged => {
                self.connection_gui.update_connected();
            }
            _ => {
                dbg!(evt);
            }
        }
    }

    fn update_right_status(&mut self) {
        let s = self.state.blocking_read();

        if let Some(c) = &s.connection {
            if let Some(r) = &c.room {
                self.gui.status_right_display.set_label(&format!(
                    "U{:0>2} Q{:0>2}",
                    r.connected_users.len(),
                    r.queue.len() + 1,
                ));
            } else {
                self.gui.status_right_display.set_label("U00 Q00");
            }
        } else {
            self.gui.status_right_display.set_label("U00 Q00");
        }
    }

    // set status based on state and priorities
    fn update_status(&mut self) {
        let status = self.get_status();
        self.gui.status_field.set_label(&status);
        // and the little thingy too
        let play_status = self.get_play_status();
        self.gui.play_status.update(play_status);
    }

    fn get_play_status(&self) -> PlayStatusIcon {
        // i love duplicated logic ^-^
        let state = self.state.blocking_read();

        if let Some(connection) = &state.connection {
            if let Some(r) = &connection.room {
                if let Some(t) = &r.playing {
                    if t.owner == state.my_id {
                        // todo: pause?
                        return PlayStatusIcon::PlayTx;
                    } else {
                        return PlayStatusIcon::PlayRx;
                    }
                }
            }
        }
        PlayStatusIcon::Stop
    }

    fn get_status(&self) -> String {
        let state = self.state.blocking_read();

        if let Some(connection) = &state.connection {
            if state.loading_count == 1 {
                return format!("Loading 1 file...");
            } else if state.loading_count > 1 {
                return format!("Loading {} files...", state.loading_count);
            }

            if let Some(r) = &connection.room {
                if r.buffering {
                    return format!("Buffering...");
                }

                if let Some(t) = &r.playing {
                    if t.owner == state.my_id {
                        return "Playing (transmitting)".to_string();
                    } else {
                        let name = r.connected_users.get(&t.owner);
                        return format!(
                            "Playing (receiving from {})",
                            name.unwrap_or(&"?".to_string())
                        );
                    }
                } else {
                    return format!("Connected to {}/{}", connection.server.addr, r.name);
                }
            }

            return format!("Connected to {}", connection.server.addr);
        } else {
            format!("<not connected>")
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
                    sender.send(UIEvent::DroppedFiles(app::event_text()));
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
