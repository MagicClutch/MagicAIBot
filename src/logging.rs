use std::{
    cell::{Cell, RefCell},
    collections::VecDeque,
    fmt::Display,
    time::{Duration, Instant},
};

use crate::config::OutputMode;

/// How important a logged line is, independent of the `[INFO]`/`[SUCCESS]`/
/// `[WARNING]`/`[ERROR]` prefix it prints with -- what actually decides
/// whether a given [`OutputMode`] shows it. Ordered so `tier <= configured
/// tier` reads naturally: `Milestone` is the most important (shown from
/// `Light` up), `Detail` the least (shown only at `Debug`/`FullDebug`).
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum Tier {
    Milestone,
    Progress,
    Detail,
}

impl Tier {
    fn visible_at(self, mode: OutputMode) -> bool {
        match mode {
            OutputMode::None => false,
            OutputMode::Light => self == Tier::Milestone,
            OutputMode::Info => self <= Tier::Progress,
            OutputMode::Debug | OutputMode::FullDebug => true,
        }
    }
}

/// How many status lines destined for Minecraft chat are held before the
/// oldest is dropped. Only matters before the first successful connection
/// (or during a long disconnection), or if [`pop_outgoing_chat`]'s rate
/// limit is falling behind a genuine burst -- see that function's doc
/// comment. See [`drain_outgoing_chat`].
const MAX_QUEUED_CHAT_LINES: usize = 100;

/// Minimum spacing [`pop_outgoing_chat`] enforces between two lines handed
/// to `App` for an actual `send_chat`. Most servers' anti-spam kicks a
/// player who sends chat messages too quickly -- often regardless of
/// whether the content repeats -- so this caps the bot's own status
/// narration at a clearly human-plausible rate no matter how many lines a
/// single tick's worth of work queues up at once (e.g. every waypoint of a
/// single `#get` run).
const MIN_CHAT_SEND_INTERVAL: Duration = Duration::from_millis(600);

thread_local! {
    // `main.rs` runs the whole app on a single dedicated OS thread
    // (`tokio::runtime::Builder::new_current_thread()`), so none of this
    // needs cross-thread synchronization.
    static CONSOLE_MODE: Cell<OutputMode> = Cell::new(OutputMode::default());
    static CHAT_MODE: Cell<OutputMode> = Cell::new(OutputMode::default());
    static OUTGOING_CHAT: RefCell<VecDeque<String>> = const { RefCell::new(VecDeque::new()) };
    static CONSOLE_REPEAT_GUARD: RefCell<RepeatGuard> = RefCell::new(RepeatGuard::default());
    // Separate from `CONSOLE_REPEAT_GUARD` -- the console and chat modes
    // are configured independently, so a line collapsed for one
    // destination must not affect whether it prints for the other.
    static CHAT_REPEAT_GUARD: RefCell<RepeatGuard> = RefCell::new(RepeatGuard::default());
    static LAST_CHAT_SEND: Cell<Option<Instant>> = const { Cell::new(None) };
}

/// Sets both output levels at once -- called once at startup with the
/// configured [`crate::config::OutputConfig`]. See [`set_console_mode`]/
/// [`set_chat_mode`] for changing either one afterward (e.g. from
/// `/outputmode`).
pub fn configure(console: OutputMode, chat: OutputMode) {
    set_console_mode(console);
    set_chat_mode(chat);
}

pub fn set_console_mode(mode: OutputMode) {
    CONSOLE_MODE.with(|m| m.set(mode));
}

pub fn set_chat_mode(mode: OutputMode) {
    CHAT_MODE.with(|m| m.set(mode));
}

pub fn console_mode() -> OutputMode {
    CONSOLE_MODE.with(Cell::get)
}

pub fn chat_mode() -> OutputMode {
    CHAT_MODE.with(Cell::get)
}

