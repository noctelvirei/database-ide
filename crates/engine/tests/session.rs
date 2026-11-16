use std::sync::Arc;
use std::time::Duration;

use dbide_engine::session::temporal::{
    Date, Error, Time, TimeWithZone, Timestamp, TimestampWithZone,
};
use dbide_engine::session::{
    Column, Event, PlanFormat, Run, Session, Severity, TypeCategory, Value,
};
use futures::{StreamExt, stream};
use tokio_util::sync::CancellationToken;

#[derive(Debug)]
struct ScriptedSession {
    events: Vec<(Duration, Event)>,
}

impl Session for ScriptedSession {
    fn plan<'a>(&'a mut self, batch: &'a str, _mode: dbide_engine::session::PlanMode) -> Run<'a> {
        self.run(batch)
    }

    fn current_database(&self) -> &str {
        "sample"
    }

    fn run<'a>(&'a mut self, _batch: &'a str) -> Run<'a> {
        let events =
            stream::iter(std::mem::take(&mut self.events)).then(|(delay, event)| async move {
                tokio::time::sleep(delay).await;
                event
            });
        Run::new(events, CancellationToken::new())
    }
}

#[tokio::test(start_paused = true)]
async fn events_keep_server_order_across_errors_and_result_sets() {
    let mut session = ScriptedSession {
        events: vec![
            (
                Duration::ZERO,
                Event::ResultSetStarted {
                    columns: vec![Column {
                        name: "alias".into(),
                        type_name: "bigint".into(),
                        category: TypeCategory::Integer,
                        nullable: Some(false),
                        base_table: Some("sample.items".into()),
                        base_column: Some("id".into()),
                    }],
                },
            ),
            (Duration::ZERO, Event::Rows(vec![vec![Value::Integer(7)]])),
            (
                Duration::ZERO,
                Event::Message {
                    text: "notice".into(),
                    severity: Severity::Info,
                    line: None,
                },
            ),
            (
                Duration::ZERO,
                Event::Error {
                    text: "bad statement".into(),
                    line: Some(2.try_into().unwrap()),
                    code: Some("42000".into()),
                },
            ),
            (Duration::ZERO, Event::RowCount(3)),
            (
                Duration::ZERO,
                Event::Plan {
                    payload: "[]".into(),
                    format: PlanFormat::ExplainJson,
                },
            ),
            (Duration::ZERO, Event::ResultSetStarted { columns: vec![] }),
            (Duration::ZERO, Event::Rows(vec![])),
        ],
    };

    assert_eq!(
        session.run("script").collect::<Vec<_>>().await,
        vec![
            Event::ResultSetStarted {
                columns: vec![Column {
                    name: "alias".into(),
                    type_name: "bigint".into(),
                    category: TypeCategory::Integer,
                    nullable: Some(false),
                    base_table: Some("sample.items".into()),
                    base_column: Some("id".into()),
                }]
            },
            Event::Rows(vec![vec![Value::Integer(7)]]),
            Event::Message {
                text: "notice".into(),
                severity: Severity::Info,
                line: None
            },
            Event::Error {
                text: "bad statement".into(),
                line: Some(2.try_into().unwrap()),
                code: Some("42000".into())
            },
            Event::RowCount(3),
            Event::Plan {
                payload: "[]".into(),
                format: PlanFormat::ExplainJson
            },
            Event::ResultSetStarted { columns: vec![] },
            Event::Rows(vec![]),
            Event::Done {
                elapsed: Duration::ZERO
            },
        ]
    );
}

