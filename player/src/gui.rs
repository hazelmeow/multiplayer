use std::ops::Deref;
use std::sync::Arc;

use fltk::app;
use fltk::app::Receiver;
use fltk::app::{App, MouseButton};
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

use protocol::PlaybackState;
use protocol::TrackArt;

pub mod connection_window;
pub mod lyric_window;
pub mod main_window;
pub mod preferences_window;
pub mod queue_window;

pub mod context_menu;

pub mod bitrate_bar;
pub mod group_box;
pub mod marquee_label;
pub mod play_status;
pub mod visualizer;

use crate::preferences::Server;
use crate::State;

use self::connection_window::ConnectionWindow;
use self::lyric_window::LyricWindow;
use self::main_window::*;
use self::play_status::PlayStatusIcon;
use self::preferences_window::PrefsWindow;
use self::queue_window::QueueWindow;
use crate::gui::context_menu::{ContextMenu, ContextMenuContent};

/// Get the global FLTK sender
macro_rules! sender {
    () => {
        fltk::app::Sender::<UIEvent>::get()
    };
}
/// Send a UIEvent through the global FLTK sender
macro_rules! ui_send {
    ($expression:expr) => {{
        use crate::gui::UIEvent;
        let s = fltk::app::Sender::<UIEvent>::get();
        s.send($expression);
    }};
}
/// Send a UIUpdateEvent through the global FLTK sender
macro_rules! ui_update {
    ($expression:expr) => {{
        use crate::gui::UIEvent;
        let s = fltk::app::Sender::<UIEvent>::get();
        s.send(UIEvent::Update($expression));
    }};
}
pub(crate) use sender;
pub(crate) use ui_send;
pub(crate) use ui_update;

// only temporary
#[derive(Debug, Clone)]
pub enum UIEvent {
    // handled by main thread
    BtnPlay,
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
    SeekBarMoved(f32),
    SeekBarFinished(f32),
    DroppedFiles(String),
    HideQueue,
    HideConnectionWindow,
    ShowPrefsWindow,
    PleaseSavePreferencesWithThisData,
    SavePreferences { name: String },
    SwitchPrefsPage(String),
    ShowLyricWindow(bool),
    PopContextMenu((i32, i32)),
    ContextMenuSelected(usize),
    ContextMenuUnfocus,
    Quit,

    Update(UIUpdateEvent),
    ConnectionDlg(ConnectionDlgEvent),
    ReFrontAll(WindowId),
}

#[derive(Debug, Clone)]
pub enum UIUpdateEvent {
    Reset,
    SetTime(f32, f32),
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
    id: Option<WindowId>,
    x: i32,
    y: i32,
}
impl DragState {
    fn with_id(mut self, id: WindowId) -> Self {
        self.id = Some(id);
        self
    }
}

#[derive(Default)]
struct DndState {
    dnd: bool,
    released: bool,
}

type UiTx = mpsc::UnboundedSender<UIEvent>;
type UiRx = mpsc::UnboundedReceiver<UIEvent>;

pub struct UIThread {
    app: App,
    receiver: Receiver<UIEvent>,
    tx: UiTx,

    state: Arc<RwLock<State>>,

    gui: MainWindow,
    queue_gui: QueueWindow,
    connection_gui: ConnectionWindow,
    prefs_gui: PrefsWindow,
    lyric_gui: LyricWindow,

    context_menu: ContextMenu,
}

#[derive(Default, Debug)]
struct WindowEdges {
    left: i32,
    right: i32,
    top: i32,
    bottom: i32,

    width: i32,
    height: i32,

    shown: bool,
    wants_front: bool,
}
impl WindowEdges {
    fn update(&mut self, win: &mut DoubleWindow) {
        self.left = win.x();
        self.right = win.x() + win.width();
        self.top = win.y();
        self.bottom = win.y() + win.height();

        self.width = win.width();
        self.height = win.height();

        self.shown = win.shown();
        if self.wants_front && self.shown {
            win.show();
            self.wants_front = false;
        }
    }
}

#[derive(Default)]
struct AllWindowEdges([WindowEdges; 4]);
impl AllWindowEdges {
    fn update_window(&mut self, id: WindowId, win: &mut DoubleWindow) {
        self.0[id as usize].update(win);
    }
    fn front(&mut self, id: WindowId) {
        self.0[id as usize].wants_front = true;
    }
    fn front_all(&mut self) {
        for w in &mut self.0 {
            w.wants_front = w.shown;
        }
    }
}

