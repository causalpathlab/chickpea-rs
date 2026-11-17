//! What the terminal front ends, `chickpea run` and `chickpea view`, share:
//! the tool their sessions belong to, the screen modes, list hits and key
//! hints, and the refusal to open a screen with no terminal.

/// The tool `chickpea run` and `chickpea view` sessions belong to: they are
/// named `chickpea-run-1`, `chickpea-view-2`, and `chickpea attach` finds
/// them.
pub const TOOL: legume_tui::Tool = legume_tui::Tool::new("chickpea");

/// The modes of a screen: keys with their modifiers where the terminal
/// reports them, and the mouse's buttons and wheel (shift+drag still
/// selects text in most terminals).
#[must_use]
pub fn modes() -> legume_tui::Modes {
    use ratatui::crossterm::event::KeyboardEnhancementFlags;
    legume_tui::Modes::new()
        .keyboard_enhancement(legume_tui::Keyboard::IfSupported(
            KeyboardEnhancementFlags::DISAMBIGUATE_ESCAPE_CODES,
        ))
        .mouse(legume_tui::Mouse::Buttons)
}

/// The detach and switch keys as a help line lists them; none when the
/// screen runs plain.
pub fn session_keys() -> impl Iterator<Item = (String, String)> {
    legume_tui::help_keys()
        .into_iter()
        .map(|k| (k.key.to_lowercase(), k.short))
}

/// A screen's key hints, the session's keys after them, as one line.
#[must_use]
pub fn key_line(keys: Vec<(String, String)>) -> String {
    keys.into_iter()
        .chain(session_keys())
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

/// A wheel turn as a step through a list: up −1, down +1.
#[must_use]
pub fn wheel(m: &ratatui::crossterm::event::MouseEvent) -> Option<isize> {
    use ratatui::crossterm::event::MouseEventKind;
    match m.kind {
        MouseEventKind::ScrollUp => Some(-1),
        MouseEventKind::ScrollDown => Some(1),
        _ => None,
    }
}

/// Whether `m` is a press of the left button.
#[must_use]
pub fn click(m: &ratatui::crossterm::event::MouseEvent) -> bool {
    use ratatui::crossterm::event::{MouseButton, MouseEventKind};
    m.kind == MouseEventKind::Down(MouseButton::Left)
}

/// Refuse to open a screen with no terminal to draw it on (input or output
/// piped, as under an agent), saying what works `instead`. A session's own
/// program draws on its host's terminal.
pub fn need_terminal(instead: &str) -> anyhow::Result<()> {
    use std::io::IsTerminal;
    if legume_tui::in_session() {
        return Ok(());
    }
    anyhow::ensure!(
        std::io::stdin().is_terminal() && std::io::stdout().is_terminal(),
        "no terminal to draw the screen on: {instead}"
    );
    Ok(())
}
