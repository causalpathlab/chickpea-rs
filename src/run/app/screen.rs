//! `chickpea run`'s loop: the screen in a session, a given line started
//! at once, and its log and end sent to its job.

use super::super::worker::Ended;
use super::super::{Step, STEPS};
use super::{App, TICK};
use crate::tui::TOOL;
use legume_mcp::job::{End, Reporter};
use legume_tui::Round;
use ratatui::crossterm::event::{Event, KeyEventKind};

/// Run the screen for `args`.
pub fn run(args: &crate::run::RunArgs) -> anyhow::Result<()> {
    legume_tui::view::need_terminal(
        "run the step itself (chickpea peak-to-gene ...), or p2g_run on chickpea mcp",
    )?;
    let (step, argv): (&'static Step, &[String]) = match args.step.split_first() {
        Some((name, rest)) => (crate::run::step(name)?, rest),
        None => (&STEPS[0], &[]),
    };
    // A step given whole, from `chickpea mcp` say: the screen ends with it,
    // and its log and end go to its job, also when it cannot start. A step
    // named alone opens its form.
    let given = args.step.len() > 1;
    let reporter = given.then(Reporter::from_env).flatten();
    let session = TOOL.session("run", &args.session())?;
    let env = session.child_env();
    let refused = |why: String| {
        if let Some(r) = &reporter {
            r.finish(End::Failed(why.clone()));
        }
        anyhow::anyhow!("chickpea {}: {why}", legume_mcp::command_line(&args.step))
    };
    // Refused as given, by the line itself, before the screen reads any of
    // its files.
    if given {
        crate::run::checked(step, argv).map_err(refused)?;
    }
    // A given line runs as given: at once, its data pane filled meanwhile.
    let mut app = App::new(step, argv);
    if given {
        app.start(env.clone());
        if app.job.is_none() {
            return Err(refused(app.note.clone().unwrap_or_default()));
        }
    }
    let mut sent = 0usize;
    let end: anyhow::Result<_> = session.with_terminal(crate::tui::modes(), |terminal| {
        loop {
            // Lines and end taken under one lock, so no last line is left
            // unsent, and written after it.
            let mut new: Vec<String> = Vec::new();
            let ended = app.job.as_ref().and_then(|job| {
                let log = job.log();
                if reporter.is_some() {
                    new.extend(log.lines_from(sent).cloned());
                    sent = log.said();
                }
                log.end.clone()
            });
            if let Some(r) = &reporter {
                new.iter().for_each(|l| r.line(l));
            }
            // A step given for a job, or a `chickpea kill`: done once the
            // run is.
            if (reporter.is_some() || session.stop_requested())
                && !app.running()
                && (ended.is_some() || session.stop_requested())
            {
                return Ok(ended);
            }
            app.tick();
            session.status(&status(&app));
            session.draw(terminal, |f| crate::run::draw::draw(f, &app))?;
            match session.poll(terminal, TICK)? {
                Round::Stopped => {
                    // `chickpea kill`: the run first, then the screen.
                    if let Some(job) = &app.job {
                        job.stop();
                    }
                    if !app.running() {
                        return Ok(ended);
                    }
                }
                Round::Event(Event::Key(k)) if k.kind != KeyEventKind::Release => {
                    app.key(k, &env);
                }
                Round::Event(Event::Mouse(m)) => app.mouse(m, &env),
                _ => {}
            }
            if app.quit {
                return Ok(ended);
            }
        }
    });
    if let Some(r) = &reporter {
        r.finish(match &end {
            Ok(Some(Ok(()))) => {
                End::Done(format!("chickpea {}", legume_mcp::command_line(&args.step)))
            }
            Ok(Some(Err(Ended::Stopped))) => End::Cancelled,
            Ok(Some(Err(Ended::Failed(why)))) => End::Failed(why.clone()),
            Ok(None) => End::Cancelled,
            Err(e) => End::Failed(format!("{e:#}")),
        });
    }
    match end? {
        Some(Err(Ended::Failed(why))) if given => anyhow::bail!("{why}"),
        _ => Ok(()),
    }
}

/// The session's status line: what the screen is doing.
fn status(app: &App) -> String {
    let what = app.step.name;
    match &app.job {
        Some(job) => {
            let log = job.log();
            match (&log.end, &log.bar) {
                (None, Some(bar)) => format!("{what}: {}", bar.now.what),
                (None, None) => format!("{what}: running"),
                (Some(Ok(())), _) => format!("{what}: done"),
                (Some(Err(Ended::Stopped)), _) => format!("{what}: stopped"),
                (Some(Err(Ended::Failed(_))), _) => format!("{what}: failed"),
            }
        }
        None => format!("{what}: setting up"),
    }
}