/// A one-shot task/command boundary: a task starting, or its final
/// outcome (whether that outcome is `success`/`error`, both count -- see
/// [`success`]/[`error`]). Shown from [`OutputMode::Light`] up. Named
/// separately from [`info`] because most `info` lines are mid-task
/// narration, not a boundary -- callers must opt into this tier explicitly.
pub fn milestone(message: impl Display) {
    emit_tiered("[INFO]", Tier::Milestone, message.to_string());
}
pub fn info(message: impl Display) {
    emit_tiered("[INFO]", Tier::Detail, message.to_string());
}
/// A task's final, successful outcome -- e.g. "Destination reached",
/// "Block broken". Shown from [`OutputMode::Light`] up. For a running
/// task's incremental progress (an item/block counter ticking up mid-task),
/// use [`progress`] instead -- that is `Info`-tier, not `Light`.
pub fn success(message: impl Display) {
    emit_tiered("[SUCCESS]", Tier::Milestone, message.to_string());
}
/// A running task's incremental progress (e.g. "Collected diamond (3/5)").
/// Shown from [`OutputMode::Info`] up -- unlike [`success`], not shown at
/// `Light`, since `Light` only shows a task's start and final outcome.
pub fn progress(message: impl Display) {
    emit_tiered("[SUCCESS]", Tier::Progress, message.to_string());
}
pub fn warning(message: impl Display) {
    emit_tiered("[WARNING]", Tier::Progress, message.to_string());
}
pub fn error(message: impl Display) {
    emit_tiered("[ERROR]", Tier::Milestone, message.to_string());
}
pub fn chat_incoming(sender: &str, message: &str) {
    println!("[CHAT] {sender}: {message}");
}
pub fn chat_outgoing(username: &str, message: &str) {
    println!("[CHAT] <{username}> {message}");
}

/// How many times in a row a line (or a short back-and-forth cycle, e.g.
/// "Going to position (x, y, z)" / "Position reached" alternating because
/// something keeps re-issuing the same destination) is allowed to print
/// before it's collapsed: shown once more with a notice, then silenced
/// until something actually different is logged.
const MAX_CONSECUTIVE_REPEATS: u32 = 3;

#[derive(Default)]
struct RepeatGuard {
    /// The last two distinct console lines printed, oldest first -- long
    /// enough to catch both an exact line repeating immediately and a
    /// two-line alternation, without needing anything fancier.
    history: [Option<String>; 2],
    repeats: u32,
    notified: bool,
}

/// Formats `message` behind `prefix`, then routes it independently to the
/// console (if `tier` clears the configured console mode) and to the
/// Minecraft-chat queue (if `tier` clears the configured chat mode) --
/// either, both, or neither, entirely independently of each other. Each
/// destination also runs the line through its own repeat-collapsing (see
/// [`decide`]) before it's actually printed/queued, unless that
/// destination's mode is `FullDebug`.
fn emit_tiered(prefix: &str, tier: Tier, message: String) {
    let line = format!("{prefix} {message}");
    let console_mode = console_mode();
    if tier.visible_at(console_mode) {
        if console_mode == OutputMode::FullDebug {
            // `fulldebug` exists specifically to show every repetition a
            // stuck loop produces, so it must bypass the collapsing below.
            println!("{line}");
        } else {
            CONSOLE_REPEAT_GUARD.with(|guard| {
                if let Some(line) = decide(&mut guard.borrow_mut(), line.clone()) {
                    println!("{line}");
                }
            });
        }
    }
    let chat_mode = chat_mode();
    if tier.visible_at(chat_mode) {
        if chat_mode == OutputMode::FullDebug {
            queue_outgoing_chat(line);
        } else if let Some(line) =
            CHAT_REPEAT_GUARD.with(|guard| decide(&mut guard.borrow_mut(), line))
        {
            queue_outgoing_chat(line);
        }
    }
}

fn queue_outgoing_chat(line: String) {
    OUTGOING_CHAT.with(|queue| {
        let mut queue = queue.borrow_mut();
        if queue.len() >= MAX_QUEUED_CHAT_LINES {
            queue.pop_front();
        }
        queue.push_back(line);
    });
}

/// Drains every status line queued for Minecraft chat delivery since the
/// last call, ignoring the send-rate limit [`pop_outgoing_chat`] enforces.
/// Test-only, to inspect the whole queue at once; `App`'s tick loop must
/// use [`pop_outgoing_chat`] instead, never this, or it would blast every
/// queued line at the server in one burst.
#[cfg(test)]
fn drain_outgoing_chat() -> Vec<String> {
    OUTGOING_CHAT.with(|queue| queue.borrow_mut().drain(..).collect())
}

