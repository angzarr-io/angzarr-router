//! Cucumber harness for `context.feature`: facts, replay, cover access, PM
//! compensator commands and projector page context, driven natively against
//! hand-built components on the Rust core.

use angzarr_router::error::CodedError;
use angzarr_router::pb;
use angzarr_router_conformance as conf;
use cucumber::{given, then, when, World};
use prost::Message;

#[derive(Debug, Default, World)]
struct ContextWorld {
    covers: conf::CoverSink,
    pages: conf::PageSink,
    facts: Option<Result<pb::EventBook, CodedError>>,
    replayed: Option<conf::CounterState>,
    pm: Option<Result<pb::ProcessManagerHandleResponse, CodedError>>,
}

#[given("a ledger aggregate")]
async fn ledger(_w: &mut ContextWorld) {}

#[given("a reserving process-manager")]
async fn reserving(_w: &mut ContextWorld) {}

#[given("a tracking projector")]
async fn tracking(_w: &mut ContextWorld) {}

#[when(regex = r"^(\d+) Increased facts are handled over (\d+) prior Increased events$")]
async fn increased_facts(w: &mut ContextWorld, facts: u32, prior: u32) {
    let ledger = conf::ledger_aggregate(w.covers.clone());
    w.facts = Some(ledger.handle_fact(&conf::fact_request("Increased", facts, prior)));
}

#[when("a Reserve fact is handled over no prior events")]
async fn reserve_fact(w: &mut ContextWorld) {
    let ledger = conf::ledger_aggregate(w.covers.clone());
    w.facts = Some(ledger.handle_fact(&conf::fact_request("Reserve", 1, 0)));
}

#[when(regex = r"^the ledger replays a snapshot of (\d+) then (\d+) Increased events$")]
async fn replay(w: &mut ContextWorld, count: u32, events: u32) {
    let ledger = conf::ledger_aggregate(w.covers.clone());
    w.replayed = Some(
        ledger
            .replay(&conf::replay_request(count, events))
            .expect("replay"),
    );
}

#[when(regex = r#"^an IncreaseBy command for ledger root "([^"]*)" is dispatched$"#)]
async fn ledger_command(w: &mut ContextWorld, label: String) {
    let ledger = conf::ledger_aggregate(w.covers.clone());
    ledger
        .dispatch(&conf::ledger_command(&label))
        .expect("dispatch");
}

#[when(
    regex = r#"^an Increased trigger of counter root "([^"]*)" at sequence (\d+) is dispatched to the reserving process-manager$"#
)]
async fn reserving_trigger(w: &mut ContextWorld, label: String, seq: u32) {
    let pm = conf::reserving_pm(w.covers.clone());
    w.pm = Some(pm.dispatch(&conf::reserving_trigger(&label, seq)));
}

#[when(
    regex = r#"^a rejection of Reserve sent to "([^"]*)" at sequence (\d+) is dispatched to the reserving process-manager$"#
)]
async fn reserving_rejection(w: &mut ContextWorld, domain: String, seq: u32) {
    let pm = conf::reserving_pm(w.covers.clone());
    w.pm = Some(pm.dispatch(&conf::reserving_rejection(&domain, seq)));
}

#[when(
    regex = r#"^Increased events of counter root "([^"]*)" at sequences (\d+) and (\d+) are projected$"#
)]
async fn projected(w: &mut ContextWorld, label: String, first: u32, second: u32) {
    let projector = conf::tracking_projector(w.pages.clone());
    projector
        .dispatch(&conf::tracked_book(&label, &[first, second]))
        .expect("dispatch");
}

fn recorded(w: &ContextWorld) -> &pb::EventBook {
    w.facts
        .as_ref()
        .expect("facts were handled")
        .as_ref()
        .expect("fact handling succeeded")
}

fn event_of(page: &pb::EventPage) -> &prost_types::Any {
    match page.payload.as_ref() {
        Some(pb::event_page::Payload::Event(any)) => any,
        _ => panic!("fact page carries no event"),
    }
}

#[then(regex = r"^(\d+) facts are recorded, each annotated with a count of (\d+)$")]
async fn annotated(w: &mut ContextWorld, facts: usize, count: u32) {
    let book = recorded(w);
    assert_eq!(book.pages.len(), facts);
    for page in &book.pages {
        let any = event_of(page);
        assert_eq!(
            angzarr_router::type_name_from_url(&any.type_url),
            "test.counter.CounterState"
        );
        assert_eq!(
            conf::CounterState::decode(any.value.as_slice())
                .unwrap()
                .count,
            count
        );
    }
}

#[then("the fact is recorded unchanged")]
async fn unchanged(w: &mut ContextWorld) {
    let book = recorded(w);
    assert_eq!(book.pages.len(), 1);
    assert_eq!(
        angzarr_router::type_name_from_url(&event_of(&book.pages[0]).type_url),
        "test.counter.Reserve"
    );
}

#[then(regex = r"^the replayed state has a count of (\d+)$")]
async fn replayed(w: &mut ContextWorld, count: u32) {
    assert_eq!(w.replayed.as_ref().expect("replayed").count, count);
}

fn single_root(sink: &conf::CoverSink) -> Vec<u8> {
    let covers = sink.lock().unwrap();
    assert_eq!(covers.len(), 1, "the handler ran once");
    covers[0]
        .as_ref()
        .and_then(|c| c.root.as_ref())
        .map(|r| r.value.clone())
        .expect("the handler saw a cover with a root")
}

#[then(regex = r#"^the ledger handler saw root "([^"]*)"$"#)]
async fn ledger_saw(w: &mut ContextWorld, label: String) {
    assert_eq!(single_root(&w.covers), conf::root_of(&label));
}

#[then(regex = r#"^the reserving process-manager saw trigger root "([^"]*)"$"#)]
async fn pm_saw(w: &mut ContextWorld, label: String) {
    assert_eq!(single_root(&w.covers), conf::root_of(&label));
}

#[then(
    regex = r"^the reserving process-manager emits one Release command deferred from source sequence (\d+)$"
)]
async fn release(w: &mut ContextWorld, seq: u32) {
    let resp =
        w.pm.as_ref()
            .expect("dispatched")
            .as_ref()
            .expect("PM dispatch succeeded");
    assert_eq!(resp.commands.len(), 1);
    let page = &resp.commands[0].pages[0];
    let Some(pb::command_page::Payload::Command(any)) = page.payload.as_ref() else {
        panic!("command page carries no command");
    };
    assert_eq!(
        angzarr_router::type_name_from_url(&any.type_url),
        "test.counter.Release"
    );
    let Some(pb::page_header::SequenceType::AngzarrDeferred(d)) =
        page.header.as_ref().and_then(|h| h.sequence_type.as_ref())
    else {
        panic!("the Release command is not deferred");
    };
    assert_eq!(d.source_seq, seq);
}

#[then(regex = r#"^the projector saw root "([^"]*)" at sequences (\d+) and (\d+)$"#)]
async fn projector_saw(w: &mut ContextWorld, label: String, first: u32, second: u32) {
    let root = conf::root_of(&label);
    assert_eq!(
        *w.pages.lock().unwrap(),
        vec![(root.clone(), first), (root, second)]
    );
}

#[tokio::main]
async fn main() {
    let feature = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../conformance/features/context.feature"
    );
    ContextWorld::cucumber().run_and_exit(feature).await;
}
