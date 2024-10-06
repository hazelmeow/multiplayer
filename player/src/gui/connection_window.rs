use crate::{
    preferences::Server,
    state::{Action, State},
};
use std::collections::HashMap;
use std::sync::Arc;
use uuid::Uuid;

use super::add_bar;
use super::sender;
use super::ui_send;
use super::ConnectionDlgEvent;
use super::UIEvent;

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
        btn_refresh.emit(sender!(), UIEvent::Action(Action::RefreshServerStatuses));
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

    pub fn update_list(
        &mut self,
        connected_server_id: Option<Uuid>,
        servers: &[Server],
        statuses: &HashMap<Uuid, ServerStatus>,
    ) {
        let tree = &mut self.tree;
        tree.clear();
        tree.begin();
        for server in servers {
            let status = statuses
                .get(&server.id)
                .cloned()
                .unwrap_or(ServerStatus::Unknown);
            let mut item = tree.add(&server.name).unwrap();
            item.set_user_data(server.id);

            Self::update_list_item(connected_server_id, tree, item, server, &status);
        }
        tree.end();
        tree.redraw();
    }

    fn update_list_item(
        connected_server_id: Option<Uuid>,
        tree: &mut Tree,
        mut item: TreeItem,
        server: &Server,
        status: &ServerStatus,
    ) {
        // for paths to work right
        item.set_label(&server.addr);

        match status {
            ServerStatus::Unknown => {
                tree.add(&format!("{}/(querying...)", server.addr));
            }
            ServerStatus::Error => {
                tree.add(&format!("{}/(unable to connect)", server.addr));
            }
            ServerStatus::Success { rooms } => {
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
            }
        }

        item.set_label(&server.name);
        item.set_label_font(Font::Helvetica);

        // connected indicator
        if connected_server_id == Some(server.id) {
            // text doesn't look that good and also it breaks tree.find_item...
            item.set_label(&format!("{}", &server.name));
            item.set_label_font(Font::HelveticaBold);
        }
    }

    pub fn update_connected(&mut self, connected_server_id: Option<Uuid>) {
        for mut item in self.tree.get_items().unwrap_or_default() {
            if item.depth() == 1 {
                // TODO: maybe we can do this safer by storing a wrapper with a TypeId or something...
                let id: Uuid = unsafe { item.user_data().unwrap() };

                item.set_label_font(Font::Helvetica);
                if connected_server_id == Some(id) {
                    item.set_label_font(Font::HelveticaBold);
                }
            }
        }
        self.tree.redraw();
    }
}

// TODO: move this enum definition to state.rs
#[derive(Debug, Clone)]
pub enum ServerStatus {
    Unknown,
    Error,
    Success { rooms: Vec<protocol::RoomListing> },
}