/// Pops and returns the single next queued chat line, but only if at least
/// [`MIN_CHAT_SEND_INTERVAL`] has passed since the last line this returned
/// -- called from every tick of `App`'s loop, this caps how fast the bot's
/// own status narration can reach Minecraft chat to a steady, safe rate no
/// matter how many lines a burst of work queues up at once (a single
/// `#get` run can queue dozens), or how often the caller happens to poll.
/// Returns `None` (leaving the line queued for the next call) when it's too
/// soon, not when the queue is merely empty -- callers cannot distinguish
/// the two from the return value alone, and don't need to.
pub fn pop_outgoing_chat() -> Option<String> {
    let ready = LAST_CHAT_SEND.with(|last| {
        let now = Instant::now();
        let ready = chat_send_is_ready(last.get(), now);
        if ready {
            last.set(Some(now));
        }
        ready
    });
    if ready {
        OUTGOING_CHAT.with(|queue| queue.borrow_mut().pop_front())
    } else {
        None
    }
}

/// Pure decision step behind [`pop_outgoing_chat`]'s rate limit, separated
/// out purely so it's testable with synthetic `Instant`s instead of real
/// sleeps.
fn chat_send_is_ready(last: Option<Instant>, now: Instant) -> bool {
    last.is_none_or(|previous| {
        now.checked_duration_since(previous)
            .is_none_or(|elapsed| elapsed >= MIN_CHAT_SEND_INTERVAL)
    })
}

