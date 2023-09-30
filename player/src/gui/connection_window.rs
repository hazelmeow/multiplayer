use std::ops::Deref;
use std::sync::Arc;

use fltk::browser::*;
use fltk::button::*;
use fltk::enums::*;
use fltk::group::Group;
use fltk::input::Input;
use fltk::prelude::*;
use fltk::tree::Tree;
use fltk::tree::TreeItem;
use fltk::tree::TreeReason;
use fltk::window::*;
use tokio::sync::RwLock;

use crate::preferences::Server;
use crate::State;

use super::add_bar;
use super::sender;
use super::ui_send;
use super::ConnectionDlgEvent;
use super::UIEvent;

pub struct ConnectionWindow {
    pub main_win: Window,
    pub tree: Tree,
    state: Arc<RwLock<State>>,
}

impl ConnectionWindow {
    pub fn make_window(state: Arc<RwLock<State>>) -> Self {
        let mut main_win = Window::new(100, 100, 330, 300, "queue");
        main_win.set_frame(FrameType::UpBox);

        add_bar(
            &mut main_win,
            UIEvent::HideConnectionWindow,
            "Connect to Server",
            false,
        );
        main_win.set_border(false);

        /* // very temp
        let mut addr_bar = Input::new(10,40,150,25,"Address").with_align(Align::TopLeft);
        let tmp = s.clone();
        addr_bar.set_trigger(CallbackTrigger::EnterKey);
        addr_bar.set_callback(move |bar| {
            tmp.send(UIEvent::Connect(bar.value()));
        }); */

        let mut btn_connect = Button::new(235, 32, 85, 25, "Connect");
        btn_connect.emit(
            sender!(),
            UIEvent::ConnectionDlg(ConnectionDlgEvent::BtnConnect),
        );
        main_win.add(&btn_connect);

        let mut btn_new_room = Button::new(0, 0, 85, 25, "New Room").below_of(&btn_connect, 4);
        btn_new_room.emit(
            sender!(),
            UIEvent::ConnectionDlg(ConnectionDlgEvent::BtnNewRoom),
        );
        main_win.add(&btn_new_room);

        let mut btn_refresh = Button::new(0, 0, 85, 25, "Refresh").below_of(&btn_new_room, 4);
        btn_refresh.emit(
            sender!(),
            UIEvent::ConnectionDlg(ConnectionDlgEvent::BtnRefresh),
        );
        main_win.add(&btn_refresh);

        let mut tree = Tree::default().with_size(210, 230).with_pos(10, 32);
        tree.set_show_root(false);

        // tree.add("");

        tree.set_trigger(CallbackTrigger::Release);
        tree.set_item_reselect_mode(fltk::tree::TreeItemReselectMode::Always);

        tree.set_callback(move |t| {
            if let Some(mut selected) = t.first_selected_item() {
                match t.callback_reason() {
                    TreeReason::Selected => match selected.depth() {
                        1 => {
                            btn_connect.activate();
                            btn_connect.set_label("Connect");
                            btn_new_room.activate();
                        }
                        2 => {
                            btn_connect.activate();
                            btn_connect.set_label("Join");
                            btn_new_room.activate();
                        }
                        // clicking room users selects room
                        3 => {
                            selected.deselect();
                            selected.parent().unwrap().select_toggle();
                        }
                        _ => unreachable!(),
                    },
                    TreeReason::Reselected => match selected.depth() {
                        1 | 2 => ui_send!(UIEvent::ConnectionDlg(ConnectionDlgEvent::BtnConnect)),
                        _ => {}
                    },
                    _ => {}
                }
            } else {
                btn_connect.deactivate();
                btn_connect.set_label("Connect");
                btn_new_room.deactivate();
            }
        });
        main_win.add(&tree);

        main_win.end();

        Self {
            main_win,
            tree,
            state,
        }
    }

    pub fn populate(&mut self, list: Vec<ServerStatus>) {
        let tree = &mut self.tree;
        tree.clear();
        tree.begin();
        for server in list {
            let mut item = tree.add(&server.name).unwrap();
            item.set_user_data(server.inner.clone());

            Self::populate_server_item(self.state.clone(), tree, item, server);
        }
        tree.end();
        tree.redraw();
    }

    pub fn update_just_one_server(&mut self, server: ServerStatus) {
        let tree = &mut self.tree;
        if let Some(mut item) = tree.find_item(&server.name) {
            item.clear_children();

            Self::populate_server_item(self.state.clone(), tree, item, server)
        } else {
            println!("couldn't find {} ????", server.name);
        }
        tree.redraw();
    }

    pub fn update_connected(&mut self) {
        let state = self.state.blocking_read();
        let tree = &mut self.tree;
        for mut item in tree.get_items().unwrap_or_default() {
            if item.depth() == 1 {
                unsafe {
                    let addr: String = item.user_data().unwrap();
                    item.set_label_font(Font::Helvetica);
                    if let Some(c) = &state.connection {
                        if c.server.addr == addr {
                            item.set_label_font(Font::HelveticaBold);
                        }
                    }
                }
            }
        }
        tree.redraw();
    }

    fn populate_server_item(
        state: Arc<RwLock<State>>,
        tree: &mut Tree,
        mut item: TreeItem,
        server: ServerStatus,
    ) {
        // for paths to work right
        item.set_label(&server.addr);

        if let Some(rooms) = &server.rooms {
            for room in rooms {
                let line = &format!("{}/{}", server.addr, room.id);
                if let Some(mut sub_item) = tree.add(&line) {
                    sub_item.set_user_data(room.id);
                    sub_item.close();
                    for user in &room.user_names {
                        tree.add(&format!("{}/{}", line, user));
                    }
                    let actual_line = &format!("{} ({})", room.name, room.user_names.len());
                    sub_item.set_label(actual_line);
                }
            }
        } else {
            if server.tried {
                tree.add(&format!("{}/(unable to connect)", server.addr));
            } else {
                tree.add(&format!("{}/(querying...)", server.addr));
            }
        }

        // connected indicator
        let state = state.blocking_read();
        item.set_label(&server.name);
        item.set_label_font(Font::Helvetica);
        if let Some(c) = &state.connection {
            if c.server.addr == server.addr {
                // text doesn't look that good and also it breaks tree.find_item...
                item.set_label(&format!("{}", &server.name));
                item.set_label_font(Font::HelveticaBold);
            }
        }
    }
}

#[derive(Debug, Clone)]
pub struct ServerStatus {
    pub inner: Server,

    pub rooms: Option<Vec<protocol::RoomListing>>,
    pub tried: bool, // we want to list them without showing unable to connect if we didn't try yet
}

impl Deref for ServerStatus {
    type Target = Server;

    fn deref(&self) -> &Self::Target {
        &self.inner
    }
}

impl From<Server> for ServerStatus {
    fn from(value: Server) -> Self {
        ServerStatus {
            inner: value,
            rooms: None,
            tried: false,
        }
    }
}
