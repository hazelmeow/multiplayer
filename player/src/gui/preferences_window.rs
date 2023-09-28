use crate::gui::ui_send;
use fltk::button::*;
use fltk::enums::*;
use fltk::frame::Frame;
use fltk::image::Image;
use fltk::input::Input;
use fltk::menu::Choice;
use fltk::prelude::*;
use fltk::tree::Tree;
use fltk::window::*;
use std::collections::HashMap;

use super::group_box::GroupBox;
use super::sender;
use super::UIEvent;
use crate::preferences::PreferencesData;

const PAGE_X_OFFSET: i32 = 170;

#[derive(Default)]
pub struct PrefItems {
    pub fld_name: Option<Input>,
}

pub struct PrefsWindow {
    pub main_win: Window,
    pub tree: Tree,
    pub pages: HashMap<String, DoubleWindow>,
    pub items: PrefItems,
}

impl PrefsWindow {
    pub fn make_window() -> Self {
        // TODO: stop calling this "main_win" it's so silly
        let mut main_win = Window::new(100, 100, 450, 300, "multiplayer preferences");
        main_win.make_modal(false); // ???
        main_win.free_position();
        main_win.emit(sender!(), UIEvent::PleaseSavePreferencesWithThisData);

        let mut tree = Tree::new(10, 10, 150, main_win.h() - 44, "");
        {
            tree.set_show_root(false);
            tree.set_item_label_font(Font::Helvetica);
            tree.set_selection_color(Color::from_rgb(0, 120, 215)); // temp: windows default highlight color
            tree.set_callback(move |t| {
                if let Some(mut selected) = t.first_selected_item() {
                    if t.callback_reason() == fltk::tree::TreeReason::Selected {
                        match selected.depth() {
                            1 => {
                                // force 2nd level (opposite of the connection dialog)
                                selected.deselect();
                                let mut was_selected = selected;
                                was_selected.open();
                                was_selected.child(0).unwrap().select_toggle();
                                ui_send!(UIEvent::SwitchPrefsPage(format!(
                                    "{}/{}",
                                    was_selected.label().unwrap(),
                                    was_selected.child(0).unwrap().label().unwrap()
                                )))
                            }
                            2 => {
                                ui_send!(UIEvent::SwitchPrefsPage(format!(
                                    "{}/{}",
                                    selected.parent().unwrap().label().unwrap(),
                                    selected.label().unwrap()
                                )))
                            }
                            _ => unreachable!(),
                        }
                    }
                }
            });
            tree.add("General/Identity").unwrap().select_toggle();
            tree.add("General/Meow");
        }
        main_win.add(&tree);

        let mut btn_close = Button::new(10, -1, 150, 21, "Save and Close").below_of(&tree, 4);
        btn_close.emit(sender!(), UIEvent::PleaseSavePreferencesWithThisData);

        let mut items = PrefItems::default();

        let pages = vec![
            Self::page_general_identity(&mut main_win, &mut items),
            Self::page_general_meow(&mut main_win, &mut items),
        ]
        .into_iter()
        .map(|mut x| {
            x.1.hide(); // hide all by default
            x
        })
        .map(|x| (x.0, x.1))
        .collect();

        main_win.end(); // lol

        Self {
            main_win,
            tree,
            pages,
            items,
        }
    }

    pub fn load_state(&mut self, p: &PreferencesData) {
        self.items.fld_name.as_mut().unwrap().set_value(&p.name);
        // TODO: load last page
        self.switch_page("General/Identity");
    }

    pub fn switch_page(&mut self, path: &str) {
        for p in &mut self.pages {
            if *p.0 == path {
                p.1.show();
            } else {
                p.1.hide();
            }
        }
    }

    fn page_general_identity(win: &mut Window, items: &mut PrefItems) -> (String, DoubleWindow) {
        let path = "General/Identity";

        let mut w = DoubleWindow::new(
            PAGE_X_OFFSET,
            10,
            win.w() - PAGE_X_OFFSET - 10,
            win.h() - 20,
            "Identity Setup",
        )
        .with_label("Identity Setup");

        {
            // CODE CODE CODE, WIDGETS AND SUCH
            let mut g1 = GroupBox::new(0, 8, w.w(), w.h() - 10).with_label("Identity");
            w.add(&*g1);
            let (x, y, w) = (g1.x(), g1.y(), g1.w() - 30);
            // name field
            let fld_name =
                Input::new(x + 10, y + 30, w, 20, "Username:").with_align(Align::TopLeft);
            g1.add(&fld_name);

            // // theme combobox
            // let mut fld_themesel = Choice::new(x + 95, 0, 140, 20, "Theme:")
            //     .with_align(Align::Left)
            //     .below_of(&fld_name, 6);
            // fld_themesel.add_choice("Default (w2k)");
            // fld_themesel.add_choice("_ ");
            // fld_themesel.add_choice("Get more!");
            // fld_themesel.set_value(0);
            // g1.add(&fld_themesel);

            g1.end();

            items.fld_name = Some(fld_name);
        }

        w.end();
        win.add(&w);
        (path.into(), w)
    }

    fn page_general_meow(win: &mut Window, items: &mut PrefItems) -> (String, DoubleWindow) {
        let path = "General/Meow";

        let mut w = DoubleWindow::new(
            PAGE_X_OFFSET,
            10,
            win.w() - PAGE_X_OFFSET - 10,
            win.h() - 20,
            "Meow Setup",
        )
        .with_label("Meow Setup");

        {
            // CODE CODE CODE, WIDGETS AND SUCH
            let lbl_1 = Frame::new(30, 30, 0, 0, "meowww");
            w.add(&lbl_1);
        }

        w.end();
        win.add(&w);
        (path.into(), w)
    }
}
