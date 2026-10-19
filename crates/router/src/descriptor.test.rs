//! The registration hello: every component builds the spec's
//! ComponentDescriptor (types.proto) from its own dispatch table, and none
//! builds one without a rules version.

use crate::aggregate::AggregateDispatch;
use crate::error::{codes, GrpcCode};
use crate::pb;
use crate::process_manager::ProcessManagerDispatch;
use crate::projector::ProjectorDispatch;
use crate::rebuild::Rebuilder;
use crate::saga::SagaDispatch;

/// A native state message: its type URL comes from its prost name.
#[derive(Clone, PartialEq, prost::Message)]
struct LedgerState {
    #[prost(uint32, tag = "1")]
    balance: u32,
}

impl prost::Name for LedgerState {
    const NAME: &'static str = "LedgerState";
    const PACKAGE: &'static str = "test.ledger";
}

fn ledger(version: &str) -> AggregateDispatch<LedgerState> {
    AggregateDispatch::new("Ledger", "ledger", Rebuilder::new(LedgerState::default))
        .with_rules_version(version)
}

fn resolutions(descriptor: &pb::ComponentDescriptor) -> Vec<pb::ConflictResolution> {
    descriptor
        .state
        .as_ref()
        .and_then(|s| s.options.as_ref())
        .map(|o| o.conflict_resolutions().collect())
        .unwrap_or_default()
}

fn inputs(descriptor: &pb::ComponentDescriptor) -> Vec<&str> {
    descriptor
        .inputs
        .iter()
        .map(|t| t.domain.as_str())
        .collect()
}

fn assert_missing_rules_version(result: Result<pb::ComponentDescriptor, crate::error::CodedError>) {
    let err = result.expect_err("an empty rules version is refused");
    assert_eq!(err.code, codes::MISSING_RULES_VERSION);
    assert_eq!(err.grpc, GrpcCode::InvalidArgument);
}

#[test]
fn an_aggregate_announces_its_name_kind_version_and_native_state_type() {
    let descriptor = ledger("1.4.2+build.7")
        .component_descriptor()
        .expect("descriptor");
    assert_eq!(descriptor.name, "Ledger");
    assert_eq!(descriptor.kind(), pb::ComponentKind::Aggregate);
    assert_eq!(descriptor.rules_version, "1.4.2+build.7");
    assert!(
        inputs(&descriptor).is_empty(),
        "an aggregate consumes no events"
    );
    let state = descriptor.state.as_ref().expect("state");
    assert_eq!(state.type_url, "/test.ledger.LedgerState");
    assert_eq!(
        resolutions(&descriptor),
        vec![
            pb::ConflictResolution::Strict,
            pb::ConflictResolution::AggregateHandles
        ]
    );
}

#[test]
fn a_commutative_aggregate_lists_commutative_first() {
    let descriptor = ledger("1")
        .with_commutative(|_, _| Vec::new())
        .component_descriptor()
        .expect("descriptor");
    assert_eq!(
        resolutions(&descriptor),
        vec![
            pb::ConflictResolution::Commutative,
            pb::ConflictResolution::Strict,
            pb::ConflictResolution::AggregateHandles
        ]
    );
}

#[test]
fn an_aggregate_over_any_state_takes_its_state_type_by_name() {
    let dispatch: AggregateDispatch<prost_types::Any> =
        AggregateDispatch::new("Counter", "counter", Rebuilder::new(Default::default))
            .with_rules_version("2");
    let descriptor = dispatch
        .component_descriptor_with_state("test.counter.CounterState")
        .expect("descriptor");
    assert_eq!(
        descriptor.state.expect("state").type_url,
        "/test.counter.CounterState"
    );
}

#[test]
fn an_aggregate_without_a_rules_version_is_refused() {
    assert_missing_rules_version(ledger("").component_descriptor());
    assert_missing_rules_version(ledger("").component_descriptor_with_state("x.Y"));
}

fn fulfillment(version: &str) -> ProcessManagerDispatch<LedgerState> {
    let noop = |_: &prost_types::Any, _: &mut LedgerState, _: Option<&pb::Cover>| {
        Ok(pb::ProcessManagerHandleResponse::default())
    };
    ProcessManagerDispatch::new(
        "Fulfillment",
        "fulfillment",
        ["shipping"],
        Rebuilder::new(LedgerState::default),
    )
    .on_event("order", "test.OrderPlaced", noop)
    .on_event("payment", "test.PaymentTaken", noop)
    .on_event("order", "test.OrderCancelled", noop)
    .with_rules_version(version)
}

#[test]
fn a_process_manager_announces_its_sources_and_state() {
    let descriptor = fulfillment("2.0.1")
        .component_descriptor()
        .expect("descriptor");
    assert_eq!(descriptor.name, "Fulfillment");
    assert_eq!(descriptor.kind(), pb::ComponentKind::ProcessManager);
    assert_eq!(descriptor.rules_version, "2.0.1");
    assert_eq!(inputs(&descriptor), vec!["order", "payment"]);
    assert_eq!(
        descriptor.state.as_ref().expect("state").type_url,
        "/test.ledger.LedgerState"
    );
    assert_eq!(
        resolutions(&descriptor),
        vec![
            pb::ConflictResolution::Strict,
            pb::ConflictResolution::AggregateHandles
        ]
    );
    let with_state = fulfillment("2.0.1")
        .component_descriptor_with_state("test.pm.State")
        .expect("descriptor");
    assert_eq!(with_state.state.expect("state").type_url, "/test.pm.State");
}

#[test]
fn a_process_manager_without_a_rules_version_is_refused() {
    assert_missing_rules_version(fulfillment("").component_descriptor());
    assert_missing_rules_version(fulfillment("").component_descriptor_with_state("x.Y"));
}

fn notifier(version: &str) -> SagaDispatch {
    SagaDispatch::new("Notifier", "order", ["email"]).with_rules_version(version)
}

#[test]
fn a_saga_announces_its_input_domain_and_no_state() {
    let descriptor = notifier("3").component_descriptor().expect("descriptor");
    assert_eq!(descriptor.name, "Notifier");
    assert_eq!(descriptor.kind(), pb::ComponentKind::Saga);
    assert_eq!(descriptor.rules_version, "3");
    assert_eq!(inputs(&descriptor), vec!["order"]);
    assert!(descriptor.state.is_none());
}

#[test]
fn a_saga_without_a_rules_version_is_refused() {
    assert_missing_rules_version(notifier("").component_descriptor());
}

fn board(version: &str) -> ProjectorDispatch<u32> {
    ProjectorDispatch::new("Board", || 0u32).with_rules_version(version)
}

#[test]
fn a_projector_announces_its_domains_sorted_and_no_state() {
    let descriptor = board("4")
        .for_domains(["shipping", "order", "shipping"])
        .component_descriptor()
        .expect("descriptor");
    assert_eq!(descriptor.name, "Board");
    assert_eq!(descriptor.kind(), pb::ComponentKind::Projector);
    assert_eq!(descriptor.rules_version, "4");
    assert_eq!(inputs(&descriptor), vec!["order", "shipping"]);
    assert!(descriptor.state.is_none());
}

#[test]
fn a_projector_declaring_no_domains_consumes_every_domain() {
    let descriptor = board("4").component_descriptor().expect("descriptor");
    assert_eq!(inputs(&descriptor), vec![crate::WILDCARD_DOMAIN]);
}

#[test]
fn a_projector_without_a_rules_version_is_refused() {
    assert_missing_rules_version(board("").component_descriptor());
}
