use std::cell::RefCell;
use std::rc::Rc;

use fltk::draw;
use fltk::enums::*;
use fltk::prelude::*;
use fltk::widget;
use fltk::widget_extends;
use spectrum_analyzer;
use spectrum_analyzer::scaling::scale_to_zero_to_one;

pub struct Visualizer {
    inner: widget::Widget,
    values: Rc<RefCell<[u8; 14]>>, // 14 bars (height 0 to 8)
    prev_vals: [u8; 14],
    timing: bool,
}

widget_extends!(Visualizer, widget::Widget, inner);

impl Visualizer {
    pub fn new(x: i32, y: i32) -> Self {
        let mut inner = widget::Widget::default().with_size(41, 20).with_pos(x, y);
        inner.set_frame(FrameType::FlatBox);

        let values = [5; 14];
        let values = Rc::from(RefCell::from(values));
        let v = values.clone();

        inner.draw(move |i| {
            draw::draw_box(i.frame(), i.x(), i.y(), i.w(), i.h(), i.color());
            draw::set_draw_color(Color::Black);
            draw::draw_xyline(i.x(), i.y() + 16, i.x() + (14 * 3) - 2);
            for bar in 0..14 {
                let value = v.borrow()[bar] as i32;
                for val in 0..value {
                    let x = i.x() + bar as i32 * 3;
                    let y = i.y() + (7 - val) * 2;
                    draw::draw_xyline(x, y, x + 1);
                }
            }
        });

        Self {
            inner,
            values,
            timing: false,
            prev_vals: [0; 14],
        }
    }
    pub fn update_values(&mut self, values: [u8; 14]) {
        {
            let prev = self.values.borrow().clone();
            let mut sv = self.values.borrow_mut();
            let temp = sv.clone();
            for i in 0..14 {
                if values[i] > prev[i] {
                    sv[i] = ((prev[i] as usize + self.prev_vals[i] as usize + values[i] as usize)
                        / 3 as usize) as u8;
                } else if sv[i] > 1 {
                    if self.timing {
                        sv[i] = sv[i] - 1;
                    }
                } else {
                    sv[i] = 0;
                }
            }
            self.timing = !self.timing;
            self.prev_vals = temp;
        }
        self.redraw();
    }
}

pub fn calculate_visualizer(samples: &[f32; 4096]) -> [u8; 14] {
    let hamming_window = spectrum_analyzer::windows::hamming_window(samples);
    let spectrum = spectrum_analyzer::samples_fft_to_spectrum(
        &hamming_window,
        48000,
        spectrum_analyzer::FrequencyLimit::All,
        Some(&scale_to_zero_to_one),
    )
    .unwrap();

    let mut bars_float = [0.0; 14];
    let min_freq = 20.0;
    let max_freq = 20000.0;

    for (fr, fr_val) in spectrum.data().iter() {
        let fr = fr.val();
        if fr < 20.0 {
            continue;
        }

        let index = (-5.35062 * fr.log(0.06) - 6.36997).round().min(13.0) as usize;
        //println!("{}, {}, {}", index, fr);

        // made up scaling based on my subjective opinion on what looks kinda fine i guess
        bars_float[index] += fr_val.val() * (4.5 - fr.log10());
    }
    //println!("{:?}", bars_float);
    let bars: [u8; 14] = bars_float
        .iter()
        .map(|&num| (num as f32).round().min(8.0) as u8)
        .collect::<Vec<u8>>()
        .try_into()
        .unwrap();
    //println!("{:?}", bars);
    bars
}
