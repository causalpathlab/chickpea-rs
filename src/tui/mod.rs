//! What the terminal front ends, `chickpea run` and `chickpea view`, share:
//! the tool their sessions belong to, the screen modes, list hits and key
//! hints.

/// The tool `chickpea run` and `chickpea view` sessions belong to: they are
/// named `chickpea-run-1`, `chickpea-view-2`, and `chickpea attach` finds
/// them.
pub const TOOL: legume_tui::Tool = legume_tui::Tool::new("chickpea");

/// The modes of a screen: keys with their modifiers where the terminal
/// reports them, and the mouse's buttons and wheel (shift+drag still
/// selects text in most terminals).
#[must_use]
pub fn modes() -> legume_tui::Modes {
    legume_tui::Modes::disambiguated().mouse(legume_tui::Mouse::Buttons)
}

/// A screen's key hints, the session's keys after them, as one line.
#[must_use]
pub fn key_line(keys: Vec<(String, String)>) -> String {
    keys.into_iter()
        .chain(legume_tui::view::session_keys())
        .map(|(k, what)| format!("{k} {what}"))
        .collect::<Vec<_>>()
        .join(" · ")
}

/// Where a list was drawn: the area of its rows and the first item shown,
/// so a click finds its item.
#[derive(Clone, Copy, Debug, Default)]
pub struct ListHit {
    pub rows: ratatui::layout::Rect,
    pub first: usize,
}

impl ListHit {
    /// The item drawn at `(x, y)`, if the point is on the list.
    #[must_use]
    pub fn row_at(&self, x: u16, y: u16) -> Option<usize> {
        self.rows
            .contains((x, y).into())
            .then(|| self.first + usize::from(y - self.rows.y))
    }
}

/// A wheel turn as a step through a list: up −1, down +1 (legume-tui's
/// [`legume_tui::keys::wheel_key`], as a step).
#[must_use]
pub fn wheel(m: &ratatui::crossterm::event::MouseEvent) -> Option<isize> {
    legume_tui::keys::wheel_key(m).map(|k| {
        if k.code == ratatui::crossterm::event::KeyCode::Up {
            -1
        } else {
            1
        }
    })
}

/// Whether `m` is a press of the left button: legume-tui's
/// [`legume_tui::keys::is_click_or_wheel`], not a turn of the wheel.
#[must_use]
pub fn click(m: &ratatui::crossterm::event::MouseEvent) -> bool {
    legume_tui::keys::is_click_or_wheel(m) && legume_tui::keys::wheel_key(m).is_none()
}
