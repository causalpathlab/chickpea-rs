//! The run screen's mouse: the wheel, a click on a tab or on a row.

use super::{App, Outcome, Pane};
use crate::tui::wheel;
use legume_tui::keys::{is_click_or_wheel, wheel_key};
use ratatui::crossterm::event::{KeyCode, KeyEvent, MouseEvent};

impl App {
    /// Take the mouse: the wheel scrolls what is under the cursor's pane,
    /// a click on a tab opens its pane, a click on a row puts the cursor
    /// there, and a click on the row it is on changes it, as enter does.
    pub fn mouse(&mut self, m: MouseEvent, env: &legume_tui::ChildEnv) {
        if !is_click_or_wheel(&m) {
            return;
        }
        if let Some(pick) = self.pick.as_mut() {
            let out = match wheel(&m) {
                Some(d) => {
                    pick.scroll(d * 3);
                    Outcome::Open
                }
                None => pick.click(m.column, m.row),
            };
            return self.picked(out);
        }
        if self.editing.is_some() {
            return;
        }
        if let Some(k) = wheel_key(&m) {
            // As the arrow keys, but the log three lines a turn.
            let turns = if self.pane == Pane::Run { 3 } else { 1 };
            for _ in 0..turns {
                self.key(k, env);
            }
            return;
        }
        let drawn = self.drawn.borrow().clone();
        if m.row == 0 {
            if let Some((pane, _)) = drawn.tabs.iter().find(|(_, r)| r.contains(&m.column)) {
                self.pane = *pane;
            }
            return;
        }
        let Some(i) = drawn.list.row_at(m.column, m.row) else {
            return;
        };
        match self.pane {
            Pane::Data => {
                let Some(d) = self.data.as_mut().filter(|d| i <= d.pairs.len()) else {
                    return;
                };
                let col = drawn.cols.iter().rposition(|&x| m.column >= x).unwrap_or(0);
                if i == d.at && (col == d.col || i == d.pairs.len()) {
                    self.data_key(KeyEvent::from(KeyCode::Enter));
                } else {
                    d.at = i;
                    d.col = col;
                }
            }
            Pane::Form if i < self.form.fields.len() => {
                if i == self.at {
                    self.change();
                } else {
                    self.at = i;
                }
            }
            _ => {}
        }
    }
}
