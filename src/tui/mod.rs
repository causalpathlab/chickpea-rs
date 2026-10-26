//! What the terminal front ends, `chickpea run` and `chickpea view`, share:
//! the tool their sessions belong to, the screen modes, and how a screen
//! takes the terminal and gives it back.

use std::path::Path;

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

/// The key that runs what a screen set up, as in senna and pinto: ctrl+r.
pub const RUN_KEY: &str = "ctrl+r";

/// Whether `k` is [`RUN_KEY`].
#[must_use]
pub fn is_run_key(k: &ratatui::crossterm::event::KeyEvent) -> bool {
    use ratatui::crossterm::event::{KeyCode, KeyModifiers};
    k.code == KeyCode::Char('r') && k.modifiers.contains(KeyModifiers::CONTROL)
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

/// Run `f` on the whole terminal of `session` and give the terminal back
/// however it ends. Logging is off meanwhile: log lines would land over the
/// screen. An error it ends with is held where it is seen
/// ([`legume_tui::Session::hold_error`]).
pub fn with_terminal<T>(
    session: &legume_tui::Session,
    f: impl FnOnce(&mut ratatui::DefaultTerminal) -> anyhow::Result<T>,
) -> anyhow::Result<T> {
    let _quiet = legume_tui::logs::quiet();
    let mut terminal = session.terminal_with(modes())?;
    let out = f(&mut terminal);
    if let Err(e) = &out {
        let _ = session.hold_error(&mut terminal, e);
    }
    session.restore();
    out
}

/// `p` as shown: relative to the working directory when it is under it.
#[must_use]
pub fn shown(p: &Path) -> String {
    std::env::current_dir()
        .ok()
        .and_then(|cwd| p.strip_prefix(cwd).ok().map(Path::to_path_buf))
        .filter(|r| !r.as_os_str().is_empty())
        .unwrap_or_else(|| p.to_path_buf())
        .to_string_lossy()
        .into_owned()
}