#[derive(Clone, Copy, PartialEq, Debug)]
pub enum WindowId {
    Main = 0,
    Queue = 1,
    Connection = 2,
    Lyric = 3,
}

impl UIThread {
    pub fn spawn(state: Arc<RwLock<State>>) -> mpsc::UnboundedReceiver<UIEvent> {
        // from anywhere to ui (including ui to ui..?)
        let (_, receiver) = fltk::app::channel::<UIEvent>();

        // from main thread logic to ui
        let (ui_tx, ui_rx) = mpsc::unbounded_channel::<UIEvent>();

        std::thread::spawn(move || {
            let mut t = UIThread::new(receiver, ui_tx, state);
            t.run();
        });

        // ui_rx can't be part of the handle because the handle needs to be clone
        // only one thing can own and consume from the receiver
        ui_rx
    }

    fn new(receiver: Receiver<UIEvent>, tx: UiTx, state: Arc<RwLock<State>>) -> Self {
        let app = fltk::app::App::default();
        let widgets = fltk_theme::WidgetTheme::new(fltk_theme::ThemeType::Classic);
        widgets.apply();

        let theme = fltk_theme::ColorTheme::new(fltk_theme::color_themes::GRAY_THEME);
        theme.apply();

        let mut gui = MainWindow::make_window();
        let queue_gui = QueueWindow::make_window();
        let connection_gui = ConnectionWindow::make_window(state.clone());
        let mut prefs_gui = PrefsWindow::make_window();
        let mut lyric_gui = LyricWindow::make_window();
        lyric_gui.position(&gui.main_win);
        prefs_gui.load_state(&state.blocking_read().preferences);

        // stuff for cool custom titlebars
        gui.main_win.set_border(false);
        gui.main_win.show();
        gui.fix_taskbar_after_show();

        gui.main_win.emit(sender!(), UIEvent::Quit);

        // TODO: we'd probably have to communicate this somehow
        //       for now just pretend
        gui.bitrate_bar.update_bitrate(256);

        // blehhhh this is all such a mess
        gui.volume_slider.set_value(0.5);

        let context_menu = ContextMenu::new();

        UIThread {
            app,
            receiver,
            tx,

            state,

            gui,
            queue_gui,
            connection_gui,
            prefs_gui,
            lyric_gui,

            context_menu,
        }
    }