#[tokio::test(start_paused = true)]
async fn cancellation_from_another_task_wakes_pending_rows() {
    let mut session = ScriptedSession {
        events: vec![
            (Duration::ZERO, Event::ResultSetStarted { columns: vec![] }),
            (Duration::ZERO, Event::Rows(vec![vec![]])),
            (Duration::from_secs(60), Event::Rows(vec![vec![]])),
        ],
    };
    let mut run = session.run("script");
    assert_eq!(
        run.next().await,
        Some(Event::ResultSetStarted { columns: vec![] })
    );
    assert_eq!(run.next().await, Some(Event::Rows(vec![vec![]])));
    let cancel = run.cancel_handle().clone();
    let task = tokio::spawn(async move {
        tokio::task::yield_now().await;
        cancel.cancel();
    });

    assert_eq!(
        tokio::time::timeout(Duration::from_secs(1), run.next())
            .await
            .unwrap(),
        Some(Event::Error {
            text: "Run was cancelled".into(),
            line: None,
            code: None
        })
    );
    assert_eq!(
        run.next().await,
        Some(Event::Done {
            elapsed: Duration::ZERO
        })
    );
    assert_eq!(run.next().await, None);
    task.await.unwrap();
}

#[tokio::test(start_paused = true)]
async fn cancel_after_done_has_no_effect_on_the_stream_or_driver_token() {
    let token = CancellationToken::new();
    let mut run = Run::new(stream::empty(), token.clone());
    let cancel = run.cancel_handle();
    assert_eq!(
        run.next().await,
        Some(Event::Done {
            elapsed: Duration::ZERO
        })
    );

    cancel.cancel();

    assert!(!token.is_cancelled());
    assert_eq!(run.next().await, None);
    assert_eq!(run.next().await, None);
}

#[tokio::test(start_paused = true)]
async fn cancel_before_first_poll_discards_ready_events() {
    let run = Run::new(
        stream::iter(vec![Event::RowCount(99)]),
        CancellationToken::new(),
    );
    let cancel = run.cancel_handle();
    cancel.cancel();
    cancel.cancel();

    assert_eq!(
        run.collect::<Vec<_>>().await,
        vec![
            Event::Error {
                text: "Run was cancelled".into(),
                line: None,
                code: None
            },
            Event::Done {
                elapsed: Duration::ZERO
            },
        ]
    );
}

#[tokio::test(start_paused = true)]
async fn done_measures_elapsed_time_from_run_creation() {
    let mut session = ScriptedSession {
        events: vec![(Duration::from_millis(20), Event::RowCount(1))],
    };

    assert_eq!(
        session.run("script").collect::<Vec<_>>().await,
        vec![
            Event::RowCount(1),
            Event::Done {
                elapsed: Duration::from_millis(20)
            },
        ]
    );
}

#[tokio::test(start_paused = true)]
async fn source_done_discards_trailing_events_and_uses_measured_time() {
    let mut run = Run::new(
        stream::iter(vec![
            Event::Done {
                elapsed: Duration::from_secs(999),
            },
            Event::RowCount(99),
        ]),
        CancellationToken::new(),
    );

    assert_eq!(
        run.next().await,
        Some(Event::Done {
            elapsed: Duration::ZERO
        })
    );
    assert_eq!(run.next().await, None);
}

#[test]
fn dropping_an_unfinished_run_requests_driver_cancellation() {
    let token = CancellationToken::new();
    let run = Run::new(stream::pending(), token.clone());

    drop(run);

    assert!(token.is_cancelled());
}

#[test]
fn temporal_text_preserves_fractional_precision_and_offsets() {
    assert_eq!(Date::parse("2000-02-29").unwrap().as_str(), "2000-02-29");
    assert_eq!(
        Time::parse("23:59:59.123456789").unwrap().as_str(),
        "23:59:59.123456789"
    );
    assert_eq!(
        TimeWithZone::parse("01:02:03-04:30").unwrap().as_str(),
        "01:02:03-04:30"
    );
    assert_eq!(
        Timestamp::parse("2026-11-16T01:02:03.1234567")
            .unwrap()
            .as_str(),
        "2026-11-16T01:02:03.1234567"
    );
    assert_eq!(
        TimestampWithZone::parse("2026-11-16T01:02:03+05:30")
            .unwrap()
            .as_str(),
        "2026-11-16T01:02:03+05:30"
    );
    assert_eq!(
        TimestampWithZone::parse("2026-11-16T01:02:03Z")
            .unwrap()
            .as_str(),
        "2026-11-16T01:02:03Z"
    );
}

