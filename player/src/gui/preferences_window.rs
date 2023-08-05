use fltk::button::*;
use fltk::enums::*;
use fltk::input::Input;
use fltk::menu::Choice;
use fltk::prelude::*;
use fltk::window::*;

use crate::preferences::PreferencesData;

use super::add_bar;
use super::UIEvent;
use super::group_box::GroupBox;
use super::sender;

pub struct PrefsWindow {
    pub main_win: Window,
    pub fld_name: Input,
    pub tmp_save_button: Button,
}

impl PrefsWindow {
    pub fn make_window() -> Self {
        let mut main_win = Window::new(100, 100, 330, 274, "Preferences");
        main_win.set_frame(FrameType::UpBox);

        add_bar(
            &mut main_win,
            UIEvent::HidePrefsWindow,
            "Preferences",
        );
        main_win.set_border(false);

        let mut g1 = GroupBox::new(8,34,main_win.w()-16,120).with_label("Customization");
        main_win.add(&*g1);
        let (x, y) = (g1.x(), g1.y());
        // name field
        let fld_name = Input::new(x+95, y+12, 140, 20, "Username:").with_align(Align::Left);
        g1.add(&fld_name);

        // theme combobox
        let mut fld_themesel = Choice::new(x+95, 0, 140, 20, "Theme:").with_align(Align::Left).below_of(&fld_name, 6);
        fld_themesel.add_choice("Default (w2k)");
        fld_themesel.add_choice("_ ");
        fld_themesel.add_choice("Get more!");
        fld_themesel.set_value(0);
        g1.add(&fld_themesel);

        g1.end();

        let mut g2 = GroupBox::new(8,0,main_win.w()-16,100).with_label("Advanced").below_of(&*g1, 10);
        let (x, y) = (g2.x(), g2.y());

        // todo: some fancy auto-save modeless dialog
        let mut tmp_save_button = Button::new(x+10, y+10, 85, 25, "saveeee");
        tmp_save_button.emit(sender!(), UIEvent::PleaseSavePreferencesWithThisData); // this is...... silly but idek
        
        main_win.add(&*g2);
        g2.end();


        Self {
            main_win,
            fld_name,
            tmp_save_button
        }
    }

    pub fn load_state(&mut self, p: &PreferencesData) {
        self.fld_name.set_value(&p.name);
    }

}