/// Routes every printed console line through repeat detection first.
/// Without this, a caller re-issuing the same command (or a stuck retry
/// loop re-logging the same message every tick) can spam the console with
/// hundreds of identical or alternating lines; this prints the first few
/// occurrences normally, then one line noting further repeats are
/// suppressed, then stays silent for that specific line/cycle until
/// something genuinely different is logged.
fn decide(guard: &mut RepeatGuard, line: String) -> Option<String> {
    let is_repeat = guard.history.contains(&Some(line.clone()));
    if is_repeat {
        guard.repeats += 1;
    } else {
        guard.repeats = 0;
        guard.notified = false;
    }
    guard.history = [guard.history[1].take(), Some(line.clone())];
    if guard.repeats < MAX_CONSECUTIVE_REPEATS {
        Some(line)
    } else if !guard.notified {
        guard.notified = true;
        Some(format!("{line} (further repeats of this suppressed)"))
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn distinct_lines_always_print() {
        let mut guard = RepeatGuard::default();
        assert_eq!(decide(&mut guard, "a".into()), Some("a".into()));
        assert_eq!(decide(&mut guard, "b".into()), Some("b".into()));
        assert_eq!(decide(&mut guard, "c".into()), Some("c".into()));
    }

    #[test]
    fn an_exact_line_repeating_is_eventually_collapsed() {
        let mut guard = RepeatGuard::default();
        assert!(decide(&mut guard, "stuck".into()).is_some());
        assert!(decide(&mut guard, "stuck".into()).is_some());
        assert!(decide(&mut guard, "stuck".into()).is_some());
        // Fourth+ occurrence: one line noting suppression, then silence.
        assert!(
            decide(&mut guard, "stuck".into())
                .unwrap()
                .contains("suppressed")
        );
        assert_eq!(decide(&mut guard, "stuck".into()), None);
        assert_eq!(decide(&mut guard, "stuck".into()), None);
    }

    #[test]
    fn a_two_line_alternation_is_also_collapsed() {
        let mut guard = RepeatGuard::default();
        assert!(decide(&mut guard, "going to (1, 2, 3)".into()).is_some());
        assert!(decide(&mut guard, "reached".into()).is_some());
        assert!(decide(&mut guard, "going to (1, 2, 3)".into()).is_some());
        assert!(decide(&mut guard, "reached".into()).is_some());
        // The cycle has now repeated enough times to collapse -- one line
        // noting it, then silence for either side of the alternation.
        let suppressed_notice = decide(&mut guard, "going to (1, 2, 3)".into()).unwrap();
        assert!(suppressed_notice.contains("suppressed"));
        assert_eq!(decide(&mut guard, "reached".into()), None);
        assert_eq!(decide(&mut guard, "going to (1, 2, 3)".into()), None);
    }

    #[test]
    fn a_genuinely_new_line_resets_suppression() {
        let mut guard = RepeatGuard::default();
        for _ in 0..5 {
            decide(&mut guard, "stuck".into());
        }
        assert_eq!(decide(&mut guard, "stuck".into()), None); // still suppressed
        assert_eq!(
            decide(&mut guard, "finally different".into()),
            Some("finally different".into())
        );
        // Suppression state reset -- the old line prints fresh again.
        assert!(decide(&mut guard, "stuck".into()).is_some());
    }

    #[test]
    fn tier_visibility_matches_each_output_mode() {
        use OutputMode::{Debug, FullDebug, Info, Light, None as NoneMode};

        assert!(!Tier::Milestone.visible_at(NoneMode));
        assert!(!Tier::Progress.visible_at(NoneMode));
        assert!(!Tier::Detail.visible_at(NoneMode));

        assert!(Tier::Milestone.visible_at(Light));
        assert!(!Tier::Progress.visible_at(Light));
        assert!(!Tier::Detail.visible_at(Light));

        assert!(Tier::Milestone.visible_at(Info));
        assert!(Tier::Progress.visible_at(Info));
        assert!(!Tier::Detail.visible_at(Info));

        assert!(Tier::Milestone.visible_at(Debug));
        assert!(Tier::Progress.visible_at(Debug));
        assert!(Tier::Detail.visible_at(Debug));

        assert!(Tier::Milestone.visible_at(FullDebug));
        assert!(Tier::Progress.visible_at(FullDebug));
        assert!(Tier::Detail.visible_at(FullDebug));
    }

    #[test]
    fn console_and_chat_modes_are_independent_and_default_to_info() {
        // Reset first -- this thread-local state is process-wide across
        // every test on this thread, so a prior test's `set_*` call must
        // not leak in here.
        configure(OutputMode::Info, OutputMode::Info);
        assert_eq!(console_mode(), OutputMode::Info);
        assert_eq!(chat_mode(), OutputMode::Info);

        set_console_mode(OutputMode::Debug);
        assert_eq!(console_mode(), OutputMode::Debug);
        assert_eq!(chat_mode(), OutputMode::Info);

        set_chat_mode(OutputMode::None);
        assert_eq!(console_mode(), OutputMode::Debug);
        assert_eq!(chat_mode(), OutputMode::None);

        // Leave global state clean for any other test on this thread.
        configure(OutputMode::Info, OutputMode::Info);
    }

    #[test]
    fn milestone_and_progress_lines_are_queued_for_chat_only_when_the_chat_mode_allows_it() {
        configure(OutputMode::Debug, OutputMode::Light);
        drain_outgoing_chat(); // clear anything left over from another test

        milestone("task started");
        progress("3/5");
        assert_eq!(drain_outgoing_chat(), vec!["[INFO] task started"]);

        configure(OutputMode::Debug, OutputMode::Info);
        milestone("task started");
        progress("3/5");
        assert_eq!(
            drain_outgoing_chat(),
            vec!["[INFO] task started", "[SUCCESS] 3/5"]
        );

        configure(OutputMode::Info, OutputMode::Info);
    }

    #[test]
    fn drain_outgoing_chat_empties_the_queue() {
        configure(OutputMode::Info, OutputMode::Info);
        drain_outgoing_chat();
        milestone("hello");
        assert_eq!(drain_outgoing_chat().len(), 1);
        assert!(drain_outgoing_chat().is_empty());
    }

    /// The exact scenario that used to get the bot kicked for spamming: one
    /// underlying event (e.g. "Position reached") firing many times in a
    /// row queues the identical line for chat every time. Chat must
    /// collapse repeats the same way the console already does, using its
    /// own independent guard -- not by inheriting whatever the console
    /// happened to already suppress.
    #[test]
    fn identical_chat_lines_are_collapsed_the_same_way_as_console_lines() {
        configure(OutputMode::Debug, OutputMode::Debug);
        drain_outgoing_chat();
        for _ in 0..7 {
            milestone("chat-dedup-test-line");
        }
        let queued = drain_outgoing_chat();
        // 3 raw occurrences, then one "further repeats suppressed" notice,
        // then silence for the remaining 3 -- exactly mirroring `decide`'s
        // console behavior (see `an_exact_line_repeating_is_eventually_collapsed`).
        assert_eq!(queued.len(), 4, "{queued:?}");
        assert!(queued[3].contains("suppressed"), "{queued:?}");
        configure(OutputMode::Info, OutputMode::Info);
    }

    #[test]
    fn chat_send_rate_limit_blocks_a_second_send_too_soon_but_allows_it_once_enough_time_passes() {
        let t0 = Instant::now();
        assert!(
            chat_send_is_ready(None, t0),
            "nothing sent yet -- always ready"
        );
        let just_under = t0 + Duration::from_millis(599);
        assert!(!chat_send_is_ready(Some(t0), just_under));
        let just_over = t0 + MIN_CHAT_SEND_INTERVAL;
        assert!(chat_send_is_ready(Some(t0), just_over));
    }
}
