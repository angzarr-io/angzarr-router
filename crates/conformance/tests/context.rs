//! Cucumber harness for `context.feature`: facts, cover access and
//! projector page context, driven natively against hand-built components on
//! the Rust core.

use angzarr_router::error::CodedError;
use angzarr_router::pb;
use angzarr_router_conformance as conf;
use cucumber::{given, then, when, World};
use prost::Message;

#[derive(Debug, Default, World)]
struct ContextWorld {
    covers: conf::CoverSink,
    applied: conf::SequenceSink,
    pages: conf::PageSink,
    fact: Option<Result<pb::CommandHandlerResponse, CodedError>>,
}

#[given("a ledger aggregate")]
async fn ledger(_w: &mut ContextWorld) {}

#[given("a reserving process-manager")]
async fn reserving(_w: &mut ContextWorld) {}

#[given("a tracking projector")]
async fn tracking(_w: &mut ContextWorld) {}

#[when(regex = r"^an? (\w+) fact is handled over (\d+|no) prior (?:Increased )?events$")]
async fn fact_handled(w: &mut ContextWorld, fact: String, prior: String) {
    let prior = prior.parse().unwrap_or(0);
    let ledger = conf::ledger_aggregate(w.covers.clone(), w.applied.clone());
    w.fact = Some(ledger.dispatch(&conf::ledger_fact(&fact, prior)));
}

#[then(regex = r"^the ledger applied Increased events at sequences (\d+) and (\d+)$")]
async fn applied_at(w: &mut ContextWorld, first: u64, second: u64) {
    assert_eq!(*w.applied.lock().unwrap(), vec![first, second]);
}

#[when(regex = r#"^an IncreaseBy command for ledger root "([^"]*)" is dispatched$"#)]
async fn ledger_command(w: &mut ContextWorld, label: String) {
    let ledger = conf::ledger_aggregate(w.covers.clone(), w.applied.clone());
    ledger
        .dispatch(&conf::ledger_command(&label))
        .expect("dispatch");
}

#[when(
    regex = r#"^an IncreaseBy command for ledger root "([^"]*)" is dispatched over Increased events at sequences (\d+) and (\d+)$"#
)]
async fn ledger_command_over(w: &mut ContextWorld, label: String, first: u64, second: u64) {
    let ledger = conf::ledger_aggregate(w.covers.clone(), w.applied.clone());
    ledger
        .dispatch(&conf::ledger_command_over(&label, &[first, second]))
        .expect("dispatch");
}

#[when(
    regex = r#"^an Increased trigger of counter root "([^"]*)" at sequence (\d+) is dispatched to the reserving process-manager$"#
)]
async fn reserving_trigger(w: &mut ContextWorld, label: String, seq: u64) {
    let pm = conf::reserving_pm(w.covers.clone());
    pm.dispatch(&conf::reserving_trigger(&label, seq))
        .expect("PM dispatch succeeded");
}

#[when(
    regex = r#"^Increased events of counter root "([^"]*)" at sequences (\d+) and (\d+) are projected$"#
)]
async fn projected(w: &mut ContextWorld, label: String, first: u64, second: u64) {
    let projector = conf::tracking_projector(w.pages.clone());
    projector
        .dispatch(&conf::tracked_book(&label, &[first, second]))
        .expect("dispatch");
}

fn recorded(w: &ContextWorld) -> &pb::EventBook {
    w.fact
        .as_ref()
        .expect("a fact was handled")
        .as_ref()
        .expect("fact handling succeeded")
        .events
        .as_ref()
        .expect("the recorded events")
}

#[then(regex = r"^the Increased fact is recorded, flagged by the count (\d+)$")]
async fn recorded_and_flagged(w: &mut ContextWorld, count: u32) {
    let recorded: Vec<(String, Option<u32>)> = recorded(w)
        .pages
        .iter()
        .map(|page| {
            let any = page
                .payload
                .as_ref()
                .expect("a recorded page carries its event");
            let name = angzarr_router::type_name_from_url(&any.type_url).to_string();
            let count = (name == "test.counter.CounterState").then(|| {
                conf::CounterState::decode(any.value.as_slice())
                    .unwrap()
                    .count
            });
            (name, count)
        })
        .collect();
    assert_eq!(
        recorded,
        vec![
            ("test.counter.Increased".to_string(), None),
            ("test.counter.CounterState".to_string(), Some(count)),
        ]
    );
}

#[then(regex = r"^the fact is refused with ([A-Z_]+) as INVALID_ARGUMENT$")]
async fn fact_refused(w: &mut ContextWorld, code: String) {
    let err = w
        .fact
        .as_ref()
        .expect("a fact was handled")
        .as_ref()
        .expect_err("the fact is refused");
    assert_eq!(err.code, code);
    assert_eq!(err.grpc, angzarr_router::error::GrpcCode::InvalidArgument);
}

fn single_root(sink: &conf::CoverSink) -> Vec<u8> {
    let covers = sink.lock().unwrap();
    assert_eq!(covers.len(), 1, "the handler ran once");
    covers[0]
        .as_ref()
        .and_then(|c| c.root.as_ref())
        .map(|r| r.value.to_vec())
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

#[then(regex = r#"^the projector saw root "([^"]*)" at sequences (\d+) and (\d+)$"#)]
async fn projector_saw(w: &mut ContextWorld, label: String, first: u64, second: u64) {
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
