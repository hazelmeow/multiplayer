use fltk::app::Sender;
use fltk::browser::*;
use fltk::button::*;
use fltk::enums::*;
use fltk::group::Group;
use fltk::input::Input;
use fltk::prelude::*;
use fltk::tree::Tree;
use fltk::tree::TreeItem;
use fltk::window::*;

use super::add_bar;
use super::ConnectionDlgEvent;
use super::UIEvent;

pub struct ConnectionWindow {
    pub main_win: Window,
    pub tree: Tree,
}

impl ConnectionWindow {
    pub fn make_window(s: Sender<UIEvent>) -> Self {
        let mut main_win = Window::new(100, 100, 330, 300, "queue");
        main_win.set_frame(FrameType::UpBox);

        add_bar(
            &mut main_win,
            s.clone(),
            UIEvent::HideConnectionWindow,
            "Connect to Server",
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
            s.clone(),
            UIEvent::ConnectionDlg(ConnectionDlgEvent::BtnConnect),
        );
        main_win.add(&btn_connect);

        let mut btn_new_room = Button::new(0, 0, 85, 25, "New Room").below_of(&btn_connect, 4);
        btn_new_room.emit(
            s.clone(),
            UIEvent::ConnectionDlg(ConnectionDlgEvent::BtnNewRoom),
        );
        main_win.add(&btn_new_room);

        let mut btn_refresh = Button::new(0, 0, 85, 25, "Refresh").below_of(&btn_new_room, 4);
        btn_refresh.emit(
            s.clone(),
            UIEvent::ConnectionDlg(ConnectionDlgEvent::BtnRefresh),
        );
        main_win.add(&btn_refresh);

        let mut tree = Tree::default().with_size(210, 230).with_pos(10, 32);
        tree.set_show_root(false);

        tree.add("unpopulated");

        tree.set_trigger(CallbackTrigger::Release);
        tree.set_callback(move |t| {
            if let Some(mut selected) = t.first_selected_item() {
                if selected.depth() == 3 {
                    selected.deselect();
                    selected.parent().unwrap().select_toggle();
                }
                btn_connect.activate();
            } else {
                btn_connect.deactivate();
            }
        });
        main_win.add(&tree);

        main_win.end();

        Self { main_win, tree }
    }

    pub fn populate(&mut self, list: Vec<Server>) {
        let tree = &mut self.tree;
        tree.clear();
        tree.begin();
        for server in list {
            let mut item = tree.add(&server.name).unwrap();
            item.set_user_data(server.addr);
            if let Some(rooms) = &server.rooms {
                for room in rooms {
                    let line = &format!("{}/{}", server.name, room.id);
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
                tree.add(&format!("{}/(unable to connect)", server.name));
            }
        }
        tree.end();
        tree.redraw();
    }

    pub fn update_just_one_server(&mut self, server: Server) {
        let tree = &mut self.tree;
        if let Some(mut item) = tree.find_item(&server.name) {
            item.clear_children();

            if let Some(rooms) = &server.rooms {
                for room in rooms {
                    let line = &format!("{}/{}", server.name, room.id);
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
                eprintln!("uhhhhhh should never happen,,,,,,,,,,,");
            }

        } else {
            println!("couldn't find {} ????", server.name);
        }
        tree.redraw();
    }

}

#[derive(Debug, Clone)]
pub struct Server {
    pub name: String,
    pub addr: String,
    pub rooms: Option<Vec<protocol::RoomListing>>,
}