    fn run(&mut self) {
        fn handle_window(
            win: &mut DoubleWindow,
            modal: bool,
            maybe_id: Option<WindowId>,
            e: Arc<RwLock<AllWindowEdges>>,
        ) {
            let mut drag_state = DragState::default();
            if let Some(id) = maybe_id {
                drag_state = drag_state.with_id(id);
            }

            let mut dnd_state = DndState::default();

            win.handle(move |w, ev| {
                // handlers return true if the event was handled
                // so we can chain them together conveniently
                handle_window_misc(w, ev)
                    || handle_window_drag(w, ev, &mut drag_state, e.clone())
                    || (!modal && handle_volume_scroll(w, ev))
                    || (!modal && handle_window_dnd(w, ev, &mut dnd_state))
            })
        }

        let mut edges_state = AllWindowEdges::default();
        edges_state.update_window(WindowId::Main, &mut self.gui.main_win);
        edges_state.update_window(WindowId::Queue, &mut self.queue_gui.main_win);
        edges_state.update_window(WindowId::Connection, &mut self.connection_gui.main_win);

        let e = Arc::new(RwLock::new(edges_state));

        handle_window(
            &mut self.gui.main_win,
            false,
            Some(WindowId::Main),
            e.clone(),
        );
        handle_window(
            &mut self.queue_gui.main_win,
            false,
            Some(WindowId::Queue),
            e.clone(),
        );
        handle_window(
            &mut self.connection_gui.main_win,
            false,
            Some(WindowId::Connection),
            e.clone(),
        );
        handle_window(
            &mut self.lyric_gui.main_win,
            false,
            Some(WindowId::Lyric),
            e.clone(),
        );
        //handle_window(&mut self.prefs_gui.main_win, true, None, e.clone());

        // i made the name short so it doesnt autoformat the lines above on to multiple lines
        // but also it probably shouldnt be in scope as `e` so (lol?)
        // TODO i forget what i was gonna do here so uhh
        let edges_state = e.clone();
        drop(e);

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

                    // handled by main thread
                    UIEvent::BtnPlay => {}
                    UIEvent::BtnPause => {}
                    UIEvent::BtnStop => {}
                    UIEvent::BtnNext => {}
                    UIEvent::BtnPrev => {}

                    UIEvent::BtnOpenConnectionDialog => {
                        self.connection_gui.main_win.show();
                        self.tx
                            .send(UIEvent::ConnectionDlg(ConnectionDlgEvent::BtnRefresh))
                            .unwrap();
                        edges_state
                            .blocking_write()
                            .update_window(WindowId::Connection, &mut self.connection_gui.main_win);
                    }
                    UIEvent::BtnQueue => {
                        self.queue_gui.main_win.show();
                        //ui_tx.send(UIEvent::Update(UIUpdateEvent::UpdateQueue(self.queue.clone())));
                        edges_state
                            .blocking_write()
                            .update_window(WindowId::Main, &mut self.queue_gui.main_win);
                    }
                    UIEvent::HideQueue => {
                        self.queue_gui.main_win.hide();
                        edges_state
                            .blocking_write()
                            .update_window(WindowId::Queue, &mut self.queue_gui.main_win);
                    }
                    UIEvent::HideConnectionWindow => {
                        self.connection_gui.main_win.hide();
                        edges_state
                            .blocking_write()
                            .update_window(WindowId::Connection, &mut self.connection_gui.main_win);
                    }
                    UIEvent::ShowPrefsWindow => {
                        self.prefs_gui.main_win.show();
                    }
                    UIEvent::PleaseSavePreferencesWithThisData => {
                        ui_send!(UIEvent::SavePreferences {
                            name: self.prefs_gui.items.fld_name.as_ref().unwrap().value(),
                        });
                        self.prefs_gui.main_win.hide();
                    }
                    UIEvent::SwitchPrefsPage(path) => {
                        self.prefs_gui.switch_page(&path);
                    }
                    UIEvent::ShowLyricWindow(show) => {
                        if show {
                            self.lyric_gui.main_win.show();
                        } else {
                            self.lyric_gui.main_win.hide();
                        }
                    }
                    UIEvent::PopContextMenu(coords) => {
                        let content = ContextMenuContent {
                            items: vec![
                                (
                                    "Connect to Server".into(),
                                    UIEvent::BtnOpenConnectionDialog,
                                    false,
                                ),
                                (
                                    if self.lyric_gui.main_win.shown() {
                                        "Hide Lyrics Viewer"
                                    } else {
                                        "Show Lyrics Viewer"
                                    }
                                    .into(),
                                    UIEvent::ShowLyricWindow(!self.lyric_gui.main_win.shown()),
                                    true,
                                ),
                                ("Preferences".into(), UIEvent::ShowPrefsWindow, true),
                                ("Quit".into(), UIEvent::Quit, false),
                            ],
                        };
                        self.context_menu.pop(coords, content);
                    }
                    UIEvent::ContextMenuSelected(temp_idx) => {
                        self.context_menu.hide();
                        dbg!(temp_idx);
                    }
                    UIEvent::ContextMenuUnfocus => {
                        self.context_menu.hide();
                    }
                    UIEvent::ReFrontAll(from_id) => {
                        let mut es = edges_state.blocking_write();
                        let all_ids = vec![
                            (WindowId::Queue, &mut self.queue_gui.main_win),
                            (WindowId::Connection, &mut self.connection_gui.main_win),
                            (WindowId::Main, &mut self.gui.main_win),
                            (WindowId::Lyric, &mut self.lyric_gui.main_win),
                        ];
                        {
                            let non_last_ids = all_ids.into_iter().filter(|x| x.0 != from_id);

                            for (id, win) in non_last_ids {
                                es.update_window(id, win);
                            }
                        }
                        // and then
                        let all_ids = vec![
                            (WindowId::Queue, &mut self.queue_gui.main_win),
                            (WindowId::Connection, &mut self.connection_gui.main_win),
                            (WindowId::Main, &mut self.gui.main_win),
                            (WindowId::Lyric, &mut self.lyric_gui.main_win),
                        ];
                        let (last_id, last_win) =
                            all_ids.into_iter().find(|x| x.0 == from_id).unwrap();
                        es.update_window(last_id, last_win);
                    }
                    UIEvent::Quit => break,
                    _ => {}
                }
            }
        }
    }

    fn handle_update(&mut self, evt: UIUpdateEvent) {
        match evt {
            UIUpdateEvent::Reset => {
                self.gui.reset();

                self.update_status();
            }
            UIUpdateEvent::SetTime(elapsed, total) => {
                // TODO: switch between elapsed and remaining on there
                self.gui.lbl_time.set_label(&min_secs(elapsed));

                // this goes here because it's the only place we know elapsed time
                if let Some(t) = self.state.blocking_read().current_track() {
                    if let Some(lyrics) = &t.metadata.lyrics {
                        let current_ms = (elapsed * 1000.0) as usize;
                        if current_ms == 0 {
                            self.lyric_gui.lyric_display.clear();
                        } else {
                            let mut set = false;
                            for (ts, line) in lyrics.lines.iter().rev() {
                                if current_ms > *ts {
                                    set = true;
                                    self.lyric_gui.lyric_display.update(line, false);
                                    break;
                                } else if current_ms + 800 > *ts
                                    && self.lyric_gui.lyric_display.musicing()
                                {
                                    // flash lyrics about to start
                                    set = true;
                                    self.lyric_gui.lyric_display.blink();
                                    break;
                                }
                            }
                            if !set {
                                self.lyric_gui.lyric_display.update("♪", true);
                            }
                        }
                    }
                } else {
                    self.lyric_gui.lyric_display.update("no lyrics", true);
                }

                let progress = {
                    if total == 0.0 {
                        0.0
                    } else {
                        elapsed / total
                    }
                };

                let dragging = self.gui.seek_bar_dragging.blocking_lock();
                if !*dragging {
                    self.gui.seek_bar.set_value(progress as f64);
                }
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
                self.lyric_gui.lyric_display.tick();
            }
            UIUpdateEvent::UserListChanged => {
                let s = self.state.blocking_read();

                self.gui.users.clear();

                if let Some(c) = &s.connection {
                    if let Some(r) = &c.room {
                        self.gui.users.add(&format!("[{}]", r.name));

                        for (id, name) in r.connected_users.iter() {
                            let line = if id == &s.my_id {
                                "@b* you".to_string()
                            } else {
                                format!("* {} ({})", name, id)
                            };
                            self.gui.users.add(&line);
                        }
                    } else {
                        self.gui.users.add("no room");
                    }
                } else {
                    // not connected
                }

                drop(s);
                self.update_status();
            }
            UIUpdateEvent::RoomChanged => {}
            UIUpdateEvent::QueueChanged => {
                let s = self.state.blocking_read();

                self.queue_gui.queue_browser.clear();

                if let Some(c) = &s.connection {
                    if let Some(r) = &c.room {
                        // TODO: metadata stuff here is TOO LONG AND ANNOYING
                        if let Some(track) = &r.current_track() {
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
                                    .artist
                                    .as_ref()
                                    .unwrap_or(&"[unknown artist]".to_string()),
                            );

                            if let Some(art) = &track.metadata.art {
                                if let Err(err) = match art {
                                    TrackArt::Jpeg(data) => JpegImage::from_data(data).map(|img| {
                                        self.gui.art_frame.set_image(Some(img));
                                        self.gui.art_frame.redraw();
                                    }),
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

                            let bold = if r.current_track == track.id {
                                "@b"
                            } else {
                                ""
                            };

                            let line = format!(
                                "{bold}{}\t[{}] {}",
                                name.unwrap_or(&"?".into()),
                                min_secs(track.metadata.duration),
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

                drop(s);
                self.update_status();
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
        }
    }

    // set status based on state and priorities
    fn update_status(&mut self) {
        let status = self.get_status();
        self.gui.status_field.set_label(&status);

        // and the little thingy too
        let play_status = self.get_play_status();
        self.gui.play_status.update(play_status);

        // and probably just do the right status at the same time too
        // if we really want fine grained updates then we should do something else
        let right_status = self.get_right_status();
        self.gui.status_right_display.set_label(&right_status);
    }

    fn get_status(&self) -> String {
        let state = self.state.blocking_read();

        if let Some(connection) = &state.connection {
            if state.loading_count > 0 {
                if state.loading_count == 1 {
                    return "Loading 1 file...".to_string();
                } else {
                    return format!("Loading {} files...", state.loading_count);
                }
            }

            if let Some(r) = &connection.room {
                if r.buffering {
                    return "Buffering...".to_string();
                }

                if r.playback_state != PlaybackState::Stopped {
                    if let Some(t) = r.current_track() {
                        let verb = match r.playback_state {
                            PlaybackState::Playing => "Playing",
                            PlaybackState::Paused => "Paused",
                            _ => unreachable!(),
                        };

                        if t.owner == state.my_id {
                            return format!("{verb} (transmitting)");
                        } else {
                            let name = r.connected_users.get(&t.owner);
                            return format!(
                                "{verb} (receiving from {})",
                                name.unwrap_or(&"?".to_string())
                            );
                        }
                    }
                }

                return format!("Connected to {}:{}", connection.server.name, r.name);
            }

            format!("Connected to {}", connection.server.name)
        } else {
            "<not connected>".to_string()
        }
    }

    fn get_play_status(&self) -> PlayStatusIcon {
        let state = self.state.blocking_read();

        // i love shared and reusable logic ^-^
        if state.is_stopped() {
            PlayStatusIcon::Stop
        } else if state.is_paused() {
            PlayStatusIcon::Pause
        } else if state.is_transmitter() {
            PlayStatusIcon::PlayTx
        } else {
            PlayStatusIcon::PlayRx
        }
    }

    fn get_right_status(&self) -> String {
        let s = self.state.blocking_read();

        if let Some(seek_pos) = s.seek_position {
            return format!("[{}]", min_secs(seek_pos));
        }

        let (users, queue) = s
            .connection
            .as_ref()
            .and_then(|c| c.room.as_ref())
            .map(|r| (r.connected_users.len(), r.queue.len()))
            .unwrap_or_default();

        format!("U{:0>2} Q{:0>2}", users, queue)
    }
}

fn handle_window_drag(
    w: &mut DoubleWindow,
    ev: Event,
    state: &mut DragState,
    edges: Arc<RwLock<AllWindowEdges>>,
) -> bool {
    match ev {
        fltk::enums::Event::Push => {
            let coords = app::event_coords();
            state.x = coords.0;
            state.y = coords.1;

            if let Some(id) = state.id {
                edges.blocking_write().front_all();
                ui_send!(UIEvent::ReFrontAll(id));
                // bwehh i know.. sending ui events from a handler.. but how else do i get out of here
            }

            true
        }
        fltk::enums::Event::Drag => {
            // to only drag on title bar:
            //if y < 20 { logic } else return false
            if app::event_mouse_button() == MouseButton::Right {
                return false;
            };

            let mut x = app::event_x_root() - state.x;
            let mut y = app::event_y_root() - state.y;

            if let Some(id) = state.id {
                let edges = edges.blocking_read();

                let width = w.width();
                let height = w.height();

                let my_left = x;
                let my_right = x + w.width();
                let my_top = y;
                let my_bottom = y + w.height(); //..

                let threshold = 16;

                let valid_edges = edges
                    .0
                    .iter()
                    .enumerate()
                    // don't snap to ourself
                    .filter_map(|(i, o)| if i == id as usize { None } else { Some(o) })
                    // don't snap to non-visible windows
                    .filter(|e| e.shown);

                let mut left_right_edges = valid_edges
                    .clone()
                    // don't snap if we're not actually near enough in the other direction
                    .filter(|o| {
                        // we know the 2 pairs of left and right bounds so check if they overlap
                        //
                        //     - (currently dragging)
                        //     |
                        //     | - (the other one) (maybe)
                        //     | |
                        //     - |
                        //       |
                        //       -

                        (my_top > o.top && my_top < o.bottom)
                            || (my_bottom > o.top && my_bottom < o.bottom)
                            || (o.top > my_top && o.top < my_bottom)
                            || (o.bottom > my_top && o.bottom < my_bottom)
                    })
                    .flat_map(|o| {
                        [
                            // if our right is near their left, set our left to their left - our width
                            ((o.left - my_right).abs(), o.left - width),
                            // if our left is near their right, set our left to their right
                            ((o.right - my_left).abs(), o.right),
                        ]
                        .into_iter()
                    })
                    .chain(
                        valid_edges
                            .clone()
                            .filter(|o| {
                                // if we are slightly under or above them, we want to snap to the inside left/right
                                ((my_top - o.bottom).abs() < threshold)
                                    || ((o.top - my_bottom).abs() < threshold)
                            })
                            .flat_map(|o| {
                                [
                                    ((my_left - o.left).abs(), o.left),
                                    ((my_right - o.right).abs(), o.right - width),
                                ]
                                .into_iter()
                            }),
                    )
                    .collect::<Vec<(i32, i32)>>();

                // snap to the nearest one
                left_right_edges.sort();
                if let Some(edge) = left_right_edges.first() {
                    if edge.0 < threshold {
                        x = edge.1;
                    }
                }

                let mut top_bottom_edges = valid_edges
                    .clone()
                    .filter(|o| {
                        // |----------|
                        //       |--------|
                        (my_left > o.left && my_left < o.right)
                            || (my_right > o.left && my_right < o.right)
                            || (o.left > my_left && o.left < my_right)
                            || (o.right > my_left && o.right < my_right)
                    })
                    .flat_map(|o| {
                        [
                            // if our bottom is near their top, set our top to their top - our height
                            ((o.top - my_bottom).abs(), o.top - height),
                            // if our top is near their bottom, set our top to their bottom
                            ((o.bottom - my_top).abs(), o.bottom),
                        ]
                        .into_iter()
                    })
                    .chain(
                        valid_edges
                            .clone()
                            .filter(|o| {
                                // if we are slightly left or right of them, we want to snap to the inside top/bottom
                                ((my_left - o.right).abs() < threshold)
                                    || ((o.left - my_right).abs() < threshold)
                            })
                            .flat_map(|o| {
                                [
                                    ((my_top - o.top).abs(), o.top),
                                    ((my_bottom - o.bottom).abs(), o.bottom - height),
                                ]
                                .into_iter()
                            }),
                    )
                    .collect::<Vec<(i32, i32)>>();

                top_bottom_edges.sort();
                if let Some(edge) = top_bottom_edges.first() {
                    if edge.0 < threshold {
                        y = edge.1;
                    }
                }
            }

            w.set_pos(x, y);

            if let Some(id) = state.id {
                let mut edges = edges.blocking_write();
                edges.update_window(id, w);
                edges.front(id); // single front on drag seems to be needed for edge cases that idk
            }

            true
        }
        _ => false,
    }
}

fn handle_window_dnd(_w: &mut DoubleWindow, ev: Event, state: &mut DndState) -> bool {
    match ev {
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
                ui_send!(UIEvent::DroppedFiles(app::event_text()));
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
        _ => false,
    }
}

fn handle_volume_scroll(_w: &mut DoubleWindow, ev: Event) -> bool {
    match ev {
        fltk::enums::Event::MouseWheel => {
            let dy = app::event_dy();
            match dy {
                // uhhhhhhhh these are the opposite of what you'd think (???)
                app::MouseWheel::Up => {
                    ui_send!(UIEvent::VolumeDown);
                    true
                }
                app::MouseWheel::Down => {
                    ui_send!(UIEvent::VolumeUp);
                    true
                }
                _ => false,
            }
        }
        _ => false,
    }
}

fn handle_window_misc(_w: &mut DoubleWindow, ev: Event) -> bool {
    match ev {
        Event::Released => {
            if app::event_mouse_button() == MouseButton::Right {
                let x = app::event_x_root();
                let y = app::event_y_root();
                ui_send!(UIEvent::PopContextMenu((x, y)));
                true
            } else {
                false
            }
        }
        fltk::enums::Event::Shortcut => {
            let key = app::event_key();
            let f1: bool = key.bits() == Key::F1.bits();
            let p = Key::from_char('p').bits();

            if key.bits() == Key::Escape.bits() {
                return true;
            }

            if key.bits() == Key::from_char('p').bits() && app::is_event_ctrl() {
                ui_send!(UIEvent::ShowPrefsWindow);
                true
            } else {
                false
            }
        }
        _ => false,
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

fn add_bar(win: &mut DoubleWindow, close_message: UIEvent, title: &str, with_icon: bool) {
    let mut bar = Group::new(3, 3, win.width() - 8, 17, "");
    let mut bar_bg = create_horizontal_gradient_frame(
        3,
        3,
        win.width() - 8,
        17,
        Color::from_rgb(8, 36, 107),    //Color::from_rgb(56, 85, 145),
        Color::from_rgb(165, 203, 247), //Color::from_rgb(166, 202, 240),
    );
    if with_icon {
        let mut icon = Group::new(4, 4, 16, 16, "");
        icon.set_image(Some(
            fltk::image::PngImage::from_data(include_bytes!("../rsrc/tiny_ryo_2.png")).unwrap(),
        ));
        icon.set_align(Align::ImageBackdrop);

        bar.add(&icon);
    }

    let t = title.to_owned();
    let mut bar_title = Frame::new(if with_icon { 20 } else { 7 }, 3, 100, 17, "").with_label(&t);
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
    bar_btn_close.emit(sender!(), close_message);
    bar.add(&bar_btn_close);

    bar.end();
    win.add(&bar);
}

fn min_secs(secs: f32) -> String {
    let int_secs = secs as u32;
    format!("{:02}:{:02}", int_secs / 60, int_secs % 60)
}
