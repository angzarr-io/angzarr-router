//! The registration hello: every component builds the spec's
//! ComponentDescriptor (types.proto) from its own dispatch table, and none
//! builds one without a rules version.

use crate::domain::{CommandContext, DomainDispatch, FactRecord};
use crate::error::{codes, GrpcCode};
use crate::pb;
use crate::policy::PolicyDispatch;
use crate::projector::ProjectorDispatch;
use crate::rebuild::Rebuilder;
use crate::test_support::LedgerState;

fn ledger(version: &str) -> DomainDispatch<LedgerState> {
    DomainDispatch::new("Ledger", "ledger", Rebuilder::of_message()).with_rules_version(version)
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
    crate::test_support::assert_bug(&err);
    assert_eq!(err.grpc, GrpcCode::Internal);
    assert!(
        err.extras.contains_key(crate::error::extras::COMPONENT),
        "the refusal names the component"
    );
}

#[test]
fn the_refusal_names_the_component() {
    let err = notifier("").component_descriptor().expect_err("refused");
    assert_eq!(
        err.extras
            .get(crate::error::extras::COMPONENT)
            .map(String::as_str),
        Some("Notifier")
    );
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
fn an_aggregate_over_any_state_takes_the_state_type_its_rebuilder_declares() {
    let dispatch: DomainDispatch<prost_types::Any> = DomainDispatch::new(
        "Counter",
        "counter",
        Rebuilder::new(Default::default).with_state_type("test.counter.CounterState"),
    )
    .with_rules_version("2");
    let descriptor = dispatch.component_descriptor().expect("descriptor");
    assert_eq!(
        descriptor.state.expect("state").type_url,
        "/test.counter.CounterState"
    );
}

#[test]
fn a_stateful_component_whose_rebuilder_declares_no_state_type_is_refused() {
    let aggregate: DomainDispatch<LedgerState> =
        DomainDispatch::new("Ledger", "ledger", Rebuilder::new(LedgerState::default))
            .with_rules_version("1");
    let pm: DomainDispatch<LedgerState> = DomainDispatch::process_manager(
        "Fulfillment",
        "fulfillment",
        Rebuilder::new(LedgerState::default),
    )
    .with_rules_version("1");
    for err in [
        aggregate.component_descriptor().expect_err("no state type"),
        pm.component_descriptor().expect_err("no state type"),
    ] {
        assert_eq!(err.code, codes::MISSING_STATE_TYPE);
        crate::test_support::assert_bug(&err);
        assert_eq!(err.grpc, GrpcCode::Internal);
        assert!(err.extras.contains_key(crate::error::extras::COMPONENT));
    }
}

#[test]
fn an_aggregate_without_a_rules_version_is_refused() {
    assert_missing_rules_version(ledger("").component_descriptor());
}

fn fulfillment(version: &str) -> DomainDispatch<LedgerState> {
    let noop = |_: &prost_types::Any, _: &mut LedgerState, _: CommandContext| Ok(None);
    DomainDispatch::process_manager("Fulfillment", "fulfillment", Rebuilder::of_message())
        .on_command("test.PlaceOrder", noop)
        .on_command("test.RecordPayment", noop)
        .on_fact("test.TimedOut", |fact, _| {
            Ok(FactRecord::from(fact.clone()))
        })
        .starts("test.RecordPayment")
        .starts("test.PlaceOrder")
        .with_rules_version(version)
}

// A process manager is a domain (decision 14.31): it subscribes to no domain,
// so it announces no inputs; it announces its state as an aggregate does, and
// its starting input types, sorted (ComponentDescriptor.starting_types).
#[test]
fn a_process_manager_announces_its_state_and_starting_types_and_no_inputs() {
    let descriptor = fulfillment("2.0.1")
        .component_descriptor()
        .expect("descriptor");
    assert_eq!(descriptor.name, "Fulfillment");
    assert_eq!(descriptor.kind(), pb::ComponentKind::ProcessManager);
    assert_eq!(descriptor.rules_version, "2.0.1");
    assert!(inputs(&descriptor).is_empty(), "a PM subscribes to nothing");
    assert_eq!(
        descriptor.starting_types,
        vec![
            "test.PlaceOrder".to_string(),
            "test.RecordPayment".to_string()
        ]
    );
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
}

// types.proto ComponentDescriptor.starting_types: "a non-PM descriptor listing
// starting_types is refused like one with an empty rules_version".
#[test]
fn an_aggregate_declaring_a_starting_type_is_refused() {
    let err = ledger("1")
        .on_command("test.Open", |_, _, _| Ok(None))
        .starts("test.Open")
        .component_descriptor()
        .expect_err("starts is a process manager's");
    assert_eq!(err.code, codes::STARTING_TYPES_ON_NON_PM);
    crate::test_support::assert_bug(&err);
    assert_eq!(err.grpc, GrpcCode::Internal);
}

#[test]
fn a_process_manager_without_a_rules_version_is_refused() {
    assert_missing_rules_version(fulfillment("").component_descriptor());
}

fn notifier(version: &str) -> PolicyDispatch {
    PolicyDispatch::new("Notifier", "order", "email").with_rules_version(version)
}

#[test]
fn a_policy_announces_its_input_domain_and_no_state() {
    let descriptor = notifier("3").component_descriptor().expect("descriptor");
    assert_eq!(descriptor.name, "Notifier");
    assert_eq!(descriptor.kind(), pb::ComponentKind::Policy);
    assert_eq!(descriptor.rules_version, "3");
    assert_eq!(inputs(&descriptor), vec!["order"]);
    assert!(descriptor.state.is_none());
}

#[test]
fn a_policy_without_a_rules_version_is_refused() {
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