#[test]
fn dates_reject_impossible_calendar_days() {
    assert_eq!(Date::parse("1900-02-29"), Err(Error::InvalidIso8601));
    assert_eq!(Date::parse("2026-04-31"), Err(Error::InvalidIso8601));
    assert_eq!(Date::parse("2026-00-01"), Err(Error::InvalidIso8601));
    assert_eq!(Date::parse("0000-00-00"), Err(Error::InvalidIso8601));
    assert_eq!(Date::parse("2026-01-00"), Err(Error::InvalidIso8601));
}

#[test]
fn clock_times_reject_overflow_and_empty_fractions() {
    assert_eq!(Time::parse("24:00:00"), Err(Error::InvalidIso8601));
    assert_eq!(Time::parse("12:60:00"), Err(Error::InvalidIso8601));
    assert_eq!(Time::parse("12:30:60"), Err(Error::InvalidIso8601));
    assert_eq!(Time::parse("12:30:59."), Err(Error::InvalidIso8601));
    assert_eq!(Time::parse("12:30:59.1x"), Err(Error::InvalidIso8601));
}

#[test]
fn zoned_values_require_a_valid_explicit_offset() {
    assert_eq!(TimeWithZone::parse("12:30:59"), Err(Error::InvalidIso8601));
    assert_eq!(
        TimeWithZone::parse("12:30:59+24:00"),
        Err(Error::InvalidIso8601)
    );
    assert_eq!(
        TimeWithZone::parse("12:30:59+00:60"),
        Err(Error::InvalidIso8601)
    );
    assert_eq!(
        TimestampWithZone::parse("2026-11-16T12:30:59"),
        Err(Error::InvalidIso8601)
    );
    assert_eq!(
        Timestamp::parse("2026-11-16T12:30:59Z"),
        Err(Error::InvalidIso8601)
    );
}

#[test]
fn temporal_parsing_rejects_malformed_and_non_ascii_text_without_panicking() {
    assert_eq!(Date::parse(""), Err(Error::InvalidIso8601));
    assert_eq!(Date::parse("２０２６-11-16"), Err(Error::InvalidIso8601));
    assert_eq!(Date::parse("2026/11/16"), Err(Error::InvalidIso8601));
    assert_eq!(Time::parse("💡:00:00"), Err(Error::InvalidIso8601));
    assert_eq!(
        TimeWithZone::parse("12:30:59+💡:00"),
        Err(Error::InvalidIso8601)
    );
    assert_eq!(
        Timestamp::parse("2026-11-16 12:30:59"),
        Err(Error::InvalidIso8601)
    );
    assert_eq!(
        TimestampWithZone::parse("2026-02-30T12:30:59Z"),
        Err(Error::InvalidIso8601)
    );
}

#[test]
fn sessions_are_usable_through_a_trait_object() {
    let session: Box<dyn Session> = Box::new(ScriptedSession { events: vec![] });

    assert_eq!(session.current_database(), "sample");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn cancellation_racing_completion_cannot_reach_the_driver_after_done() {
    let token = CancellationToken::new();
    let run = Run::new(stream::empty(), token.clone());
    let cancel = run.cancel_handle();
    let barrier = Arc::new(tokio::sync::Barrier::new(2));
    let task_barrier = barrier.clone();
    let task = tokio::spawn(async move {
        task_barrier.wait().await;
        cancel.cancel();
    });
    barrier.wait().await;
    let mut terminal =
        run.filter(|event| futures::future::ready(matches!(event, Event::Done { .. })));

    assert!(matches!(terminal.next().await, Some(Event::Done { .. })));
    let cancelled_at_done = token.is_cancelled();
    task.await.unwrap();

    assert!(cancelled_at_done || !token.is_cancelled());
}

#[tokio::test]
async fn empty_run_emits_done_once() {
    let mut session = ScriptedSession { events: vec![] };
    let mut run = session.run("");

    assert!(matches!(run.next().await, Some(Event::Done { .. })));
    assert_eq!(run.next().await, None);
    assert_eq!(run.next().await, None);
}
