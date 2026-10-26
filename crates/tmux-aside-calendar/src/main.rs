use std::process::ExitCode;
use std::time::Duration;

use anyhow::Result;
use clap::Parser;
use jiff::Zoned;
use tmux_aside::protocol::{PLUGIN_PROTOCOL, PluginMessage};
use tmux_aside_calendar::{Event, Options, rows};

/// Sidebar section listing upcoming events from the macOS Calendar, which syncs Google,
/// iCloud and Exchange accounts.
#[derive(Parser)]
#[command(version)]
struct Cli {
    /// Only list events of the calendar named NAME; every calendar when omitted
    #[arg(long, value_name = "NAME")]
    calendar: Vec<String>,
    /// How many days to list, starting today
    #[arg(long, default_value_t = 1)]
    days: u16,
    /// Most events listed
    #[arg(long, default_value_t = 8)]
    limit: usize,
    /// Events starting within DURATION show a countdown
    #[arg(long, value_name = "DURATION", default_value = "15m", value_parser = tmux_aside::config::parse_duration)]
    soon: Duration,
    /// Also list all-day events
    #[arg(long)]
    all_day: bool,
    /// Also list events you declined
    #[arg(long)]
    declined: bool,
}

fn main() -> ExitCode {
    match run(Cli::parse()) {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("{error:#}");
            ExitCode::FAILURE
        }
    }
}

fn run(cli: Cli) -> Result<()> {
    let now = Zoned::now();
    let start = now.start_of_day()?;
    let end = start.checked_add(jiff::Span::new().days(i64::from(cli.days)))?;
    let events = eventkit::events(start.timestamp(), end.timestamp(), &cli.calendar)?;
    let rows = rows(
        &events,
        now.timestamp(),
        now.time_zone(),
        Options {
            soon: cli.soon.as_secs() as i64,
            limit: cli.limit,
            all_day: cli.all_day,
            declined: cli.declined,
        },
    );
    print(&PluginMessage::Hello {
        protocol: PLUGIN_PROTOCOL,
        features: vec![],
    })?;
    print(&PluginMessage::Snapshot {
        title: Some("Calendar".to_string()),
        rows,
    })
}

fn print(message: &PluginMessage) -> Result<()> {
    println!("{}", serde_json::to_string(message)?);
    Ok(())
}

#[cfg(target_os = "macos")]
mod eventkit {
    use std::sync::mpsc;
    use std::time::Duration;

    use anyhow::{Result, bail};
    use block2::RcBlock;
    use jiff::Timestamp;
    use objc2::runtime::Bool;
    use objc2_event_kit::{
        EKAuthorizationStatus, EKEntityType, EKEvent, EKEventStore, EKParticipantStatus,
    };
    use objc2_foundation::{NSArray, NSDate, NSError};

    use super::Event;

    const ACCESS_TIMEOUT: Duration = Duration::from_secs(60);

    /// Occurrences of the events between `start` and `end`, asking for full calendar access
    /// the first time.
    pub fn events(start: Timestamp, end: Timestamp, calendars: &[String]) -> Result<Vec<Event>> {
        let store = unsafe { EKEventStore::new() };
        let status = unsafe { EKEventStore::authorizationStatusForEntityType(EKEntityType::Event) };
        if status != EKAuthorizationStatus::FullAccess && !request_access(&store)? {
            bail!(
                "no access to calendars: allow it in System Settings > Privacy & Security > Calendars"
            );
        }
        let selected: Vec<_> = unsafe { store.calendarsForEntityType(EKEntityType::Event) }
            .iter()
            .filter(|calendar| {
                calendars.is_empty() || calendars.contains(&unsafe { calendar.title() }.to_string())
            })
            .collect();
        if selected.is_empty() {
            return Ok(vec![]);
        }
        let selected = NSArray::from_retained_slice(&selected);
        let predicate = unsafe {
            store.predicateForEventsWithStartDate_endDate_calendars(
                &date(start),
                &date(end),
                Some(&selected),
            )
        };
        let events = unsafe { store.eventsMatchingPredicate(&predicate) };
        Ok(events.iter().map(|event| convert(&event)).collect())
    }

    fn request_access(store: &EKEventStore) -> Result<bool> {
        let (sender, receiver) = mpsc::channel();
        let completion = RcBlock::new(move |granted: Bool, _error: *mut NSError| {
            let _ = sender.send(granted.as_bool());
        });
        unsafe { store.requestFullAccessToEventsWithCompletion(RcBlock::as_ptr(&completion)) };
        Ok(receiver.recv_timeout(ACCESS_TIMEOUT).unwrap_or(false))
    }

    fn convert(event: &EKEvent) -> Event {
        unsafe {
            let notes = event.notes().map(|notes| notes.to_string());
            let location = event.location().map(|location| location.to_string());
            let url = event
                .URL()
                .and_then(|url| url.absoluteString())
                .map(|url| url.to_string());
            let declined = event.attendees().is_some_and(|attendees| {
                attendees.iter().any(|attendee| {
                    attendee.isCurrentUser()
                        && attendee.participantStatus() == EKParticipantStatus::Declined
                })
            });
            Event {
                title: event.title().to_string(),
                start: timestamp(&event.startDate()),
                end: timestamp(&event.endDate()),
                all_day: event.isAllDay(),
                declined,
                link: tmux_aside_calendar::meeting_link(
                    [url, location, notes].iter().flatten().map(String::as_str),
                ),
            }
        }
    }

    fn date(instant: Timestamp) -> objc2::rc::Retained<NSDate> {
        NSDate::dateWithTimeIntervalSince1970(instant.as_millisecond() as f64 / 1000.0)
    }

    fn timestamp(date: &NSDate) -> Timestamp {
        Timestamp::from_millisecond((date.timeIntervalSince1970() * 1000.0) as i64)
            .unwrap_or_default()
    }
}

#[cfg(not(target_os = "macos"))]
mod eventkit {
    use anyhow::{Result, bail};
    use jiff::Timestamp;

    use super::Event;

    pub fn events(_: Timestamp, _: Timestamp, _: &[String]) -> Result<Vec<Event>> {
        bail!("the calendar section reads the macOS Calendar and only runs on macOS")
    }
